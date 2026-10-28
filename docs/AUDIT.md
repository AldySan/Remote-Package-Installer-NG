# Audit Report — full logic / bug / performance review

Date: 2026-10-27 · Scope: every module of `rpi-ng-core` and `rpi-ng` (UI),
including the mock-backend test suite. Method: line-by-line read of all
sources, cross-checked against `docs/BASELINE.md` protocol facts; every fix
below carries a regression test or is exercised by one.

Findings are graded P0 (silent data/queue loss or stall), P1 (wrong user-visible
state), P2 (incorrect behavior in edge paths), P3 (robustness/diagnostics),
and PERF (throughput/latency/allocation).

## 1. Fixed findings

### P0 — part-id desync between the queue and the file server (engine)

`rebuild_urls` called `server.register(...)` but **discarded the returned
registration id**, then built URLs with the id stored in the queue. Those two
counters are independent: `Queue::next_part` advances when files are *added*,
the server's registry advances when parts are *submitted*. They only stay
aligned while every queued part is submitted, in order, to the same server
instance.

Broken scenarios (queue stalls **silently** — `url_for` returns `None`,
`maybe_start_next` returns without a log):

- restart with pending (never-submitted) items in `queue.json`, then reconnect —
  the fresh server restarts its counter at 0;
- add → remove-without-submitting → add → submit (the removed item's ids leave
  a gap);
- any retry-after-reconnect sequence.

Fix: `rebuild_urls` now uses the id the server hands back and writes it into
the queue part (`part.id` is server-scoped, refreshed at every registration).
Regression: `engine_submits_restored_queue_with_unaligned_part_ids`.

### P1 — poller overwrote the user's Paused state (engine)

`handle_progress` assigned `Transferring`/`Installing` on every successful
poll, so a paused item flipped back to "Transferring" within one poll interval
(~1 s). The console legitimately keeps reporting progress for a paused task.
Fix: intermediate transitions now skip when the item is `Paused`; terminal
transitions (completion, console-side error) still apply. The pause log line
also no longer overclaims ("best-effort"). Regression:
`engine_pause_survives_progress_polls`.

### P1 — pause during submit was ignored (engine)

`Pause` had no task id to act on while a request was in flight, and when the
console accepted the request the item transferred anyway. Fix: a pause
recorded while submitting is applied immediately after the console accepts
(`SubmitResult` handler checks `running`).

### P1 — disconnect orphaned in-flight items (engine)

`Disconnect` cleared the connection and poller but left items in
`Transferring`/`Installing` forever; `reconcile` only looked at
`Unknown`/`Paused`, so those items were invisible to recovery and to
`maybe_start_next`. Fix: disconnect marks in-flight items `Unknown` (never
auto-resubmitted) with an explanatory hint; late submit results record the
task id for reconcile instead of overwriting the state. Regression:
`engine_disconnect_marks_inflight_unknown_and_reattaches` (also proves
find_task-based re-attach after reconnect).

### P1 — late submit results overwrote user decisions (engine)

A submit response arriving after the user cancelled (or after a disconnect)
unconditionally flipped the item to `Transferring` and spawned a poller. Fix:
`SubmitResult` only proceeds when the item is still `Submitting`; cancelled
items get a best-effort remote stop/unregister of the ghost task.

### P1 — installs accepted without a task id were reported Failed (engine/client)

Some v3.x forks answer `{"status":"success"}` without a `task_id`. The client
classified this `MissingTaskId` (not transient) and the engine marked the item
**Failed** — implying the install never happened, when it had in fact been
accepted and was simply untrackable. Fix: the item becomes `Unknown` with a
"verify on the console" hint (evidence-based honesty). Regression:
`engine_install_without_task_id_is_unknown_not_failed`.

### P2 — reconcile spawned pollers it could not track (engine)

`self.active` is a single slot, but `reconcile` called `spawn_poller` per
matched item: every poller except the last had its progress dropped forever.
Fix: reconcile attaches exactly one poller (single-active invariant), keeps
`find_task` evidence for the rest and hints that they will be resumed after
the monitored item finishes. The task-id progress probe now also runs when a
content id exists but `find_task` is unsupported.

### P2 — per-row Resume could not resume anything (engine/UI)

The row button sent the id-less `Command::Resume`, which only acted on the
*active* task — after a reconnect there is none, so paused items had a dead
button. Fix: new `Command::ResumeItem(id)` resumes one paused item through its
console task id (guards the single-active invariant); global `Resume` falls
back to the first paused item when nothing is active.

