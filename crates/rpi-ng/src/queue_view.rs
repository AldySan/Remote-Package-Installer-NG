//! Queue view: toolbar, item list with live progress.
//!
//! The list is a plain scroll area — rows have variable heights (error
//! lines, wrapping titles, per-state action rows), and egui's virtualized
//! `show_rows` contractually requires a FIXED row height: rendering 88-101px
//! rows into 74px slots made every row overlap the next. Measured headlessly
//! during the fourth audit round: a plain row is ~88px, one with an error
//! line ~101px. egui lays out thousands of widgets per frame comfortably,
//! and the log panel already renders up to 500 entries unvirtualized, so the
//! simplicity is the right trade here.

use eframe::egui;

use rpi_ng_core::{
    fileserver::ServerCheck,
    model::{ItemState, ProgressView, QueueItem},
};

use crate::{
    app::{App, queue_summary},
    dialogs, fmt,
    palette::Palette,
};

/// Which per-item action buttons a queue row offers, derived ONLY from the
/// item state — the exact mapping the UI renders. Extracted from the row
/// layout code so the "which button for which state" contract is testable
/// without a GUI harness (this is where "button does nothing" bugs live).
///
/// Note the deliberate asymmetry carried over from the layout: pausing an
/// active item pauses the WHOLE queue (one console task at a time), while
/// resuming is per-item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RowActions {
    /// "Pause" — pauses the whole queue (only while this item is active).
    pub pause: bool,
    /// "Resume" — resumes just this item (only while paused).
    pub resume_item: bool,
    /// "Mark completed" — outcome unknown, user confirms it finished.
    pub mark_completed: bool,
    /// "Retry" — requeue for a fresh attempt.
    pub retry: bool,
    /// "Remove" — delete from the queue (never while active).
    pub remove: bool,
    /// "Cancel" — stop the in-flight install (only while active).
    pub cancel: bool,
    /// "Up" — reorder (never while active).
    pub move_up: bool,
    /// "Down" — reorder (never while active).
    pub move_down: bool,
    /// Row sits at the top of the queue: "Up" renders disabled with a reason
    /// instead of doing nothing (the engine rejects the move silently).
    pub at_top: bool,
    /// Row sits at the bottom of the queue: "Down" renders disabled.
    pub at_bottom: bool,
}

impl RowActions {
    pub(crate) fn for_state(state: ItemState) -> Self {
        let active = state.is_active();
        Self {
            pause: active,
            resume_item: state == ItemState::Paused,
            mark_completed: state == ItemState::Unknown,
            retry: state.is_finished() || state == ItemState::Unknown,
            remove: !active,
            cancel: active,
            move_up: !active,
            move_down: !active,
            at_top: false,
            at_bottom: false,
        }
    }

    /// Position-aware variant: `is_first` / `is_last` are the item's real
    /// position in the whole queue (not the filtered view), because that is
    /// what the engine's `move_item` bounds itself by.
    pub(crate) fn for_row(state: ItemState, is_first: bool, is_last: bool) -> Self {
        let mut actions = Self::for_state(state);
        actions.at_top = is_first;
        actions.at_bottom = is_last;
        actions
    }
}

/// Toolbar button enablement, derived from engine/busy state and the queue —
/// the exact contract the toolbar renders (plus the hover reason shown when
/// Add is disabled). Extracted for the same reason as [`RowActions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ToolbarState {
    /// "Add PKG files…" / "Add folder…" enabled.
    pub can_add: bool,
    /// Why Add is disabled (already accounts for which gate is blocking).
    pub add_disabled_reason: &'static str,
    /// "Start queue" enabled (nothing running, engine idle).
    pub can_start: bool,
    /// Label swaps to "Queue running" while anything is active.
    pub start_label: &'static str,
    /// "Pause" enabled.
    pub can_pause: bool,
    /// "Resume" enabled (a paused item exists).
    pub can_resume: bool,
    /// "Clear finished" enabled.
    pub can_clear_finished: bool,
}

