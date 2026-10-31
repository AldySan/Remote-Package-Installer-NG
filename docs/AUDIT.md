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

## 7. Addendum — queue failure policy & update checker (v0.0.2)

### 7.1 Queue behavior on failure was hardcoded (feature request)

Before: a failed item (console error or rejected submit) always advanced the
queue to the next package. Users asked for the choice: park the queue so the
failure gets attention, or keep going.

**Design:** new `Settings::on_failure` (`FailurePolicy::Continue | Pause`,
default `Continue` = the previous behavior, no silent semantic change). All
three failure sites route through one new engine method
(`on_transfer_failure`): submit-failure after exhausting retries, legacy
progress polls reporting `error != 0`, and v2 jobs reaching `failed`.
`Pause` sets `running = false` and logs an actionable Warn — the remaining
items stay `Queued` until the user presses *Start queue*.

**Retry now means "process now":** `Command::Retry` sets `running = true`.
Previously a retry after a pause left the item `Queued` until something else
restarted the queue — surprising under the new policy, where Retry is the
primary recovery path.

**Deliberately unchanged:** locally-invalid items (vanished/broken files) are
still skipped during queue start — that is pre-transfer validation, not a
transfer failure, and "skip known-bad files" is the useful behavior in both
policies. Reconnect-reconcile failures don't auto-advance either way (the
queue is not running at that point).

Regression tests: `engine_failure_policy_pauses_queue_until_retry` (also
proves Retry restarts), `engine_failure_policy_continue_advances_queue`,
`engine_failure_policy_pauses_after_console_error_mid_transfer`,
`engine_failure_policy_pauses_after_v2_job_failure`.

### 7.2 Update checker

One GET to `https://api.github.com/repos/…/releases/latest`, no identifiers
sent beyond the User-Agent. Version comparison (`update::is_newer`) is
strictly numeric with pre-release ranking and **fails closed**: any tag that
does not parse cleanly is treated as "not newer", so a malformed upstream tag
can never nag users. 404 from the API (repo without releases) is `Ok(None)`,
not an error; 403/429 classify as a rate limit with a human message.

**TLS:** reqwest gained the `rustls` feature (aws-lc-rs + platform verifier
— static, no OpenSSL runtime dependency). aws-lc needs NASM on Windows x64;
instead of requiring it, `.cargo/config.toml` sets
`AWS_LC_SYS_PREBUILT_NASM=1` (no effect on Linux/macOS; if NASM *is*
installed it is used normally).

**UI:** background thread with its own tokio runtime reports through a
channel polled once per frame (the GUI thread never blocks). Startup
auto-check (on by default, Settings) only pops the window when an update
exists; the manual button always shows the outcome. Every result is logged
to the activity panel.

**Verification:** `tests/update_test.rs` mocks the GitHub API end to end
(newer/same/older, 404, rate-limit, malformed body, empty tag, missing
`html_url` fallback, unreachable network); version comparison has unit tests
covering padding (`0.1` vs `0.1.0`), `v` prefixes, pre-releases and garbage.
A bug caught during review: the initial comparison was inverted
(`cur > new`) — the test suite flagged it immediately.

## 8. Addendum — findings from v0.0.2 user testing (fixed in v0.0.3)

### 8.1 Settings "saved" but never persisted (root cause: silent no-op)

**Symptom:** every setting (theme, port, policies) applied live after Save,
then reverted to defaults on the next launch.

**Root cause:** `engine::spawn` called `spawn_with(settings, None)`, and
`None` was passed through verbatim as the engine's data dir. Every persist
site did `save_in(self.data_dir.as_deref())`, and `save_in(None)` is a
**silent `Ok(())`** (the no-dir case exists for hermetic test runs). So the
Save button updated the in-memory settings — which is why the live behavior
looked correct — but no bytes ever reached the disk. `queue.json`
persistence shipped in v0.0.1 was disabled by the exact same path (its
restart-recovery tests all pass explicit dirs, which is why the suite never
caught it: `spawn_with(_, Some(dir))` works, only the `None` default was
broken).

