//! Configurable global-hotkey engine for Voice-Typing (Windows low-level
//! keyboard + mouse hooks).
//!
//! History: v0.1 hard-coded `LCtrl + LShift`. v0.2 keeps the same
//! `WH_KEYBOARD_LL` delivery but parses a user-configured combo string from
//! plugin config (`hotkey`), supporting modifier-only combos (the IME-style
//! default, which the host's `register_hotkey` cannot express — global-hotkey
//! requires a non-modifier main key), classic `mods + main key` combos
//! (e.g. `ctrl+shift+f8`) and mouse side buttons (`mouse4` / `mouse5`,
//! bare or combined with modifiers).
//!
//! Design notes:
//! * The hook callback polls `GetAsyncKeyState` on every input event and
//!   edge-triggers on the combo's down/up transition (same approach as v0.1,
//!   which is immune to missed events and auto-repeat). `GetAsyncKeyState`
//!   also reports the mouse side buttons (VK_XBUTTON1/2), so cross-device
//!   combos like `ctrl+mouse4` stay consistent from both hooks.
//! * When the combo contains a main key (keyboard or mouse), the main key's
//!   own down/up events are swallowed (hook returns 1) while the combo's
//!   modifiers are held, so the foreground app never sees the hotkey
//!   (no stray keystrokes, no browser back/forward on mouse4/5).
//! * Modifier-only combos are NEVER swallowed — that would break ordinary
//!   shortcuts like Ctrl+C for the whole system.
//! * The mouse hook is only installed while the active combo actually uses a
//!   mouse button (a bare keyboard combo pays zero mouse-hook overhead);
//!   `update_settings` re-evaluates it on the pump thread via a WM_APP post —
//!   hooks must be installed/uninstalled from the thread whose message loop
//!   will dispatch them.
//! * The effective combo/mode live in a `RwLock` snapshot that `lib.rs`
//!   updates from `handle_message` (a host-dispatched thread) when the host
//!   broadcasts `config:changed` — changes apply live, no reload needed.

use crate::audio::AudioEngine;
use crate::state_machine::TimeWindowStateMachine;
use chrono::Utc;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use windows::Win32::Foundation::*;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU,
    VK_RSHIFT, VK_RWIN, VK_XBUTTON1, VK_XBUTTON2,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// Fallback combo when config is missing or unparseable (v0.1 behaviour).
pub const DEFAULT_HOTKEY: &str = "lctrl+lshift";

// ─────────────────────────── trigger mode ───────────────────────────

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum TriggerMode {
    Toggle,
    PushToTalk,
}

impl TriggerMode {
    /// Parse the `mode` config value; anything but `push_to_talk` is Toggle.
    pub fn parse(s: &str) -> Self {
        if s.trim() == "push_to_talk" {
            TriggerMode::PushToTalk
        } else {
            TriggerMode::Toggle
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TriggerMode::Toggle => "toggle",
            TriggerMode::PushToTalk => "push_to_talk",
        }
    }
}

// ─────────────────────────── combo parsing ───────────────────────────

/// A parsed key combination.
///
/// Modifiers come in two flavours: `exact_mods` are specific physical keys
/// (VK_LCONTROL etc. — left/right distinguished), while the `any_*` flags
/// accept either side. `main_vk == 0` means a modifier-only combo.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Combo {
    exact_mods: [u16; 8],
    any_ctrl: bool,
    any_alt: bool,
    any_shift: bool,
    any_win: bool,
    main_vk: u16,
}

/// Neutral value for statics; `is_empty()` combos are inert (the hook skips
/// them), so a not-yet-configured engine can never trigger by accident.
pub const EMPTY_COMBO: Combo = Combo {
    exact_mods: [0; 8],
    any_ctrl: false,
    any_alt: false,
    any_shift: false,
    any_win: false,
    main_vk: 0,
};

impl Combo {
    fn push_mod(&mut self, vk: u16) -> Result<(), String> {
        if self.exact_mods.contains(&vk) {
            return Ok(()); // duplicate token, idempotent
        }
        let slot = self
            .exact_mods
            .iter_mut()
            .find(|s| **s == 0)
            .ok_or_else(|| "修饰键过多（最多 8 个）".to_string())?;
        *slot = vk;
        Ok(())
    }

