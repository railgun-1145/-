#![windows_subsystem = "windows"]

use dashmap::DashMap;
use eframe::egui;
use lazy_static::lazy_static;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use self_update::backends::github::Update;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::RwLock;
use std::thread;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use windows::Win32::Foundation::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// 日志系统
use log::{debug, info, warn, error};
use env_logger;

// 线程池
use rayon;

// UI 重绘节流常量
const REPAINT_THROTTLE: Duration = Duration::from_millis(50); // ~20 FPS，减少重绘频率

// --- 核心状态注册表 ---
#[derive(Clone, Default)]
struct WindowState {
    original_ex_style: u32,
    current_alpha: u8,
    original_is_topmost: bool,
    user_pref_topmost: bool,
    mouse_passthrough: bool,
    pen_passthrough: bool,
    title: String,
}

// 优化：使用更轻量的同步原语
lazy_static! {
    static ref GLOBAL_REGISTRY: DashMap<isize, WindowState> = DashMap::new();
    static ref MOUSE_BINDINGS: RwLock<MouseBindings> = RwLock::new(MouseBindings::default());
    static ref PASSTHROUGH_DRAG: Mutex<Option<PassthroughDragState>> = Mutex::new(None);
}

// 快速检查是否有拖拽状态进行中（原子变量，避免锁操作）
static HAS_ACTIVE_DRAG: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
struct PendingChange {
    hwnd_val: isize,
    alpha: Option<u8>,
    topmost: Option<bool>,
    mouse_passthrough: Option<bool>,
    pen_passthrough: Option<bool>,
}

/// 部分点透模式下，由本程序接管的按下拖拽序列（用于转发 MOVE / UP）。
#[derive(Clone, Copy)]
struct PassthroughDragState {
    root_val: isize,
    buttons: u8,
}

const WM_TRANSGLASS_SHUTDOWN: u32 = WM_USER + 88;
const DRAG_BTN_LEFT: u8 = 1;
const DRAG_BTN_RIGHT: u8 = 2;
const DRAG_BTN_MIDDLE: u8 = 4;

static EGUI_CTX: OnceLock<egui::Context> = OnceLock::new();
static WINDOW_VISIBLE: AtomicBool = AtomicBool::new(true);
static EXITING: AtomicBool = AtomicBool::new(false);
static ROOT_HWND: AtomicIsize = AtomicIsize::new(0);
static MOUSE_HOOK: AtomicIsize = AtomicIsize::new(0);
static MOUSE_HOOK_FAILED: AtomicBool = AtomicBool::new(false);
static HOTKEY_THREAD_ID: AtomicU32 = AtomicU32::new(0);
const TRANSG_GLASS_INJECT_EXTRA_INFO: usize = 0x5452474Cu64 as usize;

#[derive(Clone, Copy, PartialEq, Eq)]
enum MouseAction {
    None,
    Increase,
    Decrease,
    ToggleTopmost,
    ToggleClickThrough,
    TogglePenPassthrough,
    ResetCurrent,
    ResetAll,
    Update,
}

#[derive(Clone, Copy)]
struct MouseBindings {
    xbutton1: MouseAction,
    xbutton2: MouseAction,
}

impl Default for MouseBindings {
    fn default() -> Self {
        Self {
            xbutton1: MouseAction::Decrease,
            xbutton2: MouseAction::Increase,
        }
    }
}

use std::time::{Instant, Duration};
use std::sync::LazyLock;

// 用于 UI 更新节流的时间戳
static LAST_REPAINT: LazyLock<Mutex<Instant>> = LazyLock::new(|| Mutex::new(Instant::now()));

fn request_ui_repaint() {
    if let Some(ctx) = EGUI_CTX.get() {
        // 实现简单的节流，避免过于频繁的重绘
        if let Ok(mut last) = LAST_REPAINT.try_lock() {
            let now = Instant::now();
            if now - *last >= REPAINT_THROTTLE {
                ctx.request_repaint();
                *last = now;
            }
        }
    }
}

fn parse_mouse_action(s: &str) -> MouseAction {
    match s.trim().to_lowercase().as_str() {
        "increase" | "inc" | "up" => MouseAction::Increase,
        "decrease" | "dec" | "down" => MouseAction::Decrease,
        "toggle_topmost" | "topmost" | "toggle_top" => MouseAction::ToggleTopmost,
        "toggle_click_through" | "toggle_mouse_passthrough" | "click_through" | "toggle_click" => {
            MouseAction::ToggleClickThrough
        }
        "toggle_pen_passthrough" | "pen_passthrough" => MouseAction::TogglePenPassthrough,
        "reset_current" | "reset" => MouseAction::ResetCurrent,
        "reset_all" => MouseAction::ResetAll,
        "update" => MouseAction::Update,
        _ => MouseAction::None,
    }
}

fn set_mouse_bindings(cfg: &HotkeyConfig) {
    let mut b = MouseBindings::default();
    if let Some(spec) = cfg.mouse.as_ref() {
        if let Some(s) = spec.xbutton1.as_ref() {
            b.xbutton1 = parse_mouse_action(s);
        }
        if let Some(s) = spec.xbutton2.as_ref() {
            b.xbutton2 = parse_mouse_action(s);
        }
    }
    if let Ok(mut w) = MOUSE_BINDINGS.write() {
        *w = b;
    }
}

struct ExStyleGuard {
    hwnd: HWND,
    orig: u32,
}

impl ExStyleGuard {
    unsafe fn add_transparent(hwnd: HWND) -> Self {
        let orig = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, (orig | WS_EX_TRANSPARENT.0) as i32);
        Self { hwnd, orig }
    }
}

