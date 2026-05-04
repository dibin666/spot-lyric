use std::{
    collections::HashMap,
    path::Path,
    sync::{LazyLock, Mutex as StdMutex},
    time::Duration,
};

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use hmac::{Hmac, Mac};
use reqwest::header::DATE;
use serde::Deserialize;
use sha1::Sha1;
use tokio::sync::{broadcast, Mutex};

use crate::{
    error::{DaemonError, Result},
    storage::{CookieProfileState, CookieStore, DeviceStore},
    types::AuthSnapshot,
};

use super::{discovery::ProtocolRegistry, transport::AuthProvider};

const ACCESS_TOKEN_MARGIN_MS: i64 = 5 * 60_000;
const CLIENT_TOKEN_MARGIN_MS: i64 = 5 * 60_000;
const MAX_CONSECUTIVE_FAILURES: u32 = 3;
const DEFAULT_SECRETS_REMOTE_URL: &str =
    "https://raw.githubusercontent.com/xyloflake/spot-secrets-go/main/secrets/secretDict.json";
const DEFAULT_OPEN_SPOTIFY_HEAD_URL: &str = "https://open.spotify.com/";
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

static CACHED_SECRETS: LazyLock<StdMutex<Option<SecretDict>>> =
    LazyLock::new(|| StdMutex::new(None));

#[derive(Debug, Clone)]
pub struct AuthServiceOptions {
    pub cookie_store: CookieStore,
    pub device_store: DeviceStore,
    pub client: reqwest::Client,
    pub protocol: ProtocolRegistry,
    pub secrets_remote_url: Option<String>,
    pub open_spotify_head_url: Option<String>,
}

#[derive(Debug)]
struct AuthRuntimeState {
    access_token: Option<String>,
    client_token: Option<String>,
    consecutive_failures: u32,
    snapshot: AuthSnapshot,
}

#[derive(Debug, Clone)]
pub struct AuthService {
    client: reqwest::Client,
    cookie_store: CookieStore,
    device_store: DeviceStore,
    open_spotify_head_url: String,
    protocol: ProtocolRegistry,
    refresh_lock: std::sync::Arc<Mutex<()>>,
    secrets_remote_url: String,
    notifier: broadcast::Sender<AuthSnapshot>,
    state: std::sync::Arc<Mutex<AuthRuntimeState>>,
}

