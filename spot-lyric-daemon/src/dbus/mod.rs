pub mod auth_iface;
pub mod lyrics_iface;
pub mod playback_iface;
pub mod server;

use serde::Serialize;

use crate::error::DaemonError;

pub fn to_json_reply<T: Serialize>(value: &T) -> zbus::fdo::Result<String> {
    serde_json::to_string(value)
        .map_err(|error| zbus::fdo::Error::Failed(format!("json serialization failed: {error}")))
}

pub fn to_fdo_error(error: DaemonError) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(error.to_string())
}
