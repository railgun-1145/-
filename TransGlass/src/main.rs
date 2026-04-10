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
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

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
        let cfg = load_or_create_hotkey_config();
        bind_hotkeys(&cfg);

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
        println!("热键已注册（可通过配置文件自定义并 Alt+Shift+C 重载）。");

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
                    7 => {
                        let cfg = load_or_create_hotkey_config();
                        unregister_all_hotkeys();
                        bind_hotkeys(&cfg);
                        println!("已重载热键配置");
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

#[derive(Deserialize, Serialize, Clone)]
struct HotkeySpec {
    modifiers: String, // e.g., "ALT", "ALT+SHIFT"
    key: String,       // e.g., "Z"
}

#[derive(Deserialize, Serialize, Clone)]
struct HotkeyConfig {
    increase: HotkeySpec,
    decrease: HotkeySpec,
    toggle_top: HotkeySpec,
    reset_current: HotkeySpec,
    reset_all: HotkeySpec,
    update: HotkeySpec,
    reload: Option<HotkeySpec>, // 可选：重载配置
}

fn default_config() -> HotkeyConfig {
    HotkeyConfig {
        increase: HotkeySpec { modifiers: "ALT".into(), key: "Z".into() },
        decrease: HotkeySpec { modifiers: "ALT".into(), key: "X".into() },
        toggle_top: HotkeySpec { modifiers: "ALT".into(), key: "T".into() },
        reset_current: HotkeySpec { modifiers: "ALT".into(), key: "R".into() },
        reset_all: HotkeySpec { modifiers: "ALT+SHIFT".into(), key: "R".into() },
        update: HotkeySpec { modifiers: "ALT".into(), key: "U".into() },
        reload: Some(HotkeySpec { modifiers: "ALT+SHIFT".into(), key: "C".into() }),
    }
}

fn get_config_path() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    exe.parent().unwrap_or_else(|| std::path::Path::new(".")).join("transglass_hotkeys.json")
}

fn load_or_create_hotkey_config() -> HotkeyConfig {
    let path = get_config_path();
    if let Ok(data) = fs::read_to_string(&path) {
        if let Ok(cfg) = serde_json::from_str::<HotkeyConfig>(&data) {
            return cfg;
        }
    }
    let cfg = default_config();
    let _ = fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap_or_default());
    cfg
}

unsafe fn parse_modifiers(s: &str) -> HOT_KEY_MODIFIERS {
    let mut m = HOT_KEY_MODIFIERS(0);
    for part in s.split('+') {
        match part.trim().to_uppercase().as_str() {
            "ALT" => m |= MOD_ALT,
            "CTRL" | "CONTROL" => m |= MOD_CONTROL,
            "SHIFT" => m |= MOD_SHIFT,
            "WIN" | "WINDOWS" => m |= MOD_WIN,
            _ => {}
        }
    }
    m
}

unsafe fn parse_vk(s: &str) -> u32 {
    let up = s.trim().to_uppercase();
    if up.len() == 1 {
        let ch = up.chars().next().unwrap();
        if ch.is_ascii_alphabetic() {
            return ch as u32;
        }
        if ch.is_ascii_digit() {
            // '0'..'9'
            return ch as u32;
        }
    }
    // 回退：尝试解析十六进制或已知名
    match up.as_str() {
        "F1" => 0x70, "F2" => 0x71, "F3" => 0x72, "F4" => 0x73,
        "F5" => 0x74, "F6" => 0x75, "F7" => 0x76, "F8" => 0x77,
        "F9" => 0x78, "F10" => 0x79, "F11" => 0x7A, "F12" => 0x7B,
        _ => 0, // 不可用时返回 0
    }
}

unsafe fn try_register_hotkey(id: i32, spec: &HotkeySpec, name: &str) {
    let mods = parse_modifiers(&spec.modifiers);
    let vk = parse_vk(&spec.key);
    if vk == 0 {
        println!("热键 {} 配置的键值无效: {}", name, spec.key);
        return;
    }
    match RegisterHotKey(None, id, mods, vk) {
        Ok(_) => println!("已注册热键 {} -> {}+{}", name, spec.modifiers, spec.key),
        Err(e) => println!("注册热键失败 {} ({}/{}): {}", name, spec.modifiers, spec.key, e.to_string()),
    }
}

unsafe fn bind_hotkeys(cfg: &HotkeyConfig) {
    try_register_hotkey(1, &cfg.increase, "Increase");
    try_register_hotkey(2, &cfg.decrease, "Decrease");
    try_register_hotkey(3, &cfg.toggle_top, "ToggleTopmost");
    try_register_hotkey(4, &cfg.reset_current, "ResetCurrent");
    try_register_hotkey(5, &cfg.reset_all, "ResetAll");
    try_register_hotkey(6, &cfg.update, "Update");
    let reload = cfg.reload.clone().unwrap_or(HotkeySpec { modifiers: "ALT+SHIFT".into(), key: "C".into() });
    try_register_hotkey(7, &reload, "ReloadConfig");
}

unsafe fn unregister_all_hotkeys() {
    for id in 1..=7 {
        let _ = UnregisterHotKey(None, id);
    }
}
