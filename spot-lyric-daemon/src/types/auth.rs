use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct UserProfile {
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub product: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct AuthProfile {
    pub id: String,
    pub label: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct AuthSnapshot {
    pub device_id: String,
    pub client_id: Option<String>,
    pub access_token_expires_at: Option<i64>,
    pub client_token_expires_at: Option<i64>,
    pub active_profile_id: Option<String>,
    pub has_cookie: bool,
    pub profiles: Vec<AuthProfile>,
    pub status: String,
    pub error: Option<String>,
}
