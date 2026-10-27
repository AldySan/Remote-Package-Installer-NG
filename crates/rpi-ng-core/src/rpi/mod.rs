//! RPI dialect client (the flatz Remote Package Installer wire protocol).

pub mod client;
pub mod json_fix;
pub mod types;

pub use client::{ProbeOutcome, RpiClient, RpiError, progress_dto_to_view};
pub use types::{explain_error_code, sub_types};
