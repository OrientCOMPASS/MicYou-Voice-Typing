use crate::{bus_parser::WdisMessage, state_machine::TimeWindowStateMachine};
use crossbeam_channel::Receiver;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub fn run_typer_worker(rx: Receiver<WdisMessage>, sm: TimeWindowStateMachine) {
    let mut typer = Typer::new();
    for msg in rx {
        if sm.should_inject(msg.start_ms, msg.end_ms) {
            typer.inject(&msg.text);
        }
    }
}

struct Typer {
    last_char_was_alphanumeric: bool,
}

impl Typer {
    fn new() -> Self {
        Self { last_char_was_alphanumeric: false }
    }

    fn inject(&mut self, text: &str) {
        unsafe {
            let _current_hwnd = GetForegroundWindow();
            
            let mut inputs = Vec::new();
            
            if self.last_char_was_alphanumeric && text.chars().next().map_or(false, |c| c.is_alphanumeric()) {
                inputs.extend(self.create_key_inputs(' ' as u16));
            }

            for c in text.chars() {
                let mut buf = [0u16; 2];
                let encoded = c.encode_utf16(&mut buf);
                for &word in encoded.iter() {
                    inputs.extend(self.create_key_inputs(word));
                }
                self.last_char_was_alphanumeric = c.is_alphanumeric();
            }

            if !inputs.is_empty() {
                SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
            }
        }
    }

    unsafe fn create_key_inputs(&self, scan_code: u16) -> Vec<INPUT> {
        let down = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: scan_code,
                    dwFlags: KEYEVENTF_UNICODE,
                    time: 0,
                    dwExtraInfo: 0,
                }
            }
        };

        let mut up = down;
        up.Anonymous.ki.dwFlags |= KEYEVENTF_KEYUP;

        vec![down, up]
    }
}