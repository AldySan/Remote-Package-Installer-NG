//! Queue view: toolbar, virtualized item list with live progress.

use eframe::egui;

use rpi_ng_core::model::{ItemState, ProgressView, QueueItem};

use crate::{
    app::{App, queue_summary, state_color},
    dialogs, fmt,
};

const ROW_HEIGHT: f32 = 74.0;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    toolbar(app, ui);
    ui.add_space(4.0);

    // Filter to row indices once per frame: no QueueItem clones, and the
    // search needle is lowered exactly once (not once per item).
    let needle = app.search.trim().to_lowercase();
    let indices: Vec<usize> = app
        .snapshot
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| matches_search(&needle, item))
        .map(|(idx, _)| idx)
        .collect();

    if indices.is_empty() {
        empty_state(app, ui);
        return;
    }

    // Virtualized list: only visible rows are laid out — hundreds of items
    // stay responsive.
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, ROW_HEIGHT, indices.len(), |ui, range| {
            for idx in range {
                let item = &app.snapshot.items[indices[idx]];
                item_row(app, ui, item);
                ui.separator();
            }
        });
}

fn matches_search(needle: &str, item: &QueueItem) -> bool {
    if needle.is_empty() {
        return true;
    }
    let primary = item.primary_part();
    let title = primary.map(|p| p.display_title()).unwrap_or_default();
    let file = primary.map(|p| p.file_name.clone()).unwrap_or_default();
    let title_id = primary.and_then(|p| p.title_id()).unwrap_or_default();
    let cid = item.content_id().unwrap_or_default().to_string();
    [title, file, title_id, cid]
        .iter()
        .any(|hay| hay.to_lowercase().contains(needle))
}

fn toolbar(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let busy = app.snapshot.engine_busy;
        let any_active = app.snapshot.items.iter().any(|i| i.state.is_active());

        if ui
            .add_enabled(!busy, egui::Button::new("Add PKG files…"))
            .on_hover_text("Select one or more .pkg files")
            .clicked()
        {
            app.dialog = Some(dialogs::pick_pkg_files());
        }
        if ui
            .add_enabled(!busy, egui::Button::new("Add folder…"))
            .on_hover_text("Scan a folder for packages")
            .clicked()
        {
            app.dialog = Some(dialogs::pick_folder());
        }
        ui.separator();

        let start_label = if any_active {
            "Queue running"
        } else {
            "Start queue"
        };
        if ui
            .add_enabled(!any_active && !busy, egui::Button::new(start_label))
            .on_hover_text("Install queued items in order")
            .clicked()
        {
            app.send(rpi_ng_core::Command::Start);
        }
        if ui
            .add_enabled(any_active, egui::Button::new("Pause"))
            .on_hover_text("Pause the active console task and stop auto-advance")
            .clicked()
        {
            app.send(rpi_ng_core::Command::Pause);
        }
        if ui
            .add_enabled(
                app.snapshot
                    .items
                    .iter()
                    .any(|i| i.state == ItemState::Paused),
                egui::Button::new("Resume"),
            )
            .clicked()
        {
            app.send(rpi_ng_core::Command::Resume);
        }
        ui.separator();
        if ui
            .add_enabled(
                app.snapshot.items.iter().any(|i| i.state.is_finished()),
                egui::Button::new("Clear finished"),
            )
            .clicked()
        {
            app.send(rpi_ng_core::Command::ClearFinished);
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let summary = queue_summary(&app.snapshot.items);
            ui.label(egui::RichText::new(summary).small().weak());
            ui.add(
                egui::TextEdit::singleline(&mut app.search)
                    .hint_text("Search queue…")
                    .desired_width(180.0),
            );
        });
    });
}

fn empty_state(app: &mut App, ui: &mut egui::Ui) {
    let frame = egui::Frame::new()
        .fill(ui.style().visuals.faint_bg_color)
        .inner_margin(egui::Margin::same(28))
        .corner_radius(10.0);
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical_centered(|ui| {
            ui.add_space(20.0);
            ui.heading("Drop PKG files here");
            ui.label(egui::RichText::new("…or use “Add PKG files…” / “Add folder…” above.").weak());
            ui.add_space(12.0);
            if let Some(server) = &app.snapshot.server {
                ui.label(
                    egui::RichText::new(format!(
                        "The console will download from {}",
                        server.base_url
                    ))
                    .small()
                    .weak(),
                );
            }
            ui.add_space(20.0);
        });
    });
}

