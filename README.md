# Remote Package Installer NG

A desktop app for installing PKG files on a PS4 over your local network.
No USB stick, no FTP — you pick the files on your computer, the app serves
them over HTTP and tells the console to install them.

It's a from-scratch rebuild of the client side of
[flatz's ps4_remote_pkg_installer](https://github.com/flatz/ps4_remote_pkg_installer):
same idea, same protocol, but as a regular desktop app with a queue, real
progress and proper recovery when things go wrong.

Early days — everything listed below is implemented and tested, but treat it
as work in progress.

## What it does

- Finds consoles on your LAN automatically, or connect by IP
- Queue as many PKGs as you like — files or whole folders; multipart
  packages get grouped, duplicates get caught
- Progress, speed and ETA come from what the console actually reports;
  an item only counts as done when the console says it's done
- Pause, resume and cancel per item
- If the app dies mid-install, it asks the console what actually happened
  instead of blindly re-sending anything
- Windows, Linux and macOS — single binary, no runtime deps

## What it doesn't do (yet)

- No PS5 support. The code is laid out so a PS5 backend can be added later,
  but it's not there.
- Not validated on a real console yet. The protocol matches the original
  implementation and there's a test suite with a mock console, but no actual
  PS4 was available while writing this. Feedback from real hardware is welcome.
- No prebuilt downloads — build it yourself for now.

## What you need

- A jailbroken PS4 running [GoldHEN](https://github.com/GoldHEN/GoldHEN)
  (its built-in package installer speaks the same protocol) or flatz's
  Remote Package Installer payload — keep it in the foreground, the PS4
  suspends background apps
- Your own PKG files — this app just delivers them, it doesn't ship any
- [Rust](https://rustup.rs) 1.95 or newer to build

## Building and running

```sh
git clone https://github.com/AldySan/Remote-Package-Installer-NG.git
cd Remote-Package-Installer-NG
cargo run --release
```

First build takes a few minutes. If you build with `cargo build --release`
instead, the binary lands in `target/release/remote-package-installer-ng`.

`cargo test --workspace` runs the full test suite — it's self-contained
(mock console, generated fixtures), no PS4 needed.

Basic flow: start the installer app on the console, open this app, let it
scan (or type the IP), connect, add your PKGs, press start.

## Troubleshooting

- **No console found** — make sure the installer app is in the foreground on
  the PS4 and both machines are on the same network. Try the manual IP. Some
  GoldHEN setups listen on port 9090, so `IP:9090` is worth a shot.
- **"Console lost" mid-install** — the PS4 suspended the app. Reopen it on
  the console, then reconnect here; the queue sorts itself out without
  duplicating tasks.
- **Firewall prompt on your PC** — allow it. The console downloads the files
  from your machine.

## Docs

Longer writeups live in `docs/`: the analysis of the original payload, the
research behind this design, the architecture, a full code audit, and a PS5
roadmap. Start there if you want to know how things work.

## Contributors

- [AldySan](https://github.com/AldySan)

## License

GPL-3.0 — see [LICENSE](LICENSE).

The original [ps4_remote_pkg_installer](https://github.com/flatz/ps4_remote_pkg_installer)
ships no license, so nothing was copied from it — the protocol here was
re-implemented from reading its source. Discovery and probing patterns were
informed by [pkg-sender](https://github.com/Loopayeh/pkg-sender) (MIT),
also re-implemented from scratch.
