//! Engine: the single orchestrator that owns the tokio runtime, the queue, the
//! console connection, the file server and all background tasks.
//!
//! UI (or any frontend) talks to it through `Command` in / `Event` out —
//! see `docs/ARCHITECTURE.md` §4.1. The engine never touches UI code.

use std::{
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tokio::sync::{mpsc, watch};

use crate::{
    discovery,
    fileserver::FileServer,
    model::{
        BackendIdentity, ConsoleCandidate, ConsoleGeneration, ConsoleInfo, ConsoleStatus,
        ConsoleTarget, Event, ItemId, ItemState, LogLevel, PartId, ProgressView, QueueItem,
        ServerInfo, Snapshot, unix_ms,
    },
    pkg::{self, parse_pkg_file},
    queue::Queue,
    rpi::{RpiClient, RpiError, types::V2Job},
    settings::{FailurePolicy, RememberedConsole, Settings},
};

/// Commands from the frontend to the engine.
#[derive(Debug, Clone)]
pub enum Command {
    /// Add files and/or directories (dirs walked per `recursive`).
    AddPaths {
        paths: Vec<PathBuf>,
        recursive: bool,
    },
    Remove(ItemId),
    Move {
        id: ItemId,
        delta: i32,
    },
    /// Begin/continue processing the queue in order.
    Start,
    /// Pause the active console task and stop auto-advance.
    Pause,
    /// Resume the paused console task and auto-advance.
    Resume,
    /// Resume one specific paused item (per-row action).
    ResumeItem(ItemId),
    /// Requeue a finished/failed/cancelled/unknown item.
    Retry(ItemId),
    /// User-verified completion for an `Unknown` item (honest escape hatch).
    MarkCompleted(ItemId),
    /// Cancel an item (best-effort remote stop + unregister).
    Cancel(ItemId),
    ClearFinished,
    /// Probe and connect to a console at this address.
    Connect(SocketAddr),
    Disconnect,
    Discover,
    SetSettings(Box<Settings>),
    Shutdown,
}

/// Handle held by the frontend.
pub struct EngineHandle {
    runtime: tokio::runtime::Runtime,
    cmd_tx: mpsc::UnboundedSender<Command>,
    evt_rx: std::sync::mpsc::Receiver<Event>,
}

impl EngineHandle {
    /// Send a command (never blocks).
    pub fn send(&self, cmd: Command) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// Drain pending events (non-blocking).
    pub fn poll_events(&self) -> Vec<Event> {
        let mut out = Vec::new();
        while let Ok(ev) = self.evt_rx.try_recv() {
            out.push(ev);
        }
        out
    }

    /// Deterministic shutdown: cancels work, stops the server, flushes state,
    /// then drops the runtime. Blocks until the engine is fully stopped.
    ///
    /// Safe to call from any thread (GUI main thread, test harness, async
    /// context) — the runtime is shut down on a dedicated thread because
    /// dropping a tokio runtime inside an async context panics.
    pub fn shutdown(self) {
        let _ = self.cmd_tx.send(Command::Shutdown);
        let runtime = self.runtime;
        let handle = std::thread::Builder::new()
            .name("rpi-ng-shutdown".into())
            .spawn(move || {
                runtime.shutdown_timeout(Duration::from_secs(5));
            });
        if let Ok(h) = handle {
            let _ = h.join();
        }
    }
}

/// Spawn the engine with the given settings (persisting under the default
/// user data dir).
pub fn spawn(settings: Settings) -> EngineHandle {
    spawn_with(settings, None)
}

/// Spawn the engine with an explicit data directory (used by tests and
/// headless embedding; `None` = default user data dir).
pub fn spawn_with(settings: Settings, data_dir: Option<PathBuf>) -> EngineHandle {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Command>();
    let (evt_tx, evt_rx) = std::sync::mpsc::channel::<Event>();

    let settings = Arc::new(Mutex::new(settings));
    let data_dir = Arc::new(data_dir);
    runtime.spawn(async move {
        let (internal_tx, mut internal_rx) = mpsc::unbounded_channel();
        let mut inner = EngineInner::new(settings.clone(), evt_tx, internal_tx, data_dir.clone());
        let mut cmd_rx = cmd_rx;
        loop {
            tokio::select! {
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(Command::Shutdown) | None => break,
                        Some(c) => inner.handle(c).await,
                    }
                }
                Some(msg) = internal_rx.recv() => {
                    inner.handle_internal(msg).await;
                }
            }
            inner.emit_snapshot();
        }
        inner.shutdown().await;
    });

    EngineHandle {
        runtime,
        cmd_tx,
        evt_rx,
    }
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum Internal {
    ScanDone {
        files: Vec<PathBuf>,
    },
    ParseDone {
        results: Vec<(ItemId, PartId, Result<crate::model::PkgInfo, pkg::PkgError>)>,
    },
    ProbeDone {
        addr: SocketAddr,
        result: Result<ConsoleTarget, RpiError>,
    },
    DiscoveryDone(Vec<ConsoleCandidate>),
    SubmitResult {
        item: ItemId,
        result: Result<crate::rpi::types::InstallResponse, RpiError>,
    },
    SubmitRetry {
        item: ItemId,
        urls: Vec<String>,
    },
    ProgressUpdate {
        item: ItemId,
        result: Result<ProgressView, RpiError>,
    },
    /// v2 poller: the console's job entry, or `None` when the job vanished
    /// from the registry (console app restarted / slot reclaimed).
    V2JobUpdate {
        item: ItemId,
        result: Result<Option<V2Job>, RpiError>,
    },
}

struct ActiveTask {
    item_id: ItemId,
    task_id: i64,
    /// v2 job id when the backend tracks jobs (NextGen-class).
    job_id: Option<i64>,
    poll_cancel: watch::Sender<bool>,
}

struct EngineInner {
    settings: Arc<Mutex<Settings>>,
    data_dir: Arc<Option<PathBuf>>,
    evt: std::sync::mpsc::Sender<Event>,
    internal_tx: mpsc::UnboundedSender<Internal>,
    queue: Queue,
    console: Option<ConsoleTarget>,
    console_status: ConsoleStatus,
    client: Option<RpiClient>,
    server: Option<Arc<FileServer>>,
    running: bool,
    active: Option<ActiveTask>,
    /// (timestamp, transferred) of the last progress sample per active item.
    last_sample: Option<(Instant, u64)>,
    /// Consecutive transient poll failures while a task is active.
    poll_failures: u32,
    /// Consecutive full-transfer samples without a copy phase report.
    copy_phase_stalls: u32,
    scanning: bool,
}

const MAX_SUBMIT_ATTEMPTS: u32 = 3;
const MAX_WALK_FILES: usize = 2000;

impl EngineInner {
    fn new(
        settings: Arc<Mutex<Settings>>,
        evt: std::sync::mpsc::Sender<Event>,
        internal_tx: mpsc::UnboundedSender<Internal>,
        data_dir: Arc<Option<PathBuf>>,
    ) -> Self {
        let queue = Queue::load_in(data_dir.as_deref());
        Self {
            settings,
            data_dir,
            evt,
            internal_tx,
            queue,
            console: None,
            console_status: ConsoleStatus::None,
            client: None,
            server: None,
            running: false,
            active: None,
            last_sample: None,
            poll_failures: 0,
            copy_phase_stalls: 0,
            scanning: false,
        }
    }