    fn mod_count(&self) -> usize {
        self.exact_mods.iter().filter(|&&v| v != 0).count()
            + self.any_ctrl as usize
            + self.any_alt as usize
            + self.any_shift as usize
            + self.any_win as usize
    }

    pub fn is_empty(&self) -> bool {
        *self == EMPTY_COMBO
    }

    /// Every required modifier currently held? (unsafe: GetAsyncKeyState)
    unsafe fn mods_down(&self) -> bool {
        for &vk in self.exact_mods.iter() {
            if vk != 0 && !unsafe { key_down(vk) } {
                return false;
            }
        }
        if self.any_ctrl && !(unsafe { key_down(VK_LCONTROL.0) } || unsafe { key_down(VK_RCONTROL.0) }) {
            return false;
        }
        if self.any_alt && !(unsafe { key_down(VK_LMENU.0) } || unsafe { key_down(VK_RMENU.0) }) {
            return false;
        }
        if self.any_shift && !(unsafe { key_down(VK_LSHIFT.0) } || unsafe { key_down(VK_RSHIFT.0) }) {
            return false;
        }
        if self.any_win && !(unsafe { key_down(VK_LWIN.0) } || unsafe { key_down(VK_RWIN.0) }) {
            return false;
        }
        true
    }

    /// Whole combo (modifiers + main key) currently held?
    unsafe fn is_down(&self) -> bool {
        // SAFETY: caller contract — GetAsyncKeyState is safe to call from any
        // thread at any time; the unsafe ops are the leaf calls in mods_down /
        // key_down (this unsafe fn body is an implicit unsafe block, ed. 2021).
        self.mods_down() && (self.main_vk == 0 || key_down(self.main_vk))
    }

    /// True when the combo's main key is a mouse side button — the pump
    /// thread installs `WH_MOUSE_LL` only while this holds.
    pub fn needs_mouse(&self) -> bool {
        self.main_vk == VK_XBUTTON1.0 || self.main_vk == VK_XBUTTON2.0
    }
}

impl fmt::Display for Combo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = Vec::new();
        for &vk in self.exact_mods.iter() {
            if vk != 0 {
                parts.push(vk_to_name(vk));
            }
        }
        if self.any_ctrl {
            parts.push("ctrl".into());
        }
        if self.any_alt {
            parts.push("alt".into());
        }
        if self.any_shift {
            parts.push("shift".into());
        }
        if self.any_win {
            parts.push("win".into());
        }
        if self.main_vk != 0 {
            parts.push(vk_to_name(self.main_vk));
        }
        write!(f, "{}", parts.join("+"))
    }
}

unsafe fn key_down(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) < 0 }
}

