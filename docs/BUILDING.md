# Building from source

The README covers normal use with the prebuilt downloads. This page is for
developers who want to build, test or package the app themselves.

## Toolchain

Rust 1.95 or newer — install via [rustup](https://rustup.rs). No other
prerequisites for building and testing: the GUI stack and everything else
are plain Cargo dependencies. (Only regenerating the icon assets needs
Python 3 with Pillow — see the icon section below.)

## Build

```sh
git clone https://github.com/AldySan/Remote-Package-Installer-NG.git
cd Remote-Package-Installer-NG
cargo build --release --locked
```

The binary lands in `target/release/remote-package-installer-ng`
(`remote-package-installer-ng.exe` on Windows). The first build takes a few
minutes.

## Test and lint

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

The suite is self-contained: a mock console implements the wire protocol
(both the legacy flatz dialect and the NextGen v2 job API), and PKG/SFO
fixtures are generated on the fly. No PS4 is needed.

## Release profile

Published binaries are built with maximum compiler optimization, configured
in the workspace `Cargo.toml`:

- `opt-level = 3`, `lto = "fat"`, `codegen-units = 1` — full cross-crate
  optimization
- `strip = true` — symbols stripped, smaller downloads
- Windows builds statically link the C runtime (`.cargo/config.toml`), so no
  "VCRUNTIME140.dll missing" errors on machines without the Visual C++
  redistributable
- The update checker's TLS stack (rustls + aws-lc-rs) builds from source with
  just a C compiler. On Windows x64 it would normally want NASM for its
  assembly routines; `.cargo/config.toml` sets `AWS_LC_SYS_PREBUILT_NASM=1`
  so builds fall back to the prebuilt objects when NASM is not installed
  (Linux and macOS builds are unaffected either way).

## Application icon and platform packaging

The brand icon exists once, as `crates/rpi-ng/assets/icon.png` (1024x1024
RGBA). Every format the platforms need is derived from it by
`python3 packaging/icons/generate.py` (requires Pillow; `--redraw` redraws
the master from the description in the script):

- `crates/rpi-ng/assets/icon.ico` — embedded into the Windows `.exe` as a
  Win32 resource by `crates/rpi-ng/build.rs` (the `winresource` build
  dependency, Windows targets only). This is what Explorer, the taskbar and
  jumplists show before any window exists; the same resource also carries
  FileDescription/ProductName/version strings for Task Manager.
- `crates/rpi-ng/assets/icon_256.png` — decoded at startup in `main.rs` and
  passed to egui as the window icon (`ViewportBuilder::with_icon`): title
  bar and taskbar on Windows and X11. macOS does not support runtime window
  icons — the bundle icon covers it there.
- `crates/rpi-ng/assets/AppIcon.icns` + `packaging/macos/Info.plist` — the
  release workflow builds a universal `RemotePackageInstallerNG.app` bundle
  (lipo + ad-hoc codesign) so Finder, Dock and cmd-tab show the icon.
- `packaging/linux/` — a `.desktop` entry plus hicolor icons shipped in the
  Linux tarball. `main.rs` sets the app_id to the binary name, which X11
  docks (WM_CLASS) and Wayland compositors (app_id) match against the
  desktop file to resolve the window's icon.

`crates/rpi-ng/tests/assets_test.rs` pins every derived file and the naming
convention that ties them together (binary == app_id == `Icon=` ==
`StartupWMClass`), so a regeneration or rename mistake fails CI.

CI builds the same release binary on every push to `dev`/`main`
(`.github/workflows/ci.yml`). Tag pushes (`v*`) trigger
`.github/workflows/release.yml`, which packages a Windows zip, a Linux
tarball (with the optional desktop-integration files) and a universal macOS
app bundle (Intel + Apple Silicon merged with `lipo`) and attaches them to
a GitHub Release.
