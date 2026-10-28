# Remote Package Installer NG v0.0.3

A small update to the free desktop app that installs PS4 game files (`.pkg`) over your home network — no USB stick, no FTP.

## What's new

- **Settings now actually stick.** This release fixes a bug where everything you saved (theme, port, queue options) looked fine but silently vanished on the next launch. Sorry about that — and thanks for the report.
- **Your config lives next to the program.** Settings and the queue are now stored as small JSON files in the same folder as the app itself (e.g. `D:\RPING\`), so the whole folder stays self-contained and portable. The activity log shows the exact location on every start.
- **See your transfer activity live.** While connected, a file-server line under the toolbar shows the live speeds — megabits per second out to the console and back in — plus the totals for the session.
- **New "Check File Server" button** (also in Settings): a one-click self-test of the app's own server. If it answers OK but your PS4 still can't download, you know the problem sits between the two machines — firewall or network — not in the app.
- **Readability fixes:** the troubleshooting text on the connect screen and the small print in Settings are now regular size, and the notes moved below the buttons where they have room to breathe.

## Downloads

Pick the file for your computer:

- **Windows** — `RemotePackageInstallerNG-v0.0.3-Windows-x64.zip`
- **Mac** — `RemotePackageInstallerNG-v0.0.3-macos-universal.tar.gz` (works on every Mac)
- **Linux** — `RemotePackageInstallerNG-v0.0.3-Linux-x64.tar.gz`

First-run notes:

- **Windows** may show "Windows protected your PC" — click *More info*, then *Run anyway*.
- **macOS** may say the app "cannot be verified" — right-click it, choose *Open*, then *Open* again.

## How to use it

Same as before: open the Package Installer on your PS4, run this app on your computer, add your `.pkg` files, press Start. Line up as many as you like — they install one by one, in order. Full guide in the [README](https://github.com/AldySan/Remote-Package-Installer-NG#how-to-use).

Found a problem? Please [open an issue](https://github.com/AldySan/Remote-Package-Installer-NG/issues) — reports from real consoles help a lot.