#[derive(Debug, Clone, Deserialize)]
struct AccessTokenResponse {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "accessTokenExpirationTimestampMs")]
    access_token_expiration_timestamp_ms: i64,
    #[serde(rename = "clientId")]
    client_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ClientTokenResponse {
    granted_token: Option<GrantedToken>,
    token: Option<String>,
    expires_after_seconds: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
struct GrantedToken {
    token: String,
    expires_after_seconds: i64,
}

type SecretDict = HashMap<String, Vec<u8>>;
type CookieMap = HashMap<String, String>;

impl AuthService {
    pub async fn new(options: AuthServiceOptions) -> Result<Self> {
        let device_id = options.device_store.get_or_create_device_id().await?;
        let profile_state = options.cookie_store.list_profiles().await?;
        let has_cookie = options.cookie_store.active_cookie_text().await?.is_some();

        let (notifier, _) = broadcast::channel(64);
        let service = Self {
            client: options.client,
            cookie_store: options.cookie_store,
            device_store: options.device_store,
            open_spotify_head_url: options
                .open_spotify_head_url
                .unwrap_or_else(|| DEFAULT_OPEN_SPOTIFY_HEAD_URL.to_string()),
            protocol: options.protocol,
            refresh_lock: std::sync::Arc::new(Mutex::new(())),
            secrets_remote_url: options
                .secrets_remote_url
                .unwrap_or_else(|| DEFAULT_SECRETS_REMOTE_URL.to_string()),
            notifier,
            state: std::sync::Arc::new(Mutex::new(AuthRuntimeState {
                access_token: None,
                client_token: None,
                consecutive_failures: 0,
                snapshot: AuthSnapshot {
                    device_id,
                    client_id: None,
                    access_token_expires_at: None,
                    client_token_expires_at: None,
                    active_profile_id: profile_state.active_profile_id,
                    has_cookie,
                    profiles: profile_state.profiles,
                    status: "idle".into(),
                    error: None,
                },
            })),
        };

        if has_cookie {
            let _ = service.refresh().await;
        }

        Ok(service)
    }

    pub async fn get_snapshot(&self) -> AuthSnapshot {
        self.state.lock().await.snapshot.clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AuthSnapshot> {
        self.notifier.subscribe()
    }

    fn emit_snapshot(&self, snapshot: &AuthSnapshot) {
        let _ = self.notifier.send(snapshot.clone());
    }

    pub async fn import_cookie_file(&self, path: &str) -> Result<AuthSnapshot> {
        let label = Path::new(path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.trim().is_empty())
            .map(str::to_owned);
        let cookie = tokio::fs::read_to_string(path).await?;
        self.import_cookie_text_with_label(cookie.as_str(), label.as_deref())
            .await
    }

    pub async fn import_cookie_string(&self, cookie: &str) -> Result<AuthSnapshot> {
        self.import_cookie_text_with_label(cookie, None).await
    }

    pub async fn refresh(&self) -> Result<AuthSnapshot> {
        self.refresh_tokens(true).await
    }

    pub async fn clear_cookie(&self) -> Result<()> {
        let profile_state = self.cookie_store.clear_cookie().await?;
        self.reset_to_idle(profile_state).await?;
        Ok(())
    }

    pub async fn switch_profile(&self, profile_id: &str) -> Result<AuthSnapshot> {
        let profile_state = self.cookie_store.switch_active_profile(profile_id).await?;
        self.apply_profile_state(profile_state).await?;
        self.refresh_tokens(true).await
    }

    pub async fn active_cookie_text(&self) -> Result<Option<String>> {
        self.cookie_store.active_cookie_text().await
    }

    async fn import_cookie_text_with_label(
        &self,
        cookie: &str,
        label: Option<&str>,
    ) -> Result<AuthSnapshot> {
        let profile_state = self.cookie_store.import_cookie_text(label, cookie).await?;
        self.apply_profile_state(profile_state).await?;
        self.refresh_tokens(true).await
    }

    async fn refresh_tokens(&self, force: bool) -> Result<AuthSnapshot> {
        let active_cookie = self.cookie_store.active_cookie_text().await?;
        if active_cookie.is_none() {
            let profile_state = self.cookie_store.list_profiles().await?;
            return self.reset_to_idle(profile_state).await;
        }

        let _refresh_guard = self.refresh_lock.lock().await;
        if !force && self.tokens_are_fresh().await {
            return Ok(self.get_snapshot().await);
        }

        self.set_refreshing().await?;
        let profile_state = self.cookie_store.list_profiles().await?;

        match self.execute_refresh(active_cookie.unwrap()).await {
            Ok((access_token, access_expiry, client_id, client_token, client_expiry)) => {
                let snapshot = AuthSnapshot {
                    device_id: self.device_id().await,
                    client_id: Some(client_id),
                    access_token_expires_at: Some(access_expiry),
                    client_token_expires_at: Some(client_expiry),
                    active_profile_id: profile_state.active_profile_id.clone(),
                    has_cookie: true,
                    profiles: profile_state.profiles.clone(),
                    status: "ready".into(),
                    error: None,
                };
                let mut state = self.state.lock().await;
                state.access_token = Some(access_token);
                state.client_token = Some(client_token);
                state.consecutive_failures = 0;
                state.snapshot = snapshot.clone();
                drop(state);
                self.emit_snapshot(&snapshot);
                Ok(snapshot)
            }
            Err(error) => {
                let mut state = self.state.lock().await;
                state.access_token = None;
                state.client_token = None;
                state.consecutive_failures += 1;
                let message = if state.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    "Cookie expired".to_string()
                } else {
                    error.to_string()
                };
                let snapshot = AuthSnapshot {
                    device_id: state.snapshot.device_id.clone(),
                    client_id: None,
                    access_token_expires_at: None,
                    client_token_expires_at: None,
                    active_profile_id: profile_state.active_profile_id,
                    has_cookie: true,
                    profiles: profile_state.profiles,
                    status: "error".into(),
                    error: Some(message.clone()),
                };
                state.snapshot = snapshot.clone();
                drop(state);
                self.emit_snapshot(&snapshot);
                Err(DaemonError::Auth(message))
            }
        }
    }

    async fn execute_refresh(
        &self,
        active_cookie: String,
    ) -> Result<(String, i64, String, String, i64)> {
        let cookies = parse_cookie_file(active_cookie.as_str());
        if !cookies.contains_key("sp_dc") {
            return Err(DaemonError::Auth("sp_dc cookie not found".into()));
        }

        let access = self.request_access_token(&cookies).await?;
        let client = self.request_client_token(access.client_id.as_str()).await?;
        Ok((
            access.access_token,
            access.access_token_expiration_timestamp_ms,
            access.client_id,
            client.token,
            client.expires_at,
        ))
    }

    async fn request_access_token(&self, cookies: &CookieMap) -> Result<AccessTokenResponse> {
        let (otp, version) = self.generate_totp().await?;
        let response = self
            .client
            .get(&self.protocol.constants.open_spotify_token_url)
            .query(&[
                ("reason", "init"),
                ("productType", "web-player"),
                ("totp", otp.as_str()),
                ("totpServer", otp.as_str()),
                ("totpVer", version.as_str()),
            ])
            .header("accept", "application/json")
            .header("referer", "https://open.spotify.com/")
            .header("app-platform", &self.protocol.constants.app_platform)
            .header("user-agent", USER_AGENT)
            .header("cookie", build_cookie_header(cookies).as_str())
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(DaemonError::HttpStatus {
                status: response.status().as_u16(),
                method: "GET".into(),
                url: self.protocol.constants.open_spotify_token_url.clone(),
                response_text: response.text().await.unwrap_or_default(),
            });
        }

        Ok(response.json().await?)
    }

    async fn request_client_token(&self, client_id: &str) -> Result<ClientTokenGrant> {
        let payload = serde_json::json!({
            "client_data": {
                "client_id": client_id,
                "client_version": self.protocol.constants.app_version,
                "js_sdk_data": {
                    "device_brand": "unknown",
                    "device_model": "unknown",
                    "device_id": self.device_id().await,
                    "device_type": "computer",
                    "os": std::env::consts::OS,
                    "os_version": os_info(),
                }
            }
        });

        let response = self
            .client
            .post(&self.protocol.constants.client_token_url)
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(DaemonError::HttpStatus {
                status: response.status().as_u16(),
                method: "POST".into(),
                url: self.protocol.constants.client_token_url.clone(),
                response_text: response.text().await.unwrap_or_default(),
            });
        }

        let payload: ClientTokenResponse = response.json().await?;
        let token = payload
            .granted_token
            .as_ref()
            .map(|token| token.token.clone())
            .or(payload.token)
            .ok_or_else(|| DaemonError::InvalidResponse("missing client token".into()))?;
        let ttl = payload
            .granted_token
            .as_ref()
            .map(|token| token.expires_after_seconds)
            .or(payload.expires_after_seconds)
            .ok_or_else(|| DaemonError::InvalidResponse("missing client token ttl".into()))?;
        Ok(ClientTokenGrant {
            token,
            expires_at: now_millis() + ttl * 1_000,
        })
    }

    async fn apply_profile_state(&self, profile_state: CookieProfileState) -> Result<AuthSnapshot> {
        let mut state = self.state.lock().await;
        state.snapshot.active_profile_id = profile_state.active_profile_id;
        state.snapshot.profiles = profile_state.profiles;
        state.snapshot.has_cookie = self.cookie_store.active_cookie_text().await?.is_some();
        let snapshot = state.snapshot.clone();
        drop(state);
        self.emit_snapshot(&snapshot);
        Ok(snapshot)
    }

    async fn device_id(&self) -> String {
        self.state.lock().await.snapshot.device_id.clone()
    }

    async fn generate_totp(&self) -> Result<(String, String)> {
        let secrets = self.load_secrets().await?;
        let server_time = self.fetch_server_time().await;
        let version = latest_version(&secrets)
            .ok_or_else(|| DaemonError::InvalidResponse("no secrets available".into()))?;
        let secret = derive_secret(secrets.get(&version).expect("version just resolved"));
        Ok((compute_totp(secret.as_str(), server_time)?, version))
    }

    async fn fetch_server_time(&self) -> i64 {
        let response = self
            .client
            .head(&self.open_spotify_head_url)
            .timeout(Duration::from_secs(5))
            .send()
            .await;
        if let Ok(response) = response {
            if let Some(value) = response.headers().get(DATE) {
                if let Ok(date) = value.to_str() {
                    if let Ok(parsed) = chrono::DateTime::parse_from_rfc2822(date) {
                        return parsed.timestamp();
                    }
                }
            }
        }
        chrono::Utc::now().timestamp()
    }

    async fn load_secrets(&self) -> Result<SecretDict> {
        if let Some(cached) = CACHED_SECRETS
            .lock()
            .map_err(|_| DaemonError::Poisoned("auth secrets cache".into()))?
            .clone()
        {
            return Ok(cached);
        }

        let fetched = self
            .client
            .get(&self.secrets_remote_url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .ok()
            .filter(|response| response.status().is_success());

        let secrets = if let Some(response) = fetched {
            response
                .json::<SecretDict>()
                .await
                .unwrap_or_else(|_| embedded_secrets())
        } else {
            embedded_secrets()
        };

        *CACHED_SECRETS
            .lock()
            .map_err(|_| DaemonError::Poisoned("auth secrets cache".into()))? =
            Some(secrets.clone());
        Ok(secrets)
    }

    async fn reset_to_idle(&self, profile_state: CookieProfileState) -> Result<AuthSnapshot> {
        let device_id = self.device_store.get_or_create_device_id().await?;
        let mut state = self.state.lock().await;
        state.access_token = None;
        state.client_token = None;
        state.consecutive_failures = 0;
        let snapshot = AuthSnapshot {
            device_id,
            client_id: None,
            access_token_expires_at: None,
            client_token_expires_at: None,
            active_profile_id: profile_state.active_profile_id,
            has_cookie: false,
            profiles: profile_state.profiles,
            status: "idle".into(),
            error: None,
        };
        state.snapshot = snapshot.clone();
        drop(state);
        self.emit_snapshot(&snapshot);
        Ok(snapshot)
    }

    async fn set_refreshing(&self) -> Result<()> {
        let profile_state = self.cookie_store.list_profiles().await?;
        let mut state = self.state.lock().await;
        let snapshot = AuthSnapshot {
            device_id: state.snapshot.device_id.clone(),
            client_id: state.snapshot.client_id.clone(),
            access_token_expires_at: state.snapshot.access_token_expires_at,
            client_token_expires_at: state.snapshot.client_token_expires_at,
            active_profile_id: profile_state.active_profile_id,
            has_cookie: true,
            profiles: profile_state.profiles,
            status: "refreshing".into(),
            error: None,
        };
        state.snapshot = snapshot.clone();
        drop(state);
        self.emit_snapshot(&snapshot);
        Ok(())
    }

    async fn tokens_are_fresh(&self) -> bool {
        let state = self.state.lock().await;
        let now = now_millis();
        state
            .access_token
            .as_ref()
            .zip(state.snapshot.access_token_expires_at)
            .map(|(_, expiry)| expiry - now > ACCESS_TOKEN_MARGIN_MS)
            .unwrap_or(false)
            && state
                .client_token
                .as_ref()
                .zip(state.snapshot.client_token_expires_at)
                .map(|(_, expiry)| expiry - now > CLIENT_TOKEN_MARGIN_MS)
                .unwrap_or(false)
    }
}

