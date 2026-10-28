//! Settings window (persisted via the engine).
//!
//! The editable draft lives on [`App`] so it survives across frames: a
//! per-frame local clone would reset on every repaint and silently drop
//! every edit before the user could press Save (the v0.0.1 settings bug).

use eframe::egui;

use rpi_ng_core::{
    fileserver::ServerCheck,
    settings::{FailurePolicy, ThemeMode},
};

use crate::app::App;

pub fn show(app: &mut App, ctx: &egui::Context, open: &mut bool) {
    let Some(draft) = app.settings_draft.as_mut() else {
        // No draft means the window is not supposed to be open.
        *open = false;
        return;
    };
    // Values the window body needs from `app` without fighting the draft
    // borrow (the closure only captures `draft` + these copies).
    let server_port = app.snapshot.server.as_ref().map(|s| s.bind.port());
    let check_busy = app.server_checker.busy;
    let check_result = app.server_check_result.clone();
    let mut save = false;
    let mut kick_check: Option<u16> = None;
    egui::Window::new("Settings")
        .open(open)
        .collapsible(false)
        .resizable(false)
        .min_width(460.0)
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

                    ui.label("File server");
                    ui.horizontal(|ui| {
                        let label = if check_busy {
                            "Checking…"
                        } else {
                            "Check File Server"
                        };
                        if ui
                            .add_enabled(server_port.is_some(), egui::Button::new(label))
                            .on_disabled_hover_text(
                                "The file server starts when you connect to a console — \
                                 connect first, then check",
                            )
                            .on_hover_text(
                                "Test whether the app's own file server is answering on this \
                                 computer — if it is OK but the console still cannot download, the \
                                 block is between the two machines (firewall / network)",
                            )
                            .clicked()
                        {
                            kick_check = server_port;
                        }
                        if check_busy {
                            ui.add(egui::Spinner::new());
                        } else if let Some(result) = &check_result {
                            match result {
                                ServerCheck::Responded(code) => ui.colored_label(
                                    egui::Color32::from_rgb(34, 197, 94),
                                    format!("OK (HTTP {code})"),
                                ),
                                ServerCheck::NotRunning => {
                                    ui.colored_label(egui::Color32::GRAY, "not running")
                                }
                                ServerCheck::NoAnswer => ui.colored_label(
                                    egui::Color32::from_rgb(245, 158, 11),
                                    "no answer",
                                ),
                                ServerCheck::Error(_) => ui.colored_label(
                                    egui::Color32::from_rgb(239, 68, 68),
                                    "check failed",
                                ),
                            };
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
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut draft.auto_advance, "Start the next item automatically");
                    });
                    ui.end_row();

                    ui.label("When a transfer fails");
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("on_failure")
                            .selected_text(failure_label(draft.on_failure))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut draft.on_failure,
                                    FailurePolicy::Continue,
                                    "Continue with the next package",
                                );
                                ui.selectable_value(
                                    &mut draft.on_failure,
                                    FailurePolicy::Pause,
                                    "Pause the queue",
                                );
                            });
                    });
                    ui.end_row();

                    ui.label("Updates");
                    ui.horizontal(|ui| {
                        ui.checkbox(
                            &mut draft.auto_check_updates,
                            "Check for updates on startup",
                        );
                    });
                    ui.end_row();
                });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    save = true;
                }
            });

            // Footnotes: full width, regular size, quiet color — visible when
            // you look for them, silent when you don't. (v0.0.2 had these
            // crammed next to Save in `.small()`, which users couldn't read.)
            ui.add_space(6.0);
            ui.separator();
            ui.label(
                egui::RichText::new("The file-server port applies the next time you connect.")
                    .weak(),
            );
            ui.label(
                egui::RichText::new(
                    "The update check only asks GitHub for the latest release tag — \
                     nothing else is sent.",
                )
                .weak(),
            );
        });
    if save && let Some(draft) = app.settings_draft.clone() {
        app.settings = draft.clone();
        app.send(rpi_ng_core::Command::SetSettings(Box::new(draft)));
    }
    if let Some(port) = kick_check {
        app.server_checker.kick(port);
    }
}

fn theme_label(mode: ThemeMode) -> &'static str {
    match mode {
        ThemeMode::System => "System",
        ThemeMode::Light => "Light",
        ThemeMode::Dark => "Dark",
    }
}

fn failure_label(policy: FailurePolicy) -> &'static str {
    match policy {
        FailurePolicy::Continue => "Continue with the next package",
        FailurePolicy::Pause => "Pause the queue",
    }
}
