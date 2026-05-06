use serde::{Deserialize, Serialize};
use zbus::zvariant::{DeserializeDict, SerializeDict, Type};

pub const PLAYBACK_SOURCE_AUTO: &str = "auto";
pub const PLAYBACK_SOURCE_MPRIS: &str = "mpris";
pub const PLAYBACK_SOURCE_DEALER: &str = "dealer";
pub const PLAYBACK_SOURCE_CONNECT_STATE: &str = "connect-state";
pub const PLAYBACK_SOURCE_WEB_API: &str = "web-api";
pub const PLAYBACK_SOURCE_WEB_API_CACHE: &str = "web-api-cache";
pub const PLAYBACK_SOURCE_IDLE: &str = "idle";
pub const PLAYBACK_SOURCE_ERROR: &str = "error";

const DEFAULT_SOURCE_ORDER: [&str; 4] = [
    PLAYBACK_SOURCE_MPRIS,
    PLAYBACK_SOURCE_DEALER,
    PLAYBACK_SOURCE_CONNECT_STATE,
    PLAYBACK_SOURCE_WEB_API,
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaybackSettings {
    #[serde(default = "default_preferred_playback_source")]
    pub preferred_playback_source: String,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            preferred_playback_source: default_preferred_playback_source(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, SerializeDict, DeserializeDict, Type, Default)]
#[zvariant(signature = "a{sv}")]
pub struct PlaybackState {
    pub is_playing: bool,
    pub track_uri: String,
    pub track_name: String,
    pub artist_name: String,
    pub album_name: String,
    pub album_art_url: String,
    pub position_ms: i64,
    pub duration_ms: i64,
    pub volume: f64,
    pub player_status: String,
    pub playback_source: String,
}

pub fn default_preferred_playback_source() -> String {
    PLAYBACK_SOURCE_AUTO.to_string()
}

pub fn normalize_preferred_playback_source(source: &str) -> Option<String> {
    let normalized = source.trim().to_ascii_lowercase();
    match normalized.as_str() {
        PLAYBACK_SOURCE_AUTO
        | PLAYBACK_SOURCE_MPRIS
        | PLAYBACK_SOURCE_DEALER
        | PLAYBACK_SOURCE_CONNECT_STATE
        | PLAYBACK_SOURCE_WEB_API => Some(normalized),
        _ => None,
    }
}

pub fn playback_source_order(preferred: &str) -> Vec<&'static str> {
    let preferred = normalize_preferred_playback_source(preferred)
        .unwrap_or_else(default_preferred_playback_source);
    if preferred == PLAYBACK_SOURCE_AUTO {
        return DEFAULT_SOURCE_ORDER.to_vec();
    }

    let first = match preferred.as_str() {
        PLAYBACK_SOURCE_MPRIS => PLAYBACK_SOURCE_MPRIS,
        PLAYBACK_SOURCE_DEALER => PLAYBACK_SOURCE_DEALER,
        PLAYBACK_SOURCE_CONNECT_STATE => PLAYBACK_SOURCE_CONNECT_STATE,
        PLAYBACK_SOURCE_WEB_API => PLAYBACK_SOURCE_WEB_API,
        _ => PLAYBACK_SOURCE_MPRIS,
    };
    let mut order = vec![first];
    order.extend(
        DEFAULT_SOURCE_ORDER
            .iter()
            .copied()
            .filter(|source| *source != first),
    );
    order
}
