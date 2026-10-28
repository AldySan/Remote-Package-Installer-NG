//! Remote Package Installer NG — desktop application entry point.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// Release builds on Windows open no console window (the GUI is the app).
// Debug builds keep the console so `cargo run` output stays visible.

mod app;
mod connect_view;
mod dialogs;
mod fmt;
mod log_view;
mod queue_view;
mod servercheck;
mod settings_view;
mod updater;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 760.0])
            .with_min_inner_size([860.0, 560.0])
            .with_title("Remote Package Installer NG"),
        ..Default::default()
    };
    eframe::run_native(
        "Remote Package Installer NG",
        options,
        Box::new(|_cc| Ok(Box::new(app::App::new()))),
    )
}
