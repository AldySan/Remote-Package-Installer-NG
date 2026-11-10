//! Semantic color palette: one color per meaning, with a per-theme value.
//!
//! Every colored text in the app resolves through here so it passes WCAG AA
//! (>= 4.5:1) on the panel it renders on, in BOTH themes (the pure primaries
//! egui ships (`Color32::GREEN` etc.) are unreadable in light mode: yellow on
//! a near-white panel is 1.0:1). Centralizing also makes the contrast
//! contract testable in one place: see the tests at the bottom of this file.
//!
//! Mapping notes (deliberate, vs. the pre-palette ad-hoc colors):
//! * "Connecting" (probing) and Preparing/Submitting/Transferring share the
//!   in-progress blue; the label text tells them apart, one blue is calmer
//!   than three.
//! * Unknown maps to warn, not violet: like Paused, it waits for a user
//!   decision (mark completed or retry), and amber is the needs-attention
//!   color. "Console lost" is warn for the same reason (reconnect action).
//! * Queued and Cancelled share the neutral gray: no signal either way.

use eframe::egui;

use rpi_ng_core::{
    fileserver::ServerCheck,
    model::{ConsoleStatus, ItemState, LogLevel},
};

/// egui's default dark weak text (0.6 alpha) is ~2.3:1 on the dark panel,
/// far below AA. 0.95 keeps a hint of hierarchy while passing.
pub(crate) const DARK_WEAK_TEXT_ALPHA: f32 = 0.95;

pub(crate) struct Palette {
    /// Positive outcome: connected, completed, server OK, upload rate.
    pub ok: egui::Color32,
    /// Needs user attention: paused, unknown outcome, warnings, patch tag.
    pub warn: egui::Color32,
    /// Failure: failed/invalid items, errors, incompatible console.
    pub error: egui::Color32,
    /// Work in progress: connecting, preparing, submitting, transferring.
    pub info: egui::Color32,
    /// The console is installing the package (distinct from transferring).
    pub installing: egui::Color32,
    /// No signal: queued, cancelled, no console, server not running.
    pub neutral: egui::Color32,
}

impl Palette {
    /// Resolve against the visuals the current frame is rendering with.
    pub(crate) fn of(ui: &egui::Ui) -> Self {
        Self::for_dark_mode(ui.visuals().dark_mode)
    }

    pub(crate) fn for_dark_mode(dark: bool) -> Self {
        if dark {
            Self {
                ok: egui::Color32::from_rgb(0x4A, 0xDE, 0x80),
                warn: egui::Color32::from_rgb(0xFB, 0xBF, 0x24),
                error: egui::Color32::from_rgb(0xF8, 0x71, 0x71),
                info: egui::Color32::from_rgb(0x93, 0xC5, 0xFD),
                installing: egui::Color32::from_rgb(0xA7, 0x8B, 0xFA),
                neutral: egui::Color32::from_rgb(0xA1, 0xA1, 0xAA),
            }
        } else {
            Self {
                ok: egui::Color32::from_rgb(0x16, 0x65, 0x34),
                warn: egui::Color32::from_rgb(0xB4, 0x53, 0x09),
                error: egui::Color32::from_rgb(0xB9, 0x1C, 0x1C),
                info: egui::Color32::from_rgb(0x1D, 0x4E, 0xD8),
                installing: egui::Color32::from_rgb(0x6D, 0x28, 0xD9),
                neutral: egui::Color32::from_rgb(0x57, 0x53, 0x4E),
            }
        }
    }

    /// Queue item state -> semantic color.
    pub(crate) fn item_state(&self, state: &ItemState) -> egui::Color32 {
        match state {
            ItemState::Queued | ItemState::Cancelled => self.neutral,
            ItemState::Preparing | ItemState::Submitting | ItemState::Transferring => self.info,
            ItemState::Installing => self.installing,
            ItemState::Paused | ItemState::Unknown => self.warn,
            ItemState::Completed => self.ok,
            ItemState::Failed | ItemState::Invalid => self.error,
        }
    }

