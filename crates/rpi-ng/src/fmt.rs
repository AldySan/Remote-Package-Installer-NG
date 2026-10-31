//! Formatting helpers (bytes, durations, addresses).

use std::time::Duration;

pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = n as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        let unit = UNITS[unit];
        format!("{value:.1} {unit}")
    }
}

pub fn bps(n: u64) -> String {
    if n == 0 {
        String::new()
    } else {
        format!("{}/s", bytes(n))
    }
}

/// Megabits per second (network-style), from a bytes-per-second rate.
/// Adaptive precision so both a gigabit LAN and an idle server read well.
pub fn mbps(bytes_per_sec: f64) -> String {
    let mbps = bytes_per_sec * 8.0 / 1_000_000.0;
    if mbps >= 100.0 {
        format!("{mbps:.0} Mbps")
    } else if mbps >= 10.0 {
        format!("{mbps:.1} Mbps")
    } else {
        format!("{mbps:.2} Mbps")
    }
}

pub fn duration(sec: u64) -> String {
    let d = Duration::from_secs(sec.min(99 * 3600 + 3599));
    let h = d.as_secs() / 3600;
    let m = (d.as_secs() % 3600) / 60;
    let s = d.as_secs() % 60;
    if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

pub fn clock(unix_ms: u64) -> String {
    // No timezone dependency: log timestamps render UTC and *say so* —
    // otherwise a UTC+7 user reads every entry seven hours off their wall
    // clock with no hint (v0.0.5 fix: the marker used to be missing).
    let secs = unix_ms / 1000;
    let (h, m, s) = (secs / 3600 % 24, secs / 60 % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02} UTC")
}
