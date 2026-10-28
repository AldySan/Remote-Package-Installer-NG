//! Remote Package Installer NG — core library.
//!
//! Contains everything except the desktop UI: the RPI protocol client, PKG/SFO
//! parsing, the local file server, LAN discovery, the persistent queue and the
//! orchestration engine. See `docs/ARCHITECTURE.md` for the big picture.
//!
//! Library users drive everything through [`engine::spawn`], which returns an
//! [`engine::EngineHandle`] (commands in, events out). All other modules are
//! also usable standalone (the mock-backend tests do exactly that).

pub mod discovery;
pub mod engine;
pub mod fileserver;
pub mod model;
pub mod pkg;
pub mod queue;
pub mod rpi;
pub mod settings;
pub mod sfo;
pub mod update;

pub use engine::{Command, EngineHandle, spawn};
pub use model::Event;
pub use settings::Settings;

/// Crate version reported in logs and the UI.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
