# PS5 Roadmap

Research summary and go/no-go assessment for PlayStation 5 support. Evidence
levels follow `docs/RESEARCH.md`: **[VERIFIED]** = read in published source;
**[DOCUMENTED]** = stated by the project itself; **[REPORTED]** = third-party
claims.

## 1. Verified capabilities and sources

| Capability | Evidence | Source |
| --- | --- | --- |
| PS5 removed the BGFT install path the PS4 tool relies on (`sceBgftServiceIntDownloadRegisterTaskBy*` → `0x80990006 SCE_BGFT_ERROR_NOT_SUPPORTED`) | [VERIFIED] | etaHEN `PS5 technical writeups/pkg-writeup.md` |
| PS5 installs go through `libSceAppInstUtil` with ShellCore authid escalation (`0x3800000000000010`) or etaHEN jailbreak IPC | [VERIFIED] | same writeup |
| **etaHEN DPI v1**: JSON API on port **9090** — POST with `url`, `content_name`, `content_id`, `playgo_scenario_id`, `ex_uri`, `icon_url`; replies `{"res": N}` | [VERIFIED] | etaHEN `Source Code/util/source/DirectPKGInstaller.cpp` |
| **etaHEN DPI v2**: web UI + API on port **12800** (libmicrohttpd) | [VERIFIED] | same source |
| **LoopDPI receiver** (`pkg-receiver.elf`, shipped by pkg-sender): listens on **12800** on PS5, installed via the browser exploit once, registers a home-screen shortcut; accepts the RPI-shaped `{"type":"direct","packages":[...]}` install request plus optional `name` / `icon_url` | [VERIFIED] | Loopayeh/pkg-sender source + README |
| A third sender (DirectPackageInstaller) implements PS5 installs via **etaHEN DPI v2** | [DOCUMENTED] | its README |
| Firmware requirements | [REPORTED] | etaHEN supports current firmware ranges; see its releases |

**Conclusion: two independent, source-verified PS5 remote-install paths exist.
PS5 support is feasible — it is a compatibility/engineering problem, not a
research blocker.**

## 2. Differences from the PS4 workflow

| Aspect | PS4 (RPI) | PS5 (etaHEN / LoopDPI) |
| --- | --- | --- |
| Transport | JSON POST API, port 12800 | v1: JSON POST on 9090; v2: HTTP on 12800; LoopDPI: RPI-shaped API on 12800 |
| Task registration | BGFT task id + progress polling | DPI returns `{"res": N}` — **no task id, no documented progress polling** in DPI v1 |
| Package types | PS4GD/AC/AL/DP + sub types | PS5 PKG layout, playgo scenario ids, entitlements |
| Package validation | PKG header + SFO (documented here) | PS5 PKG format differs; the PC side must not assume PS4 layouts |
| Console-side preconditions | HEN + RPI app in foreground | etaHEN (or receiver elf) running; receiver persists across app switches |
| Progress reporting | `get_task_progress` counters | v2 web UI has progress; API contract not source-verified here |

## 3. What the current architecture already gives us

- The queue engine, file server, discovery, persistence and UI are
  generation-agnostic — `docs/ARCHITECTURE.md` §5 was designed for this.
- `ConsoleGeneration` is an explicit model field; nothing in `queue.rs`,
  `fileserver.rs` or `engine.rs` branches on “is PS4”.
- The local file server's Range/streaming contract is exactly what a
  console-pull architecture needs (both PS5 paths pull from the PC).

## 4. What a PS5 adapter needs (dedicated work)

1. **`EtaHenDpiV1Backend`** (9090): install submission, response semantics,
   capability probing, and — critically — an honest progress story (polling
   the DPI v2 endpoints or reporting “submitted, no progress available”).
2. **`LoopDpiBackend`** (12800): RPI-shaped install + `/api/status`; the
   receiver also exposes `/api/files/pull` for non-package files (out of
   scope).
3. **PS5 package validation**: reject PS4 PKGs when a PS5 console is selected
   and vice versa (`pkg::validate_for(generation)` seam already sketched).
4. **Discovery deltas**: etaHEN DPI v2 on 12800 answers like a web app, not
   necessarily like RPI — probes must stay per-generation.
5. **Hardware validation** on a PS5 with etaHEN and with a LoopDPI receiver.

## 5. Go/no-go decision

**GO — as a separate post-v1 milestone.** The PS4 application ships first and
must not depend on PS5 feasibility (it doesn't). PS5 work starts by porting
the two adapters above behind the existing `ConsoleBackend` trait, guarded by
capability detection, with no UI changes beyond rendering new backend rows.

## 6. Blockers / open questions

- DPI v1's `{"res": N}` codes are not documented in the source we inspected —
  needs empirical mapping (hardware).
- Progress reporting quality varies by backend; the UI must degrade honestly
  (show “unknown progress” instead of a fabricated bar).
- Licensing: etaHEN is GPL-3.0 (compatible with our license for *interaction*;
  we do not copy its code); pkg-sender's receiver elf is MIT-distributed
  software — we never redistribute console binaries ourselves.
