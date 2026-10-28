//! Core domain types shared by the engine, backends, UI and persistence layer.

use std::{
    fmt,
    net::SocketAddr,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

/// Task id as assigned by the console backend (BGFT task id in the RPI dialect).
pub type TaskId = i64;

/// Opaque id of a queue part (file) registered with the local file server.
pub type PartId = u64;

/// Opaque id of a queue item (a package group).
pub type ItemId = u64;

pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Console side
// ---------------------------------------------------------------------------

/// Which console generation a backend serves. Never inferred from an IP or port;
/// only from verified backend behavior or explicit user choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ConsoleGeneration {
    Ps4,
    Ps5,
}

impl fmt::Display for ConsoleGeneration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ps4 => write!(f, "PS4"),
            Self::Ps5 => write!(f, "PS5"),
        }
    }
}

/// The backend dialect detected on a console.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum BackendKind {
    /// flatz Remote Package Installer dialect (JSON POST API on port 12800).
    Rpi,
    /// A fork/variant that answered the RPI probes but differs in unknown ways.
    RpiVariant,
    /// Something answered on the port, but it did not speak the RPI dialect.
    #[default]
    Unknown,
}

impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rpi => write!(f, "Remote Package Installer"),
            Self::RpiVariant => write!(f, "RPI variant"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Operations a backend has been *verified* to support. Capabilities are only set
/// after live probing; a UI must disable unsupported operations instead of
/// simulating them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BackendCaps {
    /// `POST /api/install` answered a well-formed request (probed implicitly on
    /// connect via `is_exists`, never by a fake install).
    pub install_direct: bool,
    /// Progress reporting is available.
    pub progress: bool,
    /// pause/resume endpoints available (assumed true for Rpi until disproven).
    pub pause_resume: bool,
    /// `GET /api/status` answers JSON (newer receivers only).
    pub status_endpoint: bool,
    /// `POST /api/find_task` available for reconciliation.
    pub find_task: bool,
    /// `GET /api/v2/system` answered with an identity — the v2 job API is
    /// available (install, jobs list, per-job lifecycle ops).
    pub api_v2: bool,
}

impl Default for BackendCaps {
    fn default() -> Self {
        Self {
            install_direct: false,
            progress: true,
            pause_resume: true,
            status_endpoint: false,
            find_task: true,
            api_v2: false,
        }
    }
}

/// Connection status of the selected console.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum ConsoleStatus {
    /// Not connected; no console selected.
    #[default]
    None,
    /// Probe in progress.
    Probing,
    /// Connected and verified.
    Connected,
    /// The endpoint answered but is not a compatible backend (reason string).
    Incompatible(String),
    /// Was connected; the endpoint stopped answering.
    Lost,
}

/// Identity of a v2-capable backend, as reported by `GET /api/v2/system`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BackendIdentity {
    pub name: String,
    pub version: String,
}

impl fmt::Display for BackendIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.version.is_empty() {
            write!(f, "{}", self.name)
        } else {
            write!(f, "{} {}", self.name, self.version)
        }
    }
}

/// The console the app talks to.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConsoleTarget {
    pub addr: SocketAddr,
    pub generation: ConsoleGeneration,
    pub kind: BackendKind,
    pub caps: BackendCaps,
    /// Best-effort friendly label (e.g. remembered alias).
    pub label: Option<String>,
    /// v2 backend identity (name + version) when the backend provides one.
    #[serde(default)]
    pub backend: Option<BackendIdentity>,
}

/// A discovery result before it is promoted to a [`ConsoleTarget`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleCandidate {
    pub addr: SocketAddr,
    pub kind: BackendKind,
    /// How the candidate was found.
    pub source: DiscoverySource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoverySource {
    SubnetProbe,
    Remembered,
    Manual,
}

// ---------------------------------------------------------------------------
// Package side
// ---------------------------------------------------------------------------

/// Content type byte from the PKG header (offset 0x74).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ContentKind {
    /// 0x1A — game data (app / patch / remaster).
    Gd,
    /// 0x1B — additional content data (AC data, themes).
    Ac,
    /// 0x1C — additional content without data.
    Al,
    /// 0x1E — delta patch.
    Dp,
    Unknown(u32),
}

