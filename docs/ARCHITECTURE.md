# Architecture

## 1. Summary

Remote Package Installer NG ("RPI-NG") is a cross-platform (Windows / Linux / macOS)
desktop application for installing PKG packages on a PlayStation 4 over the local
network, designed from the start to admit PlayStation 5 backends later without
rewriting the queue engine or the UI.

Non-goals for v1: console-side software (we are a PC client only), URL/debrid source
downloads, library management with cover art, cloud anything.

## 2. Technology selection

Candidates were evaluated against the product's hard requirements — low idle CPU, small
memory footprint, fast startup, native drag-and-drop, cross-platform packaging, and a
testable protocol core that can run headless in CI:

| Option | Startup | Idle RAM | Testability (headless CI) | System dependencies | Packaging | Verdict |
| --- | --- | --- | --- | --- | --- | --- |
| **Rust + egui (eframe)** | fast (~100–300 ms) | ~40–90 MB | **excellent** — core is a plain lib; `cargo test` needs no display, no webview | none beyond libc/X11-dlopen at runtime | single static-ish binary per OS | **chosen** |
| Rust + Tauri 2 | fast | ~80–150 MB + webview | good for core; app crate needs webkit2gtk **at build time** | webkit2gtk / WebView2 / WKWebView | installer + JS bundle | rejected: webkit is a heavyweight system dependency, and the build-time requirement eliminates clean headless validation; UI polish can come later via a frontend swap against the same core |
| Go + Wails | fast | ~60–120 MB | good core; UI needs webview | webview | single binary | rejected: same webkit concern; smaller ecosystem for precise async control |
| Java 21 + JavaFX | slow (JVM) | 200–400 MB | good core tests | JDK + JavaFX runtime | JRE bundle (heavy) | rejected: footprint and startup conflict with the product principles |
| Electron | slow | 200–500 MB | good (Node) | Chromium per app | huge | rejected: violates "lightweight" outright |

The decisive factors, in order:

1. **The protocol/queue core must be provable in CI without a display or system
   webview.** Only the Rust-core options (egui or Tauri's Rust side) give that; egui
   additionally keeps the *whole app* compilable in a bare environment.
2. **Footprint.** A queue/progress utility should sit quietly at <100 MB RAM.
3. **One language, one toolchain, one artifact per platform** — no JS runtime, no
   SDK downloads for users.

Trade-off accepted: egui's default look is more "engineering tool" than "web app". The
UI is kept thin over the core so a future web frontend (Tauri or plain web) can reuse
`rpi-ng-core` unchanged if the product demands it.

Versions: Rust 2024 edition (stable ≥ 1.85 for `eframe` MSRV chain; CI pins current
stable), `eframe/egui 0.37`, `tokio 1`, `axum 0.8`, `reqwest 0.13` (no default TLS —
console APIs are plain HTTP), `serde/serde_json`, `if-addrs`, `rfd` (xdg-portal on
Linux, native on Win/macOS), `directories`.

## 3. Workspace layout

```
Cargo.toml                 # workspace
crates/
  rpi-ng-core/             # the entire product logic, no UI deps
    src/
      lib.rs               # re-exports, Engine facade
      model.rs             # ConsoleTarget, BackendKind, TaskState, QueueItem, events
      settings.rs          # persisted user preferences
      pkg.rs               # PKG header/entry-table parsing, multipart detection
      sfo.rs               # PARAM.SFO parser (TITLE, CONTENT_ID, ...)
      rpi/
        mod.rs
        client.rs          # HTTP client for the RPI dialect
        types.rs           # request/response DTOs (lenient field types)
        json_fix.rs        # hex-literal tolerant JSON preprocessing
      backend.rs           # ConsoleBackend trait + capability probing
      discovery.rs         # bounded LAN probe + remembered hosts
      fileserver.rs        # allowlist HTTP file server (Range, streaming)
      queue.rs             # queue state machine + persistence + recovery
      engine.rs            # orchestration: commands in, events out
    tests/
      rpi_mock.rs          # faithful mock RPI server + protocol tests
      fileserver_test.rs
      queue_test.rs
      pkg_test.rs          # generated PKG/SFO fixtures
  rpi-ng/                  # thin desktop UI
    src/
      main.rs              # eframe bootstrap, theme
      app.rs               # top-level layout, engine wiring, first-run
      queue_view.rs        # virtualized queue list, per-item progress
      connect_view.rs      # discovery results + manual fallback + verification
      settings_view.rs     # settings window
      log_view.rs          # activity log panel
```

Dependency direction: `rpi-ng` → `rpi-ng-core`. `rpi-ng-core` never depends on egui
and is fully usable headless (a future CLI or web frontend would sit exactly where
`rpi-ng` sits).

## 4. Core design

### 4.1 Engine = single orchestrator, commands in / events out

`Engine::spawn(settings)` starts a dedicated tokio runtime and returns a handle:

```
UI thread                          engine runtime (tokio)
   │  mpsc::Command  ─────────────────►  supervisor loop
   │                                        ├─ queue worker
   │                                        ├─ progress poller (per active task)
   │                                        ├─ file server task
   │                                        └─ discovery on demand
   ◄───────────────────  crossbeam/std mpsc::Event  ─┘
```

- `Command`: `AddPaths`, `AddFolder{recursive}`, `Remove`, `Reorder`, `Start`, `Pause`,
  `Resume`, `Retry`, `Cancel`, `ClearFinished`, `Connect{addr}`, `Disconnect`,
  `Discover`, `UpdateSettings`, `Shutdown`.
- `Event`: `Snapshot(queue + console + server state)` (coalesced), `Log{level,msg}`,
  `DiscoveryResult{candidates}`, `ConnectionChanged{status}`.

The UI polls `try_recv()` each frame and calls `ctx.request_repaint()` when work is
pending (progress active → repaint at ~4 Hz via `request_repaint_after`), so idle CPU
is zero when nothing is happening — no busy loop, honoring the lesson from
`BASELINE.md §4.1`.

### 4.2 Backend model (capability-first)

```rust
trait ConsoleBackend {
    fn kind(&self) -> BackendKind;                 // Rpi, RpiFork, Unknown
    fn capabilities(&self) -> Caps;                // install_direct, progress, pause, ...
    fn install(&self, req: &InstallRequest) -> Result<TaskHandle>;
    fn progress(&self, task: TaskId) -> Result<TaskProgress>;
    fn pause / resume / stop / unregister / find_task ...
}
```

One implementation ships today: `RpiBackend` speaking the flatz dialect
(`docs/BASELINE.md §3`). Capability detection is **probing, never assumption**:

1. TCP connect (12800, then 9090 as GoldHEN-family fallback — pkg-sender's order).
2. `GET /api` — the RPI server answers 400 with an "Unsupported method" body; anything
   else (HTML, empty) → not an RPI backend → reported as incompatible.
3. `GET /api/status` when it answers JSON (`{"busy":..}`) → richer capabilities
   (receiver-reported busy state); old receivers answer HTML → capability absent.
4. `is_exists` probe validates the full request/response path before the first install.

Unknown backends are shown as such; their unsupported operations are disabled in the
UI rather than simulated.

### 4.3 Lenient JSON (the invalid-hex quirk)

The original payload writes unquoted hex literals (`"bits": 0x20`) — invalid JSON
(`BASELINE.md §3.1`). The client preprocesses every response body through
`json_fix::repair()` (regex `:\s*0x[0-9A-Fa-f]+` → decimal), then deserializes with
`serde` into DTOs whose numeric fields additionally accept strings, so all three
observed dialects (hex literal, quoted hex, plain decimal) parse. The mock backend in
tests can emit the *broken* form on purpose to lock this behavior in.

### 4.4 Queue model

```
                        ┌─────────┐
                        │ queued  │
                        └────┬────┘
              start ┌───────┴────────┐ preparing (pkg re-validation, server up)
                    ▼                │ file missing / unreadable
              submitting ────────────┤──────────► failed(retryable? no → needs_input)
                    │ task_id         │ transient net error ×3 with backoff
                    ▼                ▼
              transferring ◄──────► paused      (progress polls)
                    │ bytes == total && error == 0
                    ▼
              installing (local_copy_percent phase, still polled)
                    │ completion evidence
                    ▼
              completed
     any active state ── cancel ──► cancelled (PC-side stop + unregister best effort)
```

- A queue **item** is a package group: one or more *parts* (multipart PKG pieces in
  order, detected by name pattern `..._0.pkg, ..._1.pkg, …` and confirmed by header
  parsing).
- Item identity = canonical file path + content id + part index → duplicate detection
  on add.
- **Completion honesty:** `submitting` success (HTTP 200 + `task_id`) is never treated
  as completion. Completion requires observed `transferred_total ≥ length_total > 0`
  plus `error == 0`, or explicit user acknowledgement.
- **Recovery after restart:** the queue persists after every state change (atomic
  write: temp file + rename). Items found in `transferring/installing` on load are
  marked `unknown` and *never auto-resubmitted*. When a console is connected, the
  engine reconciles via `find_task`/`is_exists`; the user sees "already installing /
  already installed / unknown — decide", with explicit resubmit as a deliberate act.
- **Ordering:** manual reorder (up/down/drag), plus optional "game before patch"
  ordering derived from parsed headers. Only one item is in flight at a time (BGFT on
  the console is single-task for package installs); the engine pipelines preparation
  (header parsing) ahead of submission but never overlapping submissions.

### 4.5 Local file server

- `axum` on `127.0.0.1:0` (OS-assigned; configurable fixed port) — **allowlist only**:
  each queue part registers `(opaque id → canonical path)`; URLs are
  `/pkg/<id>/<urlencoded filename>`; no directory listing, no traversal (id lookup is
  a HashMap hit, not a path walk), no `..` handling because ids are opaque.
- Streaming with 256 KiB buffers — never whole-file reads; file handles opened per
  request and closed on completion or client abort (drop guard).
- **HTTP Range support** (single and suffix ranges) — required by the payload's
  prerequisite fetcher and useful for BGFT restarts (`BASELINE.md §2`). `HEAD` is
  answered with `Content-Length` and `Accept-Ranges: bytes`.
- Concurrency: tokio's default multi-thread scheduling; no artificial limit, matching
  the console's expectation of a multi-threaded server; a semaphore bounds concurrent
  *file opens* to keep fd usage sane on 500-item queues.
- Bind preference: the interface reachable from the console (chosen during discovery);
  `0.0.0.0` is never used silently — the chosen bind is shown in the UI with the
  resulting URL, plus firewall guidance when the console cannot connect.
- Graceful shutdown via `axum::serve(...).with_graceful_shutdown(watch)`.

### 4.6 Discovery

No PS4 backend broadcasts anything (`BASELINE.md §4.5`), so discovery is honest and
bounded:

1. Enumerate local IPv4 interfaces with real netmasks (`if-addrs`) — never /24
   assumptions (pkg-sender's approach).
2. Optional bounded TCP probe of RPI ports across each subnet, capped at 1024
   candidates, 64 concurrent, 300 ms connect timeout, dedup by IP; every TCP hit is
   then **API-verified** (§4.2 step 2) before being shown as a console.
3. Candidates are remembered (path-based settings, no cloud), and re-validated on
   selection.
4. Manual `host:port` entry is always available and is the documented fallback.
5. Discovery aborts cleanly on app close; it never busy-polls (single sweep per user
   action or explicit refresh; no periodic background scanning in v1).

### 4.7 PKG & SFO intelligence

Parsed on the PC **before** submission (closes the fail-late defect,
`BASELINE.md §4.8`):

- Header (first `0x2000` bytes): magic `0x7F504B47`, `entry_count`@0x10,
  `entry_table_offset`@0x18, `content_id`@0x40 (0x24 bytes), `content_type`@0x74,
  `content_flags`@0x78, `package_size`@0x430 — big-endian.
- Entry table: `entry_count × 0x20` bytes at `entry_table_offset`; entry `id`@0x00
  (`0x1000` = PARAM.SFO, `0x1200` = icon0.png), `offset`@0x10, `size`@0x14.
- PARAM.SFO parsed for `TITLE`, `TITLE_XX`, `CONTENT_ID`, `CATEGORY`, `APP_VER`,
  `VERSION`, `PARENTAL_LEVEL` etc. (SFO: magic `\0PSF`, key/value tables).
- Derived: `title_id` from content id (middle dash-delimited field), package kind
  (`PS4GD/AC/AL/DP`), patch flags (`FIRST_PATCH/SUBSEQUENT_PATCH` bits of
  `content_flags`), expected total size for multipart = Σ part sizes — which is also
  validated against the console-reported `length_total` at runtime.
- Guardrails: `entry_count ≤ 512`, sfo size ≤ 1 MB (the same hardening the v3.55 fork
  ships — defense against crafted files), tolerant truncation handling → item flagged
  `invalid` instead of aborting the whole queue.

### 4.8 Error taxonomy and retry policy

```
ErrorKind::Transient   → retry ≤3, exponential backoff (1s/2s/4s), then failed(retryable)
ErrorKind::Permanent   → failed immediately (bad pkg, incompatible backend, 4xx semantics)
ErrorKind::Console     → surfaced verbatim with the raw error_code + known-code glossary
ErrorKind::Cancelled   → clean rollback of PC-side state
```

Cancellation (user or shutdown): abort PC-side HTTP calls and polls, best-effort
`stop_task` + `unregister_task` on the console with a short timeout, close server
sockets and file handles; any remote action that could not be confirmed is reported
("task may still exist on console — it will be reconciled next connect").

### 4.9 Shutdown (deterministic)

`Command::Shutdown` and window-close both: stop accepting queue commands → cancel
active polls and transfers (watch channel) → graceful server shutdown (≤2 s) →
flush queue + settings (atomic writes) → drop runtime. `on_exit` in the UI guarantees
the engine is joined before process exit; orphaned threads are structurally impossible
because every task is spawned with the engine's cancellation watch.

### 4.10 Persistence

One JSON document per concern, atomic replace (`tmp` + `rename`), corruption-tolerant
load (fall back to backup copy, then defaults):

- `settings.json` — theme, server port policy, discovery on/off + caps, poll interval,
  remembered consoles, last console.
- `queue.json` — items with parts, metadata, state, error history, task ids; written on
  every transition (debounced 250 ms during storms), always fsynced on
  `unknown`-marking transitions before exit.

## 5. PS5 extension points (design now, build later)

- `ConsoleBackend` implementations are per-generation; PS5 gets `EtaHenDpiBackend`
  (DPI v1: JSON on 9090; DPI v2: 12800) and/or a LoopDPI-compatible receiver backend —
  both verified to exist (`docs/RESEARCH.md §2.9`).
- `BackendKind::Console generation` is explicit; the queue engine never branches on
  "is PS4" — it talks only to the trait.
- Package metadata validation is generation-aware (`pkg::validate_for(generation)`), so
  a PS5 PKG added while a PS4 console is selected is flagged incompatible, not
  silently sent.
- UI: console list already renders generation + capability chips; PS5 rows simply
  appear when an adapter exists. Nothing in `queue.rs`, `fileserver.rs`, or `engine.rs`
  names PS4.
- Feasibility and blockers: `docs/PS5_ROADMAP.md`.

## 6. Future adapters (documented, not built)

- GoldHEN "Payload Server" injection (9090/9020/9021): mechanism not source-verified,
  requires shipping a payload binary — needs its own licensing and safety review.
- URL/debrid sources (DirectPackageInstaller territory): `InstallSource` enum seam
  exists in the model (Local today, Url later) so the file server can later proxy
  remote streams with the same Range contract.
