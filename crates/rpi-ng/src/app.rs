//! Top-level application: layout, engine wiring, first-run experience.

use std::{collections::VecDeque, path::PathBuf, time::Duration};

use eframe::egui;

use rpi_ng_core::{
    model::{ConsoleCandidate, ConsoleStatus, Event, ItemState, LogLevel, QueueItem, Snapshot},
    settings::{Settings, ThemeMode},
    Command, EngineHandle,
};

use crate::{connect_view, dialogs, log_view, queue_view, settings_view};

const MAX_LOG_LINES: usize = 500;

pub struct App {
    engine: Option<EngineHandle>,
    pub(crate) settings: Settings,
    pub(crate) snapshot: Snapshot,
    pub(crate) logs: VecDeque<(u64, LogLevel, String)>,
    /// Latest discovery results (empty until a scan finishes).
    pub(crate) discovered: Vec<ConsoleCandidate>,
    pub(crate) discovery_running: bool,
    pub(crate) manual_addr: String,
    pub(crate) search: String,
    show_settings: bool,
    show_log: bool,
    show_about: bool,
    pub(crate) dialog: Option<dialogs::DialogPick>,
    /// Folder awaiting a recursive/flat choice.
    pub(crate) pending_folder: Option<PathBuf>,
    /// Show the connection screen even though a console is connected.
    pub(crate) force_connect_view: bool,
}

impl App {
    pub fn new() -> Self {
        let settings = Settings::load();
        let auto_discovery = settings.discovery_enabled;
        let engine = rpi_ng_core::spawn(settings.clone());
        // Kick off a discovery sweep on first run if enabled.
        if auto_discovery {
            engine.send(Command::Discover);
        }
        Self {
            engine: Some(engine),
            settings,
            snapshot: Snapshot::default(),
            logs: VecDeque::new(),
            discovered: Vec::new(),
            discovery_running: auto_discovery,
            manual_addr: String::new(),
            search: String::new(),
            show_settings: false,
            show_log: true,
            show_about: false,
            dialog: None,
            pending_folder: None,
            force_connect_view: false,
        }
    }

    fn engine(&self) -> &EngineHandle {
        self.engine.as_ref().expect("engine lives until on_exit")
    }

    pub(crate) fn send(&self, cmd: Command) {
        self.engine().send(cmd);
    }

    fn apply_theme(&self, ctx: &egui::Context) {
        let pref = match self.settings.theme {
            ThemeMode::System => egui::ThemePreference::System,
            ThemeMode::Light => egui::ThemePreference::Light,
            ThemeMode::Dark => egui::ThemePreference::Dark,
        };
        ctx.set_theme(pref);
    }

    fn poll_engine(&mut self) -> bool {
        let mut busy = false;
        for ev in self.engine().poll_events() {
            match ev {
                Event::Snapshot(s) => {
                    busy |= s.engine_busy;
                    self.snapshot = s;
                }
                Event::Log(level, msg) => {
                    self.logs.push_back((rpi_ng_core::model::unix_ms(), level, msg));
                    while self.logs.len() > MAX_LOG_LINES {
                        self.logs.pop_front();
                    }
                }
                Event::DiscoveryDone(found) => {
                    self.discovered = found;
                    self.discovery_running = false;
                }
            }
        }
        busy
    }

    fn poll_dialog(&mut self) {
        let Some(dialog) = &self.dialog else { return };
        let Some(outcome) = dialog.try_result() else { return };
        self.dialog = None;
        match outcome {
            dialogs::DialogOutcome::Files(files) => {
                self.send(Command::AddPaths { paths: files, recursive: false });
            }
            dialogs::DialogOutcome::Folder(path) => {
                self.pending_folder = Some(path);
            }
            dialogs::DialogOutcome::Cancelled => {}
        }
    }

