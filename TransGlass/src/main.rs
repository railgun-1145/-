use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Accessibility::*;
use dashmap::DashMap;
use lazy_static::lazy_static;
use std::sync::atomic::{AtomicIsize, Ordering};

// --- 核心状态注册表 ---
pub struct WindowState {
    pub original_ex_style: u32,
    pub original_alpha: u8,
    pub is_topmost: bool,
}

lazy_static! {
    // 全局窗口状态注册表
    static ref GLOBAL_REGISTRY: DashMap<isize, WindowState> = DashMap::new();
    // 存储 WinEventHook 句柄，用于退出时清理
    static ref EVENT_HOOK: AtomicIsize = AtomicIsize::new(0);
}

// --- 底层核心逻辑 ---

/// 安全设置窗口透明度并记录原始状态
pub unsafe fn set_window_transparency_safe(hwnd: HWND, alpha: u8) -> Result<(), String> {
    if hwnd.0 == 0 { return Err("Invalid HWND".into()); }
    let hwnd_val = hwnd.0;

    // 1. 记录原始状态（如果尚未记录）
    if !GLOBAL_REGISTRY.contains_key(&hwnd_val) {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let state = WindowState {
            original_ex_style: ex_style,
            original_alpha: 255,
            is_topmost: (ex_style & WS_EX_TOPMOST.0) != 0,
        };
        GLOBAL_REGISTRY.insert(hwnd_val, state);
    }

    // 2. 应用透明度样式
    let mut current_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if (current_style & WS_EX_LAYERED.0) == 0 {
        SetWindowLongW(hwnd, GWL_EXSTYLE, (current_style | WS_EX_LAYERED.0) as i32);
    }
    SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA).map_err(|e| e.to_string())?;

    // 3. 置顶联动逻辑
    if alpha < 255 {
        SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE).ok();
    } else {
        SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE).ok();
    }
    Ok(())
}

/// 还原单个窗口的状态
pub unsafe fn restore_window(hwnd: HWND) {
    if let Some((_, state)) = GLOBAL_REGISTRY.remove(&hwnd.0) {
        SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA).ok();
        SetWindowLongW(hwnd, GWL_EXSTYLE, state.original_ex_style as i32);
        if !state.is_topmost {
            SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE).ok();
        }
    }
}

/// 还原所有窗口（程序退出时调用）
pub unsafe fn restore_all_windows() {
    let hwnds: Vec<isize> = GLOBAL_REGISTRY.iter().map(|kv| *kv.key()).collect();
    for hwnd_val in hwnds {
        restore_window(HWND(hwnd_val));
    }
}

// --- 事件回调与热键 ---

/// 窗口事件回调：监听窗口销毁，防止内存泄漏
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
        if GLOBAL_REGISTRY.contains_key(&hwnd.0) {
            GLOBAL_REGISTRY.remove(&hwnd.0);
        }
    }
}

fn main() -> Result<(), String> {
    unsafe {
        // 1. 设置 DPI 感知
        // SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).ok();

        // 2. 注册全局热键 (Alt + Z)
        let hotkey_id = 1;
        RegisterHotKey(None, hotkey_id, MOD_ALT, 0x5A).map_err(|e| e.to_string())?;

        // 3. 设置窗口事件钩子 (监听销毁)
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

        println!("TransGlass 已启动。按下 Alt + Z 尝试调节当前窗口透明度。");

        // 4. 消息循环
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_HOTKEY {
                let hwnd = GetForegroundWindow();
                // 模拟：切换当前窗口为 50% 透明
                let _ = set_window_transparency_safe(hwnd, 128);
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // 5. 清理退出
        UnhookWinEvent(hook);
        restore_all_windows();
        UnregisterHotKey(None, hotkey_id).ok();
    }
    Ok(())
}