    // -- plumbing ----------------------------------------------------------

    fn log(&self, level: LogLevel, msg: impl Into<String>) {
        let _ = self.evt.send(Event::Log(level, msg.into()));
    }

    fn emit_snapshot(&self) {
        let server = self.server.as_ref().map(|s| ServerInfo {
            bind: s.bind,
            base_url: s.base_url.clone(),
        });
        let snap = Snapshot {
            items: self.queue.items().to_vec(),
            console: ConsoleInfo {
                status: self.console_status.clone(),
                target: self.console.clone(),
            },
            server,
            engine_busy: self.queue.any_active() || self.scanning,
        };
        let _ = self.evt.send(Event::Snapshot(snap));
    }

    fn persist_queue(&mut self) {
        if let Err(e) = self.queue.save_in(self.data_dir.as_deref()) {
            self.log(LogLevel::Error, format!("queue save failed: {e}"));
        }
    }

    fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    // -- command handling ---------------------------------------------------

    async fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::AddPaths { paths, recursive } => self.cmd_add_paths(paths, recursive).await,
            Command::Remove(id) => self.cmd_remove(id).await,
            Command::Move { id, delta } => {
                if self.queue.move_item(id, delta) {
                    self.persist_queue();
                }
            }
            Command::Start => {
                self.running = true;
                self.maybe_start_next().await;
            }
            Command::Pause => {
                self.running = false;
                let ids = self
                    .active
                    .as_ref()
                    .map(|a| (a.task_id, a.item_id, a.job_id));
                if let Some((task_id, item_id, job_id)) = ids {
                    // Best-effort remote pause; the poller keeps watching so a
                    // completion or console-side error is still observed.
                    self.pause_remote(task_id, job_id).await;
                    if let Some(item) = self.queue.get_mut(item_id) {
                        item.state = ItemState::Paused;
                        item.updated_at = unix_ms();
                    }
                    self.persist_queue();
                    self.log(
                        LogLevel::Info,
                        "Paused (console pause was best-effort; auto-advance off)",
                    );
                }
            }
            Command::Resume => {
                // Prefer the actively monitored task; otherwise resume the
                // first paused item that still carries a console task id
                // (e.g. after a reconnect).
                let target = self.active.as_ref().map(|a| a.item_id).or_else(|| {
                    self.queue
                        .items()
                        .iter()
                        .find(|i| i.state == ItemState::Paused && i.task_id.is_some())
                        .map(|i| i.id)
                });
                if let Some(id) = target {
                    self.resume_item(id).await;
                }
                self.maybe_start_next().await;
            }
            Command::ResumeItem(id) => {
                self.resume_item(id).await;
                self.maybe_start_next().await;
            }
            Command::Retry(id) => {
                if self.queue.requeue(id) {
                    // Retry expresses "process this now" — it also un-parks a
                    // queue that was paused by the failure policy.
                    self.running = true;
                    self.persist_queue();
                    self.log(LogLevel::Info, format!("Requeued item #{id}"));
                    self.maybe_start_next().await;
                }
            }
            Command::MarkCompleted(id) => {
                if let Some(item) = self.queue.get_mut(id)
                    && item.state == ItemState::Unknown
                {
                    item.state = ItemState::Completed;
                    item.error = None;
                    item.updated_at = unix_ms();
                    self.persist_queue();
                    self.log(
                        LogLevel::Info,
                        format!("Item #{id} marked completed by user (was unknown)"),
                    );
                }
            }
            Command::Cancel(id) => self.cmd_cancel(id).await,
            Command::ClearFinished => {
                let removed_parts: Vec<PartId> = self
                    .queue
                    .items()
                    .iter()
                    .filter(|i| i.state.is_finished())
                    .flat_map(|i| i.parts.iter().map(|p| p.id))
                    .collect();
                let removed = self.queue.clear_finished();
                if let Some(server) = &self.server {
                    for pid in removed_parts {
                        server.unregister(pid);
                    }
                }
                self.persist_queue();
                self.log(
                    LogLevel::Info,
                    format!("Cleared {removed} finished item(s)"),
                );
            }
            Command::Connect(addr) => self.cmd_connect(addr).await,
            Command::Disconnect => {
                self.console = None;
                self.client = None;
                self.console_status = ConsoleStatus::None;
                self.stop_poller();
                if let Some(server) = self.server.take() {
                    server.shutdown();
                }
                // Items that were in flight can no longer be observed. Mark
                // them Unknown (never auto-resubmitted); reconcile() re-attaches
                // to console tasks on the next connect. Paused items keep their
                // task id and are resumable explicitly.
                let inflight: Vec<ItemId> = self
                    .queue
                    .items()
                    .iter()
                    .filter(|i| {
                        matches!(
                            i.state,
                            ItemState::Submitting | ItemState::Transferring | ItemState::Installing
                        )
                    })
                    .map(|i| i.id)
                    .collect();
                for id in &inflight {
                    if let Some(it) = self.queue.get_mut(*id) {
                        it.state = ItemState::Unknown;
                        it.error = Some(
                            "Connection closed while this item was in flight — the outcome is unknown; reconnect to re-attach, then verify on the console".into(),
                        );
                        it.updated_at = unix_ms();
                    }
                }
                if !inflight.is_empty() {
                    self.persist_queue();
                    self.log(
                        LogLevel::Warn,
                        format!(
                            "Disconnected with {} in-flight item(s) marked unknown",
                            inflight.len()
                        ),
                    );
                } else {
                    self.log(LogLevel::Info, "Disconnected");
                }
            }
            Command::Discover => {
                let settings = self.settings();
                let tx = self.internal_tx.clone();
                self.log(LogLevel::Info, "Scanning the local network for consoles…");
                tokio::spawn(async move {
                    let found = discovery::discover(&settings).await;
                    let _ = tx.send(Internal::DiscoveryDone(found));
                });
            }
            Command::SetSettings(s) => {
                let result = {
                    let mut guard = self.settings.lock().unwrap();
                    *guard = *s;
                    // Persist into the engine's data dir (test/headless runs
                    // must not touch the user's real settings).
                    guard.save_in(self.data_dir.as_deref())
                };
                if let Err(e) = result {
                    self.log(LogLevel::Error, format!("settings save failed: {e}"));
                }
                // Echo back so the UI's cached copy stays in sync.
                let _ = self.evt.send(Event::SettingsChanged(self.settings()));
            }
            Command::Shutdown => unreachable!("handled by the supervisor loop"),
        }
    }

    async fn cmd_add_paths(&mut self, paths: Vec<PathBuf>, recursive: bool) {
        self.scanning = true;
        let tx = self.internal_tx.clone();
        tokio::task::spawn_blocking(move || {
            let files = collect_pkg_files(&paths, recursive);
            let _ = tx.send(Internal::ScanDone { files });
        });
    }

    async fn cmd_remove(&mut self, id: ItemId) {
        // If active: stop first (best effort remote cleanup).
        if self.active.as_ref().map(|a| a.item_id) == Some(id) {
            self.stop_active_remote().await;
        }
        if let Some(item) = self.queue.remove(id) {
            if let Some(server) = &self.server {
                for part in &item.parts {
                    server.unregister(part.id);
                }
            }
            self.persist_queue();
            self.log(
                LogLevel::Info,
                format!(
                    "Removed item #{} ({})",
                    item.id,
                    item.primary_part()
                        .map(|p| p.file_name.clone())
                        .unwrap_or_default()
                ),
            );
        }
    }

    async fn cmd_cancel(&mut self, id: ItemId) {
        if self.active.as_ref().map(|a| a.item_id) == Some(id) {
            let task_id = self.active.as_ref().unwrap().task_id;
            self.stop_active_remote().await;
            if let Some(item) = self.queue.get_mut(id) {
                item.state = ItemState::Cancelled;
                item.updated_at = unix_ms();
                self.persist_queue();
                self.log(
                    LogLevel::Warn,
                    format!("Cancelled item #{id}; remote stop of task {task_id} was best-effort — verify on the console if it still appears"),
                );
            }
        } else if let Some(item) = self.queue.get_mut(id)
            && !item.state.is_active()
        {
            let task_id = item.task_id;
            let job_id = item.job_id;
            item.state = ItemState::Cancelled;
            item.updated_at = unix_ms();
            self.persist_queue();
            // A paused/unknown item may still hold a live console task:
            // clean it up best-effort too.
            if let Some(task_id) = task_id {
                self.stop_remote(task_id, job_id).await;
                self.log(
                    LogLevel::Warn,
                    format!("Cancelled item #{id}; remote stop was best-effort — verify on the console if it still appears"),
                );
            } else {
                self.log(LogLevel::Info, format!("Cancelled item #{id}"));
            }
        }
        self.maybe_start_next().await;
    }

    async fn cmd_connect(&mut self, addr: SocketAddr) {
        self.console_status = ConsoleStatus::Probing;
        self.log(LogLevel::Info, format!("Connecting to {addr}…"));
        let tx = self.internal_tx.clone();
        tokio::spawn(async move {
            let client = RpiClient::new(addr, Duration::from_secs(6));
            let result = client.probe().await.map(|outcome| ConsoleTarget {
                addr,
                generation: ConsoleGeneration::Ps4,
                kind: outcome.kind,
                caps: outcome.caps,
                label: None,
                backend: outcome.identity.map(|sys| BackendIdentity {
                    name: sys.name,
                    version: sys.version,
                }),
            });
            let _ = tx.send(Internal::ProbeDone { addr, result });
        });
    }

    // -- internal message handling ------------------------------------------

    async fn handle_internal(&mut self, msg: Internal) {
        match msg {
            Internal::ScanDone { files } => {
                self.scanning = false;
                if files.len() >= MAX_WALK_FILES {
                    self.log(
                        LogLevel::Warn,
                        format!("Scan capped at {MAX_WALK_FILES} files — some packages may be missing; add the rest from another folder"),
                    );
                }
                let (added, skipped) = self.queue.add_paths(files);
                for s in skipped {
                    self.log(LogLevel::Warn, format!("Skipped: {s}"));
                }
                self.log(LogLevel::Info, format!("Added {} item(s)", added.len()));
                self.persist_queue();
                // Kick metadata parsing in the background.
                let parts: Vec<(ItemId, PartId, PathBuf)> = self
                    .queue
                    .items()
                    .iter()
                    .filter(|i| added.contains(&i.id))
                    .flat_map(|i| i.parts.iter().map(|p| (i.id, p.id, p.path.clone())))
                    .collect();
                let tx = self.internal_tx.clone();
                tokio::task::spawn_blocking(move || {
                    let results = parts
                        .into_iter()
                        .map(|(item, part_id, path)| (item, part_id, parse_pkg_file(&path)))
                        .collect();
                    let _ = tx.send(Internal::ParseDone { results });
                });
            }
            Internal::ParseDone { results } => {
                for (item_id, part_id, result) in results {
                    if let Some(item) = self.queue.get_mut(item_id)
                        && let Some(part) = item.parts.iter_mut().find(|p| p.id == part_id)
                    {
                        match result {
                            Ok(info) => {
                                part.pkg = Some(info);
                                part.invalid_reason = None;
                            }
                            Err(e) => {
                                part.invalid_reason = Some(e.to_string());
                            }
                        }
                    }
                }
                self.finalize_parse().await;
                self.persist_queue();
                self.maybe_start_next().await;
            }
            Internal::ProbeDone { addr, result } => match result {
                Ok(target) => {
                    self.console = Some(target.clone());
                    self.console_status = ConsoleStatus::Connected;
                    self.client = Some(RpiClient::new(addr, Duration::from_secs(15)));
                    let backend = target
                        .backend
                        .as_ref()
                        .map(|b| format!(" ({b})"))
                        .unwrap_or_default();
                    let v2 = if target.caps.api_v2 { ", v2 API" } else { "" };
                    self.log(
                        LogLevel::Info,
                        format!("Connected to {addr} ({}{v2}){backend}", target.kind),
                    );
                    self.remember_console(addr);
                    self.start_server_for(addr).await;
                    self.reconcile().await;
                    self.maybe_start_next().await;
                }
                Err(e) => {
                    self.console_status = ConsoleStatus::Incompatible(e.to_string());
                    self.log(LogLevel::Error, format!("Cannot use {addr}: {e}"));
                }
            },
            Internal::DiscoveryDone(found) => {
                if found.is_empty() {
                    self.log(LogLevel::Warn, "No consoles found on the local network");
                } else {
                    self.log(
                        LogLevel::Info,
                        format!("Discovery: {} console(s)", found.len()),
                    );
                }
                let _ = self.evt.send(Event::DiscoveryDone(found));
            }
            Internal::SubmitResult { item, result } => {
                match result {
                    Ok(resp) => {
                        let task_id = resp.task_id;
                        // The console may have answered after a disconnect or a
                        // user cancel: never let a late response overwrite those
                        // decisions.
                        let still_submitting = self
                            .queue
                            .get(item)
                            .map(|i| i.state == ItemState::Submitting)
                            .unwrap_or(false);
                        if !still_submitting {
                            if let Some(it) = self.queue.get_mut(item)
                                && it.state == ItemState::Unknown
                                && task_id > 0
                            {
                                // Disconnected while the request was in flight;
                                // remember the task id for reconcile().
                                it.task_id = Some(task_id);
                                it.job_id = resp.job_id;
                                it.updated_at = unix_ms();
                                self.persist_queue();
                            }
                            // Cancelled/removed meanwhile: best-effort cleanup so the
                            // console does not keep a ghost task.
                            if task_id > 0 {
                                self.stop_remote(task_id, resp.job_id).await;
                            }
                            return;
                        }
                        if task_id == -1 {
                            // Both dialects use task_id -1 for "already
                            // installed": the console reports the outcome
                            // directly, no task to track.
                            if let Some(it) = self.queue.get_mut(item) {
                                it.task_id = None;
                                it.job_id = resp.job_id;
                                it.console_title = resp.title.clone();
                                it.state = ItemState::Completed;
                                it.error = None;
                                it.updated_at = unix_ms();
                            }
                            self.persist_queue();
                            self.log(
                                LogLevel::Info,
                                format!("Item #{item}: console reports already installed"),
                            );
                            self.maybe_start_next().await;
                            return;
                        }
                        let job_id = resp.job_id;
                        if let Some(it) = self.queue.get_mut(item) {
                            it.task_id = Some(task_id);
                            it.job_id = job_id;
                            it.console_title = resp.title.clone();
                            it.state = ItemState::Transferring;
                            it.updated_at = unix_ms();
                        }
                        self.poll_failures = 0;
                        self.copy_phase_stalls = 0;
                        self.last_sample = None;
                        self.spawn_poller(item, task_id, job_id);
                        self.persist_queue();
                        self.log(
                            LogLevel::Info,
                            format!("Console accepted item #{item} (task {task_id}) — downloading"),
                        );
                        // A pause may have arrived while the request was in
                        // flight (there was no task id to pause yet): apply it now.
                        if !self.running {
                            self.pause_remote(task_id, job_id).await;
                            if let Some(it) = self.queue.get_mut(item) {
                                it.state = ItemState::Paused;
                                it.updated_at = unix_ms();
                            }
                            self.persist_queue();
                            self.log(
                            LogLevel::Info,
                            "Console accepted the request; paused right away (pause arrived during submit)",
                        );
                        }
                    }
                    Err(e) => {
                        // A backend that answered "success" without a task id did
                        // accept the install — the outcome is simply untrackable.
                        // Reporting that as Failed would be dishonest; mark it
                        // Unknown and let the user verify on the console.
                        if matches!(e, RpiError::MissingTaskId) {
                            if let Some(it) = self.queue.get_mut(item) {
                                it.state = ItemState::Unknown;
                                it.error = Some(
                                "The console accepted the install but did not return a task id (fork dialect) — verify on the console, then mark completed or retry".into(),
                            );
                                it.updated_at = unix_ms();
                            }
                            self.persist_queue();
                            self.log(
                            LogLevel::Warn,
                            format!("Item #{item}: install accepted without a task id — outcome unknown"),
                        );
                            self.maybe_start_next().await;
                            return;
                        }
                        let attempts = self.queue.get(item).map(|i| i.attempts).unwrap_or(0);
                        let transient = e.is_transient();
                        if transient && attempts + 1 < MAX_SUBMIT_ATTEMPTS {
                            if let Some(it) = self.queue.get_mut(item) {
                                it.attempts = attempts + 1;
                                it.error = Some(e.to_string());
                            }
                            let urls = self.rebuild_urls(item).unwrap_or_default();
                            let backoff = Duration::from_secs(1u64 << attempts);
                            self.log(
                                LogLevel::Warn,
                                format!(
                                    "Submit failed ({e}); retrying in {backoff:?} (attempt {}/{})",
                                    attempts + 2,
                                    MAX_SUBMIT_ATTEMPTS
                                ),
                            );
                            let tx = self.internal_tx.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep(backoff).await;
                                let _ = tx.send(Internal::SubmitRetry { item, urls });
                            });
                        } else {
                            if let Some(it) = self.queue.get_mut(item) {
                                it.state = ItemState::Failed;
                                it.error = Some(e.to_string());
                                it.updated_at = unix_ms();
                            }
                            self.persist_queue();
                            self.log(LogLevel::Error, format!("Item #{item} failed: {e}"));
                            self.on_transfer_failure().await;
                        }
                    }
                }
            }
            Internal::SubmitRetry { item, urls } => {
                // Only retry if the item is still submitting and console alive.
                let still = matches!(
                    self.queue.get(item).map(|i| i.state),
                    Some(ItemState::Submitting)
                );
                if still && self.console.is_some() {
                    self.submit(item, urls).await;
                }
            }
            Internal::ProgressUpdate { item, result } => {
                self.handle_progress(item, result).await;
            }
            Internal::V2JobUpdate { item, result } => {
                self.handle_v2_job_update(item, result).await;
            }
        }
    }

    /// Post-parse validation: multipart consistency, duplicate content ids,
    /// invalid flagging.
    async fn finalize_parse(&mut self) {
        // Multipart consistency + invalid flagging.
        let ids: Vec<ItemId> = self.queue.items().iter().map(|i| i.id).collect();
        for id in ids {
            let Some(item) = self.queue.get_mut(id) else {
                continue;
            };
            if item.state.is_finished() {
                continue;
            }
            // All parts parsed?
            let any_invalid = item.parts.iter().any(|p| p.invalid_reason.is_some());
            let all_parsed = item.parts.iter().all(|p| p.pkg.is_some());
            if any_invalid || !all_parsed {
                if matches!(item.state, ItemState::Queued | ItemState::Preparing) {
                    let reason = item
                        .parts
                        .iter()
                        .filter_map(|p| p.invalid_reason.clone())
                        .next()
                        .unwrap_or_else(|| "metadata unavailable".into());
                    item.state = ItemState::Invalid;
                    item.error = Some(reason);
                }
                continue;
            }
            // Content id consistency across pieces.
            let cids: Vec<String> = item
                .parts
                .iter()
                .filter_map(|p| p.pkg.as_ref().map(|x| x.content_id.clone()))
                .collect();
            if cids.len() > 1 && cids.windows(2).any(|w| w[0] != w[1]) {
                item.error = Some(
                    "Multipart pieces have different content ids — grouping looks wrong".into(),
                );
                if matches!(item.state, ItemState::Queued | ItemState::Preparing) {
                    item.state = ItemState::Invalid;
                }
                continue;
            }
            // Missing piece detection: indices must be 0..n-1 consecutive.
            if item.is_multipart() {
                let idxs: Vec<u32> = item.parts.iter().filter_map(|p| p.part_index).collect();
                let expected: Vec<u32> = (0..idxs.len() as u32).collect();
                if idxs != expected {
                    item.error =
                        Some("Multipart pieces are not consecutive — a part seems missing".into());
                    if matches!(item.state, ItemState::Queued | ItemState::Preparing) {
                        item.state = ItemState::Invalid;
                    }
                }
            }
        }

        // Duplicate content ids among live items → mark later ones invalid.
        for (_first, dup) in self.queue.duplicates_by_content_id() {
            if let Some(item) = self.queue.get_mut(dup)
                && matches!(item.state, ItemState::Queued | ItemState::Preparing)
            {
                item.state = ItemState::Invalid;
                item.error = Some(format!(
                    "Duplicate content id — same package is already queued (item #{_first})"
                ));
                self.log(
                    LogLevel::Warn,
                    format!("Duplicate content id on item #{dup}"),
                );
            }
        }
    }

    // -- submission ----------------------------------------------------------

    /// Re-register an item's parts with the file server and return their URLs
    /// in install order.
    ///
    /// Part ids are *server-scoped*: a fresh server (new connect, restart)
    /// starts counting from zero again, while a restored queue may carry ids
    /// from an earlier server. The registration id the server hands back is
    /// therefore written back into the queue — building URLs from stale ids
    /// would silently stall the queue (`url_for` would miss).
    fn rebuild_urls(&mut self, item: ItemId) -> Option<Vec<String>> {
        let server = self.server.clone()?;
        let queue_item = self.queue.get_mut(item)?;
        let mut urls = Vec::with_capacity(queue_item.parts.len());
        for part in &mut queue_item.parts {
            let id = server.register(part.path.clone(), Some(part.file_name.clone()));
            part.id = id;
            urls.push(server.url_for(id)?);
        }
        Some(urls)
    }

    /// Ensure parts are registered, then POST the install request.
    /// v2 backends install through `/api/v2/install`; legacy through
    /// `/api/install`.
    async fn submit(&mut self, item: ItemId, urls: Vec<String>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let use_v2 = self.v2_mode();
        let tx = self.internal_tx.clone();
        tokio::spawn(async move {
            let result = if use_v2 {
                client.v2_install(&urls).await
            } else {
                client.install_direct(&urls).await
            };
            let _ = tx.send(Internal::SubmitResult { item, result });
        });
    }

    /// Start the next queued item if the engine is running, connected, the
    /// server is up and no task is active. Invalid items are skipped in a loop.
    async fn maybe_start_next(&mut self) {
        loop {
            if !self.running
                || self.active.is_some()
                || self.console.is_none()
                || self.server.is_none()
                || !matches!(self.console_status, ConsoleStatus::Connected)
            {
                return;
            }
            let Some(id) = self.queue.next_queued() else {
                return;
            };
            // Prepare: files still exist.
            let missing: Option<PathBuf> = {
                let item = self.queue.get(id).expect("queued item exists");
                item.parts.iter().find_map(|p| {
                    if std::fs::metadata(&p.path).is_err() {
                        Some(p.path.clone())
                    } else {
                        None
                    }
                })
            };
            if let Some(path) = missing {
                if let Some(item) = self.queue.get_mut(id) {
                    item.state = ItemState::Invalid;
                    item.error = Some(format!("file vanished: {}", path.display()));
                    item.updated_at = unix_ms();
                }
                self.persist_queue();
                self.log(LogLevel::Error, format!("Item #{id}: source file is gone"));
                continue; // try the next queued item
            }
            let Some(urls) = self.rebuild_urls(id) else {
                self.log(
                    LogLevel::Error,
                    "Could not register package files with the local server — install cannot proceed",
                );
                return;
            };
            if let Some(item) = self.queue.get_mut(id) {
                item.state = ItemState::Submitting;
                item.updated_at = unix_ms();
            }
            self.persist_queue();
            self.log(
                LogLevel::Info,
                format!("Submitting item #{id} ({})", urls.len()),
            );
            self.submit(id, urls).await;
            return;
        }
    }

    // -- progress -------------------------------------------------------------

    fn spawn_poller(&mut self, item: ItemId, task_id: i64, job_id: Option<i64>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        self.active = Some(ActiveTask {
            item_id: item,
            task_id,
            job_id,
            poll_cancel: cancel_tx,
        });
        let interval = self.settings().poll_interval;
        let tx = self.internal_tx.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {},
                    _ = cancel_rx.wait_for(|&c| c) => break,
                }
                if job_id.is_some() {
                    // v2: one jobs request covers every tracked item; find
                    // this item's job by id (task id as a safety net).
                    let result = client.v2_jobs().await.map(|jobs| {
                        jobs.into_iter()
                            .find(|j| Some(j.id) == job_id || j.bgft_task_id == task_id)
                    });
                    let _ = tx.send(Internal::V2JobUpdate { item, result });
                } else {
                    let result = client.task_progress(task_id).await;
                    let _ = tx.send(Internal::ProgressUpdate { item, result });
                }
            }
        });
    }

    fn stop_poller(&mut self) {
        if let Some(active) = self.active.take() {
            let _ = active.poll_cancel.send(true);
        }
        self.last_sample = None;
    }

    async fn handle_progress(&mut self, item: ItemId, result: Result<ProgressView, RpiError>) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        if active.item_id != item {
            return; // stale poll for a task we no longer track
        }
        match result {
            Ok(mut pv) => {
                self.poll_failures = 0;
                // Speed from delta.
                let now = Instant::now();
                if let Some((t0, b0)) = self.last_sample {
                    let dt = now.duration_since(t0).as_secs_f64();
                    if dt >= 0.4 && pv.transferred >= b0 {
                        let ema = ((pv.transferred - b0) as f64 / dt * 0.4
                            + pv.speed_bps as f64 * 0.6) as u64;
                        pv.speed_bps = ema;
                    }
                }
                self.last_sample = Some((now, pv.transferred));

                let total = pv.total;
                let transferred = pv.transferred;
                let copy = pv.local_copy_percent;
                let err = pv.raw_error;

                let Some(it) = self.queue.get_mut(item) else {
                    return;
                };
                it.progress = Some(pv);
                it.updated_at = unix_ms();

                if err != 0 {
                    // Console-side failure with evidence.
                    let gloss =
                        crate::rpi::explain_error_code(err).unwrap_or("unknown console error");
                    it.state = ItemState::Failed;
                    it.error = Some(format!("console error 0x{err:X}: {gloss}"));
                    self.on_transfer_failure().await;
                    return;
                }

                if copy >= 100 {
                    it.state = ItemState::Completed;
                    it.error = None;
                    self.log(
                        LogLevel::Info,
                        format!("Item #{item} completed (transfer + copy phases finished)"),
                    );
                    self.stop_poller_and_continue().await;
                    return;
                }

                if total > 0 && transferred >= total {
                    it.state = ItemState::Installing;
                    self.copy_phase_stalls += 1;
                    if self.copy_phase_stalls == 30 {
                        self.log(
                            LogLevel::Warn,
                            "Transfer finished but the console does not report the copy phase yet — keep the console awake and check its notifications",
                        );
                    }
                } else if it.state != ItemState::Paused {
                    // Keep the user's Paused state: the console still reports
                    // progress for a paused task, but that must not un-pause
                    // the item in the UI. Terminal transitions above
                    // (completion / console error) still apply.
                    it.state = ItemState::Transferring;
                    self.copy_phase_stalls = 0;
                }
            }
            Err(e) => {
                self.handle_poll_error(item, e).await;
            }
        }
    }

    /// Error handling shared by the legacy and v2 pollers: transient vs
    /// permanent errors, and the "task no longer known" terminal case.
    async fn handle_poll_error(&mut self, item: ItemId, e: RpiError) {
        let transient = e.is_transient();
        let item_gone = matches!(
            &e,
            RpiError::BackendError { code, .. } if *code == 0x8099_0019
        );
        if item_gone {
            if let Some(it) = self.queue.get_mut(item) {
                it.state = ItemState::Unknown;
                it.error = Some(
                    "The console no longer knows this task (it may have completed or been removed) — verify on the console, then retry or mark completed"
                        .into(),
                );
            }
            self.stop_poller();
            self.persist_queue();
            self.maybe_start_next().await;
            return;
        }
        if transient {
            self.poll_failures += 1;
            if self.poll_failures == 3 {
                self.console_status = ConsoleStatus::Lost;
                self.log(
                    LogLevel::Error,
                    format!("Console stopped answering while installing ({e}). Keep the installer app in the foreground on the console; reconnect to resume monitoring."),
                );
            }
        } else {
            if let Some(it) = self.queue.get_mut(item) {
                it.error = Some(format!("progress error: {e}"));
            }
            self.log(LogLevel::Warn, format!("Progress error: {e}"));
        }
    }

    /// v2 poller: fold a job entry (or its absence) into the item state.
    ///
    /// Terminal job states come straight from the console's own task manager
    /// (`taskman.c`), so they count as evidence: `completed` requires the
    /// console to have observed full transfer without error, `failed` carries
    /// the console error code. A job missing from the list means the console
    /// app restarted or reclaimed the slot — the outcome is genuinely unknown.
    async fn handle_v2_job_update(
        &mut self,
        item: ItemId,
        result: Result<Option<V2Job>, RpiError>,
    ) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        if active.item_id != item {
            return; // stale poll for a job we no longer track
        }
        let job = match result {
            Ok(job) => job,
            Err(e) => {
                self.handle_poll_error(item, e).await;
                return;
            }
        };
        let Some(job) = job else {
            if let Some(it) = self.queue.get_mut(item) {
                it.state = ItemState::Unknown;
                it.error = Some(
                    "The console no longer lists this job (its app may have restarted) — verify on the console, then retry or mark completed"
                        .into(),
                );
            }
            self.stop_poller();
            self.persist_queue();
            self.maybe_start_next().await;
            return;
        };
        self.poll_failures = 0;

        use crate::rpi::types::v2_states as st;
        let pv = v2_job_progress(&job);

        let Some(it) = self.queue.get_mut(item) else {
            return;
        };
        it.progress = Some(pv);
        it.updated_at = unix_ms();
        if it.task_id.is_none() && job.bgft_task_id > 0 {
            it.task_id = Some(job.bgft_task_id);
        }
        if it.job_id.is_none() {
            it.job_id = Some(job.id);
        }

        let transferred_done = job.length_total > 0 && job.transferred >= job.length_total;
        match job.state.as_str() {
            st::COMPLETED | st::ALREADY_INSTALLED => {
                it.state = ItemState::Completed;
                it.error = None;
                self.log(
                    LogLevel::Info,
                    format!("Item #{item} completed (console job state: {})", job.state),
                );
                self.stop_poller_and_continue().await;
            }
            st::FAILED => {
                let gloss =
                    crate::rpi::explain_error_code(job.error).unwrap_or("unknown console error");
                it.state = ItemState::Failed;
                it.error = Some(format!("console error 0x{:X}: {gloss}", job.error));
                self.log(
                    LogLevel::Error,
                    format!("Item #{item} failed: console job error 0x{:X}", job.error),
                );
                self.on_transfer_failure().await;
            }
            st::STOPPED => {
                it.state = ItemState::Cancelled;
                it.error = Some("Console reports the job was stopped".into());
                self.log(
                    LogLevel::Warn,
                    format!("Item #{item}: console reports the job was stopped"),
                );
                self.stop_poller_and_continue().await;
            }
            st::PAUSED => {
                // Console-side pause (dashboard or this app): keep Paused.
                it.state = ItemState::Paused;
            }
            _ => {
                if transferred_done {
                    it.state = ItemState::Installing;
                } else {
                    it.state = ItemState::Transferring;
                }
            }
        }
    }

    /// Stop the active poller, then auto-advance if enabled.
    async fn stop_poller_and_continue(&mut self) {
        self.stop_poller();
        self.persist_queue();
        if self.settings().auto_advance {
            self.maybe_start_next().await;
        }
    }

    /// A transfer failed. Honor the failure policy: keep the queue moving, or
    /// park it until the user explicitly restarts (Start queue / Retry).
    async fn on_transfer_failure(&mut self) {
        self.stop_poller();
        self.persist_queue();
        match self.settings().on_failure {
            FailurePolicy::Continue => {
                if self.settings().auto_advance {
                    self.maybe_start_next().await;
                }
            }
            FailurePolicy::Pause => {
                self.running = false;
                self.log(
                    LogLevel::Warn,
                    "Queue paused: a package failed to transfer (setting: pause on \
                     failure). Retry the item or press Start queue to continue.",
                );
            }
        }
    }

    // -- remote control --------------------------------------------------------

    /// Resume one paused item: ask the console to continue its task and
    /// re-attach a poller when this item is not the monitored one. Honors the
    /// single-active-task invariant.
    async fn resume_item(&mut self, id: ItemId) {
        let Some((task_id, job_id)) = self.queue.get(id).map(|i| (i.task_id, i.job_id)) else {
            self.log(
                LogLevel::Warn,
                format!("Item #{id} has no console task to resume — use Retry instead"),
            );
            return;
        };
        let Some(task_id) = task_id else {
            self.log(
                LogLevel::Warn,
                format!("Item #{id} has no console task to resume — use Retry instead"),
            );
            return;
        };
        let is_monitored = self.active.as_ref().map(|a| a.item_id) == Some(id);
        if !is_monitored && self.active.is_some() {
            self.log(
                LogLevel::Warn,
                "Another item is already being monitored — wait for it to finish first",
            );
            return;
        }
        self.running = true;
        self.resume_remote(task_id, job_id).await;
        if let Some(item) = self.queue.get_mut(id) {
            item.state = ItemState::Transferring;
            item.updated_at = unix_ms();
        }
        if !is_monitored {
            self.spawn_poller(id, task_id, job_id);
        }
        self.persist_queue();
        self.log(
            LogLevel::Info,
            format!("Resumed item #{id} (task {task_id})"),
        );
    }

    /// The connected backend speaks the v2 job API right now.
    fn v2_mode(&self) -> bool {
        self.console
            .as_ref()
            .map(|c| c.caps.api_v2)
            .unwrap_or(false)
    }

    /// Pause the console-side task. The endpoint is chosen by the CURRENT
    /// backend capabilities: an item carrying a v2 job id may be resumed
    /// after reconnecting to a legacy-only backend, and vice versa.
    async fn pause_remote(&mut self, task_id: i64, job_id: Option<i64>) {
        if let Some(client) = self.client.clone() {
            let _ = match (self.v2_mode(), job_id) {
                (true, Some(job)) => client.v2_job_op(job, "pause").await,
                _ => client.pause_task(task_id).await,
            };
        }
    }

    async fn resume_remote(&mut self, task_id: i64, job_id: Option<i64>) {
        if let Some(client) = self.client.clone() {
            let _ = match (self.v2_mode(), job_id) {
                (true, Some(job)) => client.v2_job_op(job, "resume").await,
                _ => client.resume_task(task_id).await,
            };
        }
    }

    /// Stop a console-side task or job (best-effort).
    async fn stop_remote(&mut self, task_id: i64, job_id: Option<i64>) {
        if let Some(client) = self.client.clone() {
            match (self.v2_mode(), job_id) {
                (true, Some(job)) => {
                    let _ = client.v2_job_op(job, "stop").await;
                }
                _ => {
                    let _ = client.stop_task(task_id).await;
                    let _ = client.unregister_task(task_id).await;
                }
            }
        }
    }

    /// Best-effort console-side cleanup for the active task.
    async fn stop_active_remote(&mut self) {
        if let Some(active) = self.active.take() {
            let _ = active.poll_cancel.send(true);
            self.stop_remote(active.task_id, active.job_id).await;
        }
        self.last_sample = None;
    }

    // -- connection lifecycle ----------------------------------------------------

    fn remember_console(&mut self, addr: SocketAddr) {
        let snapshot = {
            let mut s = self.settings.lock().unwrap();
            s.remembered.retain(|r| r.addr != addr);
            s.remembered
                .insert(0, RememberedConsole { addr, label: None });
            if s.remembered.len() > 8 {
                s.remembered.truncate(8);
            }
            let _ = s.save_in(self.data_dir.as_deref());
            s.clone()
        };
        // The UI keeps a cached copy of the settings; broadcast the new
        // remembered-console list so it never goes stale.
        let _ = self.evt.send(Event::SettingsChanged(snapshot));
    }

    /// Start (or restart) the file server on an interface that can reach the
    /// console; never `0.0.0.0` silently. A fixed port may still be held for a
    /// few milliseconds by the previous server's shutdown, so binding is
    /// retried briefly before giving up.
    async fn start_server_for(&mut self, console_addr: SocketAddr) {
        // Stop any previous server: parts are re-registered on demand.
        if let Some(old) = self.server.take() {
            old.shutdown();
        }
        let bind_ip = pick_bind_ip(console_addr.ip());
        let fixed_port = self.settings().server_port;
        let port = fixed_port.unwrap_or(0);
        let bind = SocketAddr::new(bind_ip, port);
        let attempts = if fixed_port.is_some() { 8 } else { 1 };
        let mut last_err = None;
        let mut server = None;
        for attempt in 0..attempts {
            match FileServer::start(bind).await {
                Ok(s) => {
                    server = Some(s);
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    if attempt + 1 < attempts {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                }
            }
        }
        match server {
            Some(s) => {
                let s = Arc::new(s);
                self.log(
                    LogLevel::Info,
                    format!(
                        "Serving packages from {} (console downloads from here)",
                        s.base_url
                    ),
                );
                if bind_ip.is_unspecified() {
                    self.log(
                        LogLevel::Warn,
                        "No local interface matched the console subnet — bound to all interfaces. If the console cannot download, check your firewall.",
                    );
                }
                self.server = Some(s);
            }
            None => {
                self.log(
                    LogLevel::Error,
                    format!("Could not start the local file server on {bind}: {} — installs cannot proceed", last_err.map(|e| e.to_string()).unwrap_or_default()),
                );
            }
        }
    }

    /// Reconcile `Unknown`/`Paused` items against the console after connecting:
    /// re-attach a poller to ONE task that still exists (the engine monitors a
    /// single active task); the rest keep their state with a hint so the user
    /// resumes them deliberately. Never resubmits blindly.
    async fn reconcile(&mut self) {
        if self.v2_mode() {
            self.reconcile_v2().await;
        } else {
            self.reconcile_legacy().await;
        }
    }

    /// v2 reconcile: one jobs request replaces the find_task/per-task-progress
    /// probing. Jobs are matched by task id first, content id second; terminal
    /// job states resolve `Unknown` items directly.
    async fn reconcile_v2(&mut self) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let jobs = match client.v2_jobs().await {
            Ok(jobs) => jobs,
            Err(e) => {
                self.log(
                    LogLevel::Warn,
                    format!("Reconcile: v2 jobs unavailable ({e}); falling back to task probing"),
                );
                self.reconcile_legacy().await;
                return;
            }
        };

        struct Pending {
            id: ItemId,
            task_id: Option<i64>,
            job_id: Option<i64>,
            content_id: Option<String>,
        }
        let items: Vec<Pending> = self
            .queue
            .items()
            .iter()
            .filter(|i| matches!(i.state, ItemState::Unknown | ItemState::Paused))
            .map(|i| Pending {
                id: i.id,
                task_id: i.task_id,
                job_id: i.job_id,
                content_id: i.content_id().map(|c| c.to_string()),
            })
            .collect();

        let mut attached = false;
        for pending in items {
            let (id, task_id, job_id, content_id) = (
                pending.id,
                pending.task_id,
                pending.job_id,
                pending.content_id,
            );
            // Match by task id (or remembered job id), then content id.
            let found = jobs.iter().find(|j| {
                matches!(task_id, Some(t) if t > 0 && j.bgft_task_id == t)
                    || matches!(job_id, Some(jid) if j.id == jid)
            });
            let found = match found {
                Some(j) => Some(j),
                None => jobs.iter().find(|j| {
                    content_id
                        .as_deref()
                        .is_some_and(|c| !c.is_empty() && j.content_id == c)
                }),
            };
            let Some(job) = found else {
                if let Some(it) = self.queue.get_mut(id) {
                    it.error = Some(
                        "The console does not report this job anymore — verify on the console, then retry or mark completed"
                            .into(),
                    );
                }
                continue;
            };

            let task_id = if job.bgft_task_id > 0 {
                Some(job.bgft_task_id)
            } else {
                task_id
            };
            if let Some(it) = self.queue.get_mut(id) {
                it.task_id = task_id;
                it.job_id = Some(job.id);
                it.error = None;
            }

            use crate::rpi::types::v2_states as st;
            let progress = v2_job_progress(job);
            match job.state.as_str() {
                st::COMPLETED | st::ALREADY_INSTALLED => {
                    if let Some(it) = self.queue.get_mut(id) {
                        it.state = ItemState::Completed;
                        it.error = None;
                        it.progress = Some(progress);
                        it.updated_at = unix_ms();
                    }
                    self.log(
                        LogLevel::Info,
                        format!(
                            "Item #{id} already finished on the console (job state: {})",
                            job.state
                        ),
                    );
                }
                st::FAILED => {
                    if let Some(it) = self.queue.get_mut(id) {
                        it.state = ItemState::Failed;
                        it.error = Some(format!("console error 0x{:X}", job.error));
                        it.updated_at = unix_ms();
                    }
                    self.log(
                        LogLevel::Warn,
                        format!(
                            "Item #{id} failed on the console while disconnected (error 0x{:X})",
                            job.error
                        ),
                    );
                }
                st::STOPPED => {
                    if let Some(it) = self.queue.get_mut(id) {
                        it.state = ItemState::Cancelled;
                        it.error = Some("Console reports the job was stopped".into());
                        it.updated_at = unix_ms();
                    }
                }
                st::PAUSED => {
                    if let Some(it) = self.queue.get_mut(id) {
                        it.state = ItemState::Paused;
                    }
                }
                _ => {
                    // Still active console-side.
                    if attached {
                        if let Some(it) = self.queue.get_mut(id) {
                            it.error = Some(
                                "Console still reports this job; it will be resumed after the monitored item finishes"
                                    .into(),
                            );
                        }
                        self.log(
                            LogLevel::Info,
                            format!(
                                "Console still reports item #{id} (job {}); it will be resumed after the monitored item finishes",
                                job.id
                            ),
                        );
                        continue;
                    }
                    if let Some(it) = self.queue.get_mut(id) {
                        it.state = if job.length_total > 0 && job.transferred >= job.length_total {
                            ItemState::Installing
                        } else {
                            ItemState::Transferring
                        };
                        it.progress = Some(progress);
                    }
                    if let Some(tid) = task_id {
                        self.spawn_poller(id, tid, Some(job.id));
                        attached = true;
                        self.log(
                            LogLevel::Info,
                            format!(
                                "Re-attached item #{id} to console job {} (task {tid})",
                                job.id
                            ),
                        );
                    }
                }
            }
        }
        self.persist_queue();
    }

    /// Legacy reconcile: find_task per item, one progress probe by task id.
    async fn reconcile_legacy(&mut self) {
        let items: Vec<(ItemId, Option<i64>, Option<String>, i32)> = self
            .queue
            .items()
            .iter()
            .filter(|i| matches!(i.state, ItemState::Unknown | ItemState::Paused))
            .filter(|i| i.task_id.is_some() || i.content_id().is_some())
            .map(|i| {
                (
                    i.id,
                    i.task_id,
                    i.content_id().map(|c| c.to_string()),
                    sub_type_for(i),
                )
            })
            .collect();
        let mut attached = false;
        for (id, task_id, content_id, sub_type) in items {
            let Some(client) = self.client.clone() else {
                return;
            };
            // Prefer find_task (content-id based) for fresh evidence.
            if let Some(cid) = &content_id
                && let Ok(found) = client.find_task(cid, sub_type).await
            {
                let tid = found.task_id;
                if let Some(it) = self.queue.get_mut(id) {
                    it.task_id = Some(tid);
                    it.error = None;
                }
                if attached {
                    self.log(
                        LogLevel::Info,
                        format!("Console still reports item #{id} (task {tid}); it will be resumed after the monitored item finishes"),
                    );
                    continue;
                }
                if let Some(it) = self.queue.get_mut(id) {
                    it.state = ItemState::Transferring;
                }
                self.spawn_poller(id, tid, None);
                attached = true;
                self.log(
                    LogLevel::Info,
                    format!("Re-attached item #{id} to console task {tid}"),
                );
                continue;
            }
            if let Some(tid) = task_id {
                // Probe progress once by task id — works even when the backend
                // does not implement find_task.
                if let Ok(pv) = client.task_progress(tid).await
                    && pv.raw_error == 0
                {
                    if let Some(it) = self.queue.get_mut(id) {
                        if attached {
                            it.error = Some(
                                "Console still reports this task; it will be resumed after the monitored item finishes".into(),
                            );
                            continue;
                        }
                        it.state = if pv.local_copy_percent >= 100 {
                            ItemState::Completed
                        } else if pv.transferred >= pv.total && pv.total > 0 {
                            ItemState::Installing
                        } else {
                            ItemState::Transferring
                        };
                        it.progress = Some(pv);
                    }
                    if !attached {
                        self.spawn_poller(id, tid, None);
                        attached = true;
                    }
                    continue;
                }
            }
            // Task unknown to the console: leave as Unknown with a hint.
            if let Some(it) = self.queue.get_mut(id) {
                it.error = Some(
                    "The console does not report this task anymore — verify on the console, then retry or mark completed".into(),
                );
            }
        }
        self.persist_queue();
    }

    // -- shutdown ---------------------------------------------------------------

    async fn shutdown(&mut self) {
        self.running = false;
        // Best-effort console-side cleanup with a short timeout.
        if let Some(active) = self.active.take() {
            let _ = active.poll_cancel.send(true);
            self.stop_remote(active.task_id, active.job_id).await;
        }
        if let Some(server) = self.server.take() {
            server.shutdown();
        }
        self.persist_queue();
        self.log(LogLevel::Info, "Engine shut down; state flushed");
    }
}

