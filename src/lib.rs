mod audio;
mod bus_parser;
mod hotkey;
mod state_machine;
mod typer;
mod ui;

use crossbeam_channel::{unbounded, Sender};
use std::ffi::{c_char, c_void, CStr, CString};
use std::os::raw::c_int;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

#[allow(non_camel_case_types)]
pub type mpl_result_t = c_int;
pub const MPL_OK: mpl_result_t = 0;
pub const MPL_ERR_BUFFER_TOO_SMALL: mpl_result_t = 12;

/// Host callback table (`mpl_host_api_t`).
///
/// Field order MUST match the C header byte-for-byte; the host only ever
/// appends new fields after `ctx`, so declaring a prefix is safe.
///
/// ⚠️ The pointer handed to `micyou_plugin_init` is only valid *during* init
/// (api-reference.md §使用注意事项) — we store a **by-value copy** of the
/// table and never retain the pointer itself.
#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct mpl_host_api_t {
    pub log: Option<unsafe extern "C" fn(ctx: *mut c_void, level: c_int, msg: *const c_char)>,
    pub get_config: Option<
        unsafe extern "C" fn(ctx: *mut c_void, key: *const c_char, out: *mut c_char, out_size: *mut u32) -> mpl_result_t,
    >,
    pub set_config:
        Option<unsafe extern "C" fn(ctx: *mut c_void, key: *const c_char, json_value: *const c_char) -> mpl_result_t>,
    pub emit_event:
        Option<unsafe extern "C" fn(ctx: *mut c_void, topic: *const c_char, json_payload: *const c_char) -> mpl_result_t>,
    pub send_message: Option<
        unsafe extern "C" fn(ctx: *mut c_void, target_json: *const c_char, payload: *const u8, payload_len: u32) -> mpl_result_t,
    >,
    pub audio_state:
        Option<unsafe extern "C" fn(ctx: *mut c_void, out: *mut c_char, out_size: *mut u32) -> mpl_result_t>,
    pub connected_devices:
        Option<unsafe extern "C" fn(ctx: *mut c_void, out: *mut c_char, out_size: *mut u32) -> mpl_result_t>,
    pub ctx: *mut c_void,
    pub play_sound: Option<unsafe extern "C" fn(ctx: *mut c_void, path: *const c_char) -> mpl_result_t>,
    pub plugin_dir:
        Option<unsafe extern "C" fn(ctx: *mut c_void, out: *mut c_char, out_size: *mut u32) -> mpl_result_t>,
    pub register_hotkey:
        Option<unsafe extern "C" fn(ctx: *mut c_void, shortcut: *const c_char, out_id: *mut u64) -> mpl_result_t>,
    pub open_window: Option<unsafe extern "C" fn(ctx: *mut c_void, panel_id: *const c_char) -> mpl_result_t>,
    pub fs_read:
        Option<unsafe extern "C" fn(ctx: *mut c_void, path: *const c_char, out: *mut c_char, out_size: *mut u32) -> mpl_result_t>,
}
// The table is a plain bundle of C function pointers + opaque ctx; sharing the
// by-value copy across threads is what the host's own plugins do (Focus-Capture
// abi.rs), and every call still happens on host-dispatched threads only.
unsafe impl Send for mpl_host_api_t {}
unsafe impl Sync for mpl_host_api_t {}

#[allow(non_camel_case_types)]
#[repr(C)]
pub struct mpl_plugin_info_t {
    pub abi_version: u32,
    pub api_version: u32,
    pub id: *const c_char,
}
unsafe impl Sync for mpl_plugin_info_t {}

/// By-value copy of the host table (see ⚠️ above). Cleared on deinit.
static HOST: Mutex<Option<mpl_host_api_t>> = Mutex::new(None);
static MSG_TX: OnceLock<Sender<bus_parser::WdisMessage>> = OnceLock::new();

static PLUGIN_INFO: mpl_plugin_info_t = mpl_plugin_info_t {
    abi_version: 1,
    api_version: 1,
    id: c"opss.voice-typing".as_ptr(),
};

/// Catch panics at the FFI boundary — unwinding across `extern "C"` is UB.
macro_rules! catch_panic {
    ($($body:tt)*) => {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            $($body)*
        })).unwrap_or_else(|_| -1)
    };
}

