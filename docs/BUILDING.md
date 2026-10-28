# Building from source

The README covers normal use with the prebuilt downloads. This page is for
developers who want to build, test or package the app themselves.

## Toolchain

Rust 1.95 or newer — install via [rustup](https://rustup.rs). No other
prerequisites: the GUI stack and everything else are plain Cargo dependencies.

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

CI builds the same release binary on every push to `dev`/`main`
(`.github/workflows/ci.yml`). Tag pushes (`v*`) trigger
`.github/workflows/release.yml`, which packages archives for Windows, Linux
and a universal macOS binary (Intel + Apple Silicon merged with `lipo`) and
attaches them to a GitHub Release.
