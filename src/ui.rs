use crate::state_machine::TimeWindowStateMachine;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::{SafeArrayAccessData, SafeArrayGetLBound, SafeArrayGetUBound, SafeArrayUnaccessData};
use windows::core::w;
use std::sync::atomic::Ordering;

pub fn start_ui_thread(sm: TimeWindowStateMachine) {
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            
            let class_name = w!("MicYouVoiceTypingOverlay");
            let h_instance = GetModuleHandleW(None).unwrap_or_default();
            
            let wc = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: HINSTANCE(h_instance.0),
                lpszClassName: class_name,
                hbrBackground: HBRUSH(GetStockObject(NULL_BRUSH).0),
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);

            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                class_name, w!("VoiceTyping"), WS_POPUP,
                0, 0, 32, 32, HWND::default(), HMENU::default(), HINSTANCE(h_instance.0), None 
            ).unwrap_or_default();

            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_COLORKEY);
            let _ = ShowWindow(hwnd, SW_HIDE);

            let uia: IUIAutomation = match CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
                Ok(u) => u,
                Err(_) => { loop { std::thread::sleep(std::time::Duration::from_secs(1)); } }
            };

            let mut msg = MSG::default();
            loop {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                
                let is_active = sm.is_active.load(Ordering::SeqCst);
                update_overlay_position(hwnd, &uia, is_active);
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    });
}

unsafe fn update_overlay_position(hwnd: HWND, uia: &IUIAutomation, is_active: bool) {
    // 核心优化：如果插件未处于激活状态，直接隐藏并跳过 UIA 轮询，节省性能
    if !is_active {
        let _ = ShowWindow(hwnd, SW_HIDE);
        return;
    }

    let mut visible = false;
    let mut x = 0;
    let mut y = 0;

    // 仅在激活状态下，才去检测当前焦点是否处于文本输入状态
    if let Ok(focused) = uia.GetFocusedElement() {
        if let Ok(tp) = focused.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) {
            if let Ok(selection) = tp.GetSelection() {
                if let Ok(range) = selection.GetElement(0) {
                    if let Ok(rects) = range.GetBoundingRectangles() {
                        if !rects.is_null() {
                            let mut p_data: *mut std::ffi::c_void = std::ptr::null_mut();
                            if SafeArrayAccessData(rects, &mut p_data).is_ok() {
                                let data_ptr = p_data as *const f64;
                                let lbound = SafeArrayGetLBound(rects, 1).unwrap_or(0);
                                let ubound = SafeArrayGetUBound(rects, 1).unwrap_or(-1);
                                let len = if ubound >= lbound { (ubound - lbound + 1) as usize } else { 0 };
                                
                                if len >= 4 {
                                    x = *data_ptr.add(0) as i32;
                                    y = (*data_ptr.add(1) + *data_ptr.add(3)) as i32 + 5; 
                                    visible = true;
                                }
                                let _ = SafeArrayUnaccessData(rects);
                            }
                        }
                    }
                }
            }
        }
    }

    if visible {
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        if x + 32 > screen_w { x = screen_w - 32; }

        let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
        let _ = ShowWindow(hwnd, SW_SHOWNA);
        
        // 因为只有在 active 时才会走到这里，所以直接绘制固定颜色的图标
        draw_mic_icon(hwnd);
    } else {
        // 处于激活状态，但当前焦点不在文本框内（如在桌面或纯图片浏览器），则隐藏
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
}

unsafe fn draw_mic_icon(hwnd: HWND) {
    let hdc = GetDC(hwnd);
    
    // 固定使用醒目的红色 (RGB: 220, 50, 50) 代表正在录音/激活状态
    let color = 0x3232DC; 
    let pen = CreatePen(PS_SOLID, 2, COLORREF(color));
    let brush = CreateSolidBrush(COLORREF(color));
    
    let old_pen = SelectObject(hdc, pen);
    let old_brush = SelectObject(hdc, brush);

    // 麦克风头 (圆角矩形/胶囊)
    let _ = RoundRect(hdc, 12, 4, 20, 18, 8, 8);
    
    // U型支架
    let _ = Arc(hdc, 8, 10, 24, 24, 8, 16, 24, 16);
    
    // 竖线
    let _ = MoveToEx(hdc, 16, 22, None);
    let _ = LineTo(hdc, 16, 27);
    
    // 底座横线
    let _ = MoveToEx(hdc, 12, 27, None);
    let _ = LineTo(hdc, 20, 27);

    let _ = SelectObject(hdc, old_pen);
    let _ = SelectObject(hdc, old_brush);
    let _ = DeleteObject(pen);
    let _ = DeleteObject(brush);
    let _ = ReleaseDC(hwnd, hdc);
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    match msg {
        WM_DESTROY => { PostQuitMessage(0); LRESULT(0) }
        _ => DefWindowProcW(hwnd, msg, w_param, l_param),
    }
}