/// Parse a combo string like `lctrl+lshift`, `ctrl+shift+f8` or `alt+space`.
///
/// Token vocabulary mirrors the host's global-hotkey parser (Focus-Capture
/// compatible) plus left/right-specific modifier tokens, which the low-level
/// hook can honour and which make IME-style modifier-only combos expressible.
///
/// Safety rules (reject with a precise Chinese error, shown in plugin logs):
/// * a bare letter/digit/ordinary key as main key is rejected — it would
///   hijack normal typing system-wide; bare main keys are limited to F1–F24;
/// * modifier-only combos need at least two modifiers (a lone `lctrl` would
///   fire on every Ctrl+C).
pub fn parse_combo(text: &str) -> Result<Combo, String> {
    let mut combo = EMPTY_COMBO;
    let mut main: Option<u16> = None;
    let mut tokens = 0usize;

    for raw in text.split('+') {
        let tok = raw.trim().to_ascii_lowercase();
        if tok.is_empty() {
            continue;
        }
        tokens += 1;
        match tok.as_str() {
            "ctrl" | "control" => combo.any_ctrl = true,
            "lctrl" | "leftctrl" => combo.push_mod(VK_LCONTROL.0)?,
            "rctrl" | "rightctrl" => combo.push_mod(VK_RCONTROL.0)?,
            "alt" | "option" => combo.any_alt = true,
            "lalt" | "leftalt" => combo.push_mod(VK_LMENU.0)?,
            "ralt" | "rightalt" => combo.push_mod(VK_RMENU.0)?,
            "shift" => combo.any_shift = true,
            "lshift" | "leftshift" => combo.push_mod(VK_LSHIFT.0)?,
            "rshift" | "rightshift" => combo.push_mod(VK_RSHIFT.0)?,
            "win" | "super" | "meta" | "cmd" | "command" => combo.any_win = true,
            "lwin" | "leftwin" => combo.push_mod(VK_LWIN.0)?,
            "rwin" | "rightwin" => combo.push_mod(VK_RWIN.0)?,
            other => {
                if main.is_some() {
                    return Err(format!("快捷键只能有一个主键（多余 token: {other}）"));
                }
                let vk = match other {
                    // 鼠标侧键（浏览器「后退/前进」键）——可作为主键，允许单独使用
                    "mouse4" | "xbutton1" | "x1" | "back" => VK_XBUTTON1.0,
                    "mouse5" | "xbutton2" | "x2" | "forward" => VK_XBUTTON2.0,
                    _ => key_to_vk(other)
                        .ok_or_else(|| format!("无法识别的键名: {other}"))?,
                };
                main = Some(vk);
            }
        }
    }

    if tokens == 0 {
        return Err("快捷键不能为空".into());
    }

    if let Some(vk) = main {
        combo.main_vk = vk;
        if combo.mod_count() == 0 {
            // Bare main key: only F1–F24 and the mouse side buttons are safe
            // (neither produces text input; everything else would hijack
            // normal typing / shortcuts system-wide).
            let is_fkey = (0x70..=0x87).contains(&vk);
            let is_mouse = vk == VK_XBUTTON1.0 || vk == VK_XBUTTON2.0;
            if !is_fkey && !is_mouse {
                return Err(
                    "字母/数字/普通键作主键时必须搭配至少一个修饰键（ctrl/alt/shift/win）；无修饰键时仅支持 F1-F24 与鼠标侧键（mouse4/mouse5）"
                        .into(),
                );
            }
        }
    } else if combo.mod_count() < 2 {
        return Err("纯修饰键组合至少需要两个修饰键（如 lctrl+lshift），避免误触".into());
    }

    Ok(combo)
}

/// Key name → Win32 virtual-key code (US-layout VK semantics, same table as
/// Focus-Capture's parser so combo strings are portable between the plugins).
fn key_to_vk(name: &str) -> Option<u16> {
    let b = name.as_bytes();
    if b.len() == 1 {
        return match b[0] {
            b'a'..=b'z' => Some(0x41 + (b[0] - b'a') as u16),
            b'0'..=b'9' => Some(0x30 + (b[0] - b'0') as u16),
            _ => None,
        };
    }
    if let Some(n) = name.strip_prefix('f') {
        if let Ok(n) = n.parse::<u16>() {
            if (1..=24).contains(&n) {
                return Some(0x70 + n - 1); // VK_F1..VK_F24
            }
        }
    }
    Some(match name {
        "space" => 0x20,
        "tab" => 0x09,
        "enter" | "return" => 0x0D,
        "backspace" => 0x08,
        "escape" | "esc" => 0x1B,
        "pause" => 0x13,
        "insert" => 0x2D,
        "delete" | "del" => 0x2E,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "up" => 0x26,
        "left" => 0x25,
        "down" => 0x28,
        "right" => 0x27,
        "minus" => 0xBD,
        "equal" | "equals" => 0xBB,
        "bracketleft" | "leftbracket" => 0xDB,
        "bracketright" | "rightbracket" => 0xDD,
        "backslash" => 0xDC,
        "semicolon" => 0xBA,
        "quote" | "apostrophe" => 0xDE,
        "comma" => 0xBC,
        "period" | "dot" => 0xBE,
        "slash" => 0xBF,
        "backquote" | "grave" => 0xC0,
        _ => return None,
    })
}

