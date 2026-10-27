//! PKG and SFO parser tests with synthesized fixtures.
//!
//! The fixtures are generated in-memory from the documented layout
//! (`docs/BASELINE.md` §2, §4.7) — no copyrighted sample data is used.

use std::io::Write;

use rpi_ng_core::{pkg, sfo::Sfo};

/// Build a minimal but valid PARAM.SFO with the given string entries.
fn build_sfo(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut keys = Vec::new();
    let mut values = Vec::new();
    let mut index = Vec::new(); // (key_off, fmt, used, max, val_off)

    let key_base = 0x14 + entries.len() * 0x10;
    for (k, v) in entries {
        let key_off = keys.len();
        keys.extend_from_slice(k.as_bytes());
        keys.push(0);
        let val_off = values.len();
        values.extend_from_slice(v.as_bytes());
        values.push(0);
        index.push((
            key_off as u16,
            0x0204u16,
            (v.len() + 1) as u32,
            (v.len() + 1) as u32,
            val_off as u32,
        ));
    }

    let mut out = Vec::new();
    out.extend_from_slice(b"\0PSF");
    out.extend_from_slice(&1u32.to_be_bytes());
    out.extend_from_slice(&(key_base as u32).to_be_bytes());
    out.extend_from_slice(&((key_base + keys.len()) as u32).to_be_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (k, f, u, m, v) in index {
        out.extend_from_slice(&k.to_be_bytes());
        out.extend_from_slice(&f.to_be_bytes());
        out.extend_from_slice(&u.to_be_bytes());
        out.extend_from_slice(&m.to_be_bytes());
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(&keys);
    out.extend_from_slice(&values);
    out
}

/// Build a PKG file (header + entry table + SFO payload) in a temp file.
fn build_pkg(
    dir: &std::path::Path,
    name: &str,
    content_id: &str,
    entries: &[(&str, &str)],
) -> std::path::PathBuf {
    let sfo = build_sfo(entries);

    // Entry table: one PARAM.SFO entry located after the header (0x20 bytes).
    let sfo_offset: u32 = 0x2000;
    let entry_table_offset: u32 = 0x2000 + sfo.len() as u32;
    let mut entry_table = Vec::new();
    entry_table.extend_from_slice(&0x1000u32.to_be_bytes()); // id @0x00
    entry_table.extend_from_slice(&[0u8; 0x0C]); // reserved
    entry_table.extend_from_slice(&sfo_offset.to_be_bytes()); // offset @0x10
    entry_table.extend_from_slice(&(sfo.len() as u32).to_be_bytes()); // size @0x14
    entry_table.extend_from_slice(&[0u8; 0x0C]); // trailing padding to 0x20

    let mut header = vec![0u8; 0x2000];
    header[0..4].copy_from_slice(&pkg::PKG_MAGIC);
    header[0x10..0x14].copy_from_slice(&1u32.to_be_bytes()); // entry_count
    header[0x18..0x1C].copy_from_slice(&entry_table_offset.to_be_bytes());
    let cid = content_id.as_bytes();
    header[0x40..0x40 + cid.len()].copy_from_slice(cid);
    header[0x74..0x78].copy_from_slice(&0x1Au32.to_be_bytes()); // GD
    header[0x430..0x438].copy_from_slice(&0x1000u64.to_be_bytes()); // package_size

    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(&header).unwrap();
    f.write_all(&sfo).unwrap();
    f.write_all(&entry_table).unwrap();
    // Pad so the file is big enough.
    let cur = 0x2000 + sfo.len() + entry_table.len();
    let target = 0x1000usize; // package_size declared
    if cur < target {
        f.write_all(&vec![0u8; target - cur]).unwrap();
    }
    f.sync_all().unwrap();
    path
}

#[test]
fn parses_full_pkg_with_sfo_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let path = build_pkg(
        tmp.path(),
        "UP0001-CUSA01234_00-GAME000000000000.pkg",
        "UP0001-CUSA01234_00-GAME000000000000",
        &[
            ("TITLE", "Test Game"),
            ("TITLE_01", "Test Game (EN)"),
            ("CONTENT_ID", "UP0001-CUSA01234_00-GAME000000000000"),
            ("CATEGORY", "gd"),
            ("APP_VER", "01.00"),
        ],
    );
    let info = pkg::parse_pkg_file(&path).unwrap();
    assert_eq!(info.content_id, "UP0001-CUSA01234_00-GAME000000000000");
    assert_eq!(info.title_id.as_deref(), Some("CUSA01234"));
    assert_eq!(info.title.as_deref(), Some("Test Game (EN)"));
    assert_eq!(info.app_version.as_deref(), Some("01.00"));
    assert_eq!(info.category.as_deref(), Some("gd"));
    assert!(!info.is_patch);
    assert_eq!(info.header_size, 0x1000);
    assert_eq!(info.content_kind.package_type(), Some("PS4GD"));
}

#[test]
fn detects_patch_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let path = build_pkg(
        tmp.path(),
        "patch.pkg",
        "UP0001-CUSA01234_00-PATCH00000000000",
        &[("TITLE", "P")],
    );
    // Flip the FIRST_PATCH content flag bit (offset 0x78).
    let mut buf = std::fs::read(&path).unwrap();
    buf[0x78..0x7C].copy_from_slice(&0x0010_0000u32.to_be_bytes());
    std::fs::write(&path, &buf).unwrap();
    let info = pkg::parse_pkg_file(&path).unwrap();
    assert!(info.is_patch);
}

