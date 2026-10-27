//! Connection view: discovery results, remembered consoles, manual fallback,
//! and actionable troubleshooting — the first-run experience.

use std::net::SocketAddr;

use eframe::egui;

use rpi_ng_core::model::{ConsoleStatus, DiscoverySource};

use crate::app::App;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let frame = egui::Frame::new()
        .inner_margin(egui::Margin::same(24))
        .corner_radius(8.0);
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical_centered(|ui| {
            ui.heading("Connect to your console");
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "Run the Remote Package Installer app (or a compatible backend) on your \
                     PS4 and keep it in the foreground while sending.",
                )
                .weak(),
            );
        });
        ui.add_space(12.0);

        if app.force_connect_view && matches!(app.snapshot.console.status, ConsoleStatus::Connected)
        {
            ui.horizontal(|ui| {
                ui.label("A console is already connected.");
                if ui.button("Back to the queue").clicked() {
                    app.force_connect_view = false;
                }
                if ui.button("Disconnect").clicked() {
                    app.force_connect_view = false;
                    app.send(rpi_ng_core::Command::Disconnect);
                }
            });
            ui.add_space(8.0);
        }

        status_line(app, ui);
        ui.add_space(8.0);

        candidates(app, ui);
        ui.add_space(8.0);

        remembered(app, ui);
        ui.add_space(12.0);

        manual(app, ui);
        ui.add_space(12.0);
        troubleshooting(ui);
    });
}

fn status_line(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        if app.discovery_running {
            ui.add(egui::Spinner::new());
            ui.label("Scanning the local network for consoles…");
        } else {
            ui.label(format!(
                "Found {} console(s) on the network.",
                app.discovered.len()
            ));
        }
        if ui.button("Scan again").clicked() {
            app.discovered.clear();
            app.discovery_running = true;
            app.send(rpi_ng_core::Command::Discover);
        }
    });
    if let ConsoleStatus::Incompatible(reason) = &app.snapshot.console.status {
        ui.label(
            egui::RichText::new(format!("The last address did not answer like an installer: {reason}"))
                .color(egui::Color32::from_rgb(239, 68, 68)),
        );
    }
    if matches!(app.snapshot.console.status, ConsoleStatus::Lost) {
        ui.label(
            egui::RichText::new(
                "The console stopped answering. Keep the installer app in the foreground on \
                 the console (the PS4 suspends background apps), then reconnect.",
            )
            .color(egui::Color32::from_rgb(245, 158, 11)),
        );
    }
}

fn candidates(app: &mut App, ui: &mut egui::Ui) {
    if app.discovered.is_empty() {
        return;
    }
    ui.group(|ui| {
        ui.label(egui::RichText::new("Discovered consoles").strong());
        for cand in app.discovered.clone() {
            ui.horizontal(|ui| {
                let source = match cand.source {
                    DiscoverySource::SubnetProbe => "network scan",
                    DiscoverySource::Remembered => "remembered",
                    DiscoverySource::Manual => "manual",
                };
                ui.colored_label(egui::Color32::GREEN, "●");
                ui.strong(cand.addr.to_string());
                ui.label(
                    egui::RichText::new(format!("{} · {}", cand.kind, source))
                        .small()
                        .weak(),
                );
                if ui.button("Connect").clicked() {
                    app.send(rpi_ng_core::Command::Connect(cand.addr));
                }
            });
        }
    });
}

fn remembered(app: &mut App, ui: &mut egui::Ui) {
    let remembered: Vec<SocketAddr> = app.settings.remembered.iter().map(|r| r.addr).collect();
    if remembered.is_empty() {
        return;
    }
    ui.group(|ui| {
        ui.label(egui::RichText::new("Previously used").strong());
        for addr in remembered {
            ui.horizontal(|ui| {
                ui.label(addr.to_string());
                if ui.small_button("Connect").clicked() {
                    app.send(rpi_ng_core::Command::Connect(addr));
                }
            });
        }
    });
}

fn manual(app: &mut App, ui: &mut egui::Ui) {
    ui.group(|ui| {
        ui.label(egui::RichText::new("Manual address (fallback)").strong());
        ui.horizontal(|ui| {
            let edit = egui::TextEdit::singleline(&mut app.manual_addr)
                .hint_text("192.168.1.42 or 192.168.1.42:12800")
                .desired_width(240.0);
            ui.add(edit);
            let parsed = parse_addr(&app.manual_addr);
            let enabled = parsed.is_some();
            if ui
                .add_enabled(enabled, egui::Button::new("Connect"))
                .on_disabled_hover_text("Enter a valid IP address first")
                .clicked()
                && let Some(addr) = parsed {
                    app.send(rpi_ng_core::Command::Connect(addr));
                }
        });
    });
}

fn parse_addr(text: &str) -> Option<SocketAddr> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(addr) = text.parse::<SocketAddr>() {
        return Some(addr);
    }
    if let Ok(ip) = text.parse::<std::net::IpAddr>() {
        return Some(SocketAddr::new(ip, 12800));
    }
    None
}

fn troubleshooting(ui: &mut egui::Ui) {
    egui::CollapsingHeader::new("Nothing found? Troubleshooting")
        .default_open(false)
        .show(ui, |ui| {
            ui.small(
                "• Check that the PS4 and this computer are on the same network (no guest/AP \
                 isolation).\n\
                 • On the PS4, start the Remote Package Installer app and keep it in the \
                 foreground — the PS4 suspends background apps.\n\
                 • GoldHEN users: start the installer from the GoldHEN menu, or note that \
                 some setups use port 9090 instead of 12800 — try 192.168.1.42:9090.\n\
                 • Verify the console IP in Settings → Network → View Connection Status.\n\
                 • Firewalls: this app serves files from your computer to the console; allow \
                 it on the local network when prompted.\n\
                 • Large subnets: the scan covers a bounded number of addresses — use the \
                 manual address for unusual networks.",
            );
        });
}