#[async_trait]
impl AuthProvider for AuthService {
    async fn get_access_token(&self) -> Result<String> {
        if !self.tokens_are_fresh().await {
            self.refresh_tokens(false).await?;
        }
        self.state
            .lock()
            .await
            .access_token
            .clone()
            .ok_or_else(|| DaemonError::Auth("access token unavailable".into()))
    }

    async fn get_client_token(&self) -> Result<String> {
        if !self.tokens_are_fresh().await {
            self.refresh_tokens(false).await?;
        }
        self.state
            .lock()
            .await
            .client_token
            .clone()
            .ok_or_else(|| DaemonError::Auth("client token unavailable".into()))
    }

    async fn force_refresh(&self) -> Result<AuthSnapshot> {
        self.refresh().await
    }
}

#[derive(Debug, Clone)]
struct ClientTokenGrant {
    token: String,
    expires_at: i64,
}

pub fn parse_cookie_file(raw: &str) -> CookieMap {
    let mut result = CookieMap::new();
    let trimmed = raw.trim();

    for part in trimmed.split(';') {
        if let Some((key, value)) = part.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            if !key.is_empty() {
                result.insert(key.to_string(), value.to_string());
            }
        }
    }

    if result.len() <= 1 {
        for line in trimmed.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if line.contains('\t') {
                let columns: Vec<_> = line.split('\t').collect();
                if columns.len() >= 7 {
                    result.insert(columns[5].to_string(), columns[6].to_string());
                    continue;
                }
            }

            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim();
                if !key.is_empty() {
                    result.insert(key.to_string(), value.to_string());
                }
            }
        }
    }

    result
}

