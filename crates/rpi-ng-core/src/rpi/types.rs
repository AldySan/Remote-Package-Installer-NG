//! Request/response DTOs for the RPI dialect, with lenient numeric fields.
//!
//! Numbers may arrive as plain integers (strict JSON), unquoted hex literals
//! (repaired by [`super::json_fix`] before deserialization), or quoted
//! strings (`"0x20"`, `"32"`) — some forks quote them. [`LenientNum`] accepts
//! all three.

use serde::{Deserialize, Deserializer, Serialize};

pub fn deserialize_lenient_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Num(u64),
        Neg(i64),
        Str(String),
    }
    match Repr::deserialize(deserializer)? {
        Repr::Num(v) => Ok(v),
        Repr::Neg(v) => {
            if v < 0 {
                // The backend sometimes reports -1 style sentinels; keep the raw
                // bits visible rather than failing.
                Ok(v as u64)
            } else {
                Ok(v as u64)
            }
        }
        Repr::Str(s) => {
            let t = s.trim();
            if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                u64::from_str_radix(hex, 16).map_err(serde::de::Error::custom)
            } else {
                t.parse::<u64>().map_err(serde::de::Error::custom)
            }
        }
    }
}

pub fn deserialize_lenient_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Num(i64),
        Str(String),
    }
    match Repr::deserialize(deserializer)? {
        Repr::Num(v) => Ok(v),
        Repr::Str(s) => {
            let t = s.trim();
            if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                i64::from_str_radix(hex, 16).map_err(serde::de::Error::custom)
            } else if let Some(hex) = t.strip_prefix("-0x").or_else(|| t.strip_prefix("-0X")) {
                let v = i64::from_str_radix(hex, 16).map_err(serde::de::Error::custom)?;
                Ok(-v)
            } else {
                t.parse::<i64>().map_err(serde::de::Error::custom)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct InstallDirectRequest {
    #[serde(rename = "type")]
    pub kind: &'static str, // "direct"
    pub packages: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct InstallRefRequest {
    #[serde(rename = "type")]
    pub kind: &'static str, // "ref_pkg_url"
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct TaskIdRequest {
    pub task_id: i64,
}

#[derive(Debug, Serialize)]
pub struct TitleIdRequest {
    pub title_id: String,
}

#[derive(Debug, Serialize)]
pub struct FindTaskRequest {
    pub content_id: String,
    pub sub_type: i32,
}

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
pub struct InstallResponse {
    pub status: String,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_lenient_i64")]
    pub task_id: i64,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct FailResponse {
    pub status: String,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_lenient_i64")]
    pub error_code: i64,
}

/// `POST /api/get_task_progress` response. Field names mirror the payload
/// exactly; hex-literal values are tolerated.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct TaskProgressDto {
    pub status: String,
    #[serde(default, deserialize_with = "deserialize_lenient_u64")]
    pub bits: u64,
    #[serde(default, deserialize_with = "deserialize_lenient_i64")]
    pub error: i64,
    #[serde(default, deserialize_with = "deserialize_lenient_u64")]
    pub length: u64,
    #[serde(default, deserialize_with = "deserialize_lenient_u64")]
    pub transferred: u64,
    #[serde(default, deserialize_with = "deserialize_lenient_u64")]
    pub length_total: u64,
    #[serde(default, deserialize_with = "deserialize_lenient_u64")]
    pub transferred_total: u64,
    #[serde(default, deserialize_with = "deserialize_lenient_u64")]
    pub num_index: u64,
    #[serde(default, deserialize_with = "deserialize_lenient_u64")]
    pub num_total: u64,
    #[serde(default, deserialize_with = "deserialize_lenient_u64")]
    pub rest_sec: u64,
    #[serde(default, deserialize_with = "deserialize_lenient_u64")]
    pub rest_sec_total: u64,
    #[serde(default, deserialize_with = "deserialize_lenient_i64")]
    pub preparing_percent: i64,
    #[serde(default, deserialize_with = "deserialize_lenient_i64")]
    pub local_copy_percent: i64,
}

/// u32 fields that may arrive as u64 on lenient paths — stored as u64, read as u32.
#[allow(dead_code)]
fn clamp_u32(v: u64) -> u32 {
    v.min(u32::MAX as u64) as u32
}

#[derive(Debug, Deserialize, Clone)]
pub struct IsExistsResponse {
    pub status: String,
    #[serde(default)]
    pub exists: String,
    /// Size on disk when the title exists; 0 when unknown/absent.
    #[serde(default, deserialize_with = "deserialize_lenient_u64")]
    pub size: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct FindTaskResponse {
    pub status: String,
    #[serde(default, deserialize_with = "deserialize_lenient_i64")]
    pub task_id: i64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StatusResponse {
    #[serde(default)]
    pub busy: bool,
    #[serde(default)]
    pub active: Option<u64>,
}

/// Task sub types (BGFT), from upstream README.
pub mod sub_types {
    pub const GAME: i32 = 6;
    pub const AC: i32 = 7;
    pub const PATCH: i32 = 8;
    pub const LICENSE: i32 = 9;
}

/// Glossary of console-side error codes observed in the wild. Only codes with a
/// corroborating source are listed; unknown codes are surfaced verbatim.
pub fn explain_error_code(code: i64) -> Option<&'static str> {
    match code {
        0x8099_0006 => Some("Console reports the operation is not supported (BGFT not available)."),
        0x8099_0004 => Some("Console-side install error (BGFT rejected the task)."),
        0x8099_0007 => Some("Console reports invalid task data (package manifest rejected)."),
        0x8099_000A => Some("Console-side transfer failed (network or disk error during download)."),
        0x8099_0016 => Some("Console reports the task is already running or registered."),
        0x8099_0019 => Some("Console reports the task was not found (it may have finished or been unregistered)."),
        _ => None,
    }
}
