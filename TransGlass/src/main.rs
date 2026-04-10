use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use dashmap::DashMap;
use lazy_static::lazy_static;
use std::sync::atomic::{AtomicIsize, Ordering};

// --- 核心状态注册表 ---
pub struct WindowState {
    pub original_ex_style: u32,
    pub current_alpha: u8,
    pub is_topmost: bool,
}

lazy_static! {
    static ref GLOBAL_REGISTRY: DashMap<isize, WindowState> = DashMap::new();
    static ref EVENT_HOOK: AtomicIsize = AtomicIsize::new(0);
}

// --- 底层核心逻辑 ---

pub unsafe fn adjust_window_transparency(hwnd: HWND, delta: i32) -> Result<(), String> {
    if hwnd.0 == 0 { return Err("Invalid HWND".into()); }
    let hwnd_val = hwnd.0;

    // 1. 获取或初始化状态
    let mut state = if let Some(mut s) = GLOBAL_REGISTRY.get_mut(&hwnd_val) {
        s
    } else {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        GLOBAL_REGISTRY.insert(hwnd_val, WindowState {
            original_ex_style: ex_style,
            current_alpha: 255,
            is_topmost: (ex_style & WS_EX_TOPMOST.0) != 0,
        });
        GLOBAL_REGISTRY.get_mut(&hwnd_val).unwrap()
    };

    // 2. 计算新透明度 (线性调节，步进 15)
    let new_alpha = (state.current_alpha as i32 + delta).clamp(30, 255) as u8;
    state.current_alpha = new_alpha;

    // 3. 应用样式
    let mut current_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if (current_style & WS_EX_LAYERED.0) == 0 {
        SetWindowLongW(hwnd, GWL_EXSTYLE, (current_style | WS_EX_LAYERED.0) as i32);
    }
    SetLayeredWindowAttributes(hwnd, COLORREF(0), new_alpha, LWA_ALPHA).map_err(|e| e.to_string())?;

    // 4. 置顶联动
    if new_alpha < 255 {
        SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE).ok();
    } else {
        // 如果恢复到不透明，且原本不是置顶，则还原置顶状态
        if !state.is_topmost {
            SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE).ok();
        }
    }
    
    println!("窗口 {:?} 透明度调节至: {}%", hwnd, (new_alpha as f32 / 255.0 * 100.0) as i32);
    Ok(())
}

pub unsafe fn restore_window(hwnd: HWND) {
    if let Some((_, state)) = GLOBAL_REGISTRY.remove(&hwnd.0) {
        SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA).ok();
        SetWindowLongW(hwnd, GWL_EXSTYLE, state.original_ex_style as i32);
        if !state.is_topmost {
            SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE).ok();
        }
    }
}

pub unsafe fn restore_all_windows() {
    let hwnds: Vec<isize> = GLOBAL_REGISTRY.iter().map(|kv| *kv.key()).collect();
    for hwnd_val in hwnds {
        restore_window(HWND(hwnd_val));
    }
    println!("所有窗口已恢复原状。");
}

// --- 事件回调与热键 ---

unsafe extern "system" fn win_event_proc(
    _h_win_event_hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _dw_event_thread: u32,
    _dw_ms_event_time: u32,
) {
    if event == EVENT_OBJECT_DESTROY as u32 {
        GLOBAL_REGISTRY.remove(&hwnd.0);
    }
}

fn main() -> Result<(), String> {
    unsafe {
        // 1. 注册全局热键
        // Alt + Z (增加透明度), Alt + X (减少透明度), Alt + R (重置当前), Alt + Shift + R (重置所有)
        RegisterHotKey(None, 1, MOD_ALT, 0x5A).map_err(|e| e.to_string())?; // Alt + Z (Plus)
        RegisterHotKey(None, 2, MOD_ALT, 0x58).map_err(|e| e.to_string())?; // Alt + X (Minus)
        RegisterHotKey(None, 3, MOD_ALT, 0x52).map_err(|e| e.to_string())?; // Alt + R (Reset current)
        RegisterHotKey(None, 4, MOD_ALT | MOD_SHIFT, 0x52).map_err(|e| e.to_string())?; // Alt + Shift + R (Reset all)

        // 2. 设置窗口事件钩子
        let hook = SetWinEventHook(
            EVENT_OBJECT_DESTROY,
            EVENT_OBJECT_DESTROY,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        EVENT_HOOK.store(hook.0, Ordering::SeqCst);

        println!("TransGlass 已启动。");
        println!("Alt + Z: 增加当前窗口透明度 (线性调节)");
        println!("Alt + X: 减少当前窗口透明度");
        println!("Alt + R: 还原当前窗口");
        println!("Alt + Shift + R: 一键还原所有窗口");

        // 3. 消息循环
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_HOTKEY {
                let hwnd = GetForegroundWindow();
                match msg.wParam.0 as i32 {
                    1 => { let _ = adjust_window_transparency(hwnd, -25); } // 增加透明 (减 Alpha)
                    2 => { let _ = adjust_window_transparency(hwnd, 25); }  // 减少透明 (加 Alpha)
                    3 => { restore_window(hwnd); }
                    4 => { restore_all_windows(); }
                    _ => {}
                }
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // 4. 清理
        UnhookWinEvent(hook);
        restore_all_windows();
    }
    Ok(())
}