impl ToolbarState {
    pub(crate) fn derive(busy: bool, dialog_open: bool, items: &[QueueItem]) -> Self {
        let any_active = items.iter().any(|i| i.state.is_active());
        Self {
            can_add: !busy && !dialog_open,
            add_disabled_reason: if dialog_open {
                "Close the open file dialog first"
            } else {
                "Wait for the current operation to finish"
            },
            can_start: !any_active && !busy,
            start_label: if any_active {
                "Queue running"
            } else {
                "Start queue"
            },
            can_pause: any_active,
            can_resume: items.iter().any(|i| i.state == ItemState::Paused),
            can_clear_finished: items.iter().any(|i| i.state.is_finished()),
        }
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    toolbar(app, ui);
    server_strip(app, ui);
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
    let items_len = app.snapshot.items.len();

    if indices.is_empty() {
        if app.snapshot.items.is_empty() {
            empty_state(ui);
        } else {
            // A filter that hides everything is not an empty queue: saying
            // "drop files here" while items exist looks like they vanished.
            no_matches(ui, &needle);
        }
        return;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for idx in indices {
                let item = &app.snapshot.items[idx];
                item_row(app, ui, item, idx == 0, idx + 1 == items_len);
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

/// Below this available width the toolbar wraps into two rows. Measured
/// headlessly with real egui metrics (see the layout tests): the buttons
/// need 575px in the worst case ("Queue running" label) and the right-aligned
/// search + summary group 431px (6-digit counts), so a single row needs
/// ~1015px. The 1120px default window stays single-row; the 860px minimum
/// window wraps.
const SINGLE_ROW_MIN_WIDTH: f32 = 1050.0;

fn two_row_toolbar(available_width: f32) -> bool {
    available_width < SINGLE_ROW_MIN_WIDTH
}

fn toolbar(app: &mut App, ui: &mut egui::Ui) {
    let tb = ToolbarState::derive(
        app.snapshot.engine_busy,
        app.dialog.is_some(),
        &app.snapshot.items,
    );
    if two_row_toolbar(ui.available_width()) {
        // Narrow: buttons get their own row, search + summary the next one,
        // so the right-aligned group can never paint over the buttons.
        ui.horizontal(|ui| toolbar_buttons(app, ui, &tb));
        ui.horizontal(|ui| toolbar_search(app, ui));
    } else {
        ui.horizontal(|ui| {
            toolbar_buttons(app, ui, &tb);
            toolbar_search(app, ui);
        });
    }
}

fn toolbar_buttons(app: &mut App, ui: &mut egui::Ui, tb: &ToolbarState) {
    if ui
        .add_enabled(tb.can_add, egui::Button::new("Add PKG files…"))
        .on_hover_text("Select one or more .pkg files")
        .on_disabled_hover_text(tb.add_disabled_reason)
        .clicked()
    {
        app.dialog = Some(dialogs::pick_pkg_files());
    }
    if ui
        .add_enabled(tb.can_add, egui::Button::new("Add folder…"))
        .on_hover_text("Scan a folder for packages")
        .on_disabled_hover_text(tb.add_disabled_reason)
        .clicked()
    {
        app.dialog = Some(dialogs::pick_folder());
    }
    ui.separator();

    if ui
        .add_enabled(tb.can_start, egui::Button::new(tb.start_label))
        .on_hover_text("Install queued items in order")
        .clicked()
    {
        app.send(rpi_ng_core::Command::Start);
    }
    if ui
        .add_enabled(tb.can_pause, egui::Button::new("Pause"))
        .on_hover_text("Pause the active console task and stop auto-advance")
        .clicked()
    {
        app.send(rpi_ng_core::Command::Pause);
    }
    if ui
        .add_enabled(tb.can_resume, egui::Button::new("Resume"))
        .clicked()
    {
        app.send(rpi_ng_core::Command::Resume);
    }
    ui.separator();
    if ui
        .add_enabled(tb.can_clear_finished, egui::Button::new("Clear finished"))
        .clicked()
    {
        app.send(rpi_ng_core::Command::ClearFinished);
    }
}

fn toolbar_search(app: &mut App, ui: &mut egui::Ui) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let summary = queue_summary(&app.snapshot.items);
        ui.label(egui::RichText::new(summary).small().weak());
        ui.add(
            egui::TextEdit::singleline(&mut app.search)
                .hint_text("Search queue…")
                .desired_width(180.0),
        );
    });
}

/// Live file-server line: where packages are served from, live in/out
/// transfer rates, totals and the self-check button. Visible whenever the
/// server runs (i.e. while a console session is up) — the natural home for
/// "is the pipe actually working?" questions.
fn server_strip(app: &mut App, ui: &mut egui::Ui) {
    let Some(server) = app.snapshot.server.clone() else {
        return;
    };
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        let p = Palette::of(ui);
        ui.colored_label(p.ok, "●");
        ui.strong("File server");
        ui.monospace(&server.base_url);
        ui.separator();
        ui.colored_label(p.ok, format!("↑ {}", fmt::mbps(server.tx_bps)))
            .on_hover_text("Out — what the console is downloading from you (smoothed)");
        ui.colored_label(p.info, format!("↓ {}", fmt::mbps(server.rx_bps)))
            .on_hover_text("In — request/control traffic (tiny by design)");
        let totals = format!(
            "sent {} · received {}",
            fmt::bytes(server.tx_total),
            fmt::bytes(server.rx_total)
        );

        // Right side: self-check button + outcome chip + session totals.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let busy = app.server_checker.busy;
            let label = if busy {
                "Checking…"
            } else {
                "Check File Server"
            };
            if ui
                .button(label)
                .on_hover_text(
                    "Test whether the app's own file server is answering on this \
                     computer — if it is OK but the console still cannot download, the \
                     block is between the two machines (firewall / network)",
                )
                .clicked()
            {
                // Probe the actual bound address: the server usually binds a
                // specific interface (the one that can reach the console),
                // which a loopback probe would wrongly report as dead.
                app.server_checker.kick(server.bind);
            }
            if busy {
                ui.add(egui::Spinner::new());
            } else if let Some(result) = app.server_check_result.clone() {
                check_chip(ui, &result);
            }
            ui.label(egui::RichText::new(totals).weak());
        });
    });
}

