use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct ImageResource {
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct Artist {
    pub id: Option<String>,
    pub uri: Option<String>,
    pub name: String,
    pub images: Vec<ImageResource>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct TrackInfo {
    pub added_at: Option<String>,
    pub id: String,
    pub hex_id: Option<String>,
    pub uri: Option<String>,
    pub name: String,
    pub album_name: Option<String>,
    pub album_id: Option<String>,
    pub album_uri: Option<String>,
    pub artists: Vec<Artist>,
    pub duration_ms: i64,
    pub explicit: bool,
    pub playable: bool,
    pub preview_url: Option<String>,
    pub images: Vec<ImageResource>,
}