impl Drop for ExStyleGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = SetWindowLongW(self.hwnd, GWL_EXSTYLE, self.orig as i32);
        }
    }
}

unsafe fn screen_metrics() -> (i32, i32) {
    let cx = (GetSystemMetrics(SM_CXSCREEN) - 1).max(1);
    let cy = (GetSystemMetrics(SM_CYSCREEN) - 1).max(1);
    (cx, cy)
}

fn button_drag_mask(msg: u32) -> Option<u8> {
    match msg {
        WM_LBUTTONDOWN | WM_LBUTTONUP => Some(DRAG_BTN_LEFT),
        WM_RBUTTONDOWN | WM_RBUTTONUP => Some(DRAG_BTN_RIGHT),
        WM_MBUTTONDOWN | WM_MBUTTONUP => Some(DRAG_BTN_MIDDLE),
        _ => None,
    }
}

fn is_button_down_msg(msg: u32) -> bool {
    matches!(
        msg,
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN
    )
}

fn is_button_up_msg(msg: u32) -> bool {
    matches!(
        msg,
        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP
    )
}

unsafe fn inject_mouse_button_at_pt(pt: POINT, msg: u32) -> bool {
    let (cx, cy) = screen_metrics();
    let btn = match msg {
        WM_LBUTTONDOWN => MOUSEEVENTF_LEFTDOWN,
        WM_LBUTTONUP => MOUSEEVENTF_LEFTUP,
        WM_RBUTTONDOWN => MOUSEEVENTF_RIGHTDOWN,
        WM_RBUTTONUP => MOUSEEVENTF_RIGHTUP,
        WM_MBUTTONDOWN => MOUSEEVENTF_MIDDLEDOWN,
        WM_MBUTTONUP => MOUSEEVENTF_MIDDLEUP,
        _ => MOUSE_EVENT_FLAGS(0),
    };
    let flags = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE | btn;
    let inp = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: (pt.x * 65535) / cx,
                dy: (pt.y * 65535) / cy,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: TRANSG_GLASS_INJECT_EXTRA_INFO,
            },
        },
    };
    SendInput(&[inp], std::mem::size_of::<INPUT>() as i32) != 0
}

unsafe fn inject_mouse_move_at_pt(pt: POINT) -> bool {
    let (cx, cy) = screen_metrics();
    let inp = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: (pt.x * 65535) / cx,
                dy: (pt.y * 65535) / cy,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE,
                time: 0,
                dwExtraInfo: TRANSG_GLASS_INJECT_EXTRA_INFO,
            },
        },
    };
    SendInput(&[inp], std::mem::size_of::<INPUT>() as i32) != 0
}

unsafe fn inject_mouse_wheel_delta(wheel_delta: i32) -> bool {
    let inp = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: wheel_delta as u32,
                dwFlags: MOUSEEVENTF_WHEEL,
                time: 0,
                dwExtraInfo: TRANSG_GLASS_INJECT_EXTRA_INFO,
            },
        },
    };
    SendInput(&[inp], std::mem::size_of::<INPUT>() as i32) != 0
}

unsafe fn install_mouse_hook() {
    if MOUSE_HOOK.load(Ordering::SeqCst) != 0 {
        return;
    }
    match SetWindowsHookExW(
        WH_MOUSE_LL,
        Some(mouse_hook_proc),
        HINSTANCE(std::ptr::null_mut()),
        0,
    ) {
        Ok(hook) if !hook.0.is_null() => {
            MOUSE_HOOK.store(hook.0 as isize, Ordering::SeqCst);
        }
        _ => {
            MOUSE_HOOK_FAILED.store(true, Ordering::SeqCst);
        }
    }
}

unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return CallNextHookEx(
            HHOOK(MOUSE_HOOK.load(Ordering::SeqCst) as *mut _),
            code,
            wparam,
            lparam,
        );
    }

    let msg = wparam.0 as u32;
    let info = *(lparam.0 as *const MSLLHOOKSTRUCT);

    // 快速跳过本程序注入的事件
    if info.dwExtraInfo == TRANSG_GLASS_INJECT_EXTRA_INFO {
        return CallNextHookEx(
            HHOOK(MOUSE_HOOK.load(Ordering::SeqCst) as *mut _),
            code,
            wparam,
            lparam,
        );
    }

    let pt = POINT {
        x: info.pt.x,
        y: info.pt.y,
    };

    // 快速检查是否是TransGlass自身窗口
    let hit = WindowFromPoint(pt);
    if !hit.0.is_null() {
        let root = GetAncestor(hit, GA_ROOT);
        if is_own_hwnd(root) {
            // 允许拖动TransGlass窗口（非置顶时）
            if msg == WM_LBUTTONDOWN {
                // 检查是否非置顶
                let current_style = GetWindowLongW(root, GWL_EXSTYLE) as u32;
                let is_topmost = (current_style & WS_EX_TOPMOST.0) != 0;
                if !is_topmost {
                    // 启动窗口拖动
                    let _ = ReleaseCapture();
                    SendMessageW(root, WM_SYSCOMMAND, WPARAM((SC_MOVE | HTCAPTION) as usize), LPARAM(0));
                    return LRESULT(1);
                }
            }
            // 对于TransGlass自身窗口，直接传递事件
            return CallNextHookEx(
                HHOOK(MOUSE_HOOK.load(Ordering::SeqCst) as *mut _),
                code,
                wparam,
                lparam,
            );
        }
    }

    // 1. 处理拖拽结束 - 只在按键抬起时处理
    if is_button_up_msg(msg) {
        // 快速检查是否有拖拽状态，避免不必要的锁
        if HAS_ACTIVE_DRAG.load(Ordering::Relaxed) {
            if let Some(mask) = button_drag_mask(msg) {
                if let Ok(mut guard) = PASSTHROUGH_DRAG.try_lock() {
                    if let Some(ref mut st) = *guard {
                        if st.buttons & mask != 0 {
                            let root = HWND(st.root_val as *mut _);
                            let _g = ExStyleGuard::add_transparent(root);
                            let _ = inject_mouse_button_at_pt(pt, msg);
                            st.buttons &= !mask;
                            if st.buttons == 0 {
                                *guard = None;
                                HAS_ACTIVE_DRAG.store(false, Ordering::Relaxed);
                            }
                            return LRESULT(1);
                        }
                    }
                }
            }
        }
    }

    // 2. 处理拖拽中的移动 - 高频事件，需要优化
    if msg == WM_MOUSEMOVE {
        // 快速检查是否有拖拽状态
        if HAS_ACTIVE_DRAG.load(Ordering::Relaxed) {
            // 使用 try_lock 避免阻塞
            if let Ok(guard) = PASSTHROUGH_DRAG.try_lock() {
                if let Some(ref st) = *guard {
                    if st.buttons != 0 {
                        let _ = inject_mouse_move_at_pt(pt);
                        return LRESULT(1);
                    }
                }
            }
        }
    }

    // 3. 处理侧键点击 - 低频事件
    if msg == WM_XBUTTONDOWN {
        let button = ((info.mouseData >> 16) & 0xffff) as u16;
        // 快速获取鼠标绑定，避免不必要的锁
        let action = if let Ok(r) = MOUSE_BINDINGS.try_read() {
            match button {
                1 => r.xbutton1,
                2 => r.xbutton2,
                _ => MouseAction::None,
            }
        } else {
            MouseAction::None
        };
        if action != MouseAction::None {
            // 使用线程池处理耗时操作，避免阻塞钩子
            rayon::spawn(move || {
                // 在子线程中重新获取窗口句柄，避免跨线程传递HWND的风险
                let hwnd = GetForegroundWindow();
                if hwnd.0.is_null() {
                    return;
                }
                match action {
                    MouseAction::Increase => {
                        let _ = adjust_window_transparency(hwnd, 25);
                    }
                    MouseAction::Decrease => {
                        let _ = adjust_window_transparency(hwnd, -25);
                    }
                    MouseAction::ToggleTopmost => {
                        toggle_topmost(hwnd);
                    }
                    MouseAction::ToggleClickThrough => {
                        toggle_mouse_passthrough(hwnd);
                    }
                    MouseAction::TogglePenPassthrough => {
                        toggle_pen_passthrough(hwnd);
                    }
                    MouseAction::ResetCurrent => {
                        restore_window(hwnd);
                    }
                    MouseAction::ResetAll => {
                        restore_all_windows();
                    }
                    MouseAction::Update => {
                        let _ = run_self_update();
                    }
                    MouseAction::None => {}
                }
            });
            return LRESULT(1);
        }
    }

    // 4. 处理鼠标滚轮 - 中频事件
    if msg == WM_MOUSEWHEEL {
        let hit = WindowFromPoint(pt);
        if !hit.0.is_null() {
            let root = GetAncestor(hit, GA_ROOT);
            let root_val = root.0 as isize;
            // 快速查找窗口状态
            if let Some(state) = GLOBAL_REGISTRY.get(&root_val) {
                let all = state.mouse_passthrough && state.pen_passthrough;
                if state.mouse_passthrough && !all {
                    let delta = (((info.mouseData >> 16) & 0xFFFF) as u16 as i16) as i32;
                    let _g = ExStyleGuard::add_transparent(root);
                    let _ = inject_mouse_wheel_delta(delta);
                    return LRESULT(1);
                }
            }
        }
    }

    // 5. 处理鼠标按键事件 - 中频事件
    if matches!(
        msg,
        WM_LBUTTONDOWN
            | WM_LBUTTONUP
            | WM_RBUTTONDOWN
            | WM_RBUTTONUP
            | WM_MBUTTONDOWN
            | WM_MBUTTONUP
    ) {
        let hit = WindowFromPoint(pt);
        if !hit.0.is_null() {
            let root = GetAncestor(hit, GA_ROOT);
            let root_val = root.0 as isize;
            
            // 快速查找窗口状态
            if let Some(state) = GLOBAL_REGISTRY.get(&root_val) {
                let is_pen = (info.dwExtraInfo & 0xFFFFFF00) == 0xFF515700;
                let wants = if is_pen {
                    state.pen_passthrough
                } else {
                    state.mouse_passthrough
                };
                let all = state.mouse_passthrough && state.pen_passthrough;
                if wants && !all && is_button_down_msg(msg) {
                    debug!("Processing {} event for window {:?}, is_pen: {}", 
                        if is_pen { "pen" } else { "mouse" }, root_val, is_pen);
                    let _g = ExStyleGuard::add_transparent(root);
                    if inject_mouse_button_at_pt(pt, msg) {
                        if let Some(mask) = button_drag_mask(msg) {
                            // 使用 try_lock 避免阻塞
                            if let Ok(mut g) = PASSTHROUGH_DRAG.try_lock() {
                                match *g {
                                    Some(ref mut st) if st.root_val == root_val => {
                                        st.buttons |= mask;
                                    }
                                    _ => {
                                        *g = Some(PassthroughDragState {
                                            root_val,
                                            buttons: mask,
                                        });
                                        HAS_ACTIVE_DRAG.store(true, Ordering::Relaxed);
                                        debug!("Started drag state for window: {}", root_val);
                                    }
                                }
                            } else {
                                warn!("Failed to acquire drag lock");
                            }
                        }
                        return LRESULT(1);
                    } else {
                        warn!("Failed to inject mouse button event");
                    }
                }
            }
        }
    }

    CallNextHookEx(
        HHOOK(MOUSE_HOOK.load(Ordering::SeqCst) as *mut _),
        code,
        wparam,
        lparam,
    )
}

