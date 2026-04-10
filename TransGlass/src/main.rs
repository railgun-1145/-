use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use dashmap::DashMap;
use lazy_static::lazy_static;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::thread;
use self_update::backends::github::Update;
use self_update::Status;

// --- 核心状态注册表 ---
pub struct WindowState {
    pub original_ex_style: u32,
    pub current_alpha: u8,
    pub original_is_topmost: bool,
    pub user_pref_topmost: bool, // 用户是否选择置顶
}

lazy_static! {
    // 全局窗口状态注册表，Key 使用 isize (指针地址)
    static ref GLOBAL_REGISTRY: DashMap<isize, WindowState> = DashMap::new();
    // 存储 WinEventHook 句柄，用于退出时清理
    static ref EVENT_HOOK: AtomicIsize = AtomicIsize::new(0);
}

// --- 底层核心逻辑 ---

pub unsafe fn adjust_window_transparency(hwnd: HWND, delta: i32) -> Result<(), String> {
    if hwnd.0.is_null() { return Err("Invalid HWND".into()); }
    let hwnd_val = hwnd.0 as isize;

    // 1. 获取或初始化状态
    let mut state = if let Some(s) = GLOBAL_REGISTRY.get_mut(&hwnd_val) {
        s
    } else {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let is_top = (ex_style & WS_EX_TOPMOST.0) != 0;
        GLOBAL_REGISTRY.insert(hwnd_val, WindowState {
            original_ex_style: ex_style,
            current_alpha: 255,
            original_is_topmost: is_top,
            user_pref_topmost: is_top, // 初始跟随原状态
        });
        GLOBAL_REGISTRY.get_mut(&hwnd_val).unwrap()
    };

    // 2. 计算新透明度
    let new_alpha = (state.current_alpha as i32 + delta).clamp(30, 255) as u8;
    state.current_alpha = new_alpha;

    // 3. 应用样式
    let current_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if (current_style & WS_EX_LAYERED.0) == 0 {
        let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, (current_style | WS_EX_LAYERED.0) as i32);
    }
    SetLayeredWindowAttributes(hwnd, COLORREF(0), new_alpha, LWA_ALPHA).map_err(|e| e.to_string())?;

    // 4. 置顶联动逻辑
    apply_topmost_logic(hwnd, &state);
    
    println!("窗口 {:?} 透明度: {}%, 置顶状态: {}", 
             hwnd, (new_alpha as f32 / 255.0 * 100.0) as i32, state.user_pref_topmost);
    Ok(())
}

pub unsafe fn toggle_topmost(hwnd: HWND) {
    if let Some(mut state) = GLOBAL_REGISTRY.get_mut(&(hwnd.0 as isize)) {
        state.user_pref_topmost = !state.user_pref_topmost;
        apply_topmost_logic(hwnd, &state);
        println!("窗口 {:?} 手动置顶: {}", hwnd, state.user_pref_topmost);
    }
}

unsafe fn apply_topmost_logic(hwnd: HWND, state: &WindowState) {
    if state.user_pref_topmost {
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    } else {
        let _ = SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

pub unsafe fn restore_window(hwnd: HWND) {
    if let Some((_, state)) = GLOBAL_REGISTRY.remove(&(hwnd.0 as isize)) {
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, state.original_ex_style as i32);
        if !state.original_is_topmost {
            let _ = SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

pub unsafe fn restore_all_windows() {
    let hwnds: Vec<isize> = GLOBAL_REGISTRY.iter().map(|kv| *kv.key()).collect();
    for hwnd_val in hwnds {
        restore_window(HWND(hwnd_val as *mut _));
    }
    println!("所有窗口已恢复原状。");
}

// --- 事件钩子 ---
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
        GLOBAL_REGISTRY.remove(&(hwnd.0 as isize));
    }
}

fn main() -> Result<(), String> {
    unsafe {
        // 注册热键
        // virtual keys: Z=0x5A, X=0x58, T=0x54, R=0x52
        RegisterHotKey(None, 1, MOD_ALT, 0x5A).map_err(|e| e.to_string())?; 
        RegisterHotKey(None, 2, MOD_ALT, 0x58).map_err(|e| e.to_string())?; 
        RegisterHotKey(None, 3, MOD_ALT, 0x54).map_err(|e| e.to_string())?; 
        RegisterHotKey(None, 4, MOD_ALT, 0x52).map_err(|e| e.to_string())?; 
        RegisterHotKey(None, 5, MOD_ALT | MOD_SHIFT, 0x52).map_err(|e| e.to_string())?; 

        let hook = SetWinEventHook(
            EVENT_OBJECT_DESTROY,
            EVENT_OBJECT_DESTROY,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        EVENT_HOOK.store(hook.0 as isize, Ordering::SeqCst);

        println!("TransGlass 已启动。");
        println!("Alt + Z / X: 调节透明度");
        println!("Alt + T: 开启/关闭当前窗口置顶");
        println!("Alt + R: 还原当前窗口");
        println!("Alt + Shift + R: 一键还原所有窗口");
        println!("Alt + U: 检查并更新到最新版本");

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_HOTKEY {
                let hwnd = GetForegroundWindow();
                match msg.wParam.0 as i32 {
                    1 => { let _ = adjust_window_transparency(hwnd, -25); }
                    2 => { let _ = adjust_window_transparency(hwnd, 25); }
                    3 => { toggle_topmost(hwnd); }
                    4 => { restore_window(hwnd); }
                    5 => { restore_all_windows(); }
                    6 => { 
                        thread::spawn(|| {
                            let _ = run_self_update();
                        });
                    }
                    _ => {}
                }
            }
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }

        let _ = UnhookWinEvent(HWINEVENTHOOK(EVENT_HOOK.load(Ordering::SeqCst) as *mut _));
        restore_all_windows();
        Ok(())
    }
}

fn run_self_update() -> Result<(), Box<dyn std::error::Error>> {
    let current = env!("CARGO_PKG_VERSION");
    println!("当前版本 {}", current);
    let status = Update::configure()
        .repo_owner("railgun-1145")
        .repo_name("-")
        .bin_name("transglass")
        .show_download_progress(true)
        .current_version(current)
        .build()?
        .update()?;
    match status {
        Status::UpToDate(version) => println!("已是最新版本 {}", version),
        Status::Updated(version) => println!("已更新到版本 {}", version),
    }
    Ok(())
}
