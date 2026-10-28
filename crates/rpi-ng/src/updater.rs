//! In-app update checks against the project's GitHub releases.
//!
//! The check runs on a dedicated thread with its own tiny tokio runtime so it
//! can never block the UI thread, and reports back through a channel the app
//! polls once per frame. Only a plain GET to the releases API is ever sent —
//! no telemetry, no identifiers.

use std::sync::mpsc::{Receiver, Sender, channel};

use rpi_ng_core::update::{self, UpdateInfo};

/// A finished check: `Ok(None)` = no newer release, `Ok(Some)` = update
/// available, `Err` = the check itself failed (network, rate limit, ...).
pub type CheckResult = Result<Option<UpdateInfo>, String>;

pub struct Updater {
    tx: Sender<CheckResult>,
    rx: Receiver<CheckResult>,
    /// True while a background check is in flight (used to avoid stacking
    /// checks and to keep the UI repainting).
    pub busy: bool,
}

impl Updater {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            tx,
            rx,
            busy: false,
        }
    }

    /// Start a background check unless one is already running.
    pub fn kick(&mut self) {
        if self.busy {
            return;
        }
        self.busy = true;
        let tx = self.tx.clone();
        spawn_check(tx);
    }

    /// Poll for a finished check. Returns the result of the most recent
    /// completed check, exactly once.
    pub fn poll(&mut self) -> Option<CheckResult> {
        match self.rx.try_recv() {
            Ok(result) => {
                self.busy = false;
                Some(result)
            }
            Err(_) => None,
        }
    }
}

impl Default for Updater {
    fn default() -> Self {
        Self::new()
    }
}

fn spawn_check(tx: Sender<CheckResult>) {
    // Kept for the spawn-failure path: the thread never runs, so the promise
    // of a result must still be honored or the UI would wait forever.
    let fallback_tx = tx.clone();
    let build = std::thread::Builder::new().name("rpi-ng-update-check".into());
    if build
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = tx.send(Err(format!("could not start the check: {e}")));
                    return;
                }
            };
            let result = rt.block_on(async {
                let client = update::http_client();
                match update::check_latest(&client, update::RELEASES_API_URL, rpi_ng_core::VERSION)
                    .await
                {
                    Ok(opt) => Ok(opt),
                    Err(e) => Err(e.to_string()),
                }
            });
            let _ = tx.send(result);
        })
        .is_err()
    {
        // Thread spawn failed (resource exhaustion): report instead of hanging.
        let _ = fallback_tx.send(Err("could not start the update check".into()));
    }
}