**Fixes:**

- `spawn_with` resolves `None` through the new `settings::init_data_dir()`
  (resolve → create → migrate), so the desktop app — and any embedder using
  the default — gets a real directory. The doc comment that claimed
  "`None` = default user data dir" now matches the behavior.
- The data dir itself moved **next to the executable** (portable style —
  explicit user request: an app in `D:\RPING\` keeps its config in
  `D:\RPING\`). Resolution order: `RPI_NG_DATA_DIR` env → exe dir → per-OS
  user dir fallback. A one-time migration copies pre-portable
  `settings.json`/`queue.json` from the old user-data location, never
  overwriting existing files, and never running under an env override
  (tests stay hermetic and cannot read the host user's real files).
- The first activity-log line now reports the resolved location, so "where
  is my config" is answerable from inside the app.

**Regression tests:** `settings::tests` covers resolution precedence
(env wins / exe dir / empty-env ignored / no-exe fallback) and the
one-shot migration semantics (copies once, never overwrites).

### 8.2 Transfer activity + file-server self-check (feature requests)

- **Live in/out rates:** the file server now counts body bytes sent (`tx`)
  and request bytes received (`rx`) per request; rates are EMA-smoothed
  (α = 0.5, ≥100 ms sample spacing) and surfaced through `ServerInfo` in
  every snapshot. A 250 ms engine ticker (active only while the server is
  up) keeps the UI readout live between poll ticks and lets the rate decay
  to zero when a transfer ends. Accounting is deliberately approximate and
  documented: tx = promised `Content-Length` (a mid-body disconnect
  over-counts by the unsent tail), rx = request-line+header size estimate
  via an axum middleware.
- **Check File Server:** `fileserver::self_check` is a loopback `HEAD`
  probe of an unregistered URL. Outcome taxonomy is diagnostic by design:
  `Responded(code)` (any HTTP answer — the server is fine, so a console
  that still cannot download points at the network/firewall between the
  two machines), `NotRunning` (nothing listening — the server starts with
  a console session), `NoAnswer` (port accepts, nothing replies — firewall
  / antivirus interception). Std-only, run on a plain background thread
  (same channel-poll pattern as the update checker), offered both in the
  queue's file-server strip and in Settings, and every outcome is logged
  with an actionable sentence.
- **UI text fixes from user feedback:** the troubleshooting block on the
  connect screen and the two settings footnotes were `.small()` — barely
  legible. Both are regular size now; the footnotes moved from next to the
  Save button to a full-width footnote area under a separator (still weak
  color, so they read as quiet bylines rather than competing with the
  controls).

**Verification:** `fileserver::tests` covers the EMA math (convergence,
decay, tiny-delta guard), the dead-port classification, and an end-to-end
probe + ranged download against the real server asserting exact `tx`
accounting, positive `rx`, and request counts.

---

## 9. Addendum — second full re-audit (v0.0.5)

Date: 2026-11-01 · Method: line-by-line re-read of every module plus the
test harness, cross-checked against the earlier findings (§1–§8) so nothing
is double-reported. Baseline before fixes: 85/85 tests green, `clippy -D
warnings` clean — all findings below are new.

### P1 — the submit window had no guard: two concurrent console tasks (engine)

`maybe_start_next` gated on `self.active`, but `active` is only set when the
console **answers**. Between `submit()` and `SubmitResult` (LAN round-trip,
or 1–4 s of transient-retry backoff) any re-trigger — a second Start press
(the UI's enabled-state lags a snapshot behind), a background parse
finishing (`ParseDone` → `maybe_start_next`), a Retry, or a per-row Resume —
submitted the NEXT queued item too. The second `spawn_poller` then replaced
`self.active`, and the replaced poller died silently (its watch sender
dropped), stranding the first item in "Transferring" forever.

Fix: a new `submit_in_flight()` predicate (any item in `Submitting`) is
checked by `maybe_start_next` and `resume_item`. No new state: `Submitting`
already means "install request in flight or waiting out a backoff".
Regressions: `engine_start_during_submit_window_does_not_double_submit`
(mock knob `install_delay_ms` widens the window deterministically — the test
was verified to fail against the pre-fix engine).

### P2 — cancelling during the submit window was a silent no-op (engine)

`cmd_cancel`'s local branch required `!item.state.is_active()`; a
`Submitting` item is active, so neither branch ran. The console's late
answer then flipped the item to `Transferring` — the user pressed Cancel and
the install went ahead anyway.

Fix: the local branch now accepts everything except `Transferring` /
`Installing` (owned by the active branch). For a cancelled Submitting item
the existing late-`SubmitResult` path already best-effort-stops the ghost
task. Regression: `engine_cancel_during_submit_window_is_honored` (verified
to fail pre-fix; the mock now records `stop_task` calls).

### P2 — malformed SFO u32 entry panicked the parse thread (sfo)

The `0x0404` arm read a full `u32` guarded only by
`value_abs + used_size ≤ len`. A crafted/truncated SFO (format `0x0404`,
`used_size < 4`, value at the buffer tail) indexed out of bounds → panic
inside `tokio::task::spawn_blocking` → swallowed → `ParseDone` never sent →
metadata never appeared, with no error anywhere. (§3 of the original audit
said "no reachable panic found" — this one was missed.)

Fix: the arm now guards `value_abs + 4 ≤ buf.len()` like the `0x0004` arm
and skips malformed short entries. Regression:
`short_u32_entry_at_buffer_end_is_skipped_not_panicked` plus two
well-formed controls (u32 and string entries) — `sfo.rs` had no unit tests
before.

### P3 — file-server registry grew on every re-registration (fileserver)

`rebuild_urls` re-registers a part on every submit / retry / requeue; each
`register()` allocated a fresh id and entry while the previous entry stayed
live for the whole session (stale URL served + one leaked `PathBuf` +
`String` per submit). Bounded, but monotonic — exactly the pattern a
"memory violation" audit looks for.

Fix: `Registry::register` is idempotent — the same `(path, name)` returns
the existing id. Part ids become stable within a session, so `unregister`
(on Remove / ClearFinished) now removes exactly the live entry.
Regression: `registering_the_same_file_reuses_its_id`.

### P3 — legacy speed "EMA" never smoothed (engine)

`handle_progress` blended `pv.speed_bps * 0.6`, but `progress_dto_to_view`
always sets `speed_bps: 0` in the legacy dialect — the previous sample was
never carried, so every poll displayed 40 % of the instantaneous rate (a
spiky Mbps readout; the smoothing the comment promised never happened).

Fix: the previous stored speed is read from the item's `progress` and
blended via a new `blend_speed()` helper (split out like the file server's
`update_rates` for direct unit testing: `speed_blend_converges_and_decays`).

### P3 — log timestamps were UTC but unlabeled (UI)

`fmt::clock` renders UTC. Users in UTC+7 read every entry seven hours off
their wall clock with no hint. A tz library stays out of scope (no
over-engineering); the timestamps now say "UTC".

### Release-process note

`RELEASE_NOTES.md` in the repo still carried the v0.0.3 text when the
v0.0.4 tag was pushed (the v0.0.4 release body was authored directly on
GitHub). The file is now updated per release so the tag-triggered workflow
publishes from the repo again.

### Verification

- 92/92 tests green (`cargo test --workspace`), including the two new
  submit-window regressions and the SFO/registry/speed-blend units — each
  submit-window regression was verified to fail against the pre-fix engine.
- `cargo fmt --check` and `cargo clippy --workspace --all-targets -D
  warnings` clean; `unsafe_code = "deny"` unchanged.
- Reviewed and left unchanged this round: the 4 Hz full-queue snapshot
  clone while connected (already bounded by §3's note and the 2000-file
  scan cap), per-frame search allocations over small queues, and the
  `find_task → task_id 0` degradation path (ends in the existing
  task-gone → Unknown handling).