/// Compact outcome chip for the most recent self-check.
fn check_chip(ui: &mut egui::Ui, result: &ServerCheck) {
    let p = Palette::of(ui);
    match result {
        ServerCheck::Responded(code) => {
            ui.colored_label(p.ok, format!("OK (HTTP {code})"))
                .on_hover_text("The server answered the probe — it is running normally");
        }
        ServerCheck::NotRunning => {
            ui.colored_label(p.neutral, "not running").on_hover_text(
                "Nothing is listening — the server starts with a console connection",
            );
        }
        ServerCheck::NoAnswer => {
            ui.colored_label(p.warn, "no answer").on_hover_text(
                "The port accepted the connection but never replied — a firewall or antivirus may be intercepting it",
            );
        }
        ServerCheck::Error(e) => {
            ui.colored_label(p.error, "check failed")
                .on_hover_text(format!("Unexpected I/O error: {e}"));
        }
    }
}

fn empty_state(ui: &mut egui::Ui) {
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
            // The live file-server strip above the list already shows the
            // serving URL — no need to repeat it here.
            ui.add_space(20.0);
        });
    });
}

/// The queue is not empty — the search box is hiding everything.
fn no_matches(ui: &mut egui::Ui, needle: &str) {
    let frame = egui::Frame::new()
        .fill(ui.style().visuals.faint_bg_color)
        .inner_margin(egui::Margin::same(28))
        .corner_radius(10.0);
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical_centered(|ui| {
            ui.add_space(20.0);
            ui.heading("No packages match this search");
            ui.label(egui::RichText::new(format!("Nothing matches “{needle}”.")));
            ui.label(
                egui::RichText::new("Clear the search box to see the whole queue again.").weak(),
            );
            ui.add_space(20.0);
        });
    });
}