/// Map a v2 job entry to the UI progress view (shared by the poller and the
/// reconcile path).
fn v2_job_progress(job: &V2Job) -> ProgressView {
    ProgressView {
        percent: if job.length_total > 0 {
            (job.transferred as f64 / job.length_total as f64).clamp(0.0, 1.0) * 100.0
        } else {
            job.percent.clamp(0, 100) as f64
        } as f32,
        transferred: job.transferred,
        total: job.length_total,
        speed_bps: job.speed_bps.max(0.0) as u64,
        eta_sec: if job.rest_sec > 0 && job.rest_sec < u32::MAX as u64 {
            Some(job.rest_sec)
        } else {
            None
        },
        piece_index: job.num_index,
        piece_total: job.num_total,
        preparing_percent: 0,
        local_copy_percent: 0,
        raw_error: job.error,
    }
}

/// Map an item's parsed metadata to a BGFT task sub type
/// (Game=6, AC=7, Patch=8, License=9 — `docs/BASELINE.md` §3).
fn sub_type_for(item: &QueueItem) -> i32 {
    use crate::model::ContentKind::*;
    let Some(pkg) = item.primary_part().and_then(|p| p.pkg.as_ref()) else {
        return 6;
    };
    match pkg.content_kind {
        Gd => {
            if pkg.is_patch {
                8
            } else {
                6
            }
        }
        Ac | Al => 7,
        Dp => 8,
        _ => 6,
    }
}

