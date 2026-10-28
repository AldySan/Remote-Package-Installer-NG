# Baseline: flatz/ps4_remote_pkg_installer

This document records the Phase 0 inspection of the original project that defines the
protocol and behavior this application must stay compatible with.

- **Upstream:** https://github.com/flatz/ps4_remote_pkg_installer
- **Revision inspected:** `f12834101553d0f42c36107cff2b3d1708574937` (`master`, 2 commits)
- **Inspection method:** full source review of every first-party file (`main.c`, `server.c`,
  `installer.c`, `pkg.c`, `sfo.c`, `http.c`, `net.c`, `module.c`, `util.c`, headers), the
  vendored third-party libraries, the build system, and the README.
- **Confidence labels:** every finding below is labeled `[CONFIRMED]` (read directly in
  source), `[DOCUMENTED]` (stated by the upstream README), `[REPORTED]` (claimed by third
  parties, not verifiable in this source), or `[UNVERIFIED]`.

## 1. What the original project actually is

The repository contains **only the PS4-side payload** — a C application that runs on the
console as a fake-PKG homebrew (`remote_pkg_installer`) and exposes an HTTP JSON API on
**port 12800** for receiving commands from any client on the local network. It is **not**
a desktop application: there is no PC sender in this repository.

Flatz's own workflow (from the README) was to run an arbitrary local web server
(`npm install serve`, `serve -p <port> <folder>`) to host `.pkg` files and drive the
console with `curl`. Every "Remote PKG Installer" desktop tool in the ecosystem
(pkg-sender, PS4RPIGUI, DirectPackageInstaller, RPI v3.x, ...) is a third-party client
for this API. Consequently, the compatibility contract this project must preserve is the
**server-side API of the PS4 payload**, not the behavior of any particular sender.

Repository facts:

| Fact | Value |
| --- | --- |
| Language | C (PS4 Orbis platform) |
| Commits | 2 (`Initial commit`, `Fix`) |
| License | **None.** No top-level LICENSE file exists. Vendored deps carry their own licenses (sandbird: MIT, tiny-json: MIT, uthash: BSD-1/2/3-clause style). |
| Build system | GNU Make driving the **proprietary Orbis SDK** (`orbis-clang`, `orbis-ld`, `orbis-pub-cmd`) through Wine, plus fPKG publishing tools |
| UI on console | None (splash screen only) `[DOCUMENTED]` |

> **Licensing note:** because upstream publishes no license, its code is treated as
> reference-only. This project does **not** copy upstream source; it re-implements the
> wire protocol (facts and formats) in Rust. See `docs/RESEARCH.md` §Licensing.

## 2. Runtime architecture of the original payload

`[CONFIRMED]` from `main.c`, `module.c`, `installer.c`, `server.c`:

1. Loads internal system modules: SysCore, SystemService, UserService, NetCtl, Net,
   Http, Ssl, NpCommon, Bgft, AppInstUtil.
2. Escalates privileges via **custom syscall 410** (`SYS_gain_privileges`,
   `syscalls.h:13`) — a syscall that only exists in patched kernels (HEN-class payloads).
3. Initializes `AppInstUtil` (existence checks, uninstall) and `BGFT`
   (Background File Transfer: task registration, progress).
4. Starts a **sandbird** HTTP server bound to the console IP on port **12800**
   (`main.c` `SERVER_PORT`), serving:
   - `POST /api/*` — the JSON command API (see §3),
   - `GET /static/*` — generated manifest JSON files that BGFT later downloads.
5. Working directory is `/data`; temporary prerequisite files are written there and
   cleaned up by a best-effort sweep of files older than `CLEANUP_DAY_COUNT = 3` days
   (`server.c` `cleanup_temp_files`).

Install flow (`handle_api_install`, `pkg.c pkg_setup_prerequisites`) `[CONFIRMED]`:

1. The payload **downloads the first `0x2000` bytes** (header) of the first package URL
   using its own HTTP client, which issues `Range: bytes=0-8191` requests.
2. It then downloads the **entry table** (`entry_count` entries × `0x20` bytes at
   `entry_table_offset`) via another range request.
3. It extracts `PARAM.sfo` (entry id `0x1000`) and `icon0.png` (entry id `0x1200`) via
   further range requests.
4. It parses the SFO for `TITLE`, `CONTENT_ID`, and derives package type
   (`PS4GD`/`PS4AC`/`PS4AL`/`PS4DP` from `content_type` at header offset `0x74`), patch
   flag (from `content_flags`), and total size (offset `0x430`).
5. It writes a generated manifest JSON to `/data/tmp_<id>.json`, registers a **BGFT
   download task**, and returns the new `task_id`.