// ─────────────────────────── host table access ───────────────────────────

fn store_host(host: mpl_host_api_t) {
    if let Ok(mut slot) = HOST.lock() {
        *slot = Some(host);
    }
}

fn clear_host() {
    if let Ok(mut slot) = HOST.lock() {
        *slot = None;
    }
}

/// Run `f` with the stored host table. Only call from host-dispatched
/// contexts (`init` / `deinit` / `handle_message`) — never from the hook,
/// typer or UI threads (api-reference.md §严格的线程调用限制).
fn with_host<R>(f: impl FnOnce(&mpl_host_api_t) -> R) -> Option<R> {
    HOST.lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(f))
}

// ─────────────────────────── logging ───────────────────────────

const LOG_ERROR: c_int = 0;
const LOG_WARN: c_int = 1;
const LOG_INFO: c_int = 2;

pub fn log_error(msg: &str) {
    host_log(LOG_ERROR, msg)
}
pub fn log_warn(msg: &str) {
    host_log(LOG_WARN, msg)
}
pub fn log_info(msg: &str) {
    host_log(LOG_INFO, msg)
}

fn host_log(level: c_int, msg: &str) {
    with_host(|h| {
        if let (Some(log), Ok(c)) = (h.log, CString::new(msg)) {
            unsafe { log(h.ctx, level, c.as_ptr()) };
        }
    });
}

// ─────────────────────────── config / env helpers ───────────────────────────

fn get_plugin_dir() -> String {
    with_host(|h| {
        let plugin_dir = h.plugin_dir?;
        let mut buf = vec![0u8; 4096];
        let mut size = buf.len() as u32;
        let mut res = unsafe { plugin_dir(h.ctx, buf.as_mut_ptr() as *mut c_char, &mut size) };
        if res == MPL_ERR_BUFFER_TOO_SMALL {
            buf = vec![0u8; size as usize + 1];
            size = buf.len() as u32;
            res = unsafe { plugin_dir(h.ctx, buf.as_mut_ptr() as *mut c_char, &mut size) };
        }
        if res != MPL_OK {
            return None;
        }
        buf.truncate(size.min(buf.len() as u32) as usize);
        Some(String::from_utf8_lossy(&buf).to_string())
    })
    .flatten()
    .unwrap_or_default()
}

/// Read one config value as a string. The host returns JSON, so string values
/// arrive quoted (`"toggle"`) — quotes are stripped. `None` when the key is
/// unset or the API is unavailable.
fn get_config_string(key: &str) -> Option<String> {
    with_host(|h| {
        let get_config = h.get_config?;
        let c_key = CString::new(key).ok()?;
        let mut buf = vec![0u8; 4096];
        let mut size = buf.len() as u32;
        let mut res = unsafe { get_config(h.ctx, c_key.as_ptr(), buf.as_mut_ptr() as *mut c_char, &mut size) };
        if res == MPL_ERR_BUFFER_TOO_SMALL {
            buf = vec![0u8; size as usize + 1];
            size = buf.len() as u32;
            res = unsafe { get_config(h.ctx, c_key.as_ptr(), buf.as_mut_ptr() as *mut c_char, &mut size) };
        }
        if res != MPL_OK {
            return None;
        }
        buf.truncate(size.min(buf.len() as u32) as usize);
        Some(String::from_utf8_lossy(&buf).trim().trim_matches('"').to_string())
    })
    .flatten()
}

/// Resolve the effective hotkey combo from config, falling back to the
/// default (init) or the currently active combo (live reload) on bad input.
fn resolve_combo(requested: &str, fallback: hotkey::Combo) -> hotkey::Combo {
    match hotkey::parse_combo(requested) {
        Ok(c) => c,
        Err(e) => {
            log_warn(&format!(
                "无效快捷键 [{requested}]：{e}；沿用 [{}]",
                fallback
            ));
            fallback
        }
    }
}