/// Pick the local interface IP whose subnet contains the console; fall back to
/// any non-loopback IPv4, then `0.0.0.0` with an explicit warning.
fn pick_bind_ip(console_ip: IpAddr) -> IpAddr {
    let nets = discovery::local_networks();
    for net in &nets {
        if net.prefix > 31 {
            continue;
        }
        let IpAddr::V4(target) = console_ip else {
            break;
        };
        let t = u32::from_be_bytes(target.octets());
        let mask = if net.prefix == 0 {
            0
        } else {
            u32::MAX << (32 - net.prefix)
        };
        if (t & mask) == net.network
            && let Ok(ifaces) = if_addrs::get_if_addrs()
        {
            for iface in ifaces {
                if let IpAddr::V4(v4) = iface.ip() {
                    let a = u32::from_be_bytes(v4.octets());
                    if (a & mask) == net.network && !v4.is_loopback() {
                        return IpAddr::V4(v4);
                    }
                }
            }
        }
    }
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            if let IpAddr::V4(v4) = iface.ip()
                && !v4.is_loopback()
            {
                return IpAddr::V4(v4);
            }
        }
    }
    IpAddr::from([0, 0, 0, 0])
}

/// Blocking scan: walk paths, collect `.pkg` files (bounded).
fn collect_pkg_files(paths: &[PathBuf], recursive: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for path in paths {
        if path.is_dir() {
            walk_dir(path, recursive, &mut out);
        } else if is_pkg(path) {
            out.push(path.clone());
        }
        if out.len() >= MAX_WALK_FILES {
            break;
        }
    }
    out.sort();
    out
}

fn walk_dir(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if recursive
                && path
                    .file_name()
                    .map(|n| !n.to_string_lossy().starts_with('.'))
                    .unwrap_or(true)
            {
                walk_dir(&path, recursive, out);
            }
        } else if is_pkg(&path) {
            out.push(path);
        }
        if out.len() >= MAX_WALK_FILES {
            return;
        }
    }
}

fn is_pkg(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pkg"))
        .unwrap_or(false)
        && path.is_file()
}