/// Reverse mapping, only used for logging / panel display.
fn vk_to_name(vk: u16) -> String {
    match vk {
        0x05 => "mouse4".into(),
        0x06 => "mouse5".into(),
        0xA2 => "lctrl".into(),
        0xA3 => "rctrl".into(),
        0xA4 => "lalt".into(),
        0xA5 => "ralt".into(),
        0xA0 => "lshift".into(),
        0xA1 => "rshift".into(),
        0x5B => "lwin".into(),
        0x5C => "rwin".into(),
        0x20 => "space".into(),
        0x09 => "tab".into(),
        0x0D => "enter".into(),
        0x08 => "backspace".into(),
        0x1B => "esc".into(),
        0x13 => "pause".into(),
        0x2D => "insert".into(),
        0x2E => "delete".into(),
        0x24 => "home".into(),
        0x23 => "end".into(),
        0x21 => "pageup".into(),
        0x22 => "pagedown".into(),
        0x26 => "up".into(),
        0x25 => "left".into(),
        0x28 => "down".into(),
        0x27 => "right".into(),
        0xBD => "minus".into(),
        0xBB => "equal".into(),
        0xDB => "bracketleft".into(),
        0xDD => "bracketright".into(),
        0xDC => "backslash".into(),
        0xBA => "semicolon".into(),
        0xDE => "quote".into(),
        0xBC => "comma".into(),
        0xBE => "period".into(),
        0xBF => "slash".into(),
        0xC0 => "backquote".into(),
        v @ 0x41..=0x5A => ((v - 0x41 + b'A' as u16) as u8 as char)
            .to_ascii_lowercase()
            .to_string(),
        v @ 0x30..=0x39 => ((v - 0x30 + b'0' as u16) as u8 as char).to_string(),
        v @ 0x70..=0x87 => format!("f{}", v - 0x70 + 1),
        v => format!("vk:{v:#04x}"),
    }
}

// ─────────────────────────── runtime state ───────────────────────────

/// Live snapshot read by the hook on every keyboard event.
#[derive(Clone, Copy)]
pub struct Settings {
    pub combo: Combo,
    pub mode: TriggerMode,
}

static STATE_MACHINE: OnceLock<TimeWindowStateMachine> = OnceLock::new();
static AUDIO_ENGINE: OnceLock<AudioEngine> = OnceLock::new();
static SETTINGS: RwLock<Settings> = RwLock::new(Settings {
    combo: EMPTY_COMBO,
    mode: TriggerMode::Toggle,
});
/// Edge-trigger latch: combo is currently held (suppresses auto-repeat).
static COMBO_HELD: AtomicBool = AtomicBool::new(false);
/// True while a swallowed main-key press is pending its (also swallowed) release.
static SWALLOWED_MAIN: AtomicBool = AtomicBool::new(false);
/// Hook thread handle + Win32 thread id, used by `shutdown()` to unhook
/// cleanly before the library is unloaded.
static HOOK_THREAD: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
static HOOK_TID: AtomicU32 = AtomicU32::new(0);

fn read_settings() -> Settings {
    *SETTINGS
        .read()
        .unwrap_or_else(|e| e.into_inner())
}

/// Currently effective settings (for logging / fallback on invalid config).
pub fn current_settings() -> Settings {
    read_settings()
}

/// Swap in a new combo/mode (called from `handle_message` on a host thread
/// after `config:changed`). Applies instantly: the very next keyboard event
/// is matched against the new combo and the old one stops working.
pub fn update_settings(new: Settings) {
    let old = {
        let mut s = SETTINGS.write().unwrap_or_else(|e| e.into_inner());
        let old = *s;
        *s = new;
        old
    };
    // Reset edge state — it referred to the old combo.
    COMBO_HELD.store(false, Ordering::SeqCst);
    SWALLOWED_MAIN.store(false, Ordering::SeqCst);

    // A push-to-talk session ends on combo *release*; if the combo or mode
    // just changed that release may never be recognised, so end it cleanly.
    if old.mode == TriggerMode::PushToTalk {
        if let (Some(sm), Some(audio)) = (STATE_MACHINE.get(), AUDIO_ENGINE.get()) {
            if sm.is_active.load(Ordering::SeqCst) {
                sm.on_deactivate(Utc::now().timestamp_millis());
                audio.play_end();
            }
        }
    }

    if old.combo != new.combo || old.mode != new.mode {
        crate::log_info(&format!(
            "hotkey settings applied: [{}] mode={}",
            new.combo,
            new.mode.as_str()
        ));
    }

    // Ask the pump thread to reconcile the mouse hook with the new combo
    // (hooks must be installed/uninstalled on the thread whose message loop
    // dispatches them). Best effort — if the thread isn't up yet, it reads
    // the current settings when it starts and installs accordingly.
    let tid = HOOK_TID.load(Ordering::SeqCst);
    if tid != 0 {
        let _ = unsafe { PostThreadMessageW(tid, WM_APP_REHOOK, WPARAM(0), LPARAM(0)) };
    }
}

