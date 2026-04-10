use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use dashmap::DashMap;
use lazy_static::lazy_static;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::thread;
use self_update::backends::github::Update;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use eframe::egui;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

// --- 核心状态注册表 ---
#[derive(Clone)]
pub struct WindowState {
    pub original_ex_style: u32,
    pub current_alpha: u8,
    pub original_is_topmost: bool,
    pub user_pref_topmost: bool,
    pub title: String,
}

lazy_static! {
    static ref GLOBAL_REGISTRY: DashMap<isize, WindowState> = DashMap::new();
    static ref EVENT_HOOK: AtomicIsize = AtomicIsize::new(0);
}

// --- 底层核心逻辑 ---

pub unsafe fn get_window_title(hwnd: HWND) -> String {
    let mut text: [u16; 512] = [0; 512];
    let len = GetWindowTextW(hwnd, &mut text);
    if len > 0 {
        String::from_utf16_lossy(&text[..len as usize])
    } else {
        format!("未知窗口 ({:?})", hwnd.0)
    }
}

pub unsafe fn adjust_window_transparency(hwnd: HWND, delta: i32) -> Result<(), String> {
    if hwnd.0.is_null() { return Err("Invalid HWND".into()); }
    let hwnd_val = hwnd.0 as isize;

    let mut state = if let Some(s) = GLOBAL_REGISTRY.get_mut(&hwnd_val) {
        s
    } else {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let is_top = (ex_style & WS_EX_TOPMOST.0) != 0;
        let title = get_window_title(hwnd);
        GLOBAL_REGISTRY.insert(hwnd_val, WindowState {
            original_ex_style: ex_style,
            current_alpha: 255,
            original_is_topmost: is_top,
            user_pref_topmost: is_top,
            title,
        });
        GLOBAL_REGISTRY.get_mut(&hwnd_val).unwrap()
    };

    let new_alpha = (state.current_alpha as i32 + delta).clamp(30, 255) as u8;
    state.current_alpha = new_alpha;

    apply_transparency_to_hwnd(hwnd, new_alpha, state.user_pref_topmost)?;
    Ok(())
}

unsafe fn apply_transparency_to_hwnd(hwnd: HWND, alpha: u8, topmost: bool) -> Result<(), String> {
    let current_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if (current_style & WS_EX_LAYERED.0) == 0 {
        let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, (current_style | WS_EX_LAYERED.0) as i32);
    }
    SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA).map_err(|e| e.to_string())?;
    
    let pos = if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
    let _ = SetWindowPos(hwnd, pos, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    Ok(())
}

pub unsafe fn toggle_topmost(hwnd: HWND) {
    if hwnd.0.is_null() { return; }
    if let Some(mut state) = GLOBAL_REGISTRY.get_mut(&(hwnd.0 as isize)) {
        state.user_pref_topmost = !state.user_pref_topmost;
        let _ = apply_transparency_to_hwnd(hwnd, state.current_alpha, state.user_pref_topmost);
    }
}

pub unsafe fn restore_window(hwnd: HWND) {
    if hwnd.0.is_null() { return; }
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
        if !hwnd.0.is_null() {
            GLOBAL_REGISTRY.remove(&(hwnd.0 as isize));
        }
    }
}

// --- GUI 应用程序 ---
struct TransGlassApp {
    config: HotkeyConfig,
}

impl TransGlassApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // 仿 Trae 风格的深色 UI
        let mut visuals = egui::Visuals::dark();
        visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(30, 30, 30);
        visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(45, 45, 45);
        cc.egui_ctx.set_visuals(visuals);
        
        Self {
            config: load_or_create_hotkey_config(),
        }
    }
}

impl eframe::App for TransGlassApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 1. 处理托盘事件
        if let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if event.button == MouseButton::Left {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }

        // 2. 处理菜单事件
        if let Ok(event) = MenuEvent::receiver().try_recv() {
            match event.id.0.as_str() {
                "show" => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                "reset_all" => {
                    unsafe { restore_all_windows(); }
                }
                "exit" => {
                    unsafe { restore_all_windows(); }
                    std::process::exit(0);
                }
                _ => {}
            }
        }

        // 3. 拦截关闭按钮：隐藏到托盘而非退出
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        // 4. UI 绘制 (仿 Trae 简洁风格)
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                ui.heading("TransGlass");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("隐藏").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    }
                });
            });
            ui.separator();
            ui.add_space(10.0);

            // 窗口列表区
            ui.label(egui::RichText::new("管理中的窗口").strong());
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                if GLOBAL_REGISTRY.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(40.0);
                        ui.label(egui::RichText::new("暂无已调节窗口\n使用 Alt+Z/X 开始调节").weak());
                    });
                }

                let mut to_restore = None;
                for mut entry in GLOBAL_REGISTRY.iter_mut() {
                    let hwnd_val = *entry.key();
                    let state = entry.value_mut();
                    
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&state.title).truncate());
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("还原").clicked() {
                                    to_restore = Some(hwnd_val);
                                }
                            });
                        });
                        ui.horizontal(|ui| {
                            let mut alpha_f32 = state.current_alpha as f32;
                            ui.label("透明");
                            if ui.add(egui::Slider::new(&mut alpha_f32, 30.0..=255.0).show_value(false)).changed() {
                                state.current_alpha = alpha_f32 as u8;
                                unsafe {
                                    let _ = apply_transparency_to_hwnd(HWND(hwnd_val as *mut _), state.current_alpha, state.user_pref_topmost);
                                }
                            }
                            ui.checkbox(&mut state.user_pref_topmost, "置顶");
                        });
                    });
                }
                if let Some(h) = to_restore {
                    unsafe { restore_window(HWND(h as *mut _)); }
                }
            });

            ui.add_space(15.0);
            ui.separator();
            
            // 底部操作区
            ui.horizontal(|ui| {
                if ui.button("♻️ 全部还原").clicked() {
                    unsafe { restore_all_windows(); }
                }
                if ui.button("🚀 检查更新").clicked() {
                    thread::spawn(|| { let _ = run_self_update(); });
                }
            });
            
            ui.add_space(5.0);
            ui.label(egui::RichText::new("快捷键: Alt+Z/X (透明) | Alt+T (置顶)").small().weak());
            
            // 实时重绘确保热键同步
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        });
    }
}

