use crate::{state_machine::TimeWindowStateMachine, audio::AudioEngine};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::Foundation::*;
use std::sync::OnceLock;
use chrono::Utc;

static STATE_MACHINE: OnceLock<TimeWindowStateMachine> = OnceLock::new();
static AUDIO_ENGINE: OnceLock<AudioEngine> = OnceLock::new();
static mut IS_COMBO_HELD: bool = false;

#[derive(PartialEq, Clone, Copy)]
enum TriggerMode {
    Toggle,
    PushToTalk,
}

static mut MODE: TriggerMode = TriggerMode::Toggle;

pub fn start_hotkey_thread(sm: TimeWindowStateMachine, audio: AudioEngine, mode_str: String) {
    let _ = STATE_MACHINE.set(sm);
    let _ = AUDIO_ENGINE.set(audio);
    
    unsafe {
        MODE = if mode_str.trim() == "push_to_talk" {
            TriggerMode::PushToTalk
        } else {
            TriggerMode::Toggle
        };
    }

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
        // 固定监听 LCtrl (0xA2) 和 LShift (0xA0)
        let lctrl_down = GetAsyncKeyState(0xA2) < 0;
        let lshift_down = GetAsyncKeyState(0xA0) < 0;
        let combo_down = lctrl_down && lshift_down;

        // 边缘触发逻辑：仅在组合键状态发生翻转时执行
        if combo_down {
            if !IS_COMBO_HELD {
                IS_COMBO_HELD = true; // 标记组合键已按下，防止 Auto-repeat 重复触发
                let now_ms = Utc::now().timestamp_millis();
                let sm = STATE_MACHINE.get().unwrap();
                let audio = AUDIO_ENGINE.get().unwrap();

                if MODE == TriggerMode::Toggle {
                    if sm.is_active.load(std::sync::atomic::Ordering::SeqCst) {
                        sm.on_deactivate(now_ms);
                        audio.play_end();
                    } else {
                        sm.on_activate(now_ms);
                        audio.play_start();
                    }
                } else { // PushToTalk
                    if !sm.is_active.load(std::sync::atomic::Ordering::SeqCst) {
                        sm.on_activate(now_ms);
                        audio.play_start();
                    }
                }
            }
        } else {
            if IS_COMBO_HELD {
                IS_COMBO_HELD = false; // 组合键被破坏（任意一键松开）
                if MODE == TriggerMode::PushToTalk {
                    let now_ms = Utc::now().timestamp_millis();
                    let sm = STATE_MACHINE.get().unwrap();
                    let audio = AUDIO_ENGINE.get().unwrap();
                    if sm.is_active.load(std::sync::atomic::Ordering::SeqCst) {
                        sm.on_deactivate(now_ms);
                        audio.play_end();
                    }
                }
            }
        }
    }
    CallNextHookEx(None, n_code, w_param, l_param)
}