fn build_cookie_header(cookies: &CookieMap) -> String {
    ["sp_dc", "sp_key"]
        .into_iter()
        .filter_map(|key| cookies.get(key).map(|value| format!("{key}={value}")))
        .collect::<Vec<_>>()
        .join("; ")
}

fn embedded_secrets() -> SecretDict {
    HashMap::from([
        (
            "59".into(),
            vec![
                123, 105, 79, 70, 110, 59, 52, 125, 60, 49, 80, 70, 89, 75, 80, 86, 63, 53, 123,
                37, 117, 49, 52, 93, 77, 62, 47, 86, 48, 104, 68, 72,
            ],
        ),
        (
            "60".into(),
            vec![
                79, 109, 69, 123, 90, 65, 46, 74, 94, 34, 58, 48, 70, 71, 92, 85, 122, 63, 91, 64,
                87, 87,
            ],
        ),
        (
            "61".into(),
            vec![
                44, 55, 47, 42, 70, 40, 34, 114, 76, 74, 50, 111, 120, 97, 75, 76, 94, 102, 43, 69,
                49, 120, 118, 80, 64, 78,
            ],
        ),
    ])
}

fn latest_version(dict: &SecretDict) -> Option<String> {
    dict.keys()
        .max_by_key(|key| key.parse::<u32>().unwrap_or_default())
        .cloned()
}