/// Private WM_APP message: "re-evaluate whether WH_MOUSE_LL is needed".
const WM_APP_REHOOK: u32 = WM_APP + 1;

/// Install WH_MOUSE_LL from the pump thread; None + log on failure.
unsafe fn install_mouse_hook() -> Option<HHOOK> {
    match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0) } {
        Ok(h) => Some(h),
        Err(e) => {
            crate::log_error(&format!("SetWindowsHookExW(WH_MOUSE_LL) failed: {e}"));
            None
        }
    }
}

pub fn start_hotkey_thread(sm: TimeWindowStateMachine, audio: AudioEngine, settings: Settings) {
    let _ = STATE_MACHINE.set(sm);
    let _ = AUDIO_ENGINE.set(audio);
    {
        let mut s = SETTINGS.write().unwrap_or_else(|e| e.into_inner());
        *s = settings;
    }

    let handle = std::thread::spawn(|| unsafe {
        // Register the thread id and force message-queue creation *before*
        // installing the hooks, so a later PostThreadMessageW (shutdown /
        // rehook) can always reach this thread.
        HOOK_TID.store(GetCurrentThreadId(), Ordering::SeqCst);
        let mut warm = MSG::default();
        let _ = PeekMessageW(&mut warm, None, 0, 0, PM_NOREMOVE);

        let kb_hook = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), None, 0) {
            Ok(h) => h,
            Err(e) => {
                crate::log_error(&format!("SetWindowsHookExW failed: {e}"));
                HOOK_TID.store(0, Ordering::SeqCst);
                return;
            }
        };
        // Mouse hook only while the active combo uses a side button — a plain
        // keyboard combo pays zero per-mouse-event overhead.
        let mut ms_hook = if read_settings().combo.needs_mouse() {
            install_mouse_hook()
        } else {
            None
        };

        // The hook thread must pump messages, otherwise the system stops
        // delivering low-level hook callbacks (and eventually times the hook
        // out silently). WM_QUIT (posted by `shutdown()`) ends the loop.
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_APP_REHOOK {
                // Settings changed: reconcile the mouse hook on this thread.
                if read_settings().combo.needs_mouse() {
                    if ms_hook.is_none() {
                        ms_hook = install_mouse_hook();
                    }
                } else if let Some(h) = ms_hook.take() {
                    let _ = UnhookWindowsHookEx(h);
                }
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnhookWindowsHookEx(kb_hook);
        if let Some(h) = ms_hook {
            let _ = UnhookWindowsHookEx(h);
        }
        HOOK_TID.store(0, Ordering::SeqCst);
    });

    if let Ok(mut slot) = HOOK_THREAD.lock() {
        *slot = Some(handle);
    }
}

