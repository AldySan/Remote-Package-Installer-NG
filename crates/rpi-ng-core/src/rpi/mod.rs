//! RPI dialect client (the flatz Remote Package Installer wire protocol),
//! plus the v2 job API (Remote PKG Installer NextGen and future v2 speakers).

pub mod client;
pub mod json_fix;
pub mod types;

pub use client::{ProbeOutcome, RpiClient, RpiError, progress_dto_to_view};
pub use types::{V2Job, V2SystemData, explain_error_code, sub_types, v2_states};
