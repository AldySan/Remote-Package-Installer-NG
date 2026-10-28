# Ecosystem Research

Phase 1 research of the PlayStation remote-package ecosystem. Findings are separated by
confidence: **[VERIFIED]** (read in published source code during this research),
**[DOCUMENTED]** (stated by the project's own README/releases), **[REPORTED]** (user
reports or third-party claims), **[UNVERIFIED]** (could not be confirmed).

Sources were inspected between 2026-10-27 via the GitHub/Codeberg REST APIs and raw
file fetches; repository state (license, stars, activity) was captured at that time.

## 1. Survey overview

| Project | Kind | Platform | License | Status | Verdict for this project |
| --- | --- | --- | --- | --- | --- |
| flatz/ps4_remote_pkg_installer | Console payload (API server) | PS4 | none | dormant (2 commits) | **Protocol source of truth** — see `docs/BASELINE.md` |
| rawblue33/Remote-PKG-Installer v3.53/v3.55 | Console payload fork | PS4 | none (binary-only repo) | active releases | Claims catalog; no source to verify — treat as untrusted backend variant |
| Loopayeh/pkg-sender | Desktop sender | Win/Linux | **MIT** | active (2026-10) | Best-engineered open sender; discovery + PS5 receiver; adopt patterns |
| marcussacana/DirectPackageInstaller | Desktop sender/downloader | Win/Linux | **GPL-3.0** | active (2026-06) | URL/debrid workflows, 3 backends incl. etaHEN; out of scope for v1, informs roadmap |
| NookieAI/PS4-Vault | Library manager + sender | Windows | none | active (2026-07) | Library/metadata UX ideas; no reusable protocol code (no license) |
| cy33hc/ps4-ezremote-client | Console-side client | PS4 | GPL-2.0 | very active (2026-10) | Alternative console workflow (no PC needed); informs feature honesty, not our path |
| ItsJokerZz/FPKGi | Console-side installer | PS4/PS5 | GPL-2.0 | **archived** (2026-01) | JSON-list driven installs; archived — reference only |
| GoldHEN/GoldHEN | Kernel payload / HEN | PS4 | unclear (repo ships `.bin` only) | active | Ships "Remote Package Install" feature `[DOCUMENTED]`; closed payload — treat as RPI-compatible backend family |
| etaHEN/etaHEN | Kernel payload / HEN | PS5 | **GPL-3.0** | active (2026-05) | DPI v1 (9090 JSON) + DPI v2 (12800 web) install APIs; core of PS5 feasibility |
| Gkiokan/ps4-remote-pkg-sender | Web sender | browser | none | inactive (2025-01) | Popular web sender; no license — ideas only |
| LightningMods/PS4-Store, Itemzflow | Console-side stores | PS4 | various | active | Contain RPI/BGFT-derived install code `[VERIFIED]` (appear in code search for RPI progress structs) — alternative backends family |

## 2. Project findings

### 2.1 flatz ps4_remote_pkg_installer (original)

Covered fully in `docs/BASELINE.md`. Key protocol facts reused throughout this document:
JSON POST API on port 12800; install via direct URLs or CDN manifest URL; task
lifecycle commands; progress via `get_task_progress`; **the original emits invalid JSON
with unquoted hex literals** `[VERIFIED]`; no discovery beacon `[VERIFIED]`.

### 2.2 Remote-PKG-Installer v3.53/v3.55 (Codeberg, "by Ari")

The repository contains **only a README and a donation image** — the payload is
distributed as release binaries with no source `[VERIFIED by API contents listing]`.
Therefore **none of its claims can be source-verified**. Its changelog is nonetheless
useful as a defect catalog for the original, and several claims cross-check:

- Busy-spin CPU fix (`usleep` when idle) — **consistent with the confirmed `for(;;)
  sb_poll_server()` loop** in the original `[VERIFIED in original]`.
- Web dashboard on port **12801**, mobile-first install flow, `/api/exit` endpoint,
  TV notifications, human-readable progress fields — `[DOCUMENTED]` by the fork only;
  presence of a second port is relevant: senders should not assume 12800 is the only
  service.