fn show_root_window() {
    WINDOW_VISIBLE.store(true, Ordering::Relaxed);
    if let Some(ctx) = EGUI_CTX.get() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    }
    let hwnd_val = ROOT_HWND.load(Ordering::Relaxed);
    if hwnd_val != 0 {
        let hwnd = HWND(hwnd_val as *mut _);
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
    }
    if let Some(ctx) = EGUI_CTX.get() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }
}

fn hide_root_window() {
    WINDOW_VISIBLE.store(false, Ordering::Relaxed);
    if let Some(ctx) = EGUI_CTX.get() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }
    let hwnd_val = ROOT_HWND.load(Ordering::Relaxed);
    if hwnd_val != 0 {
        let hwnd = HWND(hwnd_val as *mut _);
        unsafe {
            let _ = ShowWindow(hwnd, SW_MINIMIZE);
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
    if let Some(ctx) = EGUI_CTX.get() {
        ctx.request_repaint();
    }
}

// --- 底层核心逻辑 ---

unsafe fn get_window_title(hwnd: HWND) -> String {
    let mut text: [u16; 512] = [0; 512];
    let len = GetWindowTextW(hwnd, &mut text);
    if len > 0 {
        String::from_utf16_lossy(&text[..len as usize])
    } else {
        format!("未知窗口 ({:?})", hwnd.0)
    }
}

unsafe fn adjust_window_transparency(hwnd: HWND, delta: i32) -> Result<(), String> {
    if hwnd.0.is_null() {
        return Err("Invalid HWND".into());
    }
    if is_own_hwnd(hwnd) {
        return Ok(());
    }
    let hwnd_val = hwnd.0 as isize;

    let mut state = if let Some(s) = GLOBAL_REGISTRY.get_mut(&hwnd_val) {
        s
    } else {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let is_top = (ex_style & WS_EX_TOPMOST.0) != 0;
        let title = get_window_title(hwnd);
        GLOBAL_REGISTRY.insert(
            hwnd_val,
            WindowState {
                original_ex_style: ex_style,
                current_alpha: 255,
                original_is_topmost: is_top,
                user_pref_topmost: is_top,
                mouse_passthrough: false,
                pen_passthrough: false,
                title,
            },
        );
        GLOBAL_REGISTRY.get_mut(&hwnd_val).unwrap()
    };

    let new_alpha = (state.current_alpha as i32 + delta).clamp(30, 255) as u8;
    state.current_alpha = new_alpha;

    apply_transparency_to_hwnd(
        hwnd,
        new_alpha,
        state.user_pref_topmost,
        state.mouse_passthrough,
        state.pen_passthrough,
    )?;
    request_ui_repaint();
    Ok(())
}

unsafe fn apply_transparency_to_hwnd(
    hwnd: HWND,
    alpha: u8,
    topmost: bool,
    mouse_passthrough: bool,
    pen_passthrough: bool,
) -> Result<(), String> {
    let current_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    let mut next_style = current_style | WS_EX_LAYERED.0;
    if mouse_passthrough && pen_passthrough {
        next_style |= WS_EX_TRANSPARENT.0;
    } else {
        next_style &= !WS_EX_TRANSPARENT.0;
    }
    if next_style != current_style {
        let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, next_style as i32);
    }
    SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA).map_err(|e| e.to_string())?;

    let pos = if topmost {
        HWND_TOPMOST
    } else {
        HWND_NOTOPMOST
    };
    let _ = SetWindowPos(
        hwnd,
        pos,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS | SWP_FRAMECHANGED,
    );
    Ok(())
}

unsafe fn toggle_topmost(hwnd: HWND) {
    if hwnd.0.is_null() {
        return;
    }
    if is_own_hwnd(hwnd) {
        return;
    }
    let hwnd_val = hwnd.0 as isize;
    if GLOBAL_REGISTRY.get(&hwnd_val).is_none() {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let is_top = (ex_style & WS_EX_TOPMOST.0) != 0;
        let title = get_window_title(hwnd);
        GLOBAL_REGISTRY.insert(
            hwnd_val,
            WindowState {
                original_ex_style: ex_style,
                current_alpha: 255,
                original_is_topmost: is_top,
                user_pref_topmost: is_top,
                mouse_passthrough: false,
                pen_passthrough: false,
                title,
            },
        );
    }
    if let Some(mut state) = GLOBAL_REGISTRY.get_mut(&hwnd_val) {
        state.user_pref_topmost = !state.user_pref_topmost;
        let _ = apply_transparency_to_hwnd(
            hwnd,
            state.current_alpha,
            state.user_pref_topmost,
            state.mouse_passthrough,
            state.pen_passthrough,
        );
    }
    request_ui_repaint();
}

unsafe fn toggle_mouse_passthrough(hwnd: HWND) {
    if hwnd.0.is_null() {
        return;
    }
    if is_own_hwnd(hwnd) {
        return;
    }
    let hwnd_val = hwnd.0 as isize;
    if GLOBAL_REGISTRY.get(&hwnd_val).is_none() {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let is_top = (ex_style & WS_EX_TOPMOST.0) != 0;
        let title = get_window_title(hwnd);
        GLOBAL_REGISTRY.insert(
            hwnd_val,
            WindowState {
                original_ex_style: ex_style,
                current_alpha: 255,
                original_is_topmost: is_top,
                user_pref_topmost: is_top,
                mouse_passthrough: false,
                pen_passthrough: false,
                title,
            },
        );
    }
    if let Some(mut state) = GLOBAL_REGISTRY.get_mut(&hwnd_val) {
        state.mouse_passthrough = !state.mouse_passthrough;
        let _ = apply_transparency_to_hwnd(
            hwnd,
            state.current_alpha,
            state.user_pref_topmost,
            state.mouse_passthrough,
            state.pen_passthrough,
        );
    }
    request_ui_repaint();
}

