//! Persisted user settings (atomic JSON, corruption-tolerant).

use std::{net::SocketAddr, path::PathBuf, time::Duration};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// `System`, `Light` or `Dark` UI theme.
    pub theme: ThemeMode,
    /// Local file server port: `None` = OS-assigned ephemeral port (default).
    pub server_port: Option<u16>,
    /// Enable the bounded subnet probe during discovery.
    pub discovery_enabled: bool,
    /// Maximum candidate addresses probed per discovery sweep.
    pub discovery_max_hosts: u32,
    /// TCP connect timeout for discovery probes.
    pub discovery_timeout: Duration,
    /// Progress poll interval for an active task.
    pub poll_interval: Duration,
    /// Default for the recursive flag when adding folders.
    pub folder_scan_recursive: bool,
    /// Consoles the user selected before (validated again on use).
    pub remembered: Vec<RememberedConsole>,
    /// Auto-start the next queued item when the console finishes one.
    pub auto_advance: bool,
    /// What happens to the rest of the queue when a transfer fails.
    pub on_failure: FailurePolicy,
    /// Look for a newer release on GitHub when the app starts (the check
    /// sends nothing but a plain GET to the releases API).
    pub auto_check_updates: bool,
}

/// Queue behavior after a failed transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FailurePolicy {
    /// Keep going: mark the item failed, start the next queued one.
    #[default]
    Continue,
    /// Park the queue: nothing else starts until the user says so.
    Pause,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RememberedConsole {
    pub addr: SocketAddr,
    pub label: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::default(),
            server_port: None,
            discovery_enabled: true,
            discovery_max_hosts: 1024,
            discovery_timeout: Duration::from_millis(350),
            poll_interval: Duration::from_millis(1000),
            folder_scan_recursive: true,
            remembered: Vec::new(),
            auto_advance: true,
            on_failure: FailurePolicy::default(),
            auto_check_updates: true,
        }
    }
}

/// Serializable mirror of [`Settings`] (durations as millis).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SettingsDoc {
    #[serde(default)]
    theme: ThemeMode,
    #[serde(default)]
    server_port: Option<u16>,
    #[serde(default = "default_true")]
    discovery_enabled: bool,
    #[serde(default = "default_max_hosts")]
    discovery_max_hosts: u32,
    #[serde(default = "default_disc_timeout")]
    discovery_timeout_ms: u64,
    #[serde(default = "default_poll_ms")]
    poll_interval_ms: u64,
    #[serde(default = "default_true")]
    folder_scan_recursive: bool,
    #[serde(default)]
    remembered: Vec<RememberedConsole>,
    #[serde(default = "default_true")]
    auto_advance: bool,
    #[serde(default)]
    on_failure: FailurePolicy,
    #[serde(default = "default_true")]
    auto_check_updates: bool,
}

fn default_true() -> bool {
    true
}
fn default_max_hosts() -> u32 {
    1024
}
fn default_disc_timeout() -> u64 {
    350
}
fn default_poll_ms() -> u64 {
    1000
}

impl From<Settings> for SettingsDoc {
    fn from(s: Settings) -> Self {
        Self {
            theme: s.theme,
            server_port: s.server_port,
            discovery_enabled: s.discovery_enabled,
            discovery_max_hosts: s.discovery_max_hosts,
            discovery_timeout_ms: s.discovery_timeout.as_millis() as u64,
            poll_interval_ms: s.poll_interval.as_millis() as u64,
            folder_scan_recursive: s.folder_scan_recursive,
            remembered: s.remembered,
            auto_advance: s.auto_advance,
            on_failure: s.on_failure,
            auto_check_updates: s.auto_check_updates,
        }
    }
}

