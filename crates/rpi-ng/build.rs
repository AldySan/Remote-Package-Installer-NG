//! Embeds the brand icon and version metadata into the Windows executable's
//! resource section, so Explorer, the taskbar and jumplists show the app icon
//! before any window exists. The other platforms get their icon differently:
//! at runtime from `assets/icon_256.png` (Windows/X11 title bar, see
//! `main.rs`) or from the packaging assets (`packaging/`, macOS .app bundle
//! and Linux desktop integration) — see `docs/BUILDING.md`.

fn main() {
    // build.rs also runs while Linux/macOS CI builds the workspace; only
    // Windows binaries carry a Win32 resource section.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
        res.set(
            "FileDescription",
            "Remote Package Installer NG - PS4 remote PKG installer",
        );
        res.set("ProductName", "Remote Package Installer NG");
        res.set("ProductVersion", &version);
        res.set("FileVersion", &version);
        res.compile()
            .expect("failed to embed the Windows icon resource");
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
}