unsafe fn toggle_pen_passthrough(hwnd: HWND) {
    if hwnd.0.is_null() {
        return;
    }
    if is_own_hwnd(hwnd) {
        return;
    }
    let hwnd_val = hwnd.0 as isize;
    if GLOBAL_REGISTRY.get(&hwnd_val).is_none() {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let is_top = (ex_style & WS_EX_TOPMOST.0) != 0;
        let title = get_window_title(hwnd);
        GLOBAL_REGISTRY.insert(
            hwnd_val,
            WindowState {
                original_ex_style: ex_style,
                current_alpha: 255,
                original_is_topmost: is_top,
                user_pref_topmost: is_top,
                mouse_passthrough: false,
                pen_passthrough: false,
                title,
            },
        );
    }
    if let Some(mut state) = GLOBAL_REGISTRY.get_mut(&hwnd_val) {
        state.pen_passthrough = !state.pen_passthrough;
        let _ = apply_transparency_to_hwnd(
            hwnd,
            state.current_alpha,
            state.user_pref_topmost,
            state.mouse_passthrough,
            state.pen_passthrough,
        );
    }
    request_ui_repaint();
}

unsafe fn restore_window(hwnd: HWND) {
    if hwnd.0.is_null() {
        return;
    }
    let v = hwnd.0 as isize;
    if let Ok(mut g) = PASSTHROUGH_DRAG.lock() {
        if let Some(st) = *g {
            if st.root_val == v {
                *g = None;
                HAS_ACTIVE_DRAG.store(false, Ordering::Relaxed);
            }
        }
    }
    if let Some((_, state)) = GLOBAL_REGISTRY.remove(&v) {
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, state.original_ex_style as i32);
        if !state.original_is_topmost {
            let _ = SetWindowPos(
                hwnd,
                HWND_NOTOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            );
        }
    }
    request_ui_repaint();
}

unsafe fn restore_all_windows() {
    if let Ok(mut g) = PASSTHROUGH_DRAG.lock() {
        *g = None;
        HAS_ACTIVE_DRAG.store(false, Ordering::Relaxed);
    }
    let hwnds: Vec<isize> = GLOBAL_REGISTRY.iter().map(|kv| *kv.key()).collect();
    for hwnd_val in hwnds {
        restore_window(HWND(hwnd_val as *mut _));
    }
    request_ui_repaint();
}

unsafe fn is_own_hwnd(hwnd: HWND) -> bool {
    if hwnd.0.is_null() {
        return false;
    }
    let mut pid: u32 = 0;
    let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
    pid != 0 && pid == GetCurrentProcessId()
}

// --- 事件钩子 ---
// --- GUI 应用程序 ---
struct TransGlassApp {
    should_exit: bool,
}

impl TransGlassApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // 1. 设置中文字体 (尝试多个常用路径)
        let mut fonts = egui::FontDefinitions::default();
        let font_paths = [
            "C:\\Windows\\Fonts\\simhei.ttf", // 黑体 (TTF)
            "C:\\Windows\\Fonts\\simkai.ttf", // 楷体 (TTF)
            "C:\\Windows\\Fonts\\msyh.ttc",   // 微软雅黑 (TTC)
            "C:\\Windows\\Fonts\\msyh.ttf",
            "C:\\Windows\\Fonts\\simsun.ttc", // 宋体 (TTC)
            "C:\\Windows\\Fonts\\simsun.ttf",
        ];

        let mut font_loaded = false;
        for path in font_paths {
            if let Ok(font_data) = std::fs::read(path) {
                fonts
                    .font_data
                    .insert("my_font".to_owned(), egui::FontData::from_owned(font_data));
                fonts
                    .families
                    .get_mut(&egui::FontFamily::Proportional)
                    .unwrap()
                    .insert(0, "my_font".to_owned());
                fonts
                    .families
                    .get_mut(&egui::FontFamily::Monospace)
                    .unwrap()
                    .push("my_font".to_owned());
                font_loaded = true;
                break;
            }
        }

        let _ = font_loaded;
        cc.egui_ctx.set_fonts(fonts);

        // 2. 仿 Trae 风格的深色 UI
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = egui::Color32::from_rgb(18, 18, 18);
        visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(24, 24, 24);
        visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(32, 32, 32);
        visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(45, 45, 45);
        visuals.widgets.active.bg_fill = egui::Color32::from_rgb(55, 55, 55);
        visuals.selection.bg_fill = egui::Color32::from_rgb(0, 150, 255);
        visuals.window_rounding = egui::Rounding::same(8.0);
        cc.egui_ctx.set_visuals(visuals);
        let _ = EGUI_CTX.set(cc.egui_ctx.clone());

        if let Ok(handle) = cc.window_handle() {
            if let RawWindowHandle::Win32(h) = handle.as_raw() {
                ROOT_HWND.store(h.hwnd.get(), Ordering::Relaxed);
            }
        }

        Self { should_exit: false }
    }
}