- CE-32871-1 exit crash root-caused to privilege escalation orphaning `/app0` —
  `[REPORTED]`; the public flatz source uses syscall 410 and contains no libjbc, so the
  claim does not match the public tree. Console-side regardless — a PC sender cannot fix
  or cause it.
- FreeBSD `MSG_NOSIGNAL`, `SO_REUSEPORT`, `SO_LINGER`, threaded lifetime rework —
  `[REPORTED]`, plausible, unverifiable without source.
- "PS5-safety": not applicable; PS4-only backend.

**Decision:** support it as an opaque RPI-dialect backend (same endpoints), verify
compatibility at runtime via capability probing, and never hard-code assumptions from
its README.

### 2.3 Loopayeh/pkg-sender (MIT) — primary engineering reference

`[VERIFIED]` from source (`LoopDPI.Core/NetDiscovery.cs`, `ConsoleClient.cs`):

- **PS4 and PS5 from one client.** PS4 path: "Remote Package Installer homebrew (port
  12800) or GoldHEN with Payload Server (ports 9090/9021/9020)". PS5 path: ships a
  `pkg-receiver.elf` (branded LoopDPI) sent once via the browser exploit; it keeps
  listening on **12800** and installs a home-screen shortcut.
- **Discovery done right:** enumerates real NICs with real netmasks (never assumes
  /24), computes network/broadcast, caps host enumeration (4096), runs a UDP beacon
  listener on **12801** (magic `PKGSENDER`) with a short timeout, and falls back to a
  parallel TCP sweep of port 12800; dedupes by IP. It also broadcasts a PC-announce on
  **12802** (`PKGSENDER-PC`) so console-side browsers can find the PC.
- **Capability probing pattern:** `GET /api` on the receiver returns the RPI
  "Unsupported method" body — used as a liveness check; `GET /api/status` returns
  `{"busy":true,"active":1}` on newer receivers and HTML on old ones — used as a
  capability check.