/// Re-read `hotkey` / `mode` from host config and apply them live.
/// Called from `handle_message` (host-dispatched thread) when the host
/// broadcasts `config:changed` (or the panel sends `ui:apply`).
fn reload_settings() {
    let mode = hotkey::TriggerMode::parse(
        &get_config_string("mode").unwrap_or_else(|| hotkey::TriggerMode::Toggle.as_str().to_string()),
    );
    let current = hotkey::current_settings();
    let hotkey_text = get_config_string("hotkey")
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| hotkey::DEFAULT_HOTKEY.to_string());
    let combo = resolve_combo(&hotkey_text, current.combo);
    hotkey::update_settings(hotkey::Settings { combo, mode });
}

// ─────────────────────────── plugin ABI ───────────────────────────

#[no_mangle]
pub extern "C" fn micyou_plugin_info() -> *const mpl_plugin_info_t {
    &PLUGIN_INFO
}

#[no_mangle]
pub extern "C" fn micyou_plugin_init(host: *const mpl_host_api_t) -> mpl_result_t {
    catch_panic! {
        if host.is_null() {
            return -1;
        }
        // Store a by-value copy immediately; the pointer must not be retained.
        store_host(unsafe { *host });

        let (msg_tx, msg_rx) = unbounded();
        let _ = MSG_TX.set(msg_tx);

        let plugin_dir = get_plugin_dir();
        let start_wav = std::fs::read(Path::new(&plugin_dir).join("assets/start.wav")).unwrap_or_default();
        let end_wav = std::fs::read(Path::new(&plugin_dir).join("assets/end.wav")).unwrap_or_default();

        let audio_engine = audio::AudioEngine::new(start_wav, end_wav);
        let state_machine = state_machine::TimeWindowStateMachine::new(1500);

        let mode_str = get_config_string("mode")
            .unwrap_or_else(|| hotkey::TriggerMode::Toggle.as_str().to_string());
        let hotkey_text = get_config_string("hotkey")
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| hotkey::DEFAULT_HOTKEY.to_string());
        let fallback = hotkey::parse_combo(hotkey::DEFAULT_HOTKEY)
            .expect("DEFAULT_HOTKEY must always parse");
        let settings = hotkey::Settings {
            combo: resolve_combo(&hotkey_text, fallback),
            mode: hotkey::TriggerMode::parse(&mode_str),
        };

        let sm_clone = state_machine.clone();
        std::thread::spawn(move || typer::run_typer_worker(msg_rx, sm_clone));

        hotkey::start_hotkey_thread(state_machine.clone(), audio_engine, settings);

        ui::start_ui_thread(state_machine);

        log_info(&format!(
            "Voice-Typing v{} ready — hotkey [{}] mode={}",
            env!("CARGO_PKG_VERSION"),
            settings.combo,
            settings.mode.as_str()
        ));
        MPL_OK
    }
}

#[no_mangle]
pub extern "C" fn micyou_plugin_deinit() {
    // Host ABI declares deinit as returning void (native.rs `DeinitFn`).
    let _ = catch_panic! {
        // Disarm the hook engine and join its thread *before* the host may
        // unload the library — a live low-level hook in an unloaded DLL would
        // crash the whole system input chain.
        hotkey::shutdown();
        clear_host();
        MPL_OK
    };
}

#[no_mangle]
pub extern "C" fn micyou_plugin_handle_message(
    _source: *const c_char,
    topic: *const c_char,
    payload: *const u8,
    len: u32,
) -> mpl_result_t {
    catch_panic! {
        let topic_str = unsafe {
            if topic.is_null() {
                ""
            } else {
                CStr::from_ptr(topic).to_str().unwrap_or("")
            }
        };

        match topic_str {
            // Host broadcast after any config save (settings form or panel):
            // reload hotkey/mode live — no plugin re-enable needed.
            "config:changed" | "ui:apply" => {
                reload_settings();
                return MPL_OK;
            }
            _ => {}
        }

        // Everything else: treat as a potential WhatdidIsay transcription
        // broadcast ("WDIS" magic + timestamps + UTF-8 text).
        if payload.is_null() || len < 20 {
            return MPL_OK;
        }
        let data = unsafe { std::slice::from_raw_parts(payload, len as usize) };
        if let Some(msg) = bus_parser::parse_wdis_payload(data) {
            if let Some(tx) = MSG_TX.get() {
                let _ = tx.send(msg);
            }
        }
        MPL_OK
    }
}
