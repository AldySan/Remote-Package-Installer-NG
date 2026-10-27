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
        }
    }
}

/// Where settings/queue documents live (per-OS user data dir).
/// Overridable with `RPI_NG_DATA_DIR` (used by tests and headless runs).
pub fn data_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("RPI_NG_DATA_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    directories::ProjectDirs::from("", "", "rpi-ng").map(|d| d.data_dir().to_path_buf())
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
            if let Ok(text) = std::fs::read_to_string(candidate) {
                if let Ok(doc) = serde_json::from_str::<SettingsDoc>(&text) {
                    return doc.into();
                }
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
