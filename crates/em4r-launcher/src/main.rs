//! EM4 Revive Launcher: pick a mod, click "Spielen", land directly in it.
#![windows_subsystem = "windows"]

mod app;
mod dds;
mod mods;
mod session;
mod settings;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("EM4 Revive")
            .with_inner_size([1120.0, 720.0])
            .with_min_inner_size([920.0, 580.0]),
        ..Default::default()
    };
    eframe::run_native("EM4 Revive", options, Box::new(|cc| Ok(Box::new(app::LauncherApp::new(cc)))))
}
