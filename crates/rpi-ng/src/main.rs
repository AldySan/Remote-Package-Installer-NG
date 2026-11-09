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

/// Matches the binary name, the Linux `.desktop` file and its `Icon=` key.
/// X11 docks match `WM_CLASS` and Wayland compositors match the toplevel
/// `app_id` against this value (see `packaging/linux/`).
const APP_ID: &str = "remote-package-installer-ng";

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 760.0])
            .with_min_inner_size([860.0, 560.0])
            .with_title("Remote Package Installer NG")
            .with_app_id(APP_ID)
            .with_icon(window_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "Remote Package Installer NG",
        options,
        Box::new(|_cc| Ok(Box::new(app::App::new()))),
    )
}

/// Brand icon for the window title bar and taskbar. Windows and X11 read it
/// at runtime; the `.exe` file icon (Windows) and the macOS `.app` bundle
/// icon come from the build/packaging assets instead — see `docs/BUILDING.md`.
fn window_icon() -> egui::IconData {
    let bytes = include_bytes!("../assets/icon_256.png");
    let rgba = image::load_from_memory(bytes)
        .expect("embedded window icon is a valid PNG")
        .to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    egui::IconData {
        rgba: rgba.into_raw(),
        width,
        height,
    }
}