#[test]
fn rejects_bad_magic() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bad.pkg");
    std::fs::write(&path, vec![0u8; 0x2000]).unwrap();
    let err = pkg::parse_pkg_file(&path).unwrap_err();
    assert!(matches!(err, pkg::PkgError::BadMagic));
}

#[test]
fn rejects_oversized_entry_count() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("evil.pkg");
    let mut buf = vec![0u8; 0x2000];
    buf[0..4].copy_from_slice(&pkg::PKG_MAGIC);
    buf[0x10..0x14].copy_from_slice(&100_000u32.to_be_bytes()); // absurd entry count
    std::fs::write(&path, &buf).unwrap();
    let err = pkg::parse_pkg_file(&path).unwrap_err();
    assert!(matches!(err, pkg::PkgError::TooManyEntries(_, _)));
}

#[test]
fn rejects_entry_table_out_of_range() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("evil.pkg");
    let mut buf = vec![0u8; 0x2000];
    buf[0..4].copy_from_slice(&pkg::PKG_MAGIC);
    buf[0x10..0x14].copy_from_slice(&1u32.to_be_bytes());
    buf[0x18..0x1C].copy_from_slice(&0xFFFF_F000u32.to_be_bytes()); // wild offset
    std::fs::write(&path, &buf).unwrap();
    let err = pkg::parse_pkg_file(&path).unwrap_err();
    assert!(matches!(err, pkg::PkgError::EntryTableOffset(_)));
}

#[test]
fn sfo_bounds_and_formats() {
    // Truncated header.
    assert!(Sfo::parse(&[0u8; 4]).is_err());
    // Bad magic.
    assert!(Sfo::parse(&[b'X'; 0x14]).is_err());
    // Normal parse with a u32 entry.
    let sfo_buf = build_sfo(&[("TITLE", "Hello"), ("PARENTAL_LEVEL", "1")]);
    let sfo = Sfo::parse(&sfo_buf).unwrap();
    assert_eq!(sfo.get_str("TITLE"), Some("Hello"));
    assert_eq!(sfo.get_str("PARENTAL_LEVEL"), Some("1"));
    // Localized title fallback.
    assert_eq!(sfo.title(7).as_deref(), Some("Hello"));
}

#[test]
fn multipart_name_splitting() {
    assert_eq!(pkg::split_part_name("Game_0.pkg"), ("Game".into(), Some(0)));
    assert_eq!(pkg::split_part_name("Game_1.pkg"), ("Game".into(), Some(1)));
    assert_eq!(
        pkg::split_part_name("Game_12.pkg"),
        ("Game".into(), Some(12))
    );
    assert_eq!(pkg::split_part_name("Game.pkg"), ("Game".into(), None));
    assert_eq!(
        pkg::split_part_name("Game_v2.pkg"),
        ("Game_v2".into(), None)
    );
    assert_eq!(pkg::split_part_name("GAME_3.PKG"), ("GAME".into(), Some(3)));
}
