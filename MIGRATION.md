# Migration Guide

How to move to Remote Package Installer NG from other tools and workflows.
This is a living document — file an issue when something here is wrong or
missing.

## From the original flatz workflow (npm `serve` + curl)

If you drove the original payload by hand (`docs/BASELINE.md` §1), everything
you did manually is now automated:

| Then | Now |
| --- | --- |
| `serve -p 8080 folder` | Automatic: the app runs its own file server when you connect |
| `curl /api/install` with hand-built URLs | Add PKGs → **Start queue** |
| `curl /api/get_task_progress` in a loop | Automatic progress, speed and ETA |
| `curl /api/pause_task` / `resume_task` | **Pause** / **Resume** buttons |
| Guessing whether it finished | Completion requires console evidence (transfer + copy phase) |
| Manual IP every time | Auto-discovery + remembered consoles |
| Editing JSON by hand | GUI |

Your console-side setup does **not** change: same installer app, same port
12800, same foreground requirement.

## From Loopayeh/pkg-sender

- Your console keeps running the same backends (RPI on PS4; LoopDPI receiver
  on PS5 — the PS5 receiver is not yet supported by this app, see
  `docs/PS5_ROADMAP.md`).
- Discovery approach is equivalent (bounded subnet sweep + verification), but
  this app does not listen for the `PKGSENDER` UDP beacon yet — the PS4 side
  has no beacon to listen to anyway (`docs/RESEARCH.md` §3.5).
- What you gain: a persistent recoverable queue, PC-side package validation,
  and evidence-based completion. What you lose today: PS5 sending,
  disc-image copying (`/api/files/pull`), and the console-side browser UI.

## From DirectPackageInstaller / web senders

- URL/debrid-hosting downloads (AllDebrid, RealDebrid, Google Drive, …) are
  **not implemented** — this app intentionally handles local files first
  (`docs/RESEARCH.md` §4). Download with your usual tool, then add the files.
- GoldHEN Payload-Server injection is not implemented either; use the
  installer app on the console instead.

## From PS4-Vault / library managers

This app is an installer, not a library manager: it will not scan your whole
drive for a catalog, manage cover art, or bulk-rename. Those workflows remain
with those tools; you can still drag files from anywhere into the queue.

## Where your data lives

- Queue and settings are stored per-user:
  - Linux: `~/.local/share/rpi-ng/`
  - Windows: `%APPDATA%\rpi-ng\` (per `directories` crate)
  - macOS: `~/Library/Application Support/rpi-ng/`
- `queue.json` and `settings.json` are plain JSON with atomic writes and a
  `.bak` fallback — you can inspect, back up, or delete them freely.

## Console-side tips that still matter

- Keep the installer app **in the foreground** on the PS4 while sending
  (`docs/BASELINE.md` §4.2) — the console suspends background apps.
- If the app was minimized mid-install, progress may pause; re-open it and
  reconnect — this app reconciles tasks instead of resubmitting.
