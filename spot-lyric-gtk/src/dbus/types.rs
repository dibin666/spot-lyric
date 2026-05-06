//! Cross-thread types used by the D-Bus bridge and UI.
//!
//! These mirror the JSON shapes documented in `backend-integration.md` §3.

use serde::{Deserialize, Serialize};
use zbus::zvariant::{DeserializeDict, SerializeDict, Type};

// ─── Auth ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthProfile {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub created_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthSnapshot {
    pub device_id: String,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub access_token_expires_at: Option<i64>,
    #[serde(default)]
    pub client_token_expires_at: Option<i64>,
    #[serde(default)]
    pub active_profile_id: Option<String>,
    #[serde(default)]
    pub has_cookie: bool,
    #[serde(default)]
    pub profiles: Vec<AuthProfile>,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
}

fn default_status() -> String {
    "idle".to_string()
}

fn default_preferred_playback_source() -> String {
    "auto".to_string()
}

// ─── Playback ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlaybackSettings {
    #[serde(default = "default_preferred_playback_source")]
    pub preferred_playback_source: String,
}

/// PlaybackState — must serialize as a D-Bus dict (`a{sv}`) to match the
/// daemon's `cn.spotlyric.Playback.GetState` return / `StateChanged` signal.
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

// ─── Lyrics ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LyricsWord {
    pub text: String,
    #[serde(default)]
    pub start_time_ms: i64,
    #[serde(default)]
    pub end_time_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LyricsLine {
    pub text: String,
    #[serde(default)]
    pub translated_text: Option<String>,
    #[serde(default)]
    pub start_time_ms: i64,
    #[serde(default)]
    pub end_time_ms: i64,
    #[serde(default)]
    pub words: Vec<LyricsWord>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LyricsPayload {
    #[serde(default)]
    pub track_uri: Option<String>,
    #[serde(default)]
    pub track_id: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub sync_type: String,
    #[serde(default)]
    pub lines: Vec<LyricsLine>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LyricsCandidate {
    pub candidate_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub artists: Vec<String>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub score: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoredLyricsCandidate {
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub artists: Vec<String>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    pub id: String,
    #[serde(default)]
    pub mid: Option<String>,
    pub provider: String,
    #[serde(default)]
    pub score: Option<f64>,
    pub title: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedLyricsMatch {
    pub spotify_track_id: String,
    #[serde(flatten)]
    pub candidate: StoredLyricsCandidate,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LyricsSettings {
    #[serde(default)]
    pub lyrics_timing_offset_ms: i32,
    #[serde(default = "default_provider")]
    pub preferred_provider: String,
    #[serde(default)]
    pub saved_match: Option<SavedLyricsMatch>,
}

fn default_provider() -> String {
    "netease".to_string()
}