fn create_tray_icon() -> TrayIcon {
    let tray_menu = Menu::new();
    let show_item = MenuItem::with_id("show", "显示控制面板", true, None);
    let reset_all_item = MenuItem::with_id("reset_all", "全部还原", true, None);
    let exit_item = MenuItem::with_id("exit", "退出程序", true, None);
    
    let _ = tray_menu.append_items(&[
        &show_item,
        &PredefinedMenuItem::separator(),
        &reset_all_item,
        &PredefinedMenuItem::separator(),
        &exit_item,
    ]);

    // 创建一个简单的图标 (RGBA 32x32)
    let mut rgba = Vec::with_capacity(32 * 32 * 4);
    for _ in 0..(32 * 32) {
        rgba.extend_from_slice(&[30, 144, 255, 255]); // 道奇蓝
    }
    let icon = tray_icon::Icon::from_rgba(rgba, 32, 32).unwrap();

    TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("TransGlass - 运行中")
        .with_icon(icon)
        .build()
        .unwrap()
}

fn main() -> Result<(), eframe::Error> {
    // 启动托盘图标 (生命周期随 main)
    let _tray_icon = create_tray_icon();

    // 启动热键监听线程
    thread::spawn(|| {
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
                        6 => { thread::spawn(|| { let _ = run_self_update(); }); }
                        7 => {
                            let cfg = load_or_create_hotkey_config();
                            unregister_all_hotkeys();
                            bind_hotkeys(&cfg);
                        }
                        _ => {}
                    }
                }
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            }
            let _ = UnhookWinEvent(HWINEVENTHOOK(EVENT_HOOK.load(Ordering::SeqCst) as *mut _));
            restore_all_windows();
        }
    });

    // 启动 GUI
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([320.0, 450.0])
            .with_title("TransGlass 控制面板")
            .with_visible(true),
        ..Default::default()
    };
    
    eframe::run_native(
        "TransGlass 控制面板",
        options,
        Box::new(|cc| Ok(Box::new(TransGlassApp::new(cc)))),
    )
}

fn run_self_update() -> Result<(), Box<dyn std::error::Error>> {
    let current = env!("CARGO_PKG_VERSION");
    let _ = Update::configure()
        .repo_owner("railgun-1145")
        .repo_name("TransGlass")
        .bin_name("transglass")
        .show_download_progress(true)
        .current_version(current)
        .build()?
        .update()?;
    Ok(())
}

#[derive(Deserialize, Serialize, Clone)]
struct HotkeySpec {
    modifiers: String,
    key: String,
}

#[derive(Deserialize, Serialize, Clone)]
struct HotkeyConfig {
    increase: HotkeySpec,
    decrease: HotkeySpec,
    toggle_top: HotkeySpec,
    reset_current: HotkeySpec,
    reset_all: HotkeySpec,
    update: HotkeySpec,
    reload: Option<HotkeySpec>,
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
        if ch.is_ascii_alphabetic() || ch.is_ascii_digit() {
            return ch as u32;
        }
    }
    match up.as_str() {
        "F1" => 0x70, "F2" => 0x71, "F3" => 0x72, "F4" => 0x73,
        "F5" => 0x74, "F6" => 0x75, "F7" => 0x76, "F8" => 0x77,
        "F9" => 0x78, "F10" => 0x79, "F11" => 0x7A, "F12" => 0x7B,
        _ => 0,
    }
}

unsafe fn try_register_hotkey(id: i32, spec: &HotkeySpec, _name: &str) {
    let mods = parse_modifiers(&spec.modifiers);
    let vk = parse_vk(&spec.key);
    if vk != 0 {
        let _ = RegisterHotKey(None, id, mods, vk);
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
