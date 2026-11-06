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

## 10. Addendum — third full re-audit (v0.0.6)

Date: 2026-11-03. Same method as §9: line-by-line read of every module
(core + UI) against the protocol facts, plus the mock-backend suite as
behavioral evidence. The code base as shipped in v0.0.5 is in good shape —
this round found two real state-machine defects (both fixed with
regressions) and a handful of edge notes documented below.

### P1 — connecting to another console left the engine in a mixed state (engine)

A successful connect over a *live* console session swapped `console`,
`client`, `console_status` and the file server to the new machine, but the
active task's poller kept watching the OLD console through its captured
old client. Consequences: progress came from the old console while every
remote control call (pause/resume/stop) and the status chip pointed at the
new one; and the file-server swap that follows every connect cuts the old
console's download anyway — so the in-flight item's outcome was unknowable
either way, yet the UI kept showing live progress as if it were healthy.

Fix: a connect over a live session now applies the Disconnect semantics
before switching (stop poller, mark in-flight items `Unknown` with the
reconnect hint, never auto-resubmitted). First connects and reconnects
after an explicit Disconnect are untouched (guarded by `console.is_some()`,
which is `None` in both). The old console's task is *not* stopped — the
download keeps running there, exactly like Disconnect.
Regression: `engine_connecting_elsewhere_parks_inflight_as_unknown`.

### P2 — a user Retry inherited a spent retry budget (queue/engine)

`QueueItem::attempts` only ever grew (transient submit failures increment
it toward `MAX_SUBMIT_ATTEMPTS`) and was reset nowhere. Once a submit
cycle had spent the budget — typically "connection refused" while the
console app was being restarted — every later Retry started with
`attempts == MAX_SUBMIT_ATTEMPTS - 1`, i.e. **zero** automatic retries:
the first transient blip failed the item permanently, and only a second
manual Retry (with the console healthy again) succeeded. The stale v2
`job_id` of the previous console-side job was carried into the fresh
attempt the same way (harmless in practice — every consumer pairs it with
`task_id`, which was reset — but wrong to keep).

Fix: `requeue` resets `attempts`, `job_id` (alongside the existing
`task_id`/`progress`/`error` reset), and a successful submit acceptance
resets `attempts` — the cycle that spent it is over.
Regression: `requeue_resets_the_previous_attempts_bookkeeping`.

### Reviewed and intentionally left unchanged

- **Cross-console task-id collision in reconcile** (theoretical): when
  re-attaching `Unknown` items, the v2 matcher prefers `bgft_task_id` /
  remembered job id before content id. Two different consoles can
  independently assign the same small task id, so a user who disconnects
  from console A mid-install and reconnects to console B *with a live job
  carrying the same id* could see a false match. Requires two consoles
  with colliding ids and deliberate mid-install switching; fixing it
  (content-id-first matching) would break the more common same-console
  re-attach. Documented instead.
- **`update::http_client()` uses `expect()`** on the update-check thread:
  if the reqwest client could not be built (essentially impossible with
  the bundled rustls roots — no native cert loading), the thread would
  panic and the checker's `busy` flag would stay true forever (the UI
  repaints at 150 ms; nothing else breaks). The same file already honors
  "always deliver a result" for thread-spawn/runtime failures; closing
  this last gap would mean widening the public API for a practically
  unreachable path. Left as-is.
- **Legacy reconcile's "will be resumed after the monitored item
  finishes" hint** (attached-second-item branch): nothing auto-resumes
  that item later — the user must press Resume. The branch is only
  reachable with 2+ live tasks, which the single-active-task invariant
  makes near-impossible (a hand-edited/ancient queue.json could). Wording
  note only.
- The 4 Hz snapshot clone, per-frame search allocations, and
  `add_paths`' O(n²) dedup over the 2000-file cap — all re-measured, all
  bounded and irrelevant at realistic queue sizes (see §3/§9).

### Verification

- 94/94 tests green (`cargo test --workspace`), including the two new
  regressions; `cargo fmt --check` and `cargo clippy --workspace
  --all-targets -D warnings` clean; `unsafe_code = "deny"` unchanged.

## 11. Addendum — fourth full re-audit (post-v0.0.6, dev)

Scope: everything, with a deliberate extra pass over the UI layer for
"silent" bugs — things that render wrong or mislead without crashing. Every
finding below carries a regression test, a live experiment, or a headless
measurement.

### Fixed findings

#### P1 — queue rows overlapped each other (queue view)