impl ContentKind {
    pub fn from_raw(raw: u32) -> Self {
        match raw {
            0x1A => Self::Gd,
            0x1B => Self::Ac,
            0x1C => Self::Al,
            0x1E => Self::Dp,
            other => Self::Unknown(other),
        }
    }

    pub fn package_type(&self) -> Option<&'static str> {
        match self {
            Self::Gd => Some("PS4GD"),
            Self::Ac => Some("PS4AC"),
            Self::Al => Some("PS4AL"),
            Self::Dp => Some("PS4DP"),
            Self::Unknown(_) => None,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Gd => "Game".into(),
            Self::Ac => "Add-on / Theme".into(),
            Self::Al => "Add-on (no data)".into(),
            Self::Dp => "Delta patch".into(),
            Self::Unknown(v) => format!("Unknown (0x{v:X})"),
        }
    }
}

/// Metadata parsed from a PKG file header (and its PARAM.SFO when present).
/// See `docs/BASELINE.md` for the on-disk layout this mirrors.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PkgInfo {
    pub content_id: String,
    pub title_id: Option<String>,
    pub content_kind: ContentKind,
    pub is_patch: bool,
    /// Game title from PARAM.SFO (localized when available).
    pub title: Option<String>,
    pub app_version: Option<String>,
    pub package_version: Option<String>,
    pub category: Option<String>,
    /// `package_size` field from the header (offset 0x430).
    pub header_size: u64,
}

impl PkgInfo {
    /// Derive the title id from a content id like
    /// `UP0001-CUSA03041_00-REDEMPTION000002` (second dash-delimited field,
    /// truncated at the `_` suffix; 9 characters by format).
    pub fn title_id_from_content_id(content_id: &str) -> Option<String> {
        let mut parts = content_id.split('-');
        let _service = parts.next()?;
        let title = parts.next()?;
        let title = title.split('_').next().unwrap_or(title);
        if title.is_empty() {
            return None;
        }
        Some(title.to_string())
    }
}

/// One physical `.pkg` file on disk.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PartFile {
    pub id: PartId,
    pub path: PathBuf,
    /// Display file name (file stem).
    pub file_name: String,
    /// Actual size on disk in bytes.
    pub size: u64,
    /// Index within a multipart group (0-based) when detected.
    pub part_index: Option<u32>,
    /// Parsed metadata; `None` until parsed or if parsing failed.
    pub pkg: Option<PkgInfo>,
    /// Set when the file failed validation (unreadable, bad magic, ...).
    pub invalid_reason: Option<String>,
}

impl PartFile {
    pub fn display_title(&self) -> String {
        if let Some(pkg) = &self.pkg
            && let Some(t) = &pkg.title
        {
            return t.clone();
        }
        self.file_name.clone()
    }

    pub fn title_id(&self) -> Option<String> {
        self.pkg.as_ref().and_then(|p| p.title_id.clone())
    }
}

/// A package group: one or more parts installed as a single console task.
/// Multipart pieces must be in consecutive order.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QueueItem {
    pub id: ItemId,
    pub parts: Vec<PartFile>,
    pub state: ItemState,
    /// Human-readable error detail for failed/invalid states.
    pub error: Option<String>,
    /// Console-assigned task id once an install request succeeded.
    pub task_id: Option<TaskId>,
    /// Console-assigned v2 job id (NextGen-class backends); used for the
    /// per-job lifecycle endpoints instead of the task id.
    #[serde(default)]
    pub job_id: Option<i64>,
    /// Title echoed back by the console on install (when provided).
    pub console_title: Option<String>,
    pub progress: Option<ProgressView>,
    pub attempts: u32,
    pub added_at: u64,
    pub updated_at: u64,
}

impl QueueItem {
    pub fn total_size(&self) -> u64 {
        self.parts.iter().map(|p| p.size).sum()
    }

    pub fn is_multipart(&self) -> bool {
        self.parts.len() > 1
    }

    pub fn primary_part(&self) -> Option<&PartFile> {
        self.parts.first()
    }

