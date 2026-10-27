//! PKG header and entry-table parsing.
//!
//! Mirrors the on-disk format the upstream payload itself parses
//! (`docs/BASELINE.md` §4.7): big-endian fields, header size 0x2000,
//! entry table of 0x20-byte entries located at `entry_table_offset`,
//! PARAM.SFO entry id 0x1000 and icon0.png entry id 0x1200.
//!
//! The parser is intentionally conservative: `entry_count` is capped, every
//! offset is bounds-checked against the real file size, and failures produce
//! an `Err` with a precise reason instead of a panic.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use crate::{
    model::{ContentKind, PkgInfo},
    sfo::{Sfo, SfoValue},
};

pub const PKG_MAGIC: [u8; 4] = [0x7F, b'P', b'K', b'G'];
pub const HEADER_SIZE: usize = 0x2000;
pub const ENTRY_SIZE: usize = 0x20;
pub const ENTRY_ID_PARAM_SFO: u32 = 0x1000;
pub const ENTRY_ID_ICON0_PNG: u32 = 0x1200;

pub const MAX_ENTRY_COUNT: u32 = 512;
pub const MAX_SFO_SIZE: u32 = 1024 * 1024;
pub const CONTENT_ID_LEN: usize = 0x24;

const CONTENT_FLAGS_FIRST_PATCH: u32 = 0x0010_0000;
const CONTENT_FLAGS_SUBSEQUENT_PATCH: u32 = 0x4000_0000;

#[derive(Debug, thiserror::Error)]
pub enum PkgError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("file too small ({0} bytes, need at least {1})")]
    TooSmall(u64, u64),
    #[error("bad magic (not a PS4 PKG)")]
    BadMagic,
    #[error("entry count {0} exceeds limit {1}")]
    TooManyEntries(u32, u32),
    #[error("entry table offset 0x{0:X} out of range")]
    EntryTableOffset(u64),
    #[error("PARAM.SFO entry missing")]
    NoSfo,
    #[error("PARAM.SFO size {0} exceeds limit")]
    SfoTooLarge(u32),
    #[error("content id is not valid ASCII")]
    BadContentId,
    #[error("PARAM.SFO parse error: {0}")]
    Sfo(#[from] crate::sfo::SfoError),
}

fn be_u32(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn be_u64(b: &[u8], off: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[off..off + 8]);
    u64::from_be_bytes(v)
}

/// Parse PKG metadata from an open file, seeking as needed (at most
/// header + entry table + SFO bytes are read; large files are never scanned).
pub fn parse_pkg(file: &mut File, file_size: u64) -> Result<PkgInfo, PkgError> {
    if file_size < HEADER_SIZE as u64 {
        return Err(PkgError::TooSmall(file_size, HEADER_SIZE as u64));
    }

    let mut header = vec![0u8; HEADER_SIZE];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut header)?;
    if header[0..4] != PKG_MAGIC {
        return Err(PkgError::BadMagic);
    }

    let entry_count = be_u32(&header, 0x10);
    let entry_table_offset = be_u32(&header, 0x18) as u64;
    if entry_count == 0 {
        return Err(PkgError::NoSfo);
    }
    if entry_count > MAX_ENTRY_COUNT {
        return Err(PkgError::TooManyEntries(entry_count, MAX_ENTRY_COUNT));
    }
    let entry_table_size = entry_count as u64 * ENTRY_SIZE as u64;
    if entry_table_offset < HEADER_SIZE as u64
        || entry_table_offset.saturating_add(entry_table_size) > file_size
    {
        return Err(PkgError::EntryTableOffset(entry_table_offset));
    }

    // Entry table.
    let mut table = vec![0u8; entry_table_size as usize];
    file.seek(SeekFrom::Start(entry_table_offset))?;
    file.read_exact(&mut table)?;

    let mut sfo_off: Option<(u64, u32)> = None;
    for i in 0..entry_count as usize {
        let e = i * ENTRY_SIZE;
        let id = be_u32(&table, e);
        let offset = be_u32(&table, e + 0x10) as u64;
        let size = be_u32(&table, e + 0x14);
        if id == ENTRY_ID_PARAM_SFO {
            if size == 0 || size > MAX_SFO_SIZE {
                return Err(PkgError::SfoTooLarge(size));
            }
            if offset.saturating_add(size as u64) > file_size {
                return Err(PkgError::EntryTableOffset(offset));
            }
            sfo_off = Some((offset, size));
        }
    }
    let Some((sfo_offset, sfo_size)) = sfo_off else {
        return Err(PkgError::NoSfo);
    };

    // PARAM.SFO bytes.
    let mut sfo_buf = vec![0u8; sfo_size as usize];
    file.seek(SeekFrom::Start(sfo_offset))?;
    file.read_exact(&mut sfo_buf)?;
    let sfo = Sfo::parse(&sfo_buf)?;

    // Header fields.
    let content_id_raw: [u8; CONTENT_ID_LEN] =
        header[0x40..0x40 + CONTENT_ID_LEN].try_into().unwrap();
    let trim = content_id_raw
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(CONTENT_ID_LEN);
    if !content_id_raw[..trim].is_ascii() {
        return Err(PkgError::BadContentId);
    }
    let content_id = String::from_utf8_lossy(&content_id_raw[..trim]).into_owned();

    let content_type = be_u32(&header, 0x74);
    let content_flags = be_u32(&header, 0x78);
    let header_size = be_u64(&header, 0x430);

    let is_patch = (content_flags & CONTENT_FLAGS_FIRST_PATCH) != 0
        || (content_flags & CONTENT_FLAGS_SUBSEQUENT_PATCH) != 0;

    let title_id = match sfo.get_str("CONTENT_ID") {
        Some(cid) => PkgInfo::title_id_from_content_id(cid)
            .or_else(|| PkgInfo::title_id_from_content_id(&content_id)),
        None => PkgInfo::title_id_from_content_id(&content_id),
    };

    let title = sfo.title(1); // English localization id, mirrors upstream default
    let app_version = sfo.get_str("APP_VER").map(|s| s.to_string());
    let package_version = sfo.get_str("VERSION").map(|s| s.to_string());
    let category = sfo.get_str("CATEGORY").map(|s| s.to_string());
    let _ = SfoValue::U32(0); // keep enum import used

    Ok(PkgInfo {
        content_id,
        title_id,
        content_kind: ContentKind::from_raw(content_type),
        is_patch,
        title,
        app_version,
        package_version,
        category,
        header_size,
    })
}

/// Convenience wrapper that opens the file and parses it.
pub fn parse_pkg_file(path: &Path) -> Result<PkgInfo, PkgError> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    parse_pkg(&mut file, size)
}

/// Heuristic multipart detection from a file name: `Name_0.pkg`, `Name_1.pkg`, ...
/// Returns `(base, Some(index))` for pieces, `(full_stem, None)` otherwise.
pub fn split_part_name(file_name: &str) -> (String, Option<u32>) {
    let lower = file_name.to_ascii_lowercase();
    if !lower.ends_with(".pkg") {
        return (file_name.to_string(), None);
    }
    let stem = &file_name[..file_name.len() - 4];
    let Some(pos) = stem.rfind('_') else {
        return (stem.to_string(), None);
    };
    let tail = &stem[pos + 1..];
    if tail.is_empty() || !tail.chars().all(|c| c.is_ascii_digit()) {
        return (stem.to_string(), None);
    }
    let idx: u32 = tail.parse().unwrap_or(u32::MAX);
    if idx == u32::MAX {
        return (stem.to_string(), None);
    }
    (stem[..pos].to_string(), Some(idx))
}
