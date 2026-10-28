//! "Check File Server" — loopback self-test of the local file server.
//!
//! The check runs on a plain background thread (the probe is a single tiny
//! TCP exchange, std-only — no runtime needed) and reports through a channel
//! the app polls once per frame, exactly like the update checker. Its purpose
//! is diagnostic: it tells the user whether the app's own server is alive,
//! which separates "our server is down" from "something between the two
//! machines is blocking" (firewall, subnet, AP isolation).

use std::{
    sync::mpsc::{Receiver, Sender, channel},
    time::Duration,
};

use rpi_ng_core::fileserver::{self, ServerCheck};

/// How long the probe waits for a connect/read before declaring `NoAnswer`.
const TIMEOUT: Duration = Duration::from_millis(1500);

pub struct ServerChecker {
    tx: Sender<ServerCheck>,
    rx: Receiver<ServerCheck>,
    /// True while a probe is in flight (prevents stacking, drives repaints).
    pub busy: bool,
}

impl ServerChecker {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            tx,
            rx,
            busy: false,
        }
    }

    /// Start a background probe of `port` unless one is already running.
    pub fn kick(&mut self, port: u16) {
        if self.busy {
            return;
        }
        self.busy = true;
        let tx = self.tx.clone();
        let fallback_tx = tx.clone();
        let build = std::thread::Builder::new().name("rpi-ng-server-check".into());
        if build
            .spawn(move || {
                let result = fileserver::self_check(port, TIMEOUT);
                let _ = tx.send(result);
            })
            .is_err()
        {
            // Thread spawn failed: still honor the promise of a result.
            let _ = fallback_tx.send(ServerCheck::Error(
                "could not start the file-server check".into(),
            ));
        }
    }

    /// Poll for a finished probe. Returns the outcome exactly once.
    pub fn poll(&mut self) -> Option<ServerCheck> {
        match self.rx.try_recv() {
            Ok(result) => {
                self.busy = false;
                Some(result)
            }
            Err(_) => None,
        }
    }
}

impl Default for ServerChecker {
    fn default() -> Self {
        Self::new()
    }
}