    pub fn content_id(&self) -> Option<&str> {
        self.primary_part()
            .and_then(|p| p.pkg.as_ref())
            .map(|p| p.content_id.as_str())
    }

    /// A stable grouping key for duplicate detection: the parsed content id when
    /// available, otherwise the canonical path.
    pub fn dedup_key(&self) -> String {
        if let Some(cid) = self.content_id() {
            format!("cid:{cid}")
        } else if let Some(first) = self.parts.first() {
            format!("path:{}", first.path.display())
        } else {
            format!("empty:{}", self.id)
        }
    }
}

/// Explicit queue item state machine. Transitions are driven *only* by observed
/// evidence (HTTP results, progress counters, user commands) — never by
/// assumptions. See `docs/ARCHITECTURE.md` §4.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ItemState {
    /// Added, validated or awaiting validation; not started.
    Queued,
    /// Files being (re)validated / prepared.
    Preparing,
    /// Failed local validation (bad file, unsupported package) — not retryable
    /// without user action.
    Invalid,
    /// Install request in flight.
    Submitting,
    /// Console is downloading (progress observed, not finished).
    Transferring,
    /// Download finished; console reports the local copy phase.
    Installing,
    /// Paused by the user (backend confirmed when possible).
    Paused,
    /// Completed with evidence (full transfer + no error + copy phase done).
    Completed,
    /// Failed after retries / permanent error.
    Failed,
    /// Cancelled by the user.
    Cancelled,
    /// Outcome unknown (app restart, lost connection). Never auto-resubmitted.
    Unknown,
}

impl ItemState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Preparing => "Preparing",
            Self::Invalid => "Invalid package",
            Self::Submitting => "Submitting",
            Self::Transferring => "Transferring",
            Self::Installing => "Installing",
            Self::Paused => "Paused",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::Unknown => "Unknown",
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Preparing | Self::Submitting | Self::Transferring | Self::Installing
        )
    }

    pub fn is_finished(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Invalid
        )
    }
}

/// Progress as reported by the backend, plus client-side derivation (speed).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct ProgressView {
    /// 0.0..=100.0 overall transfer percent (transferred_total/length_total).
    pub percent: f32,
    pub transferred: u64,
    pub total: u64,
    /// Client-side computed throughput in bytes/s (EMA over poll deltas).
    pub speed_bps: u64,
    /// Backend-provided ETA seconds (total) when available.
    pub eta_sec: Option<u64>,
    /// Current multipart piece index (0-based) and count.
    pub piece_index: u32,
    pub piece_total: u32,
    /// Preparation phase percent reported by the console (BGFT).
    pub preparing_percent: i32,
    /// Post-download local copy percent reported by the console (BGFT).
    pub local_copy_percent: i32,
    /// Raw backend error code (non-zero means the task failed console-side).
    pub raw_error: i64,
}

// ---------------------------------------------------------------------------
// Events (engine → UI)
// ---------------------------------------------------------------------------

/// A point-in-time view of the whole application state, sent to the UI.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub items: Vec<QueueItem>,
    pub console: ConsoleInfo,
    pub server: Option<ServerInfo>,
    pub engine_busy: bool,
}

/// Console part of a [`Snapshot`].
#[derive(Debug, Clone, Default)]
pub struct ConsoleInfo {
    pub status: ConsoleStatus,
    pub target: Option<ConsoleTarget>,
}

/// Local file server part of a [`Snapshot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub bind: SocketAddr,
    /// Base URL the console should use, e.g. `http://192.168.1.20:53742`.
    pub base_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Info => write!(f, "INFO"),
            Self::Warn => write!(f, "WARN"),
            Self::Error => write!(f, "ERROR"),
        }
    }
}

/// Events emitted by the engine to the UI.
#[derive(Debug, Clone)]
pub enum Event {
    /// Coalesced full-state snapshot (queue + console + server).
    Snapshot(Snapshot),
    /// A single log line for the activity panel.
    Log(LogLevel, String),
    /// Discovery finished (may be empty).
    DiscoveryDone(Vec<ConsoleCandidate>),
}
