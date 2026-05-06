pub mod app_state;
pub mod config;
pub mod dbus;
pub mod domain;
pub mod error;
pub mod lyrics_external;
pub mod playback_sources;
pub mod spotify;
pub mod storage;
pub mod types;
pub mod util;

pub use error::{DaemonError, Result};