6. The console's BGFT subsystem then fetches the manifest from
   `http://<console-ip>:12800/static/tmp_<id>.json` and downloads the package pieces
   from the **sender's** HTTP server.

**Implication for any PC-side sender** `[CONFIRMED]`: the sender's local file server
must support HTTP **Range requests** (at minimum for the prerequisite phase), must
handle **concurrent requests** (prerequisite fetches plus BGFT downloads; upstream
README explicitly notes the PS4 needs a multi-threaded server), and must keep the URL
stable for the lifetime of the task.

## 3. Wire protocol reference (extracted from source)

All command endpoints are `POST` with JSON bodies. Responses are `application/json`
bodies written by `sb_writef`. `[CONFIRMED]` from `server.c` handler table and bodies.

| Endpoint | Request | Success response |
| --- | --- | --- |
| `/api/install` (direct) | `{"type":"direct","packages":["http://...", ...]}` | `{"status":"success","task_id":N,"title":"..."}` |
| `/api/install` (CDN) | `{"type":"ref_pkg_url","url":"http://.../manifest.json"}` | same as above |
| `/api/is_exists` | `{"title_id":"CUSA09311"}` | `{"status":"success","exists":"true","size":"0x..."}` (size only when exists) |
| `/api/find_task` | `{"content_id":"...","sub_type":6}` | `{"status":"success","task_id":N}` |
| `/api/get_task_progress` | `{"task_id":N}` | see below |
| `/api/start_task` | `{"task_id":N}` | `{"status":"success"}` |
| `/api/stop_task` | `{"task_id":N}` | `{"status":"success"}` |
| `/api/pause_task` | `{"task_id":N}` | `{"status":"success"}` |
| `/api/resume_task` | `{"task_id":N}` | `{"status":"success"}` |
| `/api/unregister_task` | `{"task_id":N}` | `{"status":"success"}` |
| `/api/uninstall_game` | `{"title_id":"..."}` | `{"status":"success"}` |
| `/api/uninstall_patch` | `{"title_id":"..."}` | `{"status":"success"}` |
| `/api/uninstall_ac` | `{"content_id":"..."}` | `{"status":"success"}` |
| `/api/uninstall_theme` | `{"content_id":"..."}` | `{"status":"success"}` |

Failure responses are produced by `kick_error_json`:

```json
{ "status": "fail", "error_code": 0x809900XX }
```

Task sub types `[CONFIRMED]`: Game=6, AC=7, Patch=8, License=9.
Package types `[CONFIRMED]`: content_type `0x1A`→`PS4GD` (app/patch/remaster),
`0x1B`→`PS4AC` (AC data / themes), `0x1C`→`PS4AL` (AC nodata), `0x1E`→`PS4DP` (delta patch).

`/api/get_task_progress` success body `[CONFIRMED]` (format string at `server.c:1326`):

```
{"status":"success","bits":0x…,"error":N,"length":0x…,"transferred":0x…,
 "length_total":0x…,"transferred_total":0x…,"num_index":N,"num_total":N,
 "rest_sec":N,"rest_sec_total":N,"preparing_percent":N,"local_copy_percent":N}
```

Field semantics (from the BGFT struct mirrored in `installer.h` and community usage):

- `bits` — task state bitmask from BGFT (see §4.3 for the decoding problem).
- `error` — BGFT error result (`0` while healthy).
- `length` / `transferred` — current piece; `length_total` / `transferred_total` — whole task.
- `num_index` / `num_total` — current piece index / piece count (multipart).
- `rest_sec`, `rest_sec_total` — seconds remaining per piece / total.
- `preparing_percent` — percent of the pre-download preparation phase.
- `local_copy_percent` — percent of the post-download local copy (install) phase.

### 3.1 Critical quirk: the original emits invalid JSON

`[CONFIRMED]` — `server.c` writes `bits`, `length`, `transferred`, `length_total`,
`transferred_total` and `error_code` as **unquoted hexadecimal literals**
(e.g. `{"bits":0x20,...}`), which strict JSON parsers reject. Any client that claims to
speak to the original payload must therefore parse these responses leniently (accept
`0x…` literals, quoted hex, or plain integers). This project implements a tolerant
parser for exactly this reason, and its mock backend reproduces the quirk faithfully
(see `docs/TESTING.md`).

## 4. Verified limitations and defects of the original

### 4.1 Busy-spin server loop — 100% CPU on console

`[CONFIRMED]` — `server.c server_listen()` is:

```c
for (;;) {
    sb_poll_server(s_server);
}
```

