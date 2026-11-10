//! Guards for the shipped icon and packaging assets (see `docs/BUILDING.md`).
//!
//! The app shows a real brand icon everywhere a platform allows one:
//! embedded as a Win32 resource in the `.exe` (`build.rs`), decoded at
//! runtime for the window title bar (`main.rs`), inside the macOS `.app`
//! bundle (`packaging/macos/Info.plist` + `AppIcon.icns`), and as freedesktop
//! icon-theme files for Linux (`packaging/linux/`). These tests pin every
//! file and the cross-file naming (binary == app_id == `Icon=` ==
//! `StartupWMClass`) so a regeneration or rename mistake fails CI instead
//! of shipping a broken or orphaned icon.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The name that ties the binary, the X11/Wayland `app_id` (`main.rs`), the
/// `.desktop` file name, its `Icon=`/`StartupWMClass` keys and the hicolor
/// icon file names together.
const APP_NAME: &str = "remote-package-installer-ng";

fn repo_file(rel: &[&str]) -> PathBuf {
    let mut path = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    for part in rel {
        path.push(part);
    }
    path
}

fn asset(name: &str) -> Vec<u8> {
    let path = repo_file(&["assets", name]);
    std::fs::read(&path).unwrap_or_else(|e| panic!("missing asset {}: {e}", path.display()))
}

#[test]
fn master_icon_is_a_1024_square() {
    let img = image::load_from_memory(&asset("icon.png")).expect("master icon decodes");
    assert_eq!((img.width(), img.height()), (1024, 1024));
}

#[test]
fn runtime_window_icon_decodes_the_way_main_rs_does() {
    let bytes = asset("icon_256.png");
    let rgba = image::load_from_memory(&bytes)
        .expect("runtime window icon decodes")
        .to_rgba8();
    assert_eq!((rgba.width(), rgba.height()), (256, 256));
    assert_eq!(rgba.as_raw().len(), 256 * 256 * 4);
}

#[test]
fn windows_ico_is_a_multi_size_resource() {
    let data = asset("icon.ico");
    assert!(data.len() >= 6, "truncated ICONDIR");
    assert_eq!(&data[0..4], &[0, 0, 1, 0], "ICO magic (type 1 = icon)");
    let count = u16::from_le_bytes([data[4], data[5]]) as usize;
    assert_eq!(count, 7, "expected 16..256 plus common in-betweens");

    let mut sizes = Vec::new();
    for i in 0..count {
        let e = 6 + i * 16;
        let w = if data[e] == 0 { 256 } else { data[e] as usize };
        let h = if data[e + 1] == 0 {
            256
        } else {
            data[e + 1] as usize
        };
        assert_eq!(w, h, "entry {i} is not square");
        assert_eq!(
            u16::from_le_bytes([data[e + 6], data[e + 7]]),
            32,
            "entry {i} is not 32 bpp"
        );
        let len =
            u32::from_le_bytes([data[e + 8], data[e + 9], data[e + 10], data[e + 11]]) as usize;
        let off =
            u32::from_le_bytes([data[e + 12], data[e + 13], data[e + 14], data[e + 15]]) as usize;
        assert!(off + len <= data.len(), "entry {i} points outside the file");
        assert_eq!(
            &data[off..off + 4],
            &[0x89, b'P', b'N', b'G'],
            "entry {i} is not PNG data"
        );
        sizes.push(w);
    }
    sizes.sort_unstable();
    assert_eq!(sizes, vec![16, 24, 32, 48, 64, 128, 256]);
}

#[test]
fn macos_icns_is_a_valid_png_container() {
    let data = asset("AppIcon.icns");
    assert_eq!(&data[0..4], b"icns", "icns magic");
    let total = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
    assert_eq!(total, data.len(), "length field != file size");

    let mut off = 8;
    let mut types = Vec::new();
    while off < data.len() {
        assert!(off + 8 <= data.len(), "truncated chunk header");
        let typ = String::from_utf8_lossy(&data[off..off + 4]).into_owned();
        let len = u32::from_be_bytes([data[off + 4], data[off + 5], data[off + 6], data[off + 7]])
            as usize;
        assert!(len >= 8 && off + len <= data.len(), "corrupt chunk {typ}");
        assert_eq!(
            &data[off + 8..off + 12],
            &[0x89, b'P', b'N', b'G'],
            "chunk {typ} is not PNG"
        );
        types.push(typ);
        off += len;
    }
    assert_eq!(off, data.len(), "chunks must tile the file exactly");
    for want in ["ic11", "ic12", "ic07", "ic08", "ic09", "ic10"] {
        assert!(
            types.iter().any(|t| t == want),
            "missing {want} (have {types:?})"
        );
    }
}

fn desktop_entry() -> BTreeMap<String, String> {
    let path = repo_file(&[
        "..",
        "..",
        "packaging",
        "linux",
        "remote-package-installer-ng.desktop",
    ]);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing desktop entry {}: {e}", path.display()));
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            assert!(
                map.insert(key.trim().to_owned(), value.trim().to_owned())
                    .is_none(),
                "duplicate key {key}"
            );
        }
    }
    map
}

#[test]
fn linux_desktop_entry_is_wellformed_and_matches_the_app() {
    let entry = desktop_entry();
    assert_eq!(entry.get("Type").map(String::as_str), Some("Application"));
    assert_eq!(entry.get("Exec").map(String::as_str), Some(APP_NAME));
    assert_eq!(entry.get("Icon").map(String::as_str), Some(APP_NAME));
    assert_eq!(entry.get("Terminal").map(String::as_str), Some("false"));
    assert_eq!(
        entry.get("StartupWMClass").map(String::as_str),
        Some(APP_NAME)
    );
    assert!(entry.get("Name").is_some_and(|n| !n.is_empty()));

    // The Icon= key resolves inside a freedesktop icon theme: every declared
    // hicolor size must actually ship.
    for size in [48, 64, 128, 256] {
        let dir = format!("{size}x{size}");
        let file = format!("{APP_NAME}.png");
        let icon = repo_file(&[
            "..",
            "..",
            "packaging",
            "linux",
            "icons",
            "hicolor",
            &dir,
            "apps",
            &file,
        ]);
        assert!(icon.is_file(), "missing hicolor icon {}", icon.display());
    }

    // main.rs must set the same app_id (X11 WM_CLASS / Wayland app_id), or
    // docks and taskbars will not associate the window with this entry.
    let main_rs =
        std::fs::read_to_string(repo_file(&["src", "main.rs"])).expect("main.rs readable");
    let needle = format!("const APP_ID: &str = \"{APP_NAME}\";");
    assert!(
        main_rs.contains(&needle),
        "main.rs APP_ID no longer matches the desktop entry (expected {needle})"
    );
}

#[test]
fn macos_bundle_plist_wires_up_the_icns() {
    let path = repo_file(&["..", "..", "packaging", "macos", "Info.plist"]);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing Info.plist {}: {e}", path.display()));

    assert!(text.contains("<key>CFBundleIconFile</key><string>AppIcon</string>"));
    assert!(text.contains(&format!(
        "<key>CFBundleExecutable</key><string>{APP_NAME}</string>"
    )));
    assert!(text.contains("<key>CFBundleIdentifier</key><string>io.github.aldysan."));
    // The release workflow substitutes the tag version into these fields.
    assert!(text.contains("__VERSION__"));

    // CFBundleIconFile must point at an asset that actually ships next to
    // the executable inside the .app bundle (see release.yml).
    assert!(!asset("AppIcon.icns").is_empty());
}
