//! Settings window (persisted via the engine).
//!
//! The editable draft lives on [`App`] so it survives across frames: a
//! per-frame local clone would reset on every repaint and silently drop
//! every edit before the user could press Save (the v0.0.1 settings bug).

use eframe::egui;

use rpi_ng_core::settings::ThemeMode;

use crate::app::App;

pub fn show(app: &mut App, ctx: &egui::Context, open: &mut bool) {
    let Some(draft) = app.settings_draft.as_mut() else {
        // No draft means the window is not supposed to be open.
        *open = false;
        return;
    };
    let mut save = false;
    egui::Window::new("Settings")
        .open(open)
        .collapsible(false)
        .resizable(false)
        .min_width(420.0)
        .show(ctx, |ui| {
            egui::Grid::new("settings_grid")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Theme");
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("theme")
                            .selected_text(theme_label(draft.theme))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut draft.theme, ThemeMode::System, "System");
                                ui.selectable_value(&mut draft.theme, ThemeMode::Light, "Light");
                                ui.selectable_value(&mut draft.theme, ThemeMode::Dark, "Dark");
                            });
                    });
                    ui.end_row();

                    ui.label("File server port");
                    ui.horizontal(|ui| {
                        let mut fixed = draft.server_port.is_some();
                        if ui.checkbox(&mut fixed, "Fixed port").changed() {
                            draft.server_port = if fixed { Some(12899) } else { None };
                        }
                        if let Some(port) = &mut draft.server_port {
                            ui.add(egui::DragValue::new(port).range(1024..=65535).speed(1));
                        } else {
                            ui.label(egui::RichText::new("automatic").weak());
                        }
                    });
                    ui.end_row();

                    ui.label("Network scan");
                    ui.checkbox(
                        &mut draft.discovery_enabled,
                        "Scan for consoles automatically",
                    );
                    ui.end_row();

                    ui.label("Progress polling");
                    ui.horizontal(|ui| {
                        let mut ms = draft.poll_interval.as_millis() as i64;
                        ui.add(
                            egui::DragValue::new(&mut ms)
                                .range(250..=5000)
                                .speed(50)
                                .suffix(" ms"),
                        );
                        draft.poll_interval =
                            std::time::Duration::from_millis(ms.clamp(250, 5000) as u64);
                    });
                    ui.end_row();

                    ui.label("Folder scans");
                    ui.checkbox(
                        &mut draft.folder_scan_recursive,
                        "Include subfolders by default",
                    );
                    ui.end_row();

                    ui.label("Queue");
                    ui.checkbox(&mut draft.auto_advance, "Start the next item automatically");
                    ui.end_row();
                });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    save = true;
                }
                ui.label(
                    egui::RichText::new("The file-server port applies the next time you connect.")
                        .small()
                        .weak(),
                );
            });
        });
    if save && let Some(draft) = app.settings_draft.clone() {
        app.settings = draft.clone();
        app.send(rpi_ng_core::Command::SetSettings(Box::new(draft)));
    }
}

fn theme_label(mode: ThemeMode) -> &'static str {
    match mode {
        ThemeMode::System => "System",
        ThemeMode::Light => "Light",
        ThemeMode::Dark => "Dark",
    }
}