- **Protocol quirks handled:** URLs must be encoded exactly once (spaces/special
  characters break installs — their issue #6); `https://` URLs are downgraded to
  `http://` for the console; optional `name` and `icon_url` fields are appended to the
  install JSON (receiver-dependent extras).
- **Extra receiver API** `[VERIFIED]`: `POST /api/files/pull
  {"url","path","mode":"resume"|"overwrite"}` lets the console pull arbitrary PC files
  to `/data/homebrew` (used for disc images).
- Ships self-contained installers (no .NET requirement), AppImage/deb, About-first-run
  UX.

**Adopted by this project:** real-netmask bounded subnet probing, capability probing
instead of port-only detection, single-encoding URL discipline, ports 12800/9090
probing order, and the "don't assume one backend dialect" mindset.

### 2.4 marcussacana/DirectPackageInstaller (GPL-3.0)

`[DOCUMENTED]` from README (source not deep-inspected in this pass):

- Three send methods: **RPI homebrew**, **GoldHEN Payload Server**, **etaHEN DPIv2
  API** — i.e. it independently confirms the etaHEN DPIv2 install path on PS5.
- Feature set: URL-based installs (direct links, AllDebrid/RealDebrid/DebridLink,
  Google Drive, Mediafire, PixelDrain, 1Fichier), PSN update manifests (split PKGs),
  RAR/7z support, resumable and segmented downloads, a local proxy "to speed up PS4
  downloads", CLI mode, JDownloader Click'n'Load, NFS sources, auto LAN cable
  connection (Windows).
- The "automatic proxying" concept is interesting: PC-side buffering can smooth console
  download speeds when the upstream is slow. Not needed for local-file v1.

**Verdict:** strong evidence that multi-backend (RPI/GoldHEN/etaHEN) is the right
adapter shape; URL-source workflows deferred (v1 is local files only).

### 2.5 NookieAI/PS4-Vault (no license)

`[DOCUMENTED]`: Windows-only PKG library manager — scans drives, remembers library,
extracts game name/cover/version/region (i.e. full SFO+icon parsing on PC), duplicate
detection, bulk rename, copy/move with live speed/ETA, and send-to-console. Good UX
reference for library features; **no license → no code reuse**. Our v1 keeps a light
queue-oriented metadata model (title/title-id/size/type), not a full library.

### 2.6 cy33hc/ps4-ezremote-client (GPL-2.0)

`[DOCUMENTED]`: console-side app that browses remote FTP/SFTP/SMB/NFS/WebDAV/HTTP/Google
Drive sources and installs/transfers, with an embedded web server acting as a **proxy**
between the console and remote servers (no PC app needed at all). Background installs
via a companion `ezRemote Server` payload started through GoldHEN's BinLoader; pause/
resume; rest-mode and crash caveats documented honestly ("Do not submit too many
background installs, it can crash ezRemote Server").

**Relevance:** demonstrates the console-pull architecture and honest limitation
reporting. Also confirms GoldHEN BinLoader (9090) as the payload-loading channel used
by companion installers.

### 2.7 ItsJokerZz/FPKGi (GPL-2.0, archived)

`[DOCUMENTED]`: console-side installer driven by JSON content files (like PKGi for
PSP/Vita/PS3), PS4+PS5, downloads from URLs. **Archived 2026-01** — reference only.

### 2.8 GoldHEN (PS4 HEN)

`[VERIFIED]`: the public repo ships prebuilt binaries (`goldhen_2.3_*.bin`) and docs,
not payload source; README lists **"Remote Package Install"** among features, plus FTP
(2121), BinLoader (9090), Klog (3232), internal `/data/pkg` install support.
`[REPORTED]` by both pkg-sender and DirectPackageInstaller: a "Payload Server" mode on
ports 9020/9021 (with 9090) into which senders inject their own installer payload.
Mechanism details are not published in repo docs we could verify.

**Decision for v1:** treat GoldHEN consoles as *possible* RPI-family backends, detected
by probing (12800 first, then 9090) and verified through API responses — never assumed
from brand or port alone. Native "Payload Server" injection is documented as a future
adapter in `docs/ARCHITECTURE.md`.

### 2.9 etaHEN (PS5) — the PS5 path

`[VERIFIED]` from `Source Code/util/source/DirectPKGInstaller.cpp` and
`PS5 technical writeups/pkg-writeup.md`:

- **BGFT is dead on PS5**: `sceBgftServiceIntDownloadRegisterTaskBy*` returns
  `0x80990006 (SCE_BGFT_ERROR_NOT_SUPPORTED)` even with correct auth — Sony removed the
  path the PS4 tool relied on. PS5 installs go through `libSceAppInstUtil` with
  ShellCore-authid privilege escalation (or etaHEN jailbreak IPC / plugin system).
- **DPI v1**: JSON API on port **9090** — accepts a JSON object with `url`,
  `content_name`, `content_id`, `playgo_scenario_id`, `ex_uri`, `icon_url`; replies
  `{"res": N}`.
- **DPI v2**: MHD-based web UI + API on port **12800** (asset requests, file download,
  clear-tmp handlers).
- Privilege requirements (authid `0x3800000000000010` or etaHEN jailbreak IPC) are
  documented in the writeup.
- Loopayeh's receiver independently implements an RPI-shaped `/api/install` on 12800 on
  PS5 `[VERIFIED]` — two independent PS5 remote-install implementations exist.

Full feasibility analysis and go/no-go: `docs/PS5_ROADMAP.md`.

### 2.10 Other senders (PS4RPIGUI, FrostySo Ps4-Pkg-Sender, Gkiokan web sender, CPM)

`[VERIFIED]` via code search: all define the same progress model
(`bits/error/length/transferred/length_total/transferred_total/num_index/num_total/
rest_sec/rest_sec_total/preparing_percent/local_copy_percent`) — the field set is
stable across the ecosystem. None of the inspected senders decode `bits` into named
states; they infer state from counters, error codes and the pause/resume commands they
issued (CPM models only Paused/Installing/Finished). This project follows the same
honest approach. Gkiokan's and PS4-Vault's lack of a license blocks any code reuse.

## 3. Cross-cutting conclusions

1. **The wire protocol is the product.** The console payload's API (flatz dialect) is
   the de-facto standard; ecosystem differentiation happens in the PC client.
2. **Backend fragmentation is real but narrow**: RPI dialect (12800) is the common
   denominator; GoldHEN variants and etaHEN add ports 9090 and extra fields. An
   adapter + runtime capability probing is mandatory — hardcoding "GoldHEN ⇒ works" or
   "port open ⇒ PS4" is exactly how other tools end up lying to users.
3. **Every implementer hits the same traps**: invalid hex JSON from the original,
   URL-encoding of package URLs, foreground-suspension, terse error codes. These are
   first-class requirements for our client, not edge cases.
4. **No one has an authoritative `bits` mapping** — do not invent one.
5. **Discovery has no standard on PS4**; pkg-sender's beacon only works with its own
   PS5 receiver. For PS4 the honest answer is bounded probing + API verification +
   manual IP fallback + remembered hosts.
6. **Performance claims are untrustworthy across the board** (v3.55's "<0.5% CPU",
   "<50ms prerequisites" are closed-source claims). We publish only what we measure,
   and clearly separate mock-based results from hardware results.

## 4. Feature adoption decisions

**Adopted for v1 (each traceable to evidence above):**
- Lenient hex-tolerant JSON parsing (flatz quirk) — `BASELINE.md §3.1`.
- Local HTTP file server with Range + concurrency + allowlist scoping — `BASELINE.md §2`.
- Bounded, netmask-aware subnet probe on port 12800 with API verification, manual IP
  fallback, remembered hosts — pkg-sender `NetDiscovery.cs`.
- Capability probing (`/api` liveness, `/api/status` when available) — pkg-sender
  `ConsoleClient.cs`.
- PKG header/SFO pre-validation on the PC before submission — closes the
  fail-late defect (`BASELINE.md §4.8`).
- Queue with explicit states derived only from observed evidence (counters, error
  codes, issued commands) — ecosystem-wide practice.
- Ports probed in order 12800, then 9090 — pkg-sender; avoids false negatives on
  GoldHEN-family setups.

**Deferred (documented, not built):**
- Direct URL / debrid / archive sources (DirectPackageInstaller's domain) — v1 is
  local-file only; adapter seam exists.
- GoldHEN Payload-Server payload injection — mechanism not source-verified; requires
  shipping a payload binary; licensing unclear.
- PS5 adapters — see `docs/PS5_ROADMAP.md` (feasible, separate milestone).
- Library/cover-art management (PS4-Vault's domain) — post-v1.

**Rejected:**
- Claiming any fix for console-side defects (busy-spin, CE-32871-1, suspension) —
  impossible from the PC side; we only detect and explain them.
- Web-dashboard-on-console features (v3.55) — out of scope for a PC sender.
- Inventing `bits` state names.

## 5. Licensing and attribution

| Component | License | Use in this project |
| --- | --- | --- |
| flatz upstream | none | **No code copied.** Protocol facts re-implemented clean-room in Rust; format constants (offsets, endpoints) are unprotectable facts documented in `docs/BASELINE.md`. |
| pkg-sender (Loopayeh) | MIT | Ideas and constants reused (probe order, discovery approach); re-implemented in Rust. Attribution given here and in README. |
| DirectPackageInstaller, ezRemote, FPKGi, etaHEN | GPL-2.0/GPL-3.0 | Documentation/evidence only; **no code copied** (keeps our GPL-3.0 tree free of incompatible mixing — GPL-2.0 code would be problematic; GPL-3.0 code would obligate carrying derived work notices we don't need). |
| PS4-Vault, Gkiokan sender, codeberg v3.x | none | Ideas only; no code reuse. |
| This project's dependencies | MIT / Apache-2.0 / BSD / ISC (egui, tokio, axum, reqwest, serde, if-addrs, rfd, …) | Permissive; GPL-3.0-compatible. Final license audit happens at release (`docs/TESTING.md` CI gate). |

This repository is licensed **GPL-3.0** (see `LICENSE`).