### P2 — cancelling a non-active item skipped remote cleanup (engine)

Stopping a paused/unknown item left its console-side task running. Fix:
cancel now issues best-effort `stop_task`/`unregister_task` when a task id and
connection exist, and logs the same caveat as the active path.

### P2 — settings were persisted to the wrong directory (engine)

`SetSettings` saved through `Settings::save()` (global user dir), not the
engine's `data_dir` — test/headless runs wrote to the real user settings. Fix:
`save_in(self.data_dir)`.

### P3 — silent failure paths (engine)

- Hitting the 2000-file scan cap was invisible; now logged as a warning.
- `rebuild_urls` failure returned silently from `maybe_start_next`; now logged
  as an error.

### P3 — fixed-port rebind race (engine/file server)

Restarting the file server on a user-configured fixed port can race the old
server's listener teardown. Fix: the bind is retried for ~2 s (8 × 250 ms)
before failing.

## 2. Performance findings (applied)

| Area | Before | After | Why |
| --- | --- | --- | --- |
| File server streaming | 64 KiB chunks | 256 KiB chunks | ~4× fewer read syscalls and executor wakeups per GB on gigabit LAN; memory stays bounded per connection |
| Discovery verification | sequential per hit | bounded concurrency 8 (`buffered`, order-preserving) | one unresponsive endpoint no longer stalls the whole pass |
| Discovery TCP sweep | 64 concurrent connects | 128 | halves worst-case sweep time on /24s with silent-drop firewalls |
| Connect probe | `is_exists` twice on the fallback path | exactly once | removes a wasted round-trip per probe |
| Queue view rendering | full `Vec<QueueItem>` clone per frame + needle lowercased per item | filtered index list + needle lowered once | eliminates O(items) allocations per repaint with large queues |

## 3. Reviewed and intentionally left unchanged

- `json_fix::repair` (string-aware hex rewrite) — correct on escapes; covered
  by unit tests.
- PKG/SFO parsers — bounds-checked with caps; no reachable panic found.
- File-server allowlist model (opaque ids, decorative name segment) — no
  traversal path exists; multi-range fallback is RFC 7233-compliant.
- `pick_bind_ip` — derives candidates from real interface submasks only (no
  routing-table /0 hazard).
- Engine snapshot-on-every-event — O(items) clone per event is acceptable at
  the default 1 s poll cadence; revisit only if queues exceed ~10k items.
- `rpi_mock` fidelity — the mock's prerequisite Range fetch and hex dialect
  remain the protocol-conformance harness; all new engine regressions run
  against it.

## 4. Verification

- 44 tests green (`cargo test --workspace`) across core, queue, file server,
  PKG and mock-backend suites, including the five new regression tests added
  by this audit.
- `cargo fmt --check` and `cargo clippy --workspace --all-targets -D warnings`
  clean (CI gates on the same).
- No real-console hardware was available; console-side behavior claims remain
  backed by `docs/BASELINE.md` evidence and the mock only — unchanged policy.

---

## 5. Addendum — v2 API integration audit (NextGen support)

Scope: the v2 client (`rpi/types.rs`, `rpi/client.rs`), engine integration,
mock coverage, and a re-review of every touched path. The fork's own source
(`AldySan/ps4_remote_pkg_installer_nextgen` @ v0.0.2) was read line-by-line
against its `docs/API.md`.

### Findings in the console-side fork (reported, worked around client-side)

- **P1 (fork)** — `POST /api/v2/jobs/<id>/{pause,resume,stop}` is documented
  as POST but `server.c` only routes it under GET/HEAD: the POST router never
  matches the path, so POST answers 404. Client workaround: POST first (the
  documented contract, correct for fixed builds), fall back to GET exactly
  once on 404. Covered by `v2_job_op_falls_back_to_get_after_post_404`, which
  reproduces the quirk in the mock and asserts the client exercised it.
- **P3 (fork, informational)** — the v2 jobs registry is in-memory with
  32 slots and reclaims the oldest terminal slot when full; a PC reconnect
  after a console app restart sees an empty list. The client treats
  "job missing from list" as an *unknown outcome* (never a failure, never a
  resubmission), which is the honest reading.

### Bugs found in RPI-NG while integrating (fixed)