    /// Top-bar console status chip color.
    pub(crate) fn console_status(&self, status: &ConsoleStatus) -> egui::Color32 {
        match status {
            ConsoleStatus::None => self.neutral,
            ConsoleStatus::Probing => self.info,
            ConsoleStatus::Connected => self.ok,
            ConsoleStatus::Incompatible(_) => self.error,
            ConsoleStatus::Lost => self.warn,
        }
    }

    /// File-server self-check outcome chip color.
    pub(crate) fn server_check(&self, check: &ServerCheck) -> egui::Color32 {
        match check {
            ServerCheck::Responded(_) => self.ok,
            ServerCheck::NotRunning => self.neutral,
            ServerCheck::NoAnswer => self.warn,
            ServerCheck::Error(_) => self.error,
        }
    }

    /// Activity log line color by level (Info keeps the normal text color).
    pub(crate) fn log_level(&self, ui: &egui::Ui, level: LogLevel) -> egui::Color32 {
        match level {
            LogLevel::Info => ui.style().visuals.text_color(),
            LogLevel::Warn => self.warn,
            LogLevel::Error => self.error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG 2.x relative-luminance contrast ratio.
    fn contrast(fg: egui::Color32, bg: egui::Color32) -> f32 {
        fn channel(c: u8) -> f32 {
            let c = c as f32 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        fn luminance(c: egui::Color32) -> f32 {
            0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
        }
        let (a, b) = (luminance(fg), luminance(bg));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    fn entries(p: &Palette) -> [(&'static str, egui::Color32); 6] {
        [
            ("ok", p.ok),
            ("warn", p.warn),
            ("error", p.error),
            ("info", p.info),
            ("installing", p.installing),
            ("neutral", p.neutral),
        ]
    }

    /// The reason this module exists: every semantic color must stay readable
    /// as text on its theme's panel. `ok` additionally renders as the "New:
    /// {tag}" button label, so it must also pass on the button fill
    /// (`weak_bg_fill`); it is darkened to green-800 in light mode for that.
    #[test]
    fn every_color_passes_aa_on_its_background() {
        for dark in [true, false] {
            let p = Palette::for_dark_mode(dark);
            let visuals = if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            };
            let theme = if dark { "dark" } else { "light" };
            for (name, c) in entries(&p) {
                assert!(
                    contrast(c, visuals.panel_fill) >= 4.5,
                    "{name} on {theme} panel: {:.2}",
                    contrast(c, visuals.panel_fill)
                );
            }
            let btn_bg = visuals.widgets.inactive.weak_bg_fill;
            assert!(
                contrast(p.ok, btn_bg) >= 4.5,
                "ok on {theme} button: {:.2}",
                contrast(p.ok, btn_bg)
            );
        }
    }

    /// Secondary (`.weak()`) text must also pass: light mode does on egui's
    /// default 0.6 alpha; dark mode needs the raised alpha `apply_theme`
    /// installs (gray(133) at 0.95 vs gray(84) at 0.6, which is ~2.3:1).
    #[test]
    fn weak_text_passes_aa_in_both_themes() {
        let light = egui::Visuals::light();
        assert!(
            contrast(light.weak_text_color(), light.panel_fill) >= 4.5,
            "light weak text: {:.2}",
            contrast(light.weak_text_color(), light.panel_fill)
        );
        let mut dark = egui::Visuals::dark();
        dark.weak_text_alpha = DARK_WEAK_TEXT_ALPHA;
        assert!(
            contrast(dark.weak_text_color(), dark.panel_fill) >= 4.5,
            "dark weak text: {:.2}",
            contrast(dark.weak_text_color(), dark.panel_fill)
        );
    }

    /// The state mapping must keep the three outcomes a user scans for
    /// (done / failed / waiting) distinct from each other.
    #[test]
    fn terminal_states_map_to_distinct_colors() {
        for dark in [true, false] {
            let p = Palette::for_dark_mode(dark);
            let done = p.item_state(&ItemState::Completed);
            let failed = p.item_state(&ItemState::Failed);
            let waiting = p.item_state(&ItemState::Unknown);
            assert_ne!(done, failed);
            assert_ne!(done, waiting);
            assert_ne!(failed, waiting);
        }
    }
}