impl eframe::App for TransGlassApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 1. 处理托盘和菜单事件 (逻辑保持不变)

        // 2. 拦截关闭
        if ctx.input(|i| i.viewport().close_requested()) && !self.should_exit {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            hide_root_window();
        }
        
        // 3. 处理真正的退出
        if self.should_exit {
            unsafe {
                restore_all_windows();
                let h = MOUSE_HOOK.swap(0, Ordering::SeqCst);
                if h != 0 {
                    let _ = UnhookWindowsHookEx(HHOOK(h as *mut _));
                }
                unregister_all_hotkeys();
                ExitProcess(0);
            }
        }

        if ctx.input(|i| i.viewport().minimized.unwrap_or(false)) {
            WINDOW_VISIBLE.store(false, Ordering::Relaxed);
        }

        // 3. UI 绘制 (更简洁现代的布局)
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.heading(egui::RichText::new("TransGlass").color(egui::Color32::from_rgb(0, 150, 255)).strong().size(22.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(" 🗙 隐藏 ").clicked() {
                        hide_root_window();
                    }
                });
            });
            if MOUSE_HOOK_FAILED.load(Ordering::Relaxed) {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "警告：低级鼠标钩子未能安装，「仅鼠标/仅笔点透」模式将无法工作（双点透全开仍可用）。",
                    )
                    .color(egui::Color32::from_rgb(255, 170, 70))
                    .small(),
                );
            }
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(10.0);

            // 正在管理的窗口
            ui.label(egui::RichText::new("已调节的窗口").strong().color(egui::Color32::LIGHT_GRAY));
            ui.add_space(5.0);

            egui::ScrollArea::vertical()
                .max_height(250.0)
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    if GLOBAL_REGISTRY.is_empty() {
                        ui.vertical_centered(|ui| {
                            ui.add_space(60.0);
                            ui.label(egui::RichText::new("暂无管理记录\n使用热键开始管理").weak());
                        });
                    } else {
                        // 优化：只遍历一次注册表，减少克隆操作
                        let mut to_restore: Vec<isize> = Vec::new();
                        let mut changes: Vec<PendingChange> = Vec::new();

                        // 预先分配空间，减少内存分配
                        to_restore.reserve(GLOBAL_REGISTRY.len());
                        changes.reserve(GLOBAL_REGISTRY.len());

                        for entry in GLOBAL_REGISTRY.iter() {
                            let hwnd_val = *entry.key();
                            let state = entry.value();

                            // 跳过自己的窗口
                            if unsafe { is_own_hwnd(HWND(hwnd_val as *mut _)) } {
                                continue;
                            }

                            ui.add_space(4.0);
                            ui.group(|ui| {
                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.add(egui::Label::new(egui::RichText::new(&state.title).strong().size(14.0)).truncate());
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui.button("还原").clicked() {
                                                to_restore.push(hwnd_val);
                                            }
                                        });
                                    });

                                    ui.add_space(6.0);
                                    ui.horizontal(|ui| {
                                        ui.label("透明度:");
                                        let mut alpha_f32 = state.current_alpha as f32;
                                        let slider = egui::Slider::new(&mut alpha_f32, 30.0..=255.0)
                                            .show_value(false)
                                            .trailing_fill(true);
                                        if ui.add(slider).changed() {
                                            changes.push(PendingChange {
                                                hwnd_val,
                                                alpha: Some(alpha_f32 as u8),
                                                topmost: None,
                                                mouse_passthrough: None,
                                                pen_passthrough: None,
                                            });
                                        }
                                        ui.add_space(10.0);
                                        let mut topmost = state.user_pref_topmost;
                                        if ui.checkbox(&mut topmost, "置顶").changed() {
                                            changes.push(PendingChange {
                                                hwnd_val,
                                                alpha: None,
                                                topmost: Some(topmost),
                                                mouse_passthrough: None,
                                                pen_passthrough: None,
                                            });
                                        }
                                        ui.add_space(10.0);
                                        let mut mouse_passthrough = state.mouse_passthrough;
                                        if ui.checkbox(&mut mouse_passthrough, "鼠标点透").changed() {
                                            changes.push(PendingChange {
                                                hwnd_val,
                                                alpha: None,
                                                topmost: None,
                                                mouse_passthrough: Some(mouse_passthrough),
                                                pen_passthrough: None,
                                            });
                                        }
                                        ui.add_space(10.0);
                                        let mut pen_passthrough = state.pen_passthrough;
                                        if ui.checkbox(&mut pen_passthrough, "笔点透").changed() {
                                            changes.push(PendingChange {
                                                hwnd_val,
                                                alpha: None,
                                                topmost: None,
                                                mouse_passthrough: None,
                                                pen_passthrough: Some(pen_passthrough),
                                            });
                                        }
                                    });
                                });
                            });
                        }

                        // 批量处理还原操作
                        for hwnd_val in to_restore {
                            unsafe { restore_window(HWND(hwnd_val as *mut _)); }
                        }

                        // 批量处理变更操作
                        for c in changes {
                            if let Some(mut state) = GLOBAL_REGISTRY.get_mut(&c.hwnd_val) {
                                // 应用变更
                                if let Some(a) = c.alpha {
                                    state.current_alpha = a;
                                }
                                if let Some(t) = c.topmost {
                                    state.user_pref_topmost = t;
                                }
                                if let Some(m) = c.mouse_passthrough {
                                    state.mouse_passthrough = m;
                                }
                                if let Some(p) = c.pen_passthrough {
                                    state.pen_passthrough = p;
                                }
                                // 一次性应用所有变更
                                unsafe {
                                    let _ = apply_transparency_to_hwnd(
                                        HWND(c.hwnd_val as *mut _),
                                        state.current_alpha,
                                        state.user_pref_topmost,
                                        state.mouse_passthrough,
                                        state.pen_passthrough
                                    );
                                }
                            }
                        }
                    }
                });

            ui.add_space(15.0);
            ui.separator();
            ui.add_space(10.0);

            // 底部控制
            ui.horizontal(|ui| {
                if ui.button("♻ 全部还原").clicked() {
                    unsafe { restore_all_windows(); }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("🚀 检查更新").clicked() {
                        thread::spawn(|| { let _ = run_self_update(); });
                    }
                });
            });

            ui.add_space(12.0);
            ui.group(|ui| {
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("快捷键提示").strong().size(12.0));
                    ui.label(egui::RichText::new("Alt + Z/X: 调节透明度 | Alt + T: 窗口置顶 | Alt + P: 鼠标点透 | Alt + Shift + P: 笔点透\nAlt + R: 还原当前窗口 | Alt + Shift + R: 还原全部").small().weak());
                });
            });
        });
    }
}

