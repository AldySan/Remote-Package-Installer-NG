//! Activity log panel (bounded, colored by level).

use std::collections::VecDeque;

use eframe::egui;

use rpi_ng_core::model::LogLevel;

use crate::{fmt, palette::Palette};

pub fn show(ui: &mut egui::Ui, logs: &VecDeque<(u64, LogLevel, String)>) {
    ui.heading(egui::RichText::new("Activity").small().strong());
    ui.separator();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            if logs.is_empty() {
                ui.label(egui::RichText::new("Nothing yet.").weak());
            }
            let p = Palette::of(ui);
            for (ts, level, msg) in logs {
                let color = p.log_level(ui, *level);
                ui.monospace(
                    egui::RichText::new(format!("{} [{level:>5}] {msg}", fmt::clock(*ts)))
                        .small()
                        .color(color),
                );
            }
        });
}