`ScrollArea::show_rows` requires a **fixed** row height; it positions each
row at `y = index × ROW_HEIGHT` and clips beyond the estimated content
height. Rows here are variable-height (title lines, optional error text,
per-state action row). Measured headlessly with a plain egui context:
a typical row lays out at **~88 px**, one with an error line at **~101 px**,
against the claimed `ROW_HEIGHT = 74` — every row overlapped the next by
14–27 px (the next row's title painted over the previous row's actions),
and long lists clipped their tail. Fix: render the rows in a plain
`ScrollArea` — egui comfortably lays out thousands of widgets per frame, and
the log panel already renders 500 entries unvirtualized. The filter-index
optimization (search needle lowered once, no per-row clones) is kept.

#### P1 — "Check File Server" reported a healthy server as dead

The self-check probed `127.0.0.1:<port>`, but the file server normally binds
the **specific interface** that can reach the console (`pick_bind_ip`) —
binding `192.168.x.x:p` listens only there, so the loopback probe is
refused and the chip said "not running" while a download was streaming.
Proven live in this sandbox (bind LAN IP → loopback refused, own-IP accepted).
Fix: the probe targets the server's real bound address (`self_check(bind)`),
falling back to loopback only for an unspecified bind (`0.0.0.0`) — which is
also the more honest diagnostic, since it exercises the very interface the
console downloads from.

#### P1 — speed readout frozen at poll intervals under 400 ms

The legacy-dialect speed is an EMA over poll deltas with a 0.4 s minimum
sample spacing — but the sample baseline timestamp was refreshed on *every*
poll, so at a 250 ms interval consecutive samples were always 0.25 s apart:
the delta was never used and the readout stayed 0 forever. Fix: the baseline
only advances when the sample is actually consumed (or when the counter
regresses, which reseeds). Regression:
`engine_speed_readout_works_at_fast_poll_intervals`.

#### P1 — late install answers acted on the wrong console

A delayed `POST /api/install` answer landing after a disconnect/console
switch ran through `self.client` — which by then points at a *different*
console (or nothing). Two failure modes: for a parked (`Unknown`) item the
old code recorded the foreign task id and then **stopped** it via the new
client — on the same console this killed a download the design means to
re-attach to, and on a different console the small task id could stop an
**unrelated** task (ids collide across consoles). Fix: the submitting
console travels with the request; a late answer for a parked item only
records the ids (the task keeps running — disconnecting stops *observing*,
not *controlling*), and the cancelled/removed cleanup goes through the very
client the request was sent on.
Regressions: `engine_late_submit_after_disconnect_keeps_ids_and_task_alive`,
`engine_cancel_then_console_switch_stops_only_the_owner_console`.

#### P1 — task ids adopted across consoles (reconcile/resume/cancel)

`task_id`s are console-scoped (BGFT counters start at 1 on every console),
but the engine happily aimed them wherever it was currently connected:
reconcile matched remembered ids against *any* console's job/task list
(could silently attach an item to another console's unrelated download and
drive its state), Resume on a paused item fired at whatever console was
connected, and Cancel of a parked item could stop a foreign task. Fix: the
queue now records which console each `task_id`/`job_id` was learned from
(`task_owner`, `serde(default)` so older queue files load unchanged and
stay permissive); id matching and remote ops are gated on that console.
Content ids remain a valid cross-console match (they identify the *package*).
Regression: `engine_reconcile_does_not_adopt_a_foreign_consoles_task_id`,
`legacy_queue_doc_without_task_owner_loads`. This closes the previously
documented "theoretical" cross-console collision from §10 — recording the
owner turned out to need neither content-id-first matching nor a breaking
change.

#### P3/P2 — small UI/UX hardening

- A search that filters everything out showed "Drop PKG files here" while
  items existed — now a distinct "no matches" state with a clear-search hint.
- "Scan again" and the Connect buttons stayed clickable while a sweep/probe
  was already running (stacking duplicate subnet sweeps / probe churn); the
  engine now also ignores a stacked `Discover` while one is in flight.
- Discovery sweeps no longer stack when the startup auto-sweep overlaps a
  manual "Scan again".

### Reviewed and intentionally left unchanged

- **Snapshot clones the whole queue at each tick** (4 Hz while the server
  runs): bounded, and the numbers only move when something actually happens.
  Re-measured, still irrelevant at realistic queue sizes.
- **`maybe_start_next` does one `fs::metadata` per part on the engine
  thread**: local metadata calls are ~µs; moving them to `spawn_blocking`
  would widen the submit path for no observable gain.
- **Log panel renders up to 500 entries per repaint**: egui caches galley
  layout by (text, style), so only new lines lay out; measured cost is fine.
