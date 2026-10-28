# Remote Package Installer NG v0.0.2

A small update to the free desktop app that installs PS4 game files (`.pkg`) over your home network — no USB stick, no FTP.

## What's new

- **If an install fails, you decide what happens next**: keep sending the rest of the queue, or pause everything until you take a look. Set it in *Settings* (default: keep going, same as before).
- **The app can now check for updates by itself** — on startup and via the "Check for updates" button. It only asks GitHub what the latest version is; nothing else is ever sent. You can turn it off in *Settings*.
- **More package info while transferring**: the queue now also shows the game's version (App Ver) next to the title, with progress, speed and remaining time.
- Fixes from the first release: changing settings (like the theme) now actually applies and saves, and the app no longer opens a CMD window on Windows.

## Downloads

Pick the file for your computer:

- **Windows** — `RemotePackageInstallerNG-v0.0.2-Windows-x64.zip`
- **Mac** — `RemotePackageInstallerNG-v0.0.2-macos-universal.tar.gz` (works on every Mac)
- **Linux** — `RemotePackageInstallerNG-v0.0.2-Linux-x64.tar.gz`

First-run notes:

- **Windows** may show "Windows protected your PC" — click *More info*, then *Run anyway*.
- **macOS** may say the app "cannot be verified" — right-click it, choose *Open*, then *Open* again.

## How to use it

Same as before: open the Package Installer on your PS4, run this app on your computer, add your `.pkg` files, press Start. Line up as many as you like — they install one by one, in order. Full guide in the [README](https://github.com/AldySan/Remote-Package-Installer-NG#how-to-use).

Found a problem? Please [open an issue](https://github.com/AldySan/Remote-Package-Installer-NG/issues) — reports from real consoles help a lot.
