use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Graphics::Gdi::*;
use dashmap::DashMap;
use lazy_static::lazy_static;

// --- 核心状态注册表 ---
// 记录每个被修改窗口的原始样式，用于安全还原
pub struct WindowState {
    pub original_ex_style: u32,
    pub original_alpha: u8,
    pub is_topmost: bool,
    pub process_name: String,
}

lazy_static! {
    // 高性能并发哈希表，全局注册表
    static ref GLOBAL_REGISTRY: DashMap<isize, WindowState> = DashMap::new();
}

/// 设置窗口透明度并记录原始状态
pub unsafe fn set_window_transparency_safe(hwnd: HWND, alpha: u8) -> Result<(), String> {
    if hwnd.0 == 0 { return Err("Invalid HWND".into()); }

    let hwnd_val = hwnd.0;
    
    // 如果注册表中没有，记录原始状态
    if !GLOBAL_REGISTRY.contains_key(&hwnd_val) {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        
        // 这里可以扩展获取进程名
        let state = WindowState {
            original_ex_style: ex_style,
            original_alpha: 255, // 默认为不透明
            is_topmost: (ex_style & WS_EX_TOPMOST.0) != 0,
            process_name: "Unknown".into(), 
        };
        GLOBAL_REGISTRY.insert(hwnd_val, state);
    }

    // --- 应用修改逻辑 ---
    let mut current_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if (current_style & WS_EX_LAYERED.0) == 0 {
        SetWindowLongW(hwnd, GWL_EXSTYLE, (current_style | WS_EX_LAYERED.0) as i32);
    }
    
    // 设置 Alpha
    SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA).map_err(|e| e.to_string())?;

    // 置顶联动逻辑
    if alpha < 255 {
        SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE).ok();
    } else {
        SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE).ok();
    }

    Ok(())
}

/// 一键还原所有受影响窗口 (Critical Cleanup)
pub unsafe fn restore_all_windows() {
    for entry in GLOBAL_REGISTRY.iter() {
        let hwnd = HWND(entry.key().clone());
        let state = entry.value();
        
        // 1. 恢复透明度为不透明
        SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA).ok();
        
        // 2. 恢复原始扩展样式（包括取消 Layered 和 置顶）
        SetWindowLongW(hwnd, GWL_EXSTYLE, state.original_ex_style as i32);
        
        // 3. 强制取消置顶 (如果原始不是置顶)
        if !state.is_topmost {
            SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE).ok();
        }
    }
    GLOBAL_REGISTRY.clear();
}

fn main() {
    // 这里将初始化 DPI 感知、热键监听等模块
    println!("TransGlass 底层核心注册表已就绪。");
}