- **`update::http_client()`'s `expect()`**: same reasoning as §10.
- **Non-virtualized queue at pathological sizes** (2000 items ≈ 20–40 ms
  frame during scrolling): the 2000-file cap is the pathological case itself;
  if it ever matters, a per-row height cache can restore virtualization —
  the plain scroll area removed a whole bug class for the sizes this app
  actually sees.

### Verification

- 99/99 tests green (`cargo test --workspace`), including the six new
  regressions; `cargo fmt --check` and `cargo clippy --workspace
  --all-targets -D warnings` clean; no `unsafe` anywhere in workspace code.
- The row-height overlap was verified by headless egui measurement before
  the fix; the loopback-vs-bound-IP behavior by a live TCP experiment.

## 12. Addendum — fifth full re-audit (pre-v0.0.7, dev)

Scope: everything again, line-by-line, with the same extra pass over the UI
layer for hidden bugs (items that fail to appear, silent errors). The
starting point was dev with §11 applied (99/99 green, clippy and fmt clean).

### Fixed findings

#### P1 — opening a second file dialog silently discarded the first pick (UI)

Both "Add PKG files…" / "Add folder…" wrote `app.dialog = Some(new_pick)`,
replacing any in-flight `DialogPick` — the replaced dialog's channel receiver
was dropped, so a selection the user had *confirmed* in the first dialog was
thrown away with no feedback. The native dialog does not block the egui main
window (parentless on Windows/Linux), so the buttons stay clickable while a
dialog is open; a double-click stacks two dialogs the same way. This is the
classic "I picked the files and nothing appeared" failure. Fix: the Add
buttons disable while a dialog is open (same guard pattern as "Scan again"
while probing), with a hover hint on the disabled button. Pure-UI guard; no
headless egui harness exists in this repo, so it carries no automated test
(the state it guards — `app.dialog` — is trivially inspectable).

#### P2 — the final queue flush raced the hard runtime stop (engine)

`shutdown()` awaited the best-effort remote stop (client timeout 15 s) before
`persist_queue()`, while `EngineHandle::shutdown` hard-stops the runtime after
5 s — against a black-holed console the final flush could be skipped. Every
state transition already persists immediately, so the only realistic loss was
the last progress snapshot, but the reorder is free and makes the flush
deterministic. No observable behavior change otherwise.

#### P3 — remembered consoles on custom ports never re-validated (discovery)

`verify_addr` ignored the port of the address it was given and probed only
12800/9090 — so a console the user had connected to on a custom port (manual
entry) silently failed the "remembered hosts are validated again on use"
pass of every discovery sweep, while remaining perfectly connectable.
Fix: the address's own port is probed first (standard ports follow; subnet
sweep addresses carry a standard port and are unchanged).
Regression: `discovery_verifies_remembered_hosts_on_their_own_port` (the mock
binds an ephemeral port, which is guaranteed non-standard — deterministic
without a fixed test port).

### Reviewed and intentionally left unchanged

- **Submit retries reuse URLs captured at submit time — traced to be
  unreachable-stale.** A URL only goes stale when the file server is
  replaced, and every server-replacement path (connect to another console,
  disconnect/reconnect) first parks `Submitting` items to `Unknown`, which
  fails the retry guard (`state == Submitting`). Retries therefore only ever
  fire inside the console session whose server produced the URLs. Documented
  so the next round does not re-trace it.
- **`AddPaths` during an in-flight scan can stack a second walk** — reachable
  only via drag-and-drop (the toolbar already disables Add while
  `engine_busy`); both walks complete, both file sets are added, and the
  queue dedups by path. A guard would silently *drop* files dragged during a
  long scan — strictly worse than the duplicate walk.
- **egui `raw.dropped_files` re-processing** — verified against the
  egui-winit 0.37 source: `RawInput::take()` moves the list out every frame,
  so a drop is seen by exactly one frame and processed exactly once. No bug.
- **The poller never gives up while the console is Lost** — one bounded
  request per poll interval; if the console comes back, tracking resumes
  without user action. Bounded in rate by design.
- **`queue_summary` omits Paused/Unknown/Cancelled counts** — the total
  includes them and each row carries its own state chip; no data is hidden.
- **`unwrap()`/`expect()` inventory re-checked** — all are startup-time
  builders or guarded by same-tick invariants (`next_queued().expect`,
  `active.as_ref().unwrap()` behind an equality check, lock unwraps that can
  only panic after a fatal engine panic). No new exposure.

### Verification

- 100/100 tests green (`cargo test --workspace`), including the new
  regression; `cargo fmt --check` and `cargo clippy --workspace --all-targets
  -D warnings` clean; no `unsafe` anywhere in workspace code.
