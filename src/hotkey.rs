use crate::{state_machine::TimeWindowStateMachine, audio::AudioEngine};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Foundation::*;
use std::sync::OnceLock;
use chrono::Utc;

static STATE_MACHINE: OnceLock<TimeWindowStateMachine> = OnceLock::new();
static AUDIO_ENGINE: OnceLock<AudioEngine> = OnceLock::new();

pub fn start_hotkey_thread(sm: TimeWindowStateMachine, audio: AudioEngine) {
    let _ = STATE_MACHINE.set(sm);
    let _ = AUDIO_ENGINE.set(audio);

    std::thread::spawn(|| {
        unsafe {
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), None, 0)
                .expect("Failed to set keyboard hook");

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = UnhookWindowsHookEx(hook);
        }
    });
}

unsafe extern "system" fn keyboard_hook(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if n_code >= 0 {
        let kb_struct = &*(l_param.0 as *const KBDLLHOOKSTRUCT);
        let vk_code = kb_struct.vkCode;
        
        // 默认快捷键: Alt + Space (VK_MENU + VK_SPACE)
        let is_space = vk_code == 0x20; // VK_SPACE
        let is_alt_down = kb_struct.flags.contains(LLKHF_ALTDOWN);

        if is_space && is_alt_down {
            let now_ms = Utc::now().timestamp_millis();
            let sm = STATE_MACHINE.get().unwrap();
            let audio = AUDIO_ENGINE.get().unwrap();

            if w_param.0 == WM_KEYDOWN as usize {
                if !sm.is_active.load(std::sync::atomic::Ordering::SeqCst) {
                    sm.on_activate(now_ms);
                    audio.play_start();
                }
            } else if w_param.0 == WM_KEYUP as usize {
                if sm.is_active.load(std::sync::atomic::Ordering::SeqCst) {
                    sm.on_deactivate(now_ms);
                    audio.play_end();
                }
            }
        }
    }
    CallNextHookEx(None, n_code, w_param, l_param)
}