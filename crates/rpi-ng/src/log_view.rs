//! Activity log panel (bounded, colored by level).

use std::collections::VecDeque;

use eframe::egui;

use rpi_ng_core::model::LogLevel;

use crate::fmt;

pub fn show(ui: &mut egui::Ui, logs: &VecDeque<(u64, LogLevel, String)>) {
    ui.heading(egui::RichText::new("Activity").small().strong());
    ui.separator();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            if logs.is_empty() {
                ui.label(egui::RichText::new("— nothing yet —").weak());
            }
            for (ts, level, msg) in logs {
                let color = match level {
                    LogLevel::Info => ui.style().visuals.text_color(),
                    LogLevel::Warn => egui::Color32::from_rgb(245, 158, 11),
                    LogLevel::Error => egui::Color32::from_rgb(239, 68, 68),
                };
                ui.monospace(
                    egui::RichText::new(format!("{} [{level:>5}] {msg}", fmt::clock(*ts)))
                        .small()
                        .color(color),
                );
            }
        });
}