- **P1 — `task_id: -1` was misclassified as "untrackable"** (client/engine).
  Both dialects use `-1` for "already installed"; the old `task_id <= 0`
  check turned that into `MissingTaskId` → item parked as `Unknown` with a
  misleading "no task id" error. Now `-1` completes the item with the
  console's own report (`engine_v2_already_installed_completes_without_polling`).
- **P2 — fail bodies carrying a human-readable `error` string were dropped**
  (client). v2 install failures answer HTTP 500 with
  `{"status":"fail","error":"..."}`; the parser only extracted `error_code`
  and discarded the message. The message now lands in the error gloss
  (`v2_install_failure_message_is_surfaced`).
- **P2 — mixed-backend lifecycle endpoints** (engine). Pause/resume/stop chose
  the v2 job endpoint purely by the *item's* `job_id`. An item submitted via a
  v2 backend, then reconnected to a legacy-only backend, would silently no-op
  its lifecycle ops. Endpoint choice is now gated on the *current*
  connection's `api_v2` capability.
- **P3 — v2 install did not validate the task id** (client). A malformed
  `task_id: 0` would have spawned a poller for a ghost task; `check_task_id`
  now applies to v2 installs as well.
- **P3 — reconcile left unattached v2 items without a hint** (engine). When
  more than one job was still active console-side, the second and later items
  stayed `Unknown` with `error: None` — no explanation. They now carry the
  same "console still reports this job; it will be resumed after the monitored
  item finishes" hint as the legacy path.

### Performance (applied, measured by design not benchmarks)

- v2 progress polling replaces N per-task `get_task_progress` round-trips with
  **one** `GET /api/v2/jobs` per tick — the console answers every item in a
  single response.
- v2 reconcile replaces per-item `find_task` probes with one jobs request.
- v2 speed/ETA come from the console's own 1s-sample EMA (`taskman.c`
  `slot_refresh_derived`) instead of being re-derived client-side from poll
  deltas at PC poll cadence.

### Verification

- 54 tests green (`cargo test --workspace`), including 10 new v2 tests:
  probe identity detection, legacy-backend no-v2, install/jobs roundtrip,
  POST→GET fallback with the fork's quirk reproduced, error-message
  surfacing, `task_id: -1` semantics, and four engine flows (end-to-end,
  already-installed, pause/resume via fallback, disconnect→reconnect
  reconcile via the jobs list).
- `cargo fmt --check` and `cargo clippy --workspace --all-targets -D warnings`
  clean.
- Real-console validation remains unavailable — the NextGen wire behavior is
  reproduced from its published source, not from hardware.

## 6. Addendum — findings from v0.0.1 user testing

Two bugs reported after the first real-world use of the release binaries,
plus one latent bug found while fixing them.

### 6.1 Settings window edits were dropped (P0, UI)

`settings_view::show` cloned the settings into a local `draft` **every
frame** and only applied it when Save was clicked in that same frame. Any
edit made in one frame was thrown away when the next frame re-cloned from
the unchanged settings — combo boxes snapped back, checkboxes re-appeared
unchecked, drag values reset. The window looked frozen.

**Fix:** the draft now lives on `App` (`settings_draft: Option<Settings>`),
seeded when the window opens and discarded when it closes (✕ or toggle);
edits persist across frames until Save or discard. The theme renders from
the draft while the window is open, giving a live preview before Save.

### 6.2 Engine-side settings never flowed back to the UI (P1, latent)

The engine owns the authoritative settings (it updates the
remembered-console list on every connect) but the UI kept its startup copy
forever: the "remembered" list in the connect view was stale for the whole
session, and a later Save would have clobbered the engine's list with the
stale one.

**Fix:** new `Event::SettingsChanged(Settings)` emitted on every
engine-side change (`SetSettings` echo, `remember_console`); the app
refreshes its cache from it. Regression test:
`engine_broadcasts_settings_changes`.

### 6.3 Console window opened on Windows (P2, UX)

The Windows release binary was built with the console subsystem, so a CMD
window opened next to the GUI.

**Fix:** `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`
on the binary crate — release builds show only the GUI; debug builds keep
the console for `cargo run` output.

### Verification

- UI-layer wiring (draft persistence, live theme preview) is verified by
  reasoning + manual testing; the engine-side semantics are covered by the
  new regression test. No UI automation framework was added — egui_kittest
  would be a new dependency for two lines of logic, which is not worth it.