impl From<SettingsDoc> for Settings {
    fn from(d: SettingsDoc) -> Self {
        Self {
            theme: d.theme,
            server_port: d.server_port,
            discovery_enabled: d.discovery_enabled,
            discovery_max_hosts: d.discovery_max_hosts.clamp(64, 4096),
            discovery_timeout: Duration::from_millis(d.discovery_timeout_ms.clamp(100, 2000)),
            poll_interval: Duration::from_millis(d.poll_interval_ms.clamp(250, 10_000)),
            folder_scan_recursive: d.folder_scan_recursive,
            remembered: d.remembered,
            auto_advance: d.auto_advance,
            on_failure: d.on_failure,
            auto_check_updates: d.auto_check_updates,
        }
    }
}

/// Files we persist in the data dir (migrated from the legacy location on
/// first run of a newer build).
const PERSISTED_FILES: [&str; 2] = ["settings.json", "queue.json"];

/// Where settings/queue documents live.
///
/// Resolution order:
/// 1. `RPI_NG_DATA_DIR` env var (tests, headless embedding);
/// 2. **the directory of the executable** — the app is portable, so the
///    config travels with it (e.g. `D:\\RPING\\rpi-ng.exe` → `D:\\RPING\\`);
/// 3. per-OS user data dir (fallback when the exe path cannot be determined).
pub fn data_dir() -> Option<PathBuf> {
    resolve_data_dir(
        std::env::var("RPI_NG_DATA_DIR").ok().as_deref(),
        std::env::current_exe().ok().as_deref(),
    )
}

/// Pure resolution logic (parameterized for tests). An empty env value is
/// treated as unset — same contract as the real `RPI_NG_DATA_DIR` handling.
fn resolve_data_dir(env: Option<&str>, exe: Option<&std::path::Path>) -> Option<PathBuf> {
    if let Some(dir) = env.filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    if let Some(exe) = exe
        && let Some(parent) = exe.parent()
        // Guard against the degenerate `parent() == ""` some launchers report.
        && !parent.as_os_str().is_empty()
    {
        return Some(parent.to_path_buf());
    }
    legacy_data_dir()
}

/// The pre-portable location (per-OS user data dir). Kept only so an existing
/// install can be migrated — see [`init_data_dir`].
pub fn legacy_data_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "rpi-ng").map(|d| d.data_dir().to_path_buf())
}

/// Resolve the data dir, create it, and migrate any pre-portable files into
/// it (once: files already present are never overwritten). Called at startup,
/// before anything reads or writes the data dir.
///
/// The legacy migration only runs under the *default* resolution — an
/// `RPI_NG_DATA_DIR` override (tests, headless embedding) stays hermetic and
/// never reads the host user's real files.
pub fn init_data_dir() -> Option<PathBuf> {
    let env = std::env::var("RPI_NG_DATA_DIR")
        .ok()
        .filter(|v| !v.is_empty());
    let dir = resolve_data_dir(env.as_deref(), std::env::current_exe().ok().as_deref())?;
    if std::fs::create_dir_all(&dir).is_err() {
        // Read-only location (e.g. Program Files without elevation): keep the
        // dir anyway — reads still work, and any later write failure is
        // surfaced by the engine ("settings save failed: …") with the path.
    }
    if env.is_none()
        && let Some(legacy) = legacy_data_dir()
        && legacy != dir
    {
        for file in PERSISTED_FILES {
            let from = legacy.join(file);
            let to = dir.join(file);
            if !to.exists() && from.is_file() {
                // Best effort: a failed copy just means we start fresh.
                let _ = std::fs::copy(&from, &to);
            }
        }
    }
    Some(dir)
}

impl Settings {
    pub fn load() -> Self {
        Self::load_in(data_dir().as_deref())
    }

    pub fn load_in(dir: Option<&std::path::Path>) -> Self {
        let Some(dir) = dir else {
            return Self::default();
        };
        let path = dir.join("settings.json");
        for candidate in [&path, &dir.join("settings.json.bak")] {
            if let Ok(text) = std::fs::read_to_string(candidate)
                && let Ok(doc) = serde_json::from_str::<SettingsDoc>(&text)
            {
                return doc.into();
            }
        }
        Self::default()
    }

