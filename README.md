# Remote Package Installer NG

A modern, cross-platform desktop application for installing PlayStation 4
packages over your local network — built on the [Remote Package Installer
protocol](https://github.com/flatz/ps4_remote_pkg_installer) and designed around
three principles: **easy to use**, **plug-and-play**, and **fast and reliable**.

```
Launch → Discover Console → Add PKGs → Review Queue → Install → Done
```

No accounts, no cloud, no telemetry. Everything happens on your LAN.

> **Status:** pre-release (v0.1.0) — see the
> [Feature status](#feature-status) matrix for exactly what is implemented,
> tested, and planned. Nothing is advertised here that the code and tests do
> not support.

## What it does

- **Finds your console** — a bounded, netmask-aware network scan discovers
  RPI-compatible backends (port 12800, GoldHEN-family fallback on 9090) and
  verifies they actually speak the protocol before showing them. Manual IP
  entry is always available as a fallback.
- **Manages a real queue** — add one file, dozens, or whole folders
  (recursive with your confirmation). Multipart packages are grouped
  automatically, duplicates are detected, and the queue survives app restarts.
- **Validates packages up front** — PKG headers and `PARAM.sfo` are parsed on
  your computer before anything is sent, so broken or mis-grouped files are
  caught before they reach the console.
- **Reports honest progress** — transfer percent, speed and ETA come from the
  console's own progress API. An item is marked **Completed only when the
  console reports both the transfer and copy phases finished** — never merely
  because a request succeeded.
- **Recovers safely** — after a restart, items with unknown outcomes are never
  blindly resubmitted; they are reconciled against the console and you decide.

## Supported platforms (PC)

| OS | Builds | Automated tests |
| --- | --- | --- |
| Linux (X11/Wayland) | ✅ CI | ✅ CI (this repository's test suite) |
| Windows | ✅ CI | ✅ CI |
| macOS | ✅ CI | ✅ CI |

Binaries are produced by CI on every push (workflow artifacts) and on tags
(release artifacts). No runtime dependencies — single binary per platform.

## PS4 prerequisites

1. A jailbroken PS4 with a HEN-class kernel payload (GoldHEN 2.x or
   equivalent).
2. The [Remote Package Installer](https://github.com/flatz/ps4_remote_pkg_installer)
   app (or a compatible fork, e.g. v3.x) installed and **running in the
   foreground** on the console. The PS4 suspends background apps — keep it
   open until the install is underway.
3. Console and computer on the same LAN, with no client isolation.

Supported backends and their verified capabilities:

| Backend | Port(s) | Install | Progress | Pause/Resume | Notes |
| --- | --- | --- | --- | --- | --- |
| flatz Remote Package Installer | 12800 | ✅ | ✅ | ✅ | Protocol source of truth; emits invalid-JSON hex quirks (handled) |
| RPI forks (v3.53/v3.55, …) | 12800 (+12801 dashboards) | ✅ (dialect-compatible) | ✅ | ✅ | Closed-source; detected and verified at runtime like any RPI backend |
| GoldHEN "Remote Package Install" family | 12800 / 9090 | ✅ (via RPI probes) | ✅ | ✅ | Probed in order 12800→9090; capabilities verified before use |
| GoldHEN Payload Server injection | 9020/9021 | ❌ not implemented | — | — | Mechanism not source-verified; future adapter (see `docs/ARCHITECTURE.md` §6) |

Anything that answers the probes but not the protocol is reported as
**incompatible**, not as a console.

## The simplest workflow

1. Start the installer app on your PS4 (keep it in the foreground).
2. Launch Remote Package Installer NG — it scans the network automatically.
3. Click **Connect** next to your console (or type its IP once).
4. Drag & drop `.pkg` files (or use **Add PKG files… / Add folder…**).
5. Press **Start queue**. Watch progress; the console downloads from your
   machine. Done items turn green with evidence-backed completion.

## Feature status

Legend: ✅ implemented + tested · 🟡 implemented, partial · ⏳ planned

| Feature | Status | Notes |
| --- | --- | --- |
| RPI protocol client (all endpoints) | ✅ | Mock-backed conformance tests, incl. hex-JSON quirk |
| Bounded LAN discovery + API verification | ✅ | Real netmasks, 1024-address cap, no busy-polling |
| Manual address fallback + remembered consoles | ✅ | Re-validated on use |
| Local HTTP file server (allowlist, Range, HEAD, streaming) | ✅ | Traversal-proof by construction; concurrent-friendly |
| PKG header + PARAM.SFO parsing, multipart grouping | ✅ | Bounds-hardened; fixtures in tests |
| Persistent queue with recovery semantics | ✅ | Active states → `Unknown`, never auto-resubmitted |
| Duplicate detection (path + content id) | ✅ | |
| Virtualized queue UI, search, per-item actions | ✅ | egui; hundreds of items stay responsive |
| Dark/Light/System theme, settings persistence | ✅ | |
| Deterministic shutdown + graceful server stop | ✅ | Tested at engine level |
| Real-console (hardware) validation | ⏳ | **No PS4 hardware was available** during development; see `docs/TESTING.md` §Hardware validation |
| PS5 support | ⏳ | Feasibility documented in `docs/PS5_ROADMAP.md`; separate milestone |
| URL / debrid / archive sources | ⏳ | Out of scope for v1 (local files only) |

## Building from source

Requirements: a stable Rust toolchain (≥ 1.95).

```sh
git clone https://github.com/AldySan/Remote-Package-Installer-NG
cd Remote-Package-Installer-NG
cargo run --release
```

Run the test suite:

```sh
cargo test --workspace
```

The tests are self-contained (mock backend, generated PKG fixtures) — no
console or special privileges required. See `docs/TESTING.md`.

## Troubleshooting

- **No console found** — keep the installer app in the foreground on the PS4;
  check both devices are on the same network; try the manual address; some
  GoldHEN setups use port 9090 — try `IP:9090`.
- **“Console lost” during an install** — the PS4 suspends the app when you
  switch away. Re-open it on the console, then reconnect here; the queue
  reconciles instead of duplicating tasks.
- **Item stuck in *Installing*** — the transfer is done and the console is
  copying to disk; keep the console awake and check its notifications.
- **Firewall prompt** — allow it: the app must serve files from your computer
  to the console.
- **Console-side errors (0x…)** — surfaced verbatim with a glossary for known
  codes; unknown codes stay visible for searching.

More detail: `docs/BASELINE.md` (what the original does, including its
documented quirks) and `docs/RESEARCH.md` (ecosystem findings).

## Documentation

| Document | Contents |
| --- | --- |
| [`docs/BASELINE.md`](docs/BASELINE.md) | Phase 0 analysis of the original payload: protocol reference, verified defects, compatibility contract |
| [`docs/RESEARCH.md`](docs/RESEARCH.md) | Ecosystem research (pkg-sender, DirectPackageInstaller, GoldHEN, etaHEN, …) with confidence levels and licensing |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Stack selection, engine/queue/server/discovery design, PS5 extension points |
| [`docs/TESTING.md`](docs/TESTING.md) | Test strategy, mock-backend philosophy, what is and is not covered |
| [`docs/PS5_ROADMAP.md`](docs/PS5_ROADMAP.md) | PS5 feasibility (DPI v1/v2, BGFT removal), go/no-go and plan |
| [`MIGRATION.md`](MIGRATION.md) | Coming from other senders / from the original workflow |

## Attribution & license

- This project is a clean-room re-implementation of the wire protocol
  documented by [flatz's ps4_remote_pkg_installer](https://github.com/flatz/ps4_remote_pkg_installer)
  (which ships no license). No upstream code is copied; see
  `docs/RESEARCH.md` §5.
- Discovery/probing patterns were informed by
  [Loopayeh/pkg-sender](https://github.com/Loopayeh/pkg-sender) (MIT) —
  re-implemented in Rust.
- Licensed under **GPL-3.0** — see [LICENSE](LICENSE).