fn item_row(app: &App, ui: &mut egui::Ui, item: &QueueItem) {
    let id = item.id;
    let state = item.state;
    let color = state_color(&state);
    let primary = item.primary_part();
    let title = primary
        .map(|p| p.display_title())
        .unwrap_or_else(|| "—".into());
    let size = fmt::bytes(item.total_size());

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_min_width(ui.available_width() - 240.0);
            ui.horizontal(|ui| {
                ui.strong(egui::RichText::new(title).color(ui.style().visuals.strong_text_color()));
                if item.is_multipart() {
                    ui.label(
                        egui::RichText::new(format!("{} parts", item.parts.len()))
                            .small()
                            .weak(),
                    );
                }
                if let Some(p) = primary.and_then(|p| p.pkg.as_ref()) {
                    if p.is_patch {
                        ui.label(
                            egui::RichText::new("patch")
                                .small()
                                .color(egui::Color32::from_rgb(245, 158, 11)),
                        );
                    }
                    if let Some(kind) = p.content_kind.package_type() {
                        ui.label(egui::RichText::new(kind).small().weak());
                    }
                }
            });
            ui.horizontal(|ui| {
                if let Some(t) = primary.and_then(|p| p.title_id()) {
                    ui.monospace(egui::RichText::new(t).small());
                }
                if let Some(cid) = item.content_id() {
                    ui.monospace(egui::RichText::new(cid).small().weak());
                }
            });
            ui.label(egui::RichText::new(size).small().weak());
        });
        ui.vertical(|ui| {
            ui.set_width(230.0);
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                state_and_progress(ui, item, color);
            });
        });
    });

    // Error / hint line.
    if let Some(err) = &item.error {
        ui.label(
            egui::RichText::new(err)
                .small()
                .color(egui::Color32::from_rgb(239, 68, 68)),
        );
    }

    // Actions.
    ui.horizontal(|ui| {
        let active = state.is_active();
        if state == ItemState::Paused {
            if ui.small_button("Resume").clicked() {
                app.send(rpi_ng_core::Command::ResumeItem(id));
            }
        } else if active && ui.small_button("Pause").clicked() {
            app.send(rpi_ng_core::Command::Pause);
        }
        if state == ItemState::Unknown
            && ui
                .small_button("Mark completed")
                .on_hover_text("You verified on the console that it finished")
                .clicked()
        {
            app.send(rpi_ng_core::Command::MarkCompleted(id));
        }
        if (state.is_finished() || state == ItemState::Unknown)
            && ui.small_button("Retry").clicked()
        {
            app.send(rpi_ng_core::Command::Retry(id));
        }
        if !active {
            if ui.small_button("Remove").clicked() {
                app.send(rpi_ng_core::Command::Remove(id));
            }
        } else if ui.small_button("Cancel").clicked() {
            app.send(rpi_ng_core::Command::Cancel(id));
        }
        if !active {
            if ui.small_button("Up").clicked() {
                app.send(rpi_ng_core::Command::Move { id, delta: -1 });
            }
            if ui.small_button("Down").clicked() {
                app.send(rpi_ng_core::Command::Move { id, delta: 1 });
            }
        }
    });
}

fn state_and_progress(ui: &mut egui::Ui, item: &QueueItem, color: egui::Color32) {
    ui.horizontal(|ui| {
        ui.colored_label(color, item.state.label());
        if let Some(p) = &item.progress {
            if item.state == ItemState::Transferring || item.state == ItemState::Installing {
                let pct = p.percent;
                let bar = egui::ProgressBar::new(pct.clamp(0.0, 100.0) / 100.0)
                    .text(format!(
                        "{pct:.1}% · {}{}",
                        fmt::bytes(p.transferred),
                        eta_suffix(p),
                    ))
                    .desired_height(14.0);
                ui.add(bar);
                let speed = if p.speed_bps > 0 {
                    format!(" · {}", fmt::bps(p.speed_bps))
                } else {
                    String::new()
                };
                ui.label(
                    egui::RichText::new(format!(
                        "{} / {}{speed}",
                        fmt::bytes(p.transferred),
                        fmt::bytes(p.total)
                    ))
                    .small()
                    .weak(),
                );
            } else if p.preparing_percent > 0 && p.preparing_percent < 100 {
                ui.label(
                    egui::RichText::new(format!("preparing {}%", p.preparing_percent))
                        .small()
                        .weak(),
                );
            }
        } else if item.state == ItemState::Submitting {
            ui.add(egui::Spinner::new());
        }
    });
}

fn eta_suffix(p: &ProgressView) -> String {
    match p.eta_sec {
        Some(sec) if sec > 0 => format!(" · {}", fmt::duration(sec)),
        _ => String::new(),
    }
}
