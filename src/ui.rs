use crate::state_machine::TimeWindowStateMachine;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::core::w;
use std::sync::atomic::Ordering;

pub fn start_ui_thread(sm: TimeWindowStateMachine) {
    std::thread::spawn(move || {
        unsafe {
            let class_name = w!("MicYouVoiceTypingOverlay");
            let h_instance = GetModuleHandleW(None).unwrap_or_default();
            
            let wc = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: HINSTANCE(h_instance.0), // 显式转换 HMODULE 到 HINSTANCE
                lpszClassName: class_name,
                hbrBackground: HBRUSH(GetStockObject(NULL_BRUSH).0),
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);

            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                class_name,
                w!("VoiceTyping"),
                WS_POPUP,
                0, 0, 100, 30,
                HWND::default(),
                HMENU::default(),
                HINSTANCE(h_instance.0), // 显式转换 HMODULE 到 HINSTANCE
                None 
            ).unwrap_or_default();

            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 200, LWA_COLORKEY);
            ShowWindow(hwnd, SW_SHOW);

            let mut msg = MSG::default();
            loop {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                
                update_overlay_position(hwnd, sm.is_active.load(Ordering::SeqCst));
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    });
}

unsafe fn update_overlay_position(hwnd: HWND, is_active: bool) {
    let mut gui_info = GUITHREADINFO::default();
    gui_info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
    
    let gui_ok = GetGUIThreadInfo(0, &mut gui_info).is_ok();
    
    // 修复：移除 y 的 mut，因为 y 在后续逻辑中并未被重新赋值
    let (mut x, y) = if gui_ok && gui_info.hwndCaret != HWND(std::ptr::null_mut()) {
        let mut pt = POINT { x: gui_info.rcCaret.left, y: gui_info.rcCaret.bottom };
        let _ = ClientToScreen(gui_info.hwndCaret, &mut pt);
        (pt.x, pt.y + 5)
    } else {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        (pt.x + 10, pt.y + 20)
    };

    let screen_w = GetSystemMetrics(SM_CXSCREEN);
    if x + 100 > screen_w { 
        x = screen_w - 100; 
    }

    let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
    
    let hdc = GetDC(hwnd);
    let brush = CreateSolidBrush(COLORREF(if is_active { 0x0000FF } else { 0x808080 }));
    let rect = RECT { left: 0, top: 0, right: 100, bottom: 30 };
    FillRect(hdc, &rect, brush);
    ReleaseDC(hwnd, hdc);
    DeleteObject(brush);
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    match msg {
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, w_param, l_param),
    }
}