    /// Atomic write: serialize → write tmp → fsync → rename over destination,
    /// keeping a `.bak` of the previous file.
    pub fn save(&self) -> std::io::Result<()> {
        self.save_in(data_dir().as_deref())
    }

    pub fn save_in(&self, dir: Option<&std::path::Path>) -> std::io::Result<()> {
        let Some(dir) = dir else {
            return Ok(());
        };
        std::fs::create_dir_all(dir)?;
        let path = dir.join("settings.json");
        if path.exists() {
            let _ = std::fs::copy(&path, dir.join("settings.json.bak"));
        }
        let text = serde_json::to_string_pretty(&SettingsDoc::from(self.clone()))
            .map_err(std::io::Error::other)?;
        let tmp = dir.join("settings.json.tmp");
        atomic_write(&tmp, &path, text.as_bytes())
    }
}

/// Write bytes to `tmp`, fsync, then rename to `dest`.
pub fn atomic_write(tmp: &PathBuf, dest: &PathBuf, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    {
        let mut f = std::fs::File::create(tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(tmp, dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_override_wins_over_exe_dir() {
        let dir = resolve_data_dir(
            Some("/tmp/data"),
            Some(std::path::Path::new("/opt/app/rpi-ng.exe")),
        );
        assert_eq!(dir, Some(PathBuf::from("/tmp/data")));
    }

    #[test]
    fn exe_dir_is_used_when_no_env() {
        let exe = if cfg!(windows) {
            std::path::Path::new("D:\\RPING\\rpi-ng.exe")
        } else {
            std::path::Path::new("/home/user/RPING/rpi-ng")
        };
        let dir = resolve_data_dir(None, Some(exe));
        assert_eq!(dir, Some(exe.parent().unwrap().to_path_buf()));
    }

    #[test]
    fn empty_env_is_ignored() {
        let dir = resolve_data_dir(Some(""), Some(std::path::Path::new("/opt/app/rpi-ng")));
        assert_eq!(dir, Some(PathBuf::from("/opt/app")));
    }

    #[test]
    fn falls_back_to_user_dir_without_exe() {
        // No env, no exe path: still resolves to the per-OS user dir.
        assert!(resolve_data_dir(None, None).is_some());
    }

    #[test]
    fn init_migrates_legacy_files_once() {
        let new_dir = tempfile::tempdir().unwrap();
        let legacy = tempfile::tempdir().unwrap();
        std::fs::write(legacy.path().join("settings.json"), "{\"theme\":\"Dark\"}").unwrap();
        std::fs::write(legacy.path().join("queue.json"), "{\"items\":[]}").unwrap();

        migrate(legacy.path(), new_dir.path());
        // Both files copied.
        assert!(new_dir.path().join("settings.json").is_file());
        assert!(new_dir.path().join("queue.json").is_file());
        // And loaded correctly.
        let s = Settings::load_in(Some(new_dir.path()));
        assert_eq!(s.theme, ThemeMode::Dark);

        // Second run: existing files are never overwritten.
        std::fs::write(
            new_dir.path().join("settings.json"),
            "{\"theme\":\"Light\"}",
        )
        .unwrap();
        migrate(legacy.path(), new_dir.path());
        assert_eq!(
            std::fs::read_to_string(new_dir.path().join("settings.json")).unwrap(),
            "{\"theme\":\"Light\"}"
        );
    }

    /// The migration half of [`init_data_dir`] with explicit dirs (the full
    /// function reads process-global state: env + current exe).
    fn migrate(legacy: &std::path::Path, dir: &std::path::Path) {
        for file in PERSISTED_FILES {
            let from = legacy.join(file);
            let to = dir.join(file);
            if !to.exists() && from.is_file() {
                std::fs::copy(&from, &to).unwrap();
            }
        }
    }
}
