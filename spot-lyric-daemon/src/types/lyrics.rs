use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct LyricsWord {
    pub text: String,
    pub start_time_ms: i64,
    pub end_time_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct LyricsLine {
    pub text: String,
    pub translated_text: Option<String>,
    pub start_time_ms: i64,
    pub end_time_ms: i64,
    pub words: Vec<LyricsWord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct LyricsPayload {
    pub track_uri: Option<String>,
    pub track_id: Option<String>,
    pub language: Option<String>,
    pub provider: Option<String>,
    pub source: String,
    pub sync_type: String,
    pub lines: Vec<LyricsLine>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct LyricsCandidate {
    pub candidate_id: String,
    pub album: String,
    pub artists: Vec<String>,
    pub duration_ms: Option<i64>,
    pub provider: String,
    pub score: Option<f64>,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct LyricsSettings {
    pub lyrics_timing_offset_ms: i32,
    pub preferred_provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_match: Option<SavedLyricsMatch>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct StoredLyricsCandidate {
    pub album: String,
    pub artists: Vec<String>,
    pub duration_ms: Option<i64>,
    pub id: String,
    pub mid: Option<String>,
    pub provider: String,
    pub score: Option<f64>,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct SavedLyricsMatch {
    pub spotify_track_id: String,
    #[serde(flatten)]
    pub candidate: StoredLyricsCandidate,
    pub created_at: i64,
    pub updated_at: i64,
}
