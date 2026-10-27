//! PARAM.SFO parser (format used by PlayStation packages).
//!
//! Layout (all big-endian), mirrored from the upstream payload's `sfo.c` and
//! hardened with explicit bounds checks:
//!
//! ```text
//! header (0x14 bytes): magic "\0PSF", version u32, key_table_offset u32,
//!                      value_table_offset u32, entry_count u32
//! entries  (0x20 * entry_count bytes... actually 0x10 each):
//!   key_offset u16, data_format u16, used_size u32, max_size u32, value_offset u32
//! ```

use std::collections::BTreeMap;

pub const SFO_MAGIC: &[u8; 4] = b"\0PSF";
const MAX_ENTRIES: usize = 256;
const MAX_ENTRY_SIZE: usize = 64 * 1024;
const MAX_TOTAL_SIZE: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SfoValue {
    String(String),
    U32(u32),
}

#[derive(Debug, Clone, Default)]
pub struct Sfo {
    pub entries: BTreeMap<String, SfoValue>,
}

#[derive(Debug, thiserror::Error)]
pub enum SfoError {
    #[error("too short ({len} bytes)")]
    TooShort { len: usize },
    #[error("bad magic")]
    BadMagic,
    #[error("entry count {0} exceeds limit {1}")]
    TooManyEntries(usize, usize),
    #[error("entry table exceeds buffer")]
    EntryTableOverflow,
    #[error("key offset out of bounds (key {key}, offset {offset})")]
    KeyOutOfBounds { key: usize, offset: usize },
    #[error("value out of bounds (key {key}, offset {offset}, size {size})")]
    ValueOutOfBounds { key: usize, offset: usize, size: usize },
    #[error("entry size {size} exceeds limit for key {key}")]
    EntryTooLarge { key: usize, size: usize },
    #[error("unterminated key string (key {key})")]
    UnterminatedKey { key: usize },
}

fn be_u16(b: &[u8], off: usize) -> u16 {
    u16::from_be_bytes([b[off], b[off + 1]])
}

fn be_u32(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

impl Sfo {
    /// Parse a PARAM.SFO buffer. Trailing garbage is ignored; all offsets are
    /// validated against the buffer length.
    pub fn parse(buf: &[u8]) -> Result<Self, SfoError> {
        if buf.len() < 0x14 {
            return Err(SfoError::TooShort { len: buf.len() });
        }
        if &buf[0..4] != SFO_MAGIC {
            return Err(SfoError::BadMagic);
        }
        let key_table = be_u32(buf, 0x08) as usize;
        let value_table = be_u32(buf, 0x0C) as usize;
        let entry_count = be_u32(buf, 0x10) as usize;
        if entry_count > MAX_ENTRIES {
            return Err(SfoError::TooManyEntries(entry_count, MAX_ENTRIES));
        }
        if entry_count.checked_mul(0x10).map(|n| 0x14 + n).unwrap_or(usize::MAX) > buf.len() {
            return Err(SfoError::EntryTableOverflow);
        }

        let mut entries = BTreeMap::new();
        for i in 0..entry_count {
            let e = 0x14 + i * 0x10;
            let key_offset = be_u16(buf, e) as usize;
            let data_format = be_u16(buf, e + 2);
            let used_size = be_u32(buf, e + 4) as usize;
            let value_offset = be_u32(buf, e + 0x0C) as usize;

            let key_abs = key_table.checked_add(key_offset).ok_or(SfoError::KeyOutOfBounds {
                key: i,
                offset: usize::MAX,
            })?;
            if key_abs >= buf.len() {
                return Err(SfoError::KeyOutOfBounds { key: i, offset: key_abs });
            }
            let key_end = buf[key_abs..]
                .iter()
                .position(|&b| b == 0)
                .ok_or(SfoError::UnterminatedKey { key: i })? + key_abs;
            if key_end > buf.len() {
                return Err(SfoError::UnterminatedKey { key: i });
            }
            let key = String::from_utf8_lossy(&buf[key_abs..key_end]).into_owned();

            if used_size > MAX_ENTRY_SIZE {
                return Err(SfoError::EntryTooLarge { key: i, size: used_size });
            }
            let value_abs = value_table.checked_add(value_offset).ok_or(
                SfoError::ValueOutOfBounds { key: i, offset: usize::MAX, size: used_size },
            )?;
            let value_end = value_abs
                .checked_add(used_size)
                .ok_or(SfoError::ValueOutOfBounds { key: i, offset: usize::MAX, size: used_size })?;
            if value_abs > buf.len() || value_end > buf.len() {
                return Err(SfoError::ValueOutOfBounds { key: i, offset: value_abs, size: used_size });
            }

            let value = match data_format {
                0x0204 => {
                    let raw = &buf[value_abs..value_end];
                    let len = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
                    SfoValue::String(String::from_utf8_lossy(&raw[..len]).into_owned())
                }
                0x0404 => SfoValue::U32(be_u32(buf, value_abs)),
                0x0004 => {
                    // Special/int8-ish format; treat as u32 read when possible.
                    if value_abs + 4 <= buf.len() {
                        SfoValue::U32(be_u32(buf, value_abs))
                    } else {
                        SfoValue::U32(buf[value_abs] as u32)
                    }
                }
                _ => continue, // unknown formats are skipped, not fatal
            };
            entries.insert(key, value);
        }
        Ok(Self { entries })
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        match self.entries.get(key) {
            Some(SfoValue::String(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn get_u32(&self, key: &str) -> Option<u32> {
        match self.entries.get(key) {
            Some(SfoValue::U32(v)) => Some(*v),
            _ => None,
        }
    }

    /// Localized title: prefers `TITLE_XX` for `lang_id`, falls back to `TITLE`.
    pub fn title(&self, lang_id: u8) -> Option<String> {
        let localized = format!("TITLE_{lang_id:02}");
        if let Some(t) = self.get_str(&localized)
            && !t.is_empty() {
                return Some(t.to_string());
            }
        self.get_str("TITLE").map(|t| t.to_string())
    }

    /// Total buffer size cap used by callers.
    pub fn max_total_size() -> usize {
        MAX_TOTAL_SIZE
    }
}