with the server configured via `opts.timeout = "0"` (non-blocking poll). Combined, this
spins one CPU core continuously while the app is open, matching the v3.53 fork's public
claim ("100% CPU busy-spin & screaming fans"). This is a **console-side** defect; a PC
sender cannot fix it, and this project makes no such claim.

### 4.2 App must remain in the foreground to receive commands

`[DOCUMENTED]` — the PS4 suspends background applications, which stops networking. The
upstream README instructs users to keep the app in focus until the "waiting to install"
phase completes, and describes recovery (re-open the app, resume the task) if the
connection is lost. A sender can mitigate this only by detecting task progress and
guiding the user; it cannot prevent suspension.

### 4.3 Progress `bits` are undocumented

`[CONFIRMED]` — the payload forwards BGFT's raw `bits` bitmask with a TODO
("make bits field more user-friendly") and no definition. No first-party documentation
of the bit layout exists in the repository. Community senders variously ignore `bits`
and infer state from byte counters, error codes, and pause/resume calls. This project
does the same (see `docs/ARCHITECTURE.md` §Progress interpretation) and does **not**
invent bit meanings.

### 4.4 Small JSON parser pool

`[CONFIRMED]` — every handler allocates a 256-node `tiny-json` pool. Long `packages`
arrays (many multipart pieces) or long URLs can exhaust it, failing the request. The
v3.55 fork's changelog (pool raised to 2048 nodes) is consistent with this limit. This
is a console-side constraint; senders should keep install requests small.

### 4.5 No discovery mechanism

`[CONFIRMED]` — the payload exposes no beacon, mDNS, SSDP, or UDP service; the server
listens on plain TCP 12800 only. Auto-discovery from the PC side can therefore only be
a bounded port probe plus API verification, or a fork-specific beacon (e.g. pkg-sender's
receiver broadcasts UDP 12801, which the original does not).

### 4.6 Console-side error surfaces are terse

`[CONFIRMED]` — failures are reported as `{"status":"fail","error_code":0x…}` with raw
Orbis error codes and no message. The PC client must map known codes to actionable
text and leave unknown codes visible verbatim.

### 4.7 Reported but not verifiable in this source

- `[REPORTED]` **CE-32871-1 after closing the app** — the v3.53 fork attributes this to
  root escalation orphaning the `/app0` mount and claims a fix. The public source here
  escalates via custom syscall 410 and contains **no libjbc reference**, so the fork's
  description does not match this source tree; possibly it refers to different base
  binaries. Recorded as unverified; not reproduced.
- `[REPORTED]` **Multipart installs "unstable/truncated"** in the base — the source
  handles multiple pieces correctly in principle (pieces are passed in order and summed
  into `package_size`); no reproducing evidence is available here. Recorded as
  unverified.
- `[REPORTED]` **SIGPIPE crashes on client disconnect mid-transfer** — would live in
  sandbird's send paths; not reproduced (requires console hardware). The fork's claim
  that `MSG_NOSIGNAL` constants were wrong for Orbis is plausible but unverifiable here.

### 4.8 Prerequisites make installs fail late

`[CONFIRMED]` — the payload validates packages (header parse, SFO extraction) **after**
the install request is submitted. A malformed PKG surfaces as a 500 "Internal server
error" with a terse reason. A PC-side sender that parses headers up front (as this
project does) catches most of these before touching the console.

## 5. Baseline reproduction status

| Activity | Status | Reason |
| --- | --- | --- |
| Build original payload | **Not attempted** | Requires the proprietary Orbis SDK + publishing tools under Wine; not available in this environment. Environmental limitation, documented rather than worked around. |
| Run original payload | Not possible | Requires a jailbroken PS4 with HEN-class kernel payload (custom syscall 410). No console hardware in this environment. |
| Idle CPU / memory measurements of original | Not possible | Follows from the above. No numbers are claimed anywhere in this project. |
| Protocol behavior tests | Via mock backend | `rpi-ng-core` integration tests implement a faithful mock of the documented API (including the invalid-JSON quirk). Mocks are **not** evidence of real-console compatibility. |

## 6. Compatibility contract this project must preserve

1. Target address form `http://<console-ip>:12800`, plain HTTP, JSON POST commands.
2. `install` request shapes: `{"type":"direct","packages":[...]}` (multipart pieces in
   order) and `{"type":"ref_pkg_url","url":...}`.
3. Lenient parsing of success/error bodies, including unquoted hex values.
4. Progress polling via `/api/get_task_progress` with byte-counter semantics.
5. Task lifecycle commands: start/stop/pause/resume/unregister.
6. Serving local packages over HTTP with **Range support** and concurrent request
   handling, at a stable URL.
7. Not fabricating completion: an install is complete only when progress reports full
   transfer and no error, never merely when the HTTP request succeeded.