/// Stop the hook engine and join its thread. Called from `deinit` — the
/// library may be unloaded right after, and a low-level hook left behind in
/// an unloaded DLL would fault inside the system-wide input chain.
pub fn shutdown() {
    // Disarm first: even before the thread exits, an empty combo is inert.
    if let Ok(mut s) = SETTINGS.write() {
        s.combo = EMPTY_COMBO;
    }
    COMBO_HELD.store(false, Ordering::SeqCst);
    SWALLOWED_MAIN.store(false, Ordering::SeqCst);

    // Wait (bounded) for the hook thread to publish its thread id, then post
    // WM_QUIT. The retry covers two races: the thread not yet scheduled right
    // after init, and PostThreadMessageW arriving before the queue exists
    // (the thread forces queue creation before installing the hook, so this
    // normally succeeds on the first attempt).
    for _ in 0..200 {
        let tid = HOOK_TID.load(Ordering::SeqCst);
        if tid != 0 {
            let ok = unsafe { PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0)) }.is_ok();
            if ok {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    if let Ok(mut slot) = HOOK_THREAD.lock() {
        if let Some(h) = slot.take() {
            let _ = h.join();
        }
    }
}

/// Low-level keyboard hook callback. `pub(crate)` only for testability —
/// not an exported symbol (the DLL exports just the `#[no_mangle]` ABI fns).
pub(crate) unsafe extern "system" fn keyboard_hook(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if n_code >= 0 {
        let settings = read_settings();
        let combo = settings.combo;
        // An unconfigured (empty) combo is inert — never triggers, never swallows.
        if !combo.is_empty() {
            let kb = unsafe { &*(l_param.0 as *const KBDLLHOOKSTRUCT) };
            let vk = kb.vkCode as u16;
            let msg = w_param.0 as u32;
            let is_down_msg = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
            let is_up_msg = msg == WM_KEYUP || msg == WM_SYSKEYUP;
            let main_event = combo.main_vk != 0 && vk == combo.main_vk;
            let mods_down = unsafe { combo.mods_down() };

            // Swallow the main key's own events while it acts as part of the
            // combo, so the foreground app never receives the hotkey.
            if main_event && is_down_msg && mods_down {
                SWALLOWED_MAIN.store(true, Ordering::SeqCst);
                press_edge(settings.mode);
                return LRESULT(1);
            }
            if main_event && is_up_msg && SWALLOWED_MAIN.load(Ordering::SeqCst) {
                SWALLOWED_MAIN.store(false, Ordering::SeqCst);
                release_edge(settings.mode);
                return LRESULT(1);
            }

            // Polling edge detection — identical semantics for modifier-only
            // and main-key combos, and robust against event ordering: the
            // combo counts as pressed exactly while all of its keys are down.
            let combo_down = unsafe { combo.is_down() };
            if combo_down {
                press_edge(settings.mode);
            } else {
                release_edge(settings.mode);
            }
        }
    }
    unsafe { CallNextHookEx(None, n_code, w_param, l_param) }
}

/// Low-level mouse hook — active only while the combo's main key is a side
/// button (`mouse4`/`mouse5`). Mirrors the keyboard hook's semantics:
/// trigger on the button's press edge (with the combo's modifiers held),
/// swallow the button events so the app never sees them (no stray
/// back/forward navigation), and end push-to-talk on release.
pub(crate) unsafe extern "system" fn mouse_hook(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if n_code >= 0 {
        let settings = read_settings();
        let combo = settings.combo;
        let msg = w_param.0 as u32;
        if !combo.is_empty()
            && combo.needs_mouse()
            && (msg == WM_XBUTTONDOWN || msg == WM_XBUTTONUP)
        {
            let ms = unsafe { &*(l_param.0 as *const MSLLHOOKSTRUCT) };
            // HIWORD(mouseData) carries XBUTTON1 (1) / XBUTTON2 (2).
            let vk = match (ms.mouseData >> 16) & 0xFFFF {
                1 => VK_XBUTTON1.0,
                2 => VK_XBUTTON2.0,
                _ => 0,
            };
            if vk != 0 && vk == combo.main_vk {
                if msg == WM_XBUTTONDOWN {
                    if unsafe { combo.mods_down() } {
                        SWALLOWED_MAIN.store(true, Ordering::SeqCst);
                        press_edge(settings.mode);
                        return LRESULT(1);
                    }
                } else if SWALLOWED_MAIN.swap(false, Ordering::SeqCst) {
                    release_edge(settings.mode);
                    return LRESULT(1);
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, n_code, w_param, l_param) }
}

/// Combo transitioned to "held": fire once (auto-repeat suppressed).
fn press_edge(mode: TriggerMode) {
    if COMBO_HELD.swap(true, Ordering::SeqCst) {
        return; // already held — ignore repeats / further events
    }
    let (Some(sm), Some(audio)) = (STATE_MACHINE.get(), AUDIO_ENGINE.get()) else {
        return;
    };
    let now_ms = Utc::now().timestamp_millis();
    match mode {
        TriggerMode::Toggle => {
            if sm.is_active.load(Ordering::SeqCst) {
                sm.on_deactivate(now_ms);
                audio.play_end();
            } else {
                sm.on_activate(now_ms);
                audio.play_start();
            }
        }
        TriggerMode::PushToTalk => {
            if !sm.is_active.load(Ordering::SeqCst) {
                sm.on_activate(now_ms);
                audio.play_start();
            }
        }
    }
}

/// Combo transitioned to "released": only meaningful for push-to-talk.
fn release_edge(mode: TriggerMode) {
    if !COMBO_HELD.swap(false, Ordering::SeqCst) {
        return; // wasn't held — nothing to do
    }
    if mode != TriggerMode::PushToTalk {
        return;
    }
    let (Some(sm), Some(audio)) = (STATE_MACHINE.get(), AUDIO_ENGINE.get()) else {
        return;
    };
    if sm.is_active.load(Ordering::SeqCst) {
        let now_ms = Utc::now().timestamp_millis();
        sm.on_deactivate(now_ms);
        audio.play_end();
    }
}

// ─────────────────────────── tests (pure parsing) ───────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_modifier_only_combo() {
        let c = parse_combo("lctrl+lshift").unwrap();
        assert_eq!(c.main_vk, 0);
        assert_eq!(c.mod_count(), 2);
        assert!(c.exact_mods.contains(&VK_LCONTROL.0));
        assert!(c.exact_mods.contains(&VK_LSHIFT.0));
        assert_eq!(c.to_string(), "lctrl+lshift");
    }

    #[test]
    fn parses_mixed_mods_and_main_key() {
        let c = parse_combo("Ctrl + Shift + F8").unwrap();
        assert!(c.any_ctrl && c.any_shift);
        assert_eq!(c.main_vk, 0x77); // VK_F8
        let c2 = parse_combo("alt+space").unwrap();
        assert_eq!(c2.main_vk, 0x20);
        let c3 = parse_combo("lalt+rwin+a").unwrap();
        assert_eq!(c3.main_vk, 0x41);
    }

    #[test]
    fn bare_fkey_allowed_bare_letter_rejected() {
        assert!(parse_combo("f13").is_ok());
        assert!(parse_combo("a").is_err());
        assert!(parse_combo("space").is_err());
        assert!(parse_combo("ctrl+a").is_ok());
    }

    #[test]
    fn single_modifier_only_combo_rejected() {
        assert!(parse_combo("lctrl").is_err());
        assert!(parse_combo("shift").is_err());
        assert!(parse_combo("lctrl+rctrl").is_ok());
    }

    #[test]
    fn invalid_inputs_rejected() {
        assert!(parse_combo("").is_err());
        assert!(parse_combo("  +  ").is_err());
        assert!(parse_combo("ctrl+nope").is_err());
        assert!(parse_combo("ctrl+a+b").is_err());
        assert!(parse_combo("mouse4+mouse5").is_err()); // two main keys
        assert!(parse_combo("wheel").is_err());
    }

    #[test]
    fn mouse_side_buttons_supported() {
        // Bare side buttons are valid hotkeys (no text input to hijack).
        let m4 = parse_combo("mouse4").unwrap();
        assert_eq!(m4.main_vk, 0x05); // VK_XBUTTON1
        assert!(m4.needs_mouse());
        assert_eq!(m4.to_string(), "mouse4");
        // Aliases map to the same buttons.
        assert_eq!(parse_combo("xbutton1").unwrap(), m4);
        assert_eq!(parse_combo("back").unwrap(), m4);
        let m5 = parse_combo("mouse5").unwrap();
        assert_eq!(m5.main_vk, 0x06); // VK_XBUTTON2
        assert_eq!(parse_combo("forward").unwrap(), m5);
        // Combined with modifiers.
        let c = parse_combo("ctrl+mouse5").unwrap();
        assert!(c.any_ctrl && c.needs_mouse() && c.main_vk == 0x06);
        // Keyboard combos never need the mouse hook.
        assert!(!parse_combo("lctrl+lshift").unwrap().needs_mouse());
        assert!(!parse_combo("ctrl+shift+f8").unwrap().needs_mouse());
    }

    #[test]
    fn duplicate_tokens_are_idempotent() {
        assert_eq!(parse_combo("lctrl+lctrl+lshift").unwrap().mod_count(), 2);
    }

    #[test]
    fn default_hotkey_constant_parses() {
        assert!(parse_combo(DEFAULT_HOTKEY).is_ok());
    }
}
