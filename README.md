# Remote Package Installer NG

**Put PS4 games on your console over your home network — no USB stick, no FTP.**

A free, open-source desktop app for **Windows**, **macOS** and **Linux**. You pick the game files (`.pkg`) on your computer, and the app sends them straight to your PS4 — with real progress, pause and resume.

[Download](https://github.com/AldySan/Remote-Package-Installer-NG/releases) · [How to use](#how-to-use) · [Report a problem](https://github.com/AldySan/Remote-Package-Installer-NG/issues)

## What you need

- A PS4 with [GoldHEN](https://github.com/GoldHEN/GoldHEN) — you'll use the **Package Installer** inside it. ([RPI NextGen](https://github.com/AldySan/ps4_remote_pkg_installer_nextgen) works too.)
- Your PS4 and your computer on the **same network** — same Wi-Fi or same router.
- Your own `.pkg` game files, saved anywhere on your computer.

No accounts, no console-side setup, and nothing ever leaves your own network.

## How to use

1. **On the PS4**, open GoldHEN and start **Package Installer**. Keep it on screen the whole time — the PS4 pauses apps that go to the background.
2. **On your computer**, download the app from the [Releases](https://github.com/AldySan/Remote-Package-Installer-NG/releases) page and unpack it:
   - Windows → `RemotePackageInstallerNG-vX.Y.Z-Windows-x64.zip`
   - Mac → `RemotePackageInstallerNG-vX.Y.Z-macos-universal.tar.gz` (works on every Mac)
   - Linux → `RemotePackageInstallerNG-vX.Y.Z-Linux-x64.tar.gz`
3. Run the app. It looks for your PS4 on its own. If nothing shows up, type the PS4's IP address instead — you'll find it on the console under *Settings → Network → View Connection Status*.
4. Add your `.pkg` files (one or many, whole folders too) and press **Start**.
5. When the app says an item is **Done**, the game appears on your PS4's home screen.

You can line up as many files as you like — they install one by one, in order, with speed and remaining time shown for each. Pause or cancel any of them. If a transfer fails, you choose what happens: continue with the rest or pause the queue (in *Settings*). If the app or the console gets interrupted, it checks with the PS4 what actually finished instead of blindly sending everything again.

## Common problems

- **A blue "Windows protected your PC" window appears** — normal for unsigned free apps. Click *More info*, then *Run anyway*.
- **macOS says the app "cannot be verified"** — right-click the app, choose *Open*, then *Open* again. Needed only the first time.
- **No PS4 shows up** — make sure the installer is open on the console (not sitting in the background) and both machines are on the same network. Typing the IP manually helps; with some GoldHEN versions you need to add the port, like `192.168.1.20:9090`.
- **A firewall message appears on your computer** — allow it. The PS4 downloads the files from your computer, so the app needs to accept that connection.
- **An install stops halfway** — the PS4 probably put the installer app in the background. Open it again on the console, then reconnect in the app — it will figure out where things actually stand and continue from there.
- **Anything else** — [open an issue](https://github.com/AldySan/Remote-Package-Installer-NG/issues) and describe what you did and what happened. This is an early release; reports genuinely help.

## Good to know

- The app **does not come with games** and does not download anything. You bring your own `.pkg` files — it just delivers them to the console.
- The app **checks GitHub for a newer release** on startup (and when you press *Check for updates*). That request carries nothing but the app's name and version — no accounts, no identifiers — and you can turn it off in *Settings*.
- **It's portable:** your settings and queue are stored as small JSON files **next to the program itself** (same folder as the executable), so everything travels together — copy the folder, and your setup comes with it. The activity log tells you the exact location on every start.
- While you're connected, the app shows a live **file-server line** with the in/out transfer rates (Mbps), the totals for the session and a **Check File Server** button — a one-click self-test that tells you whether the app's own server is fine or whether something between the two machines (firewall, network) is the problem.
- Installing happens through the console's own installer. The app doesn't modify or "hack" your PS4.
- **PS4 only.** PS5 is not supported.

## For developers

Written in Rust (egui + tokio). Building from source, tests, architecture and protocol documentation: see [docs/BUILDING.md](docs/BUILDING.md) and the [docs folder](docs/).

## Contributors

- [AldySan](https://github.com/AldySan)

## License

GPL-3.0 — see [LICENSE](LICENSE).

The original [ps4_remote_pkg_installer](https://github.com/flatz/ps4_remote_pkg_installer) ships no license, so nothing was copied from it — the protocol here was re-implemented from reading its source. Discovery and probing patterns were informed by [pkg-sender](https://github.com/Loopayeh/pkg-sender) (MIT), also re-implemented from scratch.