fn item_row(app: &App, ui: &mut egui::Ui, item: &QueueItem, is_first: bool, is_last: bool) {
    let id = item.id;
    let state = item.state;
    let color = Palette::of(ui).item_state(&state);
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
                                .color(Palette::of(ui).warn),
                        );
                    }
                    if let Some(kind) = p.content_kind.package_type() {
                        ui.label(egui::RichText::new(kind).small().weak());
                    }
                    // Version straight from the package's PARAM.SFO: the app
                    // version for games/patches, the package version otherwise
                    // (themes and some add-ons only carry the latter).
                    if let Some(av) = &p.app_version {
                        ui.label(egui::RichText::new(format!("App v{av}")).small().weak());
                    } else if let Some(pv) = &p.package_version {
                        ui.label(egui::RichText::new(format!("Pkg v{pv}")).small().weak());
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
                .color(Palette::of(ui).error),
        );
    }

    // Actions.
    let actions = RowActions::for_row(state, is_first, is_last);
    ui.horizontal(|ui| {
        if actions.resume_item && ui.small_button("Resume").clicked() {
            app.send(rpi_ng_core::Command::ResumeItem(id));
        }
        if actions.pause && ui.small_button("Pause").clicked() {
            app.send(rpi_ng_core::Command::Pause);
        }
        if actions.mark_completed
            && ui
                .small_button("Mark completed")
                .on_hover_text("You verified on the console that it finished")
                .clicked()
        {
            app.send(rpi_ng_core::Command::MarkCompleted(id));
        }
        if actions.retry && ui.small_button("Retry").clicked() {
            app.send(rpi_ng_core::Command::Retry(id));
        }
        if actions.remove && ui.small_button("Remove").clicked() {
            app.send(rpi_ng_core::Command::Remove(id));
        }
        if actions.cancel && ui.small_button("Cancel").clicked() {
            app.send(rpi_ng_core::Command::Cancel(id));
        }
        if actions.move_up
            && ui
                .add_enabled(!actions.at_top, egui::Button::new("Up").small())
                .on_disabled_hover_text("Already at the top of the queue")
                .clicked()
        {
            app.send(rpi_ng_core::Command::Move { id, delta: -1 });
        }
        if actions.move_down
            && ui
                .add_enabled(!actions.at_bottom, egui::Button::new("Down").small())
                .on_disabled_hover_text("Already at the bottom of the queue")
                .clicked()
        {
            app.send(rpi_ng_core::Command::Move { id, delta: 1 });
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

// ---------------------------------------------------------------------------
// Pure UI-state logic tests (no GUI harness needed — these pin the contracts
// the egui code renders: which rows are visible, which buttons exist).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rpi_ng_core::model::{ContentKind, PartFile, PkgInfo};

    fn item_with(id: u64, state: ItemState) -> QueueItem {
        QueueItem {
            id,
            parts: vec![PartFile {
                id,
                path: "/tmp/game.pkg".into(),
                file_name: "game.pkg".into(),
                size: 10,
                part_index: None,
                pkg: None,
                invalid_reason: None,
            }],
            state,
            error: None,
            task_id: None,
            task_owner: None,
            job_id: None,
            console_title: None,
            progress: None,
            attempts: 0,
            added_at: 0,
            updated_at: 0,
        }
    }

    fn item_with_pkg(
        id: u64,
        state: ItemState,
        title: &str,
        cid: &str,
        title_id: &str,
    ) -> QueueItem {
        let mut item = item_with(id, state);
        let part = &mut item.parts[0];
        part.pkg = Some(PkgInfo {
            content_id: cid.into(),
            title_id: Some(title_id.into()),
            content_kind: ContentKind::Gd,
            is_patch: false,
            title: Some(title.into()),
            app_version: None,
            package_version: None,
            category: None,
            header_size: 0,
        });
        item
    }

    // -- search visibility ("item not showing" bug class) -------------------

    #[test]
    fn empty_needle_shows_everything() {
        // The caller passes search.trim().to_lowercase(); an empty or
        // whitespace-only box must show the whole queue.
        assert!(matches_search("", &item_with(0, ItemState::Queued)));
        assert!(matches_search(
            "   ".trim(),
            &item_with(1, ItemState::Failed)
        ));
    }

    #[test]
    fn search_matches_title_file_title_id_and_content_id() {
        let item = item_with_pkg(
            0,
            ItemState::Queued,
            "Red Dead Redemption",
            "UP0001-CUSA03041_00-REDEMPTION000002",
            "CUSA03041",
        );
        // Parsed metadata wins the display title, but the file name must
        // still match — users search what they see on disk.
        assert!(matches_search("red dead", &item), "title match");
        assert!(matches_search("game.pkg", &item), "file name match");
        assert!(matches_search("cusa03041", &item), "title id match");
        assert!(
            matches_search("up0001-cusa03041", &item),
            "content id match"
        );
        assert!(
            matches_search("redemption000002", &item),
            "content id tail match"
        );
    }

    #[test]
    fn search_no_match_is_false_not_panic() {
        let item = item_with(0, ItemState::Queued);
        assert!(!matches_search("zzz-not-there", &item));
        // An unparsed item (pkg: None) must not match on empty fields.
        assert!(!matches_search("CUSA", &item));
    }

    // -- per-row action buttons ("button does nothing" bug class) ----------

    #[test]
    fn row_actions_match_every_state() {
        // (state, expected RowActions)
        let cases: Vec<(ItemState, RowActions)> = vec![
            // Freshly queued: manage freely, nothing to pause or cancel.
            (
                ItemState::Queued,
                RowActions {
                    pause: false,
                    resume_item: false,
                    mark_completed: false,
                    retry: false,
                    remove: true,
                    cancel: false,
                    move_up: true,
                    move_down: true,
                    at_top: false,
                    at_bottom: false,
                },
            ),
            // Preparing/Submitting/Transferring/Installing: the engine owns
            // the item — only whole-queue Pause and per-item Cancel exist.
            (
                ItemState::Preparing,
                RowActions {
                    pause: true,
                    resume_item: false,
                    mark_completed: false,
                    retry: false,
                    remove: false,
                    cancel: true,
                    move_up: false,
                    move_down: false,
                    at_top: false,
                    at_bottom: false,
                },
            ),
            (
                ItemState::Submitting,
                RowActions {
                    pause: true,
                    resume_item: false,
                    mark_completed: false,
                    retry: false,
                    remove: false,
                    cancel: true,
                    move_up: false,
                    move_down: false,
                    at_top: false,
                    at_bottom: false,
                },
            ),
            (
                ItemState::Transferring,
                RowActions {
                    pause: true,
                    resume_item: false,
                    mark_completed: false,
                    retry: false,
                    remove: false,
                    cancel: true,
                    move_up: false,
                    move_down: false,
                    at_top: false,
                    at_bottom: false,
                },
            ),
            (
                ItemState::Installing,
                RowActions {
                    pause: true,
                    resume_item: false,
                    mark_completed: false,
                    retry: false,
                    remove: false,
                    cancel: true,
                    move_up: false,
                    move_down: false,
                    at_top: false,
                    at_bottom: false,
                },
            ),
            // Paused: per-item Resume (NOT whole-queue Pause) + management.
            (
                ItemState::Paused,
                RowActions {
                    pause: false,
                    resume_item: true,
                    mark_completed: false,
                    retry: false,
                    remove: true,
                    cancel: false,
                    move_up: true,
                    move_down: true,
                    at_top: false,
                    at_bottom: false,
                },
            ),
            // Completed/Failed/Cancelled/Invalid: only management + Retry.
            (
                ItemState::Completed,
                RowActions {
                    pause: false,
                    resume_item: false,
                    mark_completed: false,
                    retry: true,
                    remove: true,
                    cancel: false,
                    move_up: true,
                    move_down: true,
                    at_top: false,
                    at_bottom: false,
                },
            ),
            (
                ItemState::Failed,
                RowActions {
                    pause: false,
                    resume_item: false,
                    mark_completed: false,
                    retry: true,
                    remove: true,
                    cancel: false,
                    move_up: true,
                    move_down: true,
                    at_top: false,
                    at_bottom: false,
                },
            ),
            (
                ItemState::Cancelled,
                RowActions {
                    pause: false,
                    resume_item: false,
                    mark_completed: false,
                    retry: true,
                    remove: true,
                    cancel: false,
                    move_up: true,
                    move_down: true,
                    at_top: false,
                    at_bottom: false,
                },
            ),
            (
                ItemState::Invalid,
                RowActions {
                    pause: false,
                    resume_item: false,
                    mark_completed: false,
                    retry: true,
                    remove: true,
                    cancel: false,
                    move_up: true,
                    move_down: true,
                    at_top: false,
                    at_bottom: false,
                },
            ),
            // Unknown: user decides the outcome — Mark completed OR Retry.
            (
                ItemState::Unknown,
                RowActions {
                    pause: false,
                    resume_item: false,
                    mark_completed: true,
                    retry: true,
                    remove: true,
                    cancel: false,
                    move_up: true,
                    move_down: true,
                    at_top: false,
                    at_bottom: false,
                },
            ),
        ];
        for (state, expected) in cases {
            assert_eq!(RowActions::for_state(state), expected, "state {state:?}");
        }
    }

    #[test]
    fn row_actions_never_mix_cancel_and_remove() {
        // The pair is mutually exclusive per state: Cancel needs an in-flight
        // task, Remove must never yank one out from under the engine.
        for state in [
            ItemState::Queued,
            ItemState::Preparing,
            ItemState::Submitting,
            ItemState::Transferring,
            ItemState::Installing,
            ItemState::Paused,
            ItemState::Completed,
            ItemState::Failed,
            ItemState::Cancelled,
            ItemState::Invalid,
            ItemState::Unknown,
        ] {
            let a = RowActions::for_state(state);
            assert_ne!(a.cancel, a.remove, "state {state:?}");
            assert_eq!(a.cancel, a.pause, "cancel tracks active (pause) state");
        }
    }

    #[test]
    fn row_actions_know_the_queue_boundaries() {
        // Middle of the queue: no caps.
        let a = RowActions::for_row(ItemState::Queued, false, false);
        assert!(!a.at_top && !a.at_bottom);
        assert_eq!(a, RowActions::for_state(ItemState::Queued));
        // First row: Up is capped; the state flags stay untouched.
        let a = RowActions::for_row(ItemState::Queued, true, false);
        assert!(a.at_top && !a.at_bottom);
        assert!(a.move_up && a.move_down, "state flags remain state-only");
        // Last row: Down is capped.
        let a = RowActions::for_row(ItemState::Completed, false, true);
        assert!(!a.at_top && a.at_bottom);
        // A single-item queue caps both sides: two honest disabled buttons
        // instead of two dead ones.
        let a = RowActions::for_row(ItemState::Queued, true, true);
        assert!(a.at_top && a.at_bottom);
    }

    // -- toolbar gating -----------------------------------------------------

    #[test]
    fn toolbar_on_empty_queue() {
        let tb = ToolbarState::derive(false, false, &[]);
        assert!(tb.can_add);
        assert!(tb.can_start);
        assert_eq!(tb.start_label, "Start queue");
        assert!(!tb.can_pause);
        assert!(!tb.can_resume);
        assert!(!tb.can_clear_finished);
    }

    #[test]
    fn toolbar_while_queue_running() {
        let items = vec![
            item_with(0, ItemState::Transferring),
            item_with(1, ItemState::Queued),
        ];
        let tb = ToolbarState::derive(false, false, &items);
        assert!(tb.can_add, "adding while running is allowed");
        assert!(!tb.can_start, "no double-start");
        assert_eq!(tb.start_label, "Queue running");
        assert!(tb.can_pause);
        assert!(!tb.can_resume);
    }

    #[test]
    fn toolbar_with_paused_item() {
        let items = vec![item_with(0, ItemState::Paused)];
        let tb = ToolbarState::derive(false, false, &items);
        assert!(
            tb.can_start,
            "start is allowed when only paused items exist"
        );
        assert!(!tb.can_pause);
        assert!(tb.can_resume);
    }

    #[test]
    fn toolbar_with_finished_item() {
        let items = vec![item_with(0, ItemState::Completed)];
        let tb = ToolbarState::derive(false, false, &items);
        assert!(tb.can_clear_finished);
        assert!(!tb.can_pause);
    }

    #[test]
    fn toolbar_busy_and_dialog_gates() {
        // Engine busy: everything that starts work goes off.
        let tb = ToolbarState::derive(true, false, &[]);
        assert!(!tb.can_add);
        assert_eq!(
            tb.add_disabled_reason,
            "Wait for the current operation to finish"
        );
        assert!(!tb.can_start);

        // A native dialog open: Add must go off AND the disabled hover must
        // blame the dialog (the one gate the user can actually close).
        let tb = ToolbarState::derive(false, true, &[]);
        assert!(!tb.can_add);
        assert_eq!(tb.add_disabled_reason, "Close the open file dialog first");
        // Start does not depend on the dialog.
        assert!(tb.can_start);
    }

    // -- toolbar layout (R-03: no collisions at the 860x560 minimum) -------

    #[test]
    fn toolbar_wraps_below_the_single_row_threshold() {
        // Usable central-panel width at the 860x560 minimum window
        // (8px panel margin each side) and at the 1120x760 default.
        assert!(two_row_toolbar(844.0), "minimum window must wrap");
        assert!(!two_row_toolbar(1104.0), "default window stays single-row");
        assert!(!two_row_toolbar(SINGLE_ROW_MIN_WIDTH));
        assert!(
            !two_row_toolbar(1920.0 - 16.0),
            "wide windows stay single-row"
        );
    }

    /// Measured with real egui metrics (headless pass), not estimates: the
    /// two wrapped rows must fit the minimum window, and the single-row
    /// threshold must cover what the single row actually needs.
    #[test]
    fn toolbar_rows_fit_the_minimum_window_when_measured() {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let min_usable = 860.0 - 2.0 * 8.0;
            let buttons_row = ui
                .horizontal(|ui| {
                    for label in [
                        "Add PKG files…",
                        "Add folder…",
                        "Queue running",
                        "Pause",
                        "Resume",
                        "Clear finished",
                    ] {
                        let _ = ui.button(label);
                    }
                    ui.separator();
                    ui.separator();
                    ui.min_rect()
                })
                .inner;
            eprintln!("MEASURE buttons_row {:.0}", buttons_row.width());
            assert!(
                buttons_row.width() <= min_usable,
                "buttons row {:.0}px exceeds the {:.0}px minimum window",
                buttons_row.width(),
                min_usable
            );

            let search_row = ui
                .horizontal(|ui| {
                    // Worst-case summary text (6-digit counts in every slot).
                    ui.label(
                        egui::RichText::new(
                            "999999 item(s) · 999999 done · 999999 active · 999999 failed",
                        )
                        .small()
                        .weak(),
                    );
                    let mut search = String::new();
                    ui.add(
                        egui::TextEdit::singleline(&mut search)
                            .hint_text("Search queue…")
                            .desired_width(180.0),
                    );
                    ui.min_rect()
                })
                .inner;
            eprintln!("MEASURE search_row {:.0}", search_row.width());
            assert!(
                search_row.width() <= min_usable,
                "search row {:.0}px exceeds the {:.0}px minimum window",
                search_row.width(),
                min_usable
            );

            // A single row needs both groups plus one inter-group gap; the
            // threshold must cover it or the wrap would be pointless. The
            // 30px headroom absorbs small metric drift across platforms.
            let gap = ui.style().spacing.item_spacing.x;
            let single_row_needs = buttons_row.width() + gap + search_row.width();
            assert!(
                SINGLE_ROW_MIN_WIDTH >= single_row_needs + 30.0,
                "threshold {:.0}px leaves <30px over the {:.0}px single-row need",
                SINGLE_ROW_MIN_WIDTH,
                single_row_needs
            );
        });
        output.drop_without_applying_deltas();
    }
}
