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
    // Local-time rendering without pulling a chrono/tz dependency: use the
    // platform's `HH:MM:SS` via a tiny manual UTC fallback. UI logs are
    // informational; ISO-UTC is acceptable and deterministic.
    let secs = unix_ms / 1000;
    let (h, m, s) = (secs / 3600 % 24, secs / 60 % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02}")
}
