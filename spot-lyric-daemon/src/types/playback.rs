use zbus::zvariant::{DeserializeDict, SerializeDict, Type};

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
}
