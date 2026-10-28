# Testing

Testing is a first-class part of this project, not an afterthought. The suite
runs on every push, on all three desktop platforms (see
`.github/workflows/ci.yml`).

## Philosophy

1. **Protocol conformance is tested against a faithful mock.** The original
   payload's API is reproduced byte-for-byte in `tests/rpi_mock.rs` — including
   its **invalid JSON with unquoted hex literals** (`docs/BASELINE.md` §3.1).
   A client that only parses strict JSON would pass a naive mock and fail on a
   real console; ours does not.
2. **Mocks are protocol evidence, not compatibility evidence.** A green mock
   test proves we speak the documented dialect. It does **not** prove a real
   PS4 accepts us — that requires hardware (§Hardware validation).
3. **State transitions require evidence.** The engine tests assert that items
   reach `Completed` only when the (mock) console reports full transfer *plus*
   the copy phase, and that failures surface the console's error codes
   verbatim.
4. **Recovery is tested, not assumed.** Queue documents with active items are
   round-tripped and must come back as `Unknown` with their task ids retained.

## Layout

| Suite | File | Covers |
| --- | --- | --- |
| Unit | `src/rpi/json_fix.rs` (inline) | Hex-literal repair, string-awareness, escapes |
| Unit | `src/rpi/client.rs` (inline) | DTO parsing in all three dialects (decimal, quoted hex, unquoted hex); error glossary |
| Integration | `tests/rpi_mock.rs` | Probe/dialect recognition; install + progress over both dialects; backend error codes; task-gone; connection-refused; **engine end-to-end** (connect → add → parse → submit → poll → completed with evidence, persistence round-trip); **engine failure path** (console-side error → `Failed` with code) |
| Integration | `tests/fileserver_test.rs` | Full/suffix/open-ended/multi ranges (206/416/200), HEAD, allowlist enforcement, traversal attempts, unregister, URL encoding, concurrent streams |
| Integration | `tests/queue_test.rs` | Multipart grouping order, duplicate detection, skip-missing, move/requeue/clear, stats, recovery mapping via `Queue::from_json` |
| Integration | `tests/pkg_test.rs` | Generated PKG/SFO fixtures: full metadata parse, patch flag, bad magic, entry-count/offset hardening, SFO bounds, multipart name splitting |

Run everything:

```sh
cargo test --workspace
```

The suite is fully self-contained: no console, no root, no network beyond
loopback. Engine tests use isolated data directories (`spawn_with`), so they
run in parallel without polluting user state.

## What the mock console reproduces (and why)

From the source-verified behavior of the original payload:

- `GET /api` answers 400 “Unsupported method” — the liveness probe used by the
  ecosystem.
- `POST /api/install` requires `{"type":"direct","packages":[...]}` and
  performs a **Range request against the first package URL** before accepting
  (the prerequisite fetch, `docs/BASELINE.md` §2) — so the tests also prove our
  file server serves ranges to a realistic client.
- `POST /api/get_task_progress` returns the original's broken-hex body shape
  by default (configurable to strict JSON) with per-poll transfer advancement
  and a post-transfer copy phase.
- Task lifecycle endpoints, `find_task`, `is_exists`, and a `/api/status`
  capability probe.

## Hardware validation

**Status: not yet performed — no PS4 hardware was available in the development
environment.** This limitation is stated rather than papered over:

- All protocol claims in this repository are source-derived
  (`docs/BASELINE.md`) or mock-verified (this suite).
- Real-console validation must cover, at minimum: flatz RPI on a HEN console;
  a v3.x fork; a GoldHEN 2.4b console via both 12800 and 9090; a multipart
  patch install; suspension/reconnect mid-install; and console-side error
  paths (e.g. `CE-32871-1`-adjacent failures, which are console-side and not
  fixable by any sender).
- Results will be recorded here with firmware/backend versions when hardware
  is available.

## Performance & benchmarking

The master plan calls for measured claims only. What we currently assert:

- **Idle CPU: structurally zero.** The UI repaints only on interaction or
  engine events (`request_repaint_after` is issued solely while work is
  active); the engine has no polling loops when no task is active — a direct
  answer to the original payload's confirmed busy-spin defect
  (`docs/BASELINE.md` §4.1). This is enforced by design and review, not
  measured on target hardware yet.
- **Transfer throughput:** bounded 64 KiB chunks streamed from disk
  (`fileserver.rs`), verified for correctness under 4 concurrent range
  streams in tests. Sender-side throughput on real hardware is a console-side
  network property — to be benchmarked with hardware and reported honestly.
- **Startup time / memory footprint:** not yet measured; will be added to this
  document together with platform numbers from release builds.

No fabricated benchmark numbers appear anywhere in this project.