    fn poll_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx
            .input(|i| i.raw.dropped_files.clone())
            .into_iter()
            .map(|f| f.path().to_path_buf())
            .collect();
        if dropped.is_empty() {
            return;
        }
        let files: Vec<PathBuf> = dropped.iter().filter(|p| p.is_file()).cloned().collect();
        if let Some(first_dir) = dropped.iter().find(|p| p.is_dir()) {
            self.pending_folder = Some(first_dir.clone());
        }
        if !files.is_empty() {
            self.send(Command::AddPaths { paths: files, recursive: false });
        }
    }

    fn folder_choice(&mut self, ctx: &egui::Context) {
        let Some(path) = self.pending_folder.clone() else { return };
        // None = still deciding, Some(None) = cancelled, Some(Some(rec)) = scan.
        let mut action: Option<Option<bool>> = None;
        egui::Window::new("Add folder")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!("Add packages from:\n{}", path.display()));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Scan subfolders").clicked() {
                        action = Some(Some(true));
                    }
                    if ui.button("Scan top level only").clicked() {
                        action = Some(Some(false));
                    }
                    if ui.button("Cancel").clicked() {
                        action = Some(None);
                    }
                });
            });
        match action {
            Some(Some(recursive)) => {
                self.pending_folder = None;
                self.send(Command::AddPaths { paths: vec![path], recursive });
            }
            Some(None) => self.pending_folder = None,
            None => {}
        }
    }

    fn connected(&self) -> bool {
        matches!(self.snapshot.console.status, ConsoleStatus::Connected)
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.apply_theme(ui.ctx());
        let busy = self.poll_engine();
        self.poll_dialog();
        self.poll_dropped_files(ui.ctx());
        self.folder_choice(ui.ctx());

        egui::Panel::top("top_bar").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.strong("Remote Package Installer NG");
                ui.separator();
                self.console_status_chip(ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("About").clicked() {
                        self.show_about = !self.show_about;
                    }
                    let log_label = if self.show_log { "Hide log" } else { "Show log" };
                    if ui.button(log_label).clicked() {
                        self.show_log = !self.show_log;
                    }
                    if ui.button("Settings…").clicked() {
                        self.show_settings = !self.show_settings;
                    }
                });
            });
            ui.add_space(4.0);
        });

        if self.show_log {
            egui::Panel::bottom("log_panel")
                .resizable(true)
                .default_size(140.0)
                .show(ui, |ui| {
                    log_view::show(ui, &self.logs);
                });
        }

        let show_connect = !self.connected() || self.force_connect_view;
        if show_connect {
            egui::CentralPanel::default().show(ui, |ui| {
                connect_view::show(self, ui);
            });
        } else {
            egui::CentralPanel::default().show(ui, |ui| {
                queue_view::show(self, ui);
            });
        }

        if self.show_settings {
            let mut open = self.show_settings;
            settings_view::show(self, ui.ctx(), &mut open);
            self.show_settings = open;
        }
        if self.show_about {
            let mut open = self.show_about;
            about_window(ui.ctx(), &mut open);
            self.show_about = open;
        }

        // Repaint cadence: only while something is actually happening —
        // an idle queue costs zero CPU (no busy redraw loop).
        let active = busy
            || self.snapshot.engine_busy
            || self.discovery_running
            || matches!(self.snapshot.console.status, ConsoleStatus::Probing)
            || self.dialog.is_some()
            || self.pending_folder.is_some();
        if active {
            ui.ctx().request_repaint_after(Duration::from_millis(150));
        }
    }

    fn on_exit(&mut self) {
        // Deterministic engine shutdown before the process exits: cancels
        // work, stops the file server, flushes the queue.
        if let Some(engine) = self.engine.take() {
            engine.shutdown();
        }
    }
}

impl App {
    fn console_status_chip(&mut self, ui: &mut egui::Ui) {
        let (color, label) = match &self.snapshot.console.status {
            ConsoleStatus::None => (egui::Color32::GRAY, "No console".to_string()),
            ConsoleStatus::Probing => (egui::Color32::YELLOW, "Connecting…".into()),
            ConsoleStatus::Connected => {
                let t = self
                    .snapshot
                    .console
                    .target
                    .as_ref()
                    .map(|t| format!("{} ({})", t.kind, t.addr))
                    .unwrap_or_else(|| "Connected".into());
                (egui::Color32::GREEN, t)
            }
            ConsoleStatus::Incompatible(reason) => {
                (egui::Color32::RED, format!("Incompatible: {reason}"))
            }
            ConsoleStatus::Lost => (egui::Color32::RED, "Console lost".into()),
        };
        ui.colored_label(color, format!("● {label}"));
        if self.connected()
            && ui
                .small_button("Switch")
                .on_hover_text("Pick a different console")
                .clicked()
            {
                self.force_connect_view = true;
            }
    }
}

fn about_window(ctx: &egui::Context, open: &mut bool) {
    egui::Window::new("About")
        .open(open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.heading("Remote Package Installer NG");
            ui.label(format!("Version {}", rpi_ng_core::VERSION));
            ui.add_space(6.0);
            ui.label(
                "A cross-platform installer for PS4 packages over the local network, \
                 speaking the Remote Package Installer protocol.",
            );
            ui.add_space(6.0);
            ui.label("Distributed under GPL-3.0. See LICENSE and docs/ for details.");
            ui.add_space(6.0);
            ui.hyperlink_to(
                "Repository",
                "https://github.com/AldySan/Remote-Package-Installer-NG",
            );
        });
}

pub(crate) fn state_color(state: &ItemState) -> egui::Color32 {
    match state {
        ItemState::Queued => egui::Color32::GRAY,
        ItemState::Preparing | ItemState::Submitting => egui::Color32::from_rgb(96, 165, 250),
        ItemState::Transferring => egui::Color32::from_rgb(59, 130, 246),
        ItemState::Installing => egui::Color32::from_rgb(147, 51, 234),
        ItemState::Paused => egui::Color32::from_rgb(245, 158, 11),
        ItemState::Completed => egui::Color32::from_rgb(34, 197, 94),
        ItemState::Failed | ItemState::Invalid => egui::Color32::from_rgb(239, 68, 68),
        ItemState::Cancelled => egui::Color32::from_rgb(120, 113, 108),
        ItemState::Unknown => egui::Color32::from_rgb(168, 85, 247),
    }
}

pub(crate) fn queue_summary(items: &[QueueItem]) -> String {
    let total = items.len();
    let done = items.iter().filter(|i| i.state == ItemState::Completed).count();
    let failed = items
        .iter()
        .filter(|i| matches!(i.state, ItemState::Failed | ItemState::Invalid))
        .count();
    let active = items.iter().filter(|i| i.state.is_active()).count();
    format!("{total} item(s) · {done} done · {active} active · {failed} failed")
}