fn create_tray_icon() -> TrayIcon {
    let tray_menu = Menu::new();
    let show_item = MenuItem::with_id("show", "打开 TransGlass", true, None);
    let reset_all_item = MenuItem::with_id("reset_all", "全部窗口还原", true, None);
    let exit_item = MenuItem::with_id("exit", "退出程序", true, None);

    let _ = tray_menu.append_items(&[
        &show_item,
        &PredefinedMenuItem::separator(),
        &reset_all_item,
        &PredefinedMenuItem::separator(),
        &exit_item,
    ]);

    // 加载自定义图标
    let icon = load_custom_icon().unwrap_or_else(|| {
        let mut rgba = Vec::with_capacity(32 * 32 * 4);
        for y in 0..32 {
            for x in 0..32 {
                let dx = x as f32 - 15.5;
                let dy = y as f32 - 15.5;
                let dist = (dx * dx + dy * dy).sqrt();
                if dist < 14.0 {
                    if dist > 11.0 {
                        // 外圈 (更深的蓝色)
                        rgba.extend_from_slice(&[0, 120, 215, 255]);
                    } else {
                        // 内圈 (半透明蓝色，模仿玻璃)
                        rgba.extend_from_slice(&[0, 150, 255, 128]);
                    }
                } else {
                    rgba.extend_from_slice(&[0, 0, 0, 0]);
                }
            }
        }
        tray_icon::Icon::from_rgba(rgba, 32, 32).unwrap()
    });

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("TransGlass - 运行中")
        .with_icon(icon)
        .build()
        .unwrap();

    tray.set_show_menu_on_left_click(false);
    tray
}

fn load_custom_icon() -> Option<tray_icon::Icon> {
    // 优先级：icon2.png (用户指定的第二张图片) -> icon.png -> tray_icon.png
    let paths = [
        "icon2.png",
        "icon.png",
        "tray_icon.png",
    ];

    // 只检查可执行文件所在目录，减少文件系统操作
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for rel in paths {
                let candidate = dir.join(rel);
                if let Ok(img) = image::open(&candidate) {
                    let rgba = img.to_rgba8();
                    let (width, height) = rgba.dimensions();
                    if let Ok(icon) = tray_icon::Icon::from_rgba(rgba.into_raw(), width, height) {
                        return Some(icon);
                    }
                }
            }
        }
    }
    None
}

