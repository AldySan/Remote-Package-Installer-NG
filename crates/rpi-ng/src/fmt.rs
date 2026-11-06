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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_units_and_boundaries() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1024), "1.0 KiB");
        // A PS4 game: tens of GiB with one decimal.
        assert_eq!(bytes(50 * 1024 * 1024 * 1024), "50.0 GiB");
        // Unit growth stops at PiB; huge values clamp, never panic.
        assert!(bytes(u64::MAX).ends_with("PiB"));
    }

    #[test]
    fn bps_zero_renders_empty() {
        // A zero rate shows nothing (not "0 B/s") — the label it feeds is
        // only drawn while a transfer is live.
        assert_eq!(bps(0), "");
        assert_eq!(bps(1024), "1.0 KiB/s");
    }

    #[test]
    fn mbps_tiers_and_precision() {
        // Network-style megabits from a bytes/s rate — the v0.0.3 Mbps fix.
        // 2.5 MiB/s = 20.97152 Mbps → one decimal in the 10..100 tier.
        assert_eq!(mbps(2.5 * 1024.0 * 1024.0), "21.0 Mbps");
        assert_eq!(
            mbps(125_000_000.0),
            "1000 Mbps",
            ">=100 tier drops decimals"
        );
        assert_eq!(mbps(15.0 * 1_000_000.0 / 8.0), "15.0 Mbps");
        assert_eq!(
            mbps(1.0 * 1_000_000.0 / 8.0),
            "1.00 Mbps",
            "<10 tier keeps two decimals"
        );
        // Idle server must not render "-0.00 Mbps" style noise.
        assert_eq!(mbps(0.0), "0.00 Mbps");
    }

    #[test]
    fn duration_renders_h_m_s() {
        assert_eq!(duration(0), "0s");
        assert_eq!(duration(59), "59s");
        assert_eq!(duration(60), "1m 00s");
        assert_eq!(duration(125), "2m 05s");
        // Past an hour only h+m render (seconds drop, they are noise there).
        assert_eq!(duration(3_725), "1h 2m");
        assert_eq!(duration(3600), "1h 0m");
        // The 99h59m59s cap keeps a garbage ETA from printing absurd hours.
        assert_eq!(duration(400 * 3600), "99h 59m");
        assert_eq!(duration(u64::MAX), "99h 59m");
    }

    #[test]
    fn clock_marks_utc_and_wraps_at_24h() {
        // The UTC marker is the fix for the "logs are 7 hours off" report:
        // it must survive every value, and the day wraps.
        assert_eq!(clock(0), "00:00:00 UTC");
        assert_eq!(clock(86_399_000), "23:59:59 UTC");
        assert_eq!(clock(86_400_000), "00:00:00 UTC");
        // 2025-01-01 07:15:30 UTC in ms.
        assert_eq!(clock(1_735_715_730_000), "07:15:30 UTC");
    }
}
