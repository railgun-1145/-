#![windows_subsystem = "windows"]

use eframe::egui;
use std::thread;
use tray_icon::{menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem}, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};

fn create_tray_icon() -> Option<TrayIcon> {
    let quit = MenuItem::new("退出", true, None);
    let menu = Menu::new();
    menu.append_items(&[&quit]);

    let icon = include_bytes!("../icon.ico");
    match TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("TransGlass")
        .with_icon_from_buffer(icon)
        .build()
    {
        Ok(tray) => {
            thread::spawn(|| {
                while let Ok(event) = MenuEvent::receiver().recv() {
                    if event.id == quit.id() {
                        std::process::exit(0);
                    }
                }
            });
            Some(tray)
        }
        Err(_) => None,
    }
}

struct TransGlassApp {
    should_exit: bool,
}

impl eframe::App for TransGlassApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("TransGlass 测试版");
            ui.label("这是一个最小化的测试版本");
            if ui.button("退出").clicked() {
                self.should_exit = true;
            }
        });

        if self.should_exit {
            ctx.request_repaint();
            std::process::exit(0);
        }
    }
}

fn main() -> Result<(), eframe::Error> {
    // 创建托盘图标
    let _tray_icon = create_tray_icon();

    // 启动 GUI
    let options = eframe::NativeOptions {
        initial_window_size: Some(egui::vec2(400.0, 300.0)),
        ..Default::default()
    };

    eframe::run_native(
        "TransGlass",
        options,
        Box::new(|_| Box::<TransGlassApp>::default()),
    )
}