fn main() -> Result<(), eframe::Error> {
    // 初始化日志系统
    env_logger::init();
    info!("TransGlass v0.2.0 starting...");
    
    unsafe {
        let _ = windows::Win32::System::Console::FreeConsole();
    }

    // 1. 首先创建托盘图标，这样用户可以立即看到程序正在运行
    info!("Creating tray icon...");
    let _tray_icon = create_tray_icon();
    info!("Tray icon created successfully");

    // 2. 启动菜单事件处理线程
    thread::spawn(|| {
        while let Ok(event) = MenuEvent::receiver().recv() {
            match event.id.0.as_str() {
                "show" => {
                    show_root_window();
                }
                "reset_all" => unsafe { restore_all_windows() },
                "exit" => {
                    if EXITING.swap(true, Ordering::SeqCst) {
                        continue;
                    }
                    unsafe { restore_all_windows() };
                    let tid = HOTKEY_THREAD_ID.load(Ordering::SeqCst);
                    if tid != 0 {
                        unsafe {
                            let _ = PostThreadMessageW(tid, WM_TRANSGLASS_SHUTDOWN, WPARAM(0), LPARAM(0));
                        }
                    } else {
                        thread::sleep(std::time::Duration::from_millis(120));
                        unsafe { ExitProcess(0) };
                    }
                }
                _ => {}
            }
        }
    });

    // 3. 启动托盘点击事件处理线程
    thread::spawn(|| {
        while let Ok(event) = TrayIconEvent::receiver().recv() {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            ) {
                show_root_window();
            }
        }
    });

    // 4. 启动热键和鼠标钩子线程（优先级较低，避免阻塞启动）
    thread::spawn(|| unsafe {
        HOTKEY_THREAD_ID.store(GetCurrentThreadId(), Ordering::SeqCst);
        info!("Hotkey and mouse hook thread started");

        // 延迟加载配置，避免阻塞启动
        thread::sleep(std::time::Duration::from_millis(100));
        
        info!("Loading hotkey configuration...");
        let cfg = load_or_create_hotkey_config();
        set_mouse_bindings(&cfg);
        bind_hotkeys(&cfg);
        info!("Installing mouse hook...");
        install_mouse_hook();
        if MOUSE_HOOK.load(Ordering::SeqCst) != 0 {
            info!("Mouse hook installed successfully");
        } else {
            error!("Failed to install mouse hook");
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_TRANSGLASS_SHUTDOWN {
                info!("Received shutdown message");
                let h = MOUSE_HOOK.swap(0, Ordering::SeqCst);
                if h != 0 {
                    info!("Unhooking mouse hook");
                    let _ = UnhookWindowsHookEx(HHOOK(h as *mut _));
                }
                info!("Unregistering hotkeys");
                unregister_all_hotkeys();
                info!("Restoring all windows");
                restore_all_windows();
                info!("Exiting process");
                ExitProcess(0);
            }
            if msg.message == WM_HOTKEY {
                let hwnd = GetForegroundWindow();
                match msg.wParam.0 as i32 {
                    1 => {
                        let _ = adjust_window_transparency(hwnd, -25);
                    }
                    2 => {
                        let _ = adjust_window_transparency(hwnd, 25);
                    }
                    3 => {
                        toggle_topmost(hwnd);
                    }
                    4 => {
                        toggle_mouse_passthrough(hwnd);
                    }
                    5 => {
                        toggle_pen_passthrough(hwnd);
                    }
                    6 => {
                        restore_window(hwnd);
                    }
                    7 => {
                        restore_all_windows();
                    }
                    8 => {
                        thread::spawn(|| {
                            let _ = run_self_update();
                        });
                    }
                    9 => {
                        let cfg = load_or_create_hotkey_config();
                        unregister_all_hotkeys();
                        set_mouse_bindings(&cfg);
                        bind_hotkeys(&cfg);
                    }
                    _ => {}
                }
            }
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
        let h = MOUSE_HOOK.swap(0, Ordering::SeqCst);
        if h != 0 {
            let _ = UnhookWindowsHookEx(HHOOK(h as *mut _));
        }
        unregister_all_hotkeys();
        restore_all_windows();
    });

    // 5. 最后启动 GUI，这样用户可以尽快看到界面
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([320.0, 450.0])
            .with_title("TransGlass 控制面板")
            .with_visible(true),
        run_and_return: false,
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
struct MouseBindingsSpec {
    xbutton1: Option<String>,
    xbutton2: Option<String>,
}

#[derive(Deserialize, Serialize, Clone)]
struct HotkeyConfig {
    increase: HotkeySpec,
    decrease: HotkeySpec,
    toggle_top: HotkeySpec,
    toggle_click_through: Option<HotkeySpec>,
    toggle_pen_passthrough: Option<HotkeySpec>,
    reset_current: HotkeySpec,
    reset_all: HotkeySpec,
    update: HotkeySpec,
    reload: Option<HotkeySpec>,
    mouse: Option<MouseBindingsSpec>,
}

fn default_config() -> HotkeyConfig {
    HotkeyConfig {
        increase: HotkeySpec {
            modifiers: "ALT".into(),
            key: "Z".into(),
        },
        decrease: HotkeySpec {
            modifiers: "ALT".into(),
            key: "X".into(),
        },
        toggle_top: HotkeySpec {
            modifiers: "ALT".into(),
            key: "T".into(),
        },
        toggle_click_through: Some(HotkeySpec {
            modifiers: "ALT".into(),
            key: "P".into(),
        }),
        toggle_pen_passthrough: Some(HotkeySpec {
            modifiers: "ALT+SHIFT".into(),
            key: "P".into(),
        }),
        reset_current: HotkeySpec {
            modifiers: "ALT".into(),
            key: "R".into(),
        },
        reset_all: HotkeySpec {
            modifiers: "ALT+SHIFT".into(),
            key: "R".into(),
        },
        update: HotkeySpec {
            modifiers: "ALT".into(),
            key: "U".into(),
        },
        reload: Some(HotkeySpec {
            modifiers: "ALT+SHIFT".into(),
            key: "C".into(),
        }),
        mouse: Some(MouseBindingsSpec {
            xbutton1: Some("decrease".into()),
            xbutton2: Some("increase".into()),
        }),
    }
}

fn get_config_path() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    exe.parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("transglass_hotkeys.json")
}

fn load_or_create_hotkey_config() -> HotkeyConfig {
    let path = get_config_path();
    if let Ok(data) = fs::read_to_string(&path) {
        if let Ok(cfg) = serde_json::from_str::<HotkeyConfig>(&data) {
            return cfg;
        }
    }
    let cfg = default_config();
    let _ = fs::write(
        &path,
        serde_json::to_string_pretty(&cfg).unwrap_or_default(),
    );
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
        "F1" => 0x70,
        "F2" => 0x71,
        "F3" => 0x72,
        "F4" => 0x73,
        "F5" => 0x74,
        "F6" => 0x75,
        "F7" => 0x76,
        "F8" => 0x77,
        "F9" => 0x78,
        "F10" => 0x79,
        "F11" => 0x7A,
        "F12" => 0x7B,
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
    let toggle_click = cfg.toggle_click_through.clone().unwrap_or(HotkeySpec {
        modifiers: "ALT".into(),
        key: "P".into(),
    });
    try_register_hotkey(4, &toggle_click, "ToggleClickThrough");
    let toggle_pen = cfg.toggle_pen_passthrough.clone().unwrap_or(HotkeySpec {
        modifiers: "ALT+SHIFT".into(),
        key: "P".into(),
    });
    try_register_hotkey(5, &toggle_pen, "TogglePenPassthrough");
    try_register_hotkey(6, &cfg.reset_current, "ResetCurrent");
    try_register_hotkey(7, &cfg.reset_all, "ResetAll");
    try_register_hotkey(8, &cfg.update, "Update");
    let reload = cfg.reload.clone().unwrap_or(HotkeySpec {
        modifiers: "ALT+SHIFT".into(),
        key: "C".into(),
    });
    try_register_hotkey(9, &reload, "ReloadConfig");
}

unsafe fn unregister_all_hotkeys() {
    for id in 1..=9 {
        let _ = UnregisterHotKey(None, id);
    }
}