const BASE32_CHARS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

fn base32_encode(buffer: &[u8]) -> String {
    let mut bits = String::new();
    for byte in buffer {
        bits.push_str(format!("{byte:08b}").as_str());
    }
    while bits.len() % 5 != 0 {
        bits.push('0');
    }

    (0..bits.len())
        .step_by(5)
        .map(|index| {
            let chunk = &bits[index..index + 5];
            let value = u8::from_str_radix(chunk, 2).unwrap_or_default() as usize;
            BASE32_CHARS.chars().nth(value).unwrap_or('A')
        })
        .collect()
}

fn base32_decode(value: &str) -> Vec<u8> {
    let mut bits = String::new();
    for character in value.chars() {
        if let Some(index) = BASE32_CHARS.find(character) {
            bits.push_str(format!("{index:05b}").as_str());
        }
    }

    let mut bytes = Vec::new();
    for index in (0..bits.len()).step_by(8) {
        if index + 8 <= bits.len() {
            bytes.push(u8::from_str_radix(&bits[index..index + 8], 2).unwrap_or_default());
        }
    }
    bytes
}

fn derive_secret(cipher_bytes: &[u8]) -> String {
    let transformed: Vec<u8> = cipher_bytes
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ (((index % 33) + 9) as u8))
        .collect();
    let joined = transformed
        .iter()
        .map(|byte| byte.to_string())
        .collect::<Vec<_>>()
        .join("");
    let encoded = STANDARD.encode(joined.as_bytes());
    let decoded = STANDARD.decode(encoded).unwrap_or_default();
    base32_encode(&decoded)
}

fn compute_totp(secret: &str, server_time: i64) -> Result<String> {
    let counter = server_time / 30;
    let mut counter_bytes = [0u8; 8];
    counter_bytes[..8].copy_from_slice(&(counter as u64).to_be_bytes());

    let key = base32_decode(secret);
    let mut hmac = Hmac::<Sha1>::new_from_slice(&key)
        .map_err(|error| DaemonError::Auth(format!("failed to initialize TOTP HMAC: {error}")))?;
    hmac.update(&counter_bytes);
    let digest = hmac.finalize().into_bytes();
    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let code = (((digest[offset] & 0x7f) as u32) << 24
        | (digest[offset + 1] as u32) << 16
        | (digest[offset + 2] as u32) << 8
        | digest[offset + 3] as u32)
        % 1_000_000;
    Ok(format!("{code:06}"))
}

fn os_info() -> String {
    std::env::consts::OS.to_string()
}

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
