use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};
use reqwest::{Method, Url};
use serde::Deserialize;
use serde_json::{json, Value};
use sha1::{Digest as _, Sha1};
use tokio::{sync::RwLock, task::JoinHandle, time::timeout};
use tokio_tungstenite::{
    connect_async, tungstenite::protocol::Message as WebSocketMessage, MaybeTlsStream,
    WebSocketStream,
};

use crate::{
    error::{DaemonError, Result},
    types::{
        playback::{
            PLAYBACK_SOURCE_CONNECT_STATE, PLAYBACK_SOURCE_DEALER, PLAYBACK_SOURCE_WEB_API,
            PLAYBACK_SOURCE_WEB_API_CACHE,
        },
        Artist, ImageResource, PlaybackState, TrackInfo,
    },
    util::spotify::{extract_track_id, hex_to_base62, is_hex_track_id, to_hex_track_id},
};

use super::{
    discovery::ProtocolRegistry,
    transport::{ResponseType, SpotifyTransport, TransportBody, TransportRequest},
};

const PLAYER_API_BASE: &str = "https://api.spotify.com/v1/me/player";
const CURRENTLY_PLAYING_API: &str = "https://api.spotify.com/v1/me/player/currently-playing";
const WEB_PLAYER_FALLBACK_INTERVAL: Duration = Duration::from_millis(500);
const MAX_CACHED_PLAYING_EXTRAPOLATION: Duration = Duration::from_secs(3);
const MAX_FRESH_TIMESTAMP_AGE: Duration = Duration::from_secs(3);
const WEB_PLAYER_RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(60);
const CONNECT_STATE_ERROR_BACKOFF: Duration = Duration::from_secs(60);
const DEALER_CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);
const DEALER_PING_INTERVAL: Duration = Duration::from_secs(30);
const DEALER_SNAPSHOT_TTL: Duration = Duration::from_secs(5);
const SPOTIFY_WEB_CLIENT_VERSION: &str = "harmony:4.72.0-a9118221e";
const TRACK_PLAYBACK_DEVICE_NAME: &str = "Spot-Lyric";
const TRACK_PLAYBACK_PLATFORM_IDENTIFIER: &str =
    "web_player linux undefined;chrome 147.0.0.0;desktop";
const TRACK_PLAYBACK_STATE_DEBUG_SOURCE: &str = "video_visibility_changed";

type DealerWebSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug, Clone, PartialEq)]
pub struct ConnectPlaybackSnapshot {
    pub state: PlaybackState,
    pub track: Option<TrackInfo>,
    pub active_device_id: Option<String>,
}

#[derive(Debug, Clone)]
struct CachedPlaybackSnapshot {
    snapshot: ConnectPlaybackSnapshot,
    observed_at: Instant,
}

#[derive(Debug)]
struct PlaybackFallbackState {
    connect_state_next_allowed: Instant,
    web_player_next_allowed: Instant,
    last_web_player_snapshot: Option<CachedPlaybackSnapshot>,
}

#[derive(Debug, Deserialize)]
struct DealerResolveResponse {
    #[serde(rename = "dealer-g2")]
    dealer_g2: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct TrackPlaybackRegistrationResponse {
    initial_seq_num: u64,
}

impl Default for PlaybackFallbackState {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            connect_state_next_allowed: now,
            web_player_next_allowed: now,
            last_web_player_snapshot: None,
        }
    }
}

impl PlaybackFallbackState {
    fn cached_web_player_snapshot(&self, now: Instant) -> Option<ConnectPlaybackSnapshot> {
        let cached = self.last_web_player_snapshot.as_ref()?;
        let mut snapshot = cached.snapshot.clone();
        snapshot.state.playback_source = PLAYBACK_SOURCE_WEB_API_CACHE.into();
        if snapshot.state.is_playing {
            let elapsed = now.saturating_duration_since(cached.observed_at);
            let elapsed_ms = elapsed
                .min(MAX_CACHED_PLAYING_EXTRAPOLATION)
                .as_millis()
                .min(i64::MAX as u128) as i64;
            snapshot.state.position_ms = snapshot.state.position_ms.saturating_add(elapsed_ms);
            if snapshot.state.duration_ms > 0 {
                snapshot.state.position_ms =
                    snapshot.state.position_ms.min(snapshot.state.duration_ms);
            }
            if elapsed > MAX_CACHED_PLAYING_EXTRAPOLATION {
                snapshot.state.is_playing = false;
            }
        }
        Some(snapshot)
    }
}

#[derive(Debug)]
struct ConnectSession {
    _dealer_task: JoinHandle<()>,
    connection_id: String,
    observer_device_id: String,
}

impl Drop for ConnectSession {
    fn drop(&mut self) {
        self._dealer_task.abort();
    }
}

#[derive(Clone)]
pub struct ConnectStateClient {
    device_id: String,
    protocol: ProtocolRegistry,
    transport: SpotifyTransport,
    dealer_snapshot: Arc<RwLock<Option<CachedPlaybackSnapshot>>>,
    fallback: Arc<tokio::sync::Mutex<PlaybackFallbackState>>,
    session: Arc<tokio::sync::Mutex<Option<ConnectSession>>>,
}

impl ConnectStateClient {
    pub fn new(transport: SpotifyTransport, protocol: ProtocolRegistry, device_id: String) -> Self {
        Self {
            device_id,
            protocol,
            transport,
            dealer_snapshot: Arc::new(RwLock::new(None)),
            fallback: Arc::new(tokio::sync::Mutex::new(PlaybackFallbackState::default())),
            session: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    pub async fn fetch_state(&self) -> Result<Option<ConnectPlaybackSnapshot>> {
        if self.should_skip_connect_state().await {
            return self.fetch_web_player_state().await;
        }

        match self.fetch_connect_state().await {
            Ok(Some(snapshot)) => Ok(Some(snapshot)),
            Ok(None) => self.fetch_web_player_state().await,
            Err(error) => {
                self.record_connect_state_error(&error).await;
                tracing::debug!(%error, "connect-state unavailable; falling back to Spotify Web API player state");
                self.fetch_web_player_state().await
            }
        }
    }

    pub async fn fetch_source(&self, source: &str) -> Result<Option<ConnectPlaybackSnapshot>> {
        match source {
            PLAYBACK_SOURCE_DEALER => self.fetch_dealer_state().await,
            PLAYBACK_SOURCE_CONNECT_STATE => self.fetch_connect_state().await,
            PLAYBACK_SOURCE_WEB_API => self.fetch_web_player_state().await,
            _ => Ok(None),
        }
    }

    async fn fetch_dealer_state(&self) -> Result<Option<ConnectPlaybackSnapshot>> {
        self.ensure_session().await?;
        let now = Instant::now();
        let snapshot = self.dealer_snapshot.read().await.clone();
        let Some(cached) = snapshot else {
            return Ok(None);
        };
        if now.saturating_duration_since(cached.observed_at) > DEALER_SNAPSHOT_TTL {
            return Ok(None);
        }
        let mut snapshot = cached.snapshot;
        snapshot.state.playback_source = PLAYBACK_SOURCE_DEALER.into();
        Ok(Some(snapshot))
    }

    async fn fetch_connect_state(&self) -> Result<Option<ConnectPlaybackSnapshot>> {
        self.ensure_session().await?;

        let (observer_device_id, connection_id) = {
            let session = self.session.lock().await;
            let session = session
                .as_ref()
                .expect("session established before fetching connect-state");
            (
                session.observer_device_id.clone(),
                session.connection_id.clone(),
            )
        };

        let snapshot = self
            .fetch_connect_state_snapshot(&observer_device_id, &connection_id)
            .await;
        if snapshot.is_err() {
            self.session.lock().await.take();
        }
        snapshot
    }

    async fn should_skip_connect_state(&self) -> bool {
        Instant::now() < self.fallback.lock().await.connect_state_next_allowed
    }

    async fn record_connect_state_error(&self, error: &DaemonError) {
        if matches!(error, DaemonError::HttpStatus { status: 400, .. }) {
            self.fallback.lock().await.connect_state_next_allowed =
                Instant::now() + CONNECT_STATE_ERROR_BACKOFF;
        }
    }

    async fn ensure_session(&self) -> Result<()> {
        {
            let session = self.session.lock().await;
            if session.is_some() {
                return Ok(());
            }
        }

        let established = self.establish_session().await?;
        let mut session = self.session.lock().await;
        if session.is_none() {
            *session = Some(established);
        }

        Ok(())
    }

    async fn establish_session(&self) -> Result<ConnectSession> {
        let playback_device_id = playback_device_id_from_seed(&self.device_id);
        let observer_device_id = observer_device_id(&playback_device_id);
        let (dealer_socket, connection_id) = self.connect_dealer().await?;
        let registration = self
            .register_track_playback_device(&playback_device_id, &connection_id)
            .await?;
        let state_seq_num = registration.initial_seq_num.saturating_add(1);
        self.push_track_playback_state(&playback_device_id, state_seq_num)
            .await?;
        let dealer_task = spawn_dealer_reader(dealer_socket, self.dealer_snapshot.clone());

        Ok(ConnectSession {
            _dealer_task: dealer_task,
            connection_id,
            observer_device_id,
        })
    }

    async fn connect_dealer(&self) -> Result<(DealerWebSocket, String)> {
        let dealer_url = self.resolve_dealer_url().await?;
        let access_token = self.transport.access_token().await?;
        let mut url = Url::parse(&dealer_url).map_err(|error| {
            DaemonError::InvalidArgument(format!("invalid dealer url: {error}"))
        })?;
        url.query_pairs_mut()
            .append_pair("access_token", &access_token);

        let (mut socket, _) = connect_async(url.as_str()).await.map_err(|error| {
            DaemonError::InvalidResponse(format!("dealer websocket connect failed: {error}"))
        })?;

        let connection_id = timeout(DEALER_CONNECTION_TIMEOUT, async {
            loop {
                let Some(frame) = socket.next().await else {
                    return Err(DaemonError::InvalidResponse(
                        "dealer websocket closed before providing a connection id".into(),
                    ));
                };
                let frame = frame.map_err(|error| {
                    DaemonError::InvalidResponse(format!("dealer websocket frame error: {error}"))
                })?;
                if let Some(connection_id) = dealer_connection_id_from_message(frame)? {
                    return Ok(connection_id);
                }
            }
        })
        .await
        .map_err(|_| {
            DaemonError::InvalidResponse("timed out waiting for dealer connection id".into())
        })??;

        Ok((socket, connection_id))
    }

    async fn resolve_dealer_url(&self) -> Result<String> {
        let request = TransportRequest {
            url: Url::parse(&self.protocol.constants.apresolve_url).map_err(|error| {
                DaemonError::InvalidArgument(format!("invalid apresolve url: {error}"))
            })?,
            method: Method::GET,
            headers: Default::default(),
            body: None,
            response_type: ResponseType::Json,
            with_auth: false,
        };
        let resolved: DealerResolveResponse = self.transport.request_json(request).await?;
        let host = resolved
            .dealer_g2
            .and_then(|hosts| hosts.into_iter().next())
            .ok_or_else(|| DaemonError::InvalidResponse("dealer-g2 not resolved".into()))?;
        Ok(format!(
            "wss://{}",
            host.trim_end_matches('/').trim_end_matches(":443")
        ))
    }

    async fn register_track_playback_device(
        &self,
        playback_device_id: &str,
        connection_id: &str,
    ) -> Result<TrackPlaybackRegistrationResponse> {
        let url = self
            .protocol
            .build_spclient_url(self.protocol.constants.track_playback_devices_path.as_str())?;
        self.transport
            .post_json(
                url,
                track_playback_registration_body(
                    playback_device_id,
                    connection_id,
                    &uuid::Uuid::new_v4().to_string(),
                ),
                None,
            )
            .await
    }

    async fn push_track_playback_state(
        &self,
        playback_device_id: &str,
        state_seq_num: u64,
    ) -> Result<()> {
        let url = self.protocol.build_spclient_url(
            format!(
                "{}/{}/state",
                self.protocol.constants.track_playback_devices_path, playback_device_id
            )
            .as_str(),
        )?;
        let request = TransportRequest {
            url: Url::parse(&url).map_err(|error| {
                DaemonError::InvalidArgument(format!("invalid track state url: {error}"))
            })?,
            method: Method::PUT,
            headers: Default::default(),
            body: Some(TransportBody::Json(track_playback_state_body(
                state_seq_num,
            ))),
            response_type: ResponseType::Text,
            with_auth: true,
        };
        let _ = self.transport.request_text(request).await?;
        Ok(())
    }

    async fn fetch_connect_state_snapshot(
        &self,
        observer_device_id: &str,
        connection_id: &str,
    ) -> Result<Option<ConnectPlaybackSnapshot>> {
        let url = self.protocol.build_spclient_url(
            format!(
                "{}/{}",
                self.protocol.constants.connect_state_path, observer_device_id
            )
            .as_str(),
        )?;
        let mut headers = HashMap::new();
        headers.insert(
            "X-Spotify-Connection-Id".to_string(),
            connection_id.to_string(),
        );
        let raw: Value = self
            .transport
            .put_json(url, connect_state_observer_body(), Some(headers))
            .await?;
        Ok(map_connect_state_response(&raw))
    }

    async fn fetch_web_player_state(&self) -> Result<Option<ConnectPlaybackSnapshot>> {
        let now = Instant::now();
        {
            let state = self.fallback.lock().await;
            if now < state.web_player_next_allowed {
                return Ok(state.cached_web_player_snapshot(now));
            }
        }

        let result = self.fetch_web_player_state_now().await;
        let mut state = self.fallback.lock().await;
        let observed_at = Instant::now();

        match result {
            Ok(snapshot) => {
                state.web_player_next_allowed = observed_at + WEB_PLAYER_FALLBACK_INTERVAL;
                state.last_web_player_snapshot =
                    snapshot.clone().map(|snapshot| CachedPlaybackSnapshot {
                        snapshot,
                        observed_at,
                    });
                Ok(snapshot)
            }
            Err(error) => {
                state.web_player_next_allowed = observed_at
                    + if is_rate_limited(&error) {
                        WEB_PLAYER_RATE_LIMIT_BACKOFF
                    } else {
                        WEB_PLAYER_FALLBACK_INTERVAL
                    };

                if let Some(snapshot) = state.cached_web_player_snapshot(observed_at) {
                    tracing::debug!(
                        %error,
                        "using cached Spotify Web API player state after fallback failure"
                    );
                    Ok(Some(snapshot))
                } else {
                    Err(error)
                }
            }
        }
    }

    async fn fetch_web_player_state_now(&self) -> Result<Option<ConnectPlaybackSnapshot>> {
        let mut url = Url::parse(CURRENTLY_PLAYING_API).map_err(|error| {
            DaemonError::InvalidArgument(format!("invalid player url: {error}"))
        })?;
        url.query_pairs_mut()
            .append_pair("additional_types", "track,episode");

        let request = TransportRequest {
            url,
            method: Method::GET,
            headers: Default::default(),
            body: None,
            response_type: ResponseType::Json,
            with_auth: true,
        };
        let raw = self
            .transport
            .request_optional_json::<Value>(request)
            .await?;
        Ok(raw.as_ref().and_then(map_web_player_response))
    }

    pub async fn play(&self, active_device_id: Option<&str>) -> Result<()> {
        self.player_command(Method::PUT, "play", active_device_id)
            .await
    }

    pub async fn pause(&self, active_device_id: Option<&str>) -> Result<()> {
        self.player_command(Method::PUT, "pause", active_device_id)
            .await
    }

    pub async fn skip_next(&self, active_device_id: Option<&str>) -> Result<()> {
        self.player_command(Method::POST, "next", active_device_id)
            .await
    }

    pub async fn skip_previous(&self, active_device_id: Option<&str>) -> Result<()> {
        self.player_command(Method::POST, "previous", active_device_id)
            .await
    }

    async fn player_command(
        &self,
        method: Method,
        command: &str,
        active_device_id: Option<&str>,
    ) -> Result<()> {
        let mut url = Url::parse(&format!("{PLAYER_API_BASE}/{command}")).map_err(|error| {
            DaemonError::InvalidArgument(format!("invalid player url: {error}"))
        })?;
        if let Some(device_id) = active_device_id.filter(|value| !value.trim().is_empty()) {
            url.query_pairs_mut().append_pair("device_id", device_id);
        }

        let request = TransportRequest {
            url,
            method,
            headers: Default::default(),
            body: Some(TransportBody::Json(json!({}))),
            response_type: ResponseType::Text,
            with_auth: true,
        };
        let _ = self.transport.request_text(request).await?;
        Ok(())
    }
}

fn playback_device_id_from_seed(seed: &str) -> String {
    format!("{:x}", Sha1::digest(seed.as_bytes()))
}

fn observer_device_id(playback_device_id: &str) -> String {
    format!("hobs_{playback_device_id}")
        .chars()
        .take(40)
        .collect()
}

fn track_playback_registration_body(
    playback_device_id: &str,
    connection_id: &str,
    correlation_id: &str,
) -> Value {
    json!({
        "device": {
            "brand": "spotify",
            "capabilities": {
                "change_volume": true,
                "enable_play_token": true,
                "supports_file_media_type": true,
                "play_token_lost_behavior": "pause",
                "disable_connect": false,
                "audio_podcasts": true,
                "video_playback": true,
                "manifest_formats": [
                    "file_ids_mp3",
                    "file_urls_mp3",
                    "manifest_urls_audio_ad",
                    "manifest_ids_video",
                    "file_urls_external",
                    "file_ids_mp4",
                    "file_ids_mp4_dual",
                    "manifest_urls_audio_ad"
                ],
                "supports_preferred_media_type": true,
                "supports_playback_offsets": true,
                "supports_playback_speed": true
            },
            "device_id": playback_device_id,
            "device_type": "computer",
            "metadata": {},
            "model": "web_player",
            "name": TRACK_PLAYBACK_DEVICE_NAME,
            "platform_identifier": TRACK_PLAYBACK_PLATFORM_IDENTIFIER,
            "is_group": false,
            "correlation_id": correlation_id,
            "client_version": SPOTIFY_WEB_CLIENT_VERSION
        },
        "outro_endcontent_snooping": false,
        "connection_id": connection_id,
        "client_version": SPOTIFY_WEB_CLIENT_VERSION,
        "volume": 65535
    })
}

fn track_playback_state_body(state_seq_num: u64) -> Value {
    json!({
        "seq_num": state_seq_num,
        "state_ref": null,
        "sub_state": {
            "playback_speed": 1,
            "format": "file_urls_external",
            "is_video_on": false
        },
        "debug_source": TRACK_PLAYBACK_STATE_DEBUG_SOURCE
    })
}

fn connect_state_observer_body() -> Value {
    json!({
        "member_type": "CONNECT_STATE",
        "device": {
            "device_info": {
                "capabilities": {
                    "can_be_player": false,
                    "hidden": true,
                    "needs_full_player_state": true
                }
            }
        }
    })
}

fn dealer_connection_id_from_message(message: WebSocketMessage) -> Result<Option<String>> {
    let text = match message {
        WebSocketMessage::Text(text) => Some(text.to_string()),
        WebSocketMessage::Binary(bytes) => {
            Some(String::from_utf8(bytes.to_vec()).map_err(|error| {
                DaemonError::InvalidResponse(format!(
                    "dealer websocket sent non-utf8 binary frame: {error}"
                ))
            })?)
        }
        _ => None,
    };

    let Some(text) = text else {
        return Ok(None);
    };
    let event: Value = serde_json::from_str(&text)?;
    Ok(dealer_connection_id_from_event(&event))
}

fn dealer_connection_id_from_event(event: &Value) -> Option<String> {
    event
        .pointer("/headers/Spotify-Connection-Id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            event
                .get("uri")
                .and_then(Value::as_str)
                .and_then(|uri| uri.split("/connections/").nth(1))
                .map(str::to_owned)
        })
}

fn spawn_dealer_reader(
    mut socket: DealerWebSocket,
    dealer_snapshot: Arc<RwLock<Option<CachedPlaybackSnapshot>>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(DEALER_PING_INTERVAL);
        ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = ping_interval.tick() => {
                    if let Err(error) = socket.send(WebSocketMessage::Ping(Vec::new().into())).await {
                        tracing::debug!(%error, "dealer websocket ping failed");
                        break;
                    }
                }
                frame = socket.next() => {
                    let Some(frame) = frame else {
                        break;
                    };

                    match frame {
                        Ok(WebSocketMessage::Ping(payload)) => {
                            if let Err(error) = socket.send(WebSocketMessage::Pong(payload)).await {
                                tracing::debug!(%error, "dealer websocket pong failed");
                                break;
                            }
                        }
                        Ok(WebSocketMessage::Text(text)) => {
                            update_dealer_snapshot(text.as_ref(), &dealer_snapshot).await;
                        }
                        Ok(WebSocketMessage::Binary(bytes)) => {
                            match std::str::from_utf8(bytes.as_ref()) {
                                Ok(text) => update_dealer_snapshot(text, &dealer_snapshot).await,
                                Err(error) => tracing::debug!(%error, "dealer websocket sent non-utf8 binary frame"),
                            }
                        }
                        Ok(WebSocketMessage::Close(_)) => break,
                        Ok(_) => {}
                        Err(error) => {
                            tracing::debug!(%error, "dealer websocket read failed");
                            break;
                        }
                    }
                }
            }
        }
    })
}

async fn update_dealer_snapshot(
    text: &str,
    dealer_snapshot: &Arc<RwLock<Option<CachedPlaybackSnapshot>>>,
) {
    match dealer_snapshot_from_text(text) {
        Ok(Some(snapshot)) => {
            *dealer_snapshot.write().await = Some(CachedPlaybackSnapshot {
                snapshot,
                observed_at: Instant::now(),
            });
        }
        Ok(None) => {}
        Err(error) => tracing::debug!(%error, "failed to decode dealer connect-state push"),
    }
}

fn dealer_snapshot_from_text(text: &str) -> Result<Option<ConnectPlaybackSnapshot>> {
    let event: Value = serde_json::from_str(text)?;
    for candidate in dealer_payload_candidates(&event) {
        if let Some(snapshot) = map_connect_state_response(candidate) {
            return Ok(Some(snapshot));
        }
    }
    Ok(None)
}

fn dealer_payload_candidates<'a>(event: &'a Value) -> Vec<&'a Value> {
    let mut candidates = vec![event];

    if let Some(payloads) = event.get("payloads").and_then(Value::as_array) {
        for payload in payloads {
            candidates.push(payload);
            for path in [
                "/cluster",
                "/body",
                "/body/cluster",
                "/update",
                "/update/cluster",
                "/message",
                "/message/body",
                "/message/body/cluster",
            ] {
                if let Some(value) = payload.pointer(path) {
                    candidates.push(value);
                }
            }
        }
    }

    candidates
}

pub(crate) fn map_connect_state_response(raw: &Value) -> Option<ConnectPlaybackSnapshot> {
    let player = raw
        .get("player_state")
        .or_else(|| raw.pointer("/cluster/player_state"))
        .or_else(|| raw.pointer("/state/player_state"))?;
    let track_raw = player
        .get("track")
        .or_else(|| player.pointer("/context_track"));
    let mut track = track_raw.and_then(map_track);
    let uri = track
        .as_ref()
        .and_then(|track| track.uri.clone())
        .or_else(|| track_raw.and_then(extract_track_uri));

    if uri.as_deref().unwrap_or_default().is_empty() && track.is_none() {
        return None;
    }

    let duration_ms = first_duration_ms(player, &["duration", "duration_ms"])
        .or_else(|| track.as_ref().map(|track| track.duration_ms))
        .unwrap_or_default()
        .max(0);
    fill_track_duration_from_player(&mut track, duration_ms);
    let is_playing = player_is_playing(player);
    let position_ms = corrected_position_ms(player, duration_ms, is_playing);
    let volume = extract_volume(raw).unwrap_or(1.0).clamp(0.0, 1.0);

    let state = PlaybackState {
        is_playing,
        track_uri: uri.unwrap_or_default(),
        track_name: track
            .as_ref()
            .map(|track| track.name.clone())
            .unwrap_or_default(),
        artist_name: track
            .as_ref()
            .map(|track| {
                track
                    .artists
                    .iter()
                    .map(|artist| artist.name.as_str())
                    .filter(|name| !name.is_empty())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
        album_name: track
            .as_ref()
            .and_then(|track| track.album_name.clone())
            .unwrap_or_default(),
        album_art_url: track
            .as_ref()
            .and_then(|track| track.images.first().map(|image| image.url.clone()))
            .unwrap_or_default(),
        position_ms,
        duration_ms,
        volume,
        player_status: "ready".into(),
        playback_source: PLAYBACK_SOURCE_CONNECT_STATE.into(),
    };

    Some(ConnectPlaybackSnapshot {
        state,
        track,
        active_device_id: active_device_id(raw),
    })
}

pub(crate) fn map_web_player_response(raw: &Value) -> Option<ConnectPlaybackSnapshot> {
    let item = raw.get("item").filter(|value| !value.is_null())?;
    let mut track = map_track(item);
    let uri = track
        .as_ref()
        .and_then(|track| track.uri.clone())
        .or_else(|| extract_track_uri(item));

    if uri.as_deref().unwrap_or_default().is_empty() && track.is_none() {
        return None;
    }

    let duration_ms = track
        .as_ref()
        .map(|track| track.duration_ms)
        .or_else(|| first_duration_ms(item, &["duration_ms", "duration"]))
        .unwrap_or_default()
        .max(0);
    fill_track_duration_from_player(&mut track, duration_ms);
    let is_playing = player_is_playing(raw);
    let position_ms = corrected_web_player_position_ms(raw, duration_ms, is_playing);
    let volume = raw
        .pointer("/device")
        .and_then(|device| first_f64(device, &["volume", "volume_percent"]))
        .map(|volume| if volume > 1.0 { volume / 100.0 } else { volume })
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);

    let state = PlaybackState {
        is_playing,
        track_uri: uri.unwrap_or_default(),
        track_name: track
            .as_ref()
            .map(|track| track.name.clone())
            .unwrap_or_default(),
        artist_name: track
            .as_ref()
            .map(|track| {
                track
                    .artists
                    .iter()
                    .map(|artist| artist.name.as_str())
                    .filter(|name| !name.is_empty())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
        album_name: track
            .as_ref()
            .and_then(|track| track.album_name.clone())
            .unwrap_or_default(),
        album_art_url: track
            .as_ref()
            .and_then(|track| track.images.first().map(|image| image.url.clone()))
            .unwrap_or_default(),
        position_ms,
        duration_ms,
        volume,
        player_status: "ready".into(),
        playback_source: PLAYBACK_SOURCE_WEB_API.into(),
    };

    Some(ConnectPlaybackSnapshot {
        state,
        track,
        active_device_id: string_at(raw, &["device", "id"]),
    })
}

fn map_track(raw: &Value) -> Option<TrackInfo> {
    let uri = extract_track_uri(raw);
    let raw_id = first_string(raw, &["id", "gid"])
        .or_else(|| uri.as_deref().map(extract_track_id))
        .filter(|value| !value.is_empty());
    let id = raw_id
        .as_deref()
        .and_then(|value| {
            if is_hex_track_id(value) {
                hex_to_base62(value)
            } else {
                Some(value.to_string())
            }
        })
        .or_else(|| uri.as_deref().map(extract_track_id))?;
    let hex_id = raw_id
        .as_deref()
        .and_then(|value| {
            if is_hex_track_id(value) {
                Some(value.to_ascii_lowercase())
            } else {
                to_hex_track_id(value)
            }
        })
        .or_else(|| uri.as_deref().and_then(to_hex_track_id));
    let name = first_string(raw, &["name", "title"])
        .or_else(|| string_at(raw, &["metadata", "title"]))
        .unwrap_or_default();
    let album_name =
        string_at(raw, &["album", "name"]).or_else(|| string_at(raw, &["metadata", "album_title"]));
    let images = extract_images(raw);

    Some(TrackInfo {
        added_at: None,
        id: id.clone(),
        hex_id,
        uri: uri.or_else(|| Some(format!("spotify:track:{id}"))),
        name,
        album_name,
        album_id: None,
        album_uri: string_at(raw, &["album", "uri"])
            .or_else(|| string_at(raw, &["metadata", "album_uri"])),
        artists: extract_artists(raw),
        duration_ms: first_duration_ms(raw, &["duration_ms", "duration"])
            .or_else(|| {
                string_at(raw, &["metadata", "duration"])
                    .and_then(|value| value.parse().ok())
                    .map(normalize_second_precision_duration_ms)
            })
            .unwrap_or_default(),
        explicit: first_bool(raw, &["explicit"]).unwrap_or_default(),
        playable: true,
        preview_url: None,
        images,
    })
}

fn fill_track_duration_from_player(track: &mut Option<TrackInfo>, duration_ms: i64) {
    if duration_ms <= 0 {
        return;
    }
    if let Some(track) = track.as_mut() {
        if track.duration_ms <= 0 {
            track.duration_ms = duration_ms;
        }
    }
}

fn extract_track_uri(raw: &Value) -> Option<String> {
    first_string(raw, &["uri"])
        .or_else(|| string_at(raw, &["metadata", "uri"]))
        .or_else(|| {
            first_string(raw, &["gid"]).and_then(|gid| {
                if is_hex_track_id(&gid) {
                    hex_to_base62(&gid).map(|id| format!("spotify:track:{id}"))
                } else {
                    None
                }
            })
        })
}

fn extract_artists(raw: &Value) -> Vec<Artist> {
    if let Some(artists) = raw.get("artists").and_then(Value::as_array) {
        return artists
            .iter()
            .filter_map(|artist| {
                let name = first_string(artist, &["name"])
                    .or_else(|| artist.as_str().map(str::to_owned))?;
                Some(Artist {
                    id: first_string(artist, &["id"]),
                    uri: first_string(artist, &["uri"]),
                    name,
                    images: Vec::new(),
                })
            })
            .collect();
    }

    string_at(raw, &["metadata", "artist_name"])
        .map(|name| {
            name.split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(|name| Artist {
                    id: None,
                    uri: None,
                    name: name.to_string(),
                    images: Vec::new(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn extract_images(raw: &Value) -> Vec<ImageResource> {
    let mut images = Vec::new();
    if let Some(album_images) = raw.pointer("/album/images").and_then(Value::as_array) {
        for image in album_images {
            if let Some(url) = first_string(image, &["url"]) {
                images.push(ImageResource {
                    url,
                    width: first_i64(image, &["width"]).and_then(|value| u32::try_from(value).ok()),
                    height: first_i64(image, &["height"])
                        .and_then(|value| u32::try_from(value).ok()),
                });
            }
        }
    }
    for path in [
        &["metadata", "image_url"][..],
        &["metadata", "album_image_url"][..],
        &["metadata", "image_xlarge_url"][..],
    ] {
        if let Some(url) = string_at(raw, path) {
            if !images.iter().any(|image| image.url == url) {
                images.push(ImageResource {
                    url,
                    width: None,
                    height: None,
                });
            }
        }
    }
    images
}

fn corrected_position_ms(player: &Value, duration_ms: i64, is_playing: bool) -> i64 {
    let base = first_i64(
        player,
        &["position_as_of_timestamp", "position", "position_ms"],
    )
    .unwrap_or_default();
    let timestamp = first_i64(player, &["timestamp"]);
    correct_position_for_timestamp(base, timestamp, duration_ms, is_playing)
}

fn corrected_web_player_position_ms(raw: &Value, duration_ms: i64, is_playing: bool) -> i64 {
    let base = first_i64(raw, &["progress_ms", "position_ms", "position"]).unwrap_or_default();
    let timestamp = first_i64(raw, &["timestamp"]);
    correct_position_for_timestamp(base, timestamp, duration_ms, is_playing)
}

fn correct_position_for_timestamp(
    base: i64,
    timestamp: Option<i64>,
    duration_ms: i64,
    is_playing: bool,
) -> i64 {
    let mut corrected = base;
    if is_playing {
        if let Some(timestamp) = timestamp {
            let elapsed_ms = chrono::Utc::now()
                .timestamp_millis()
                .saturating_sub(timestamp)
                .max(0);
            if elapsed_ms <= MAX_FRESH_TIMESTAMP_AGE.as_millis() as i64 {
                corrected = corrected.saturating_add(elapsed_ms);
            }
        }
    }
    if duration_ms > 0 {
        corrected.clamp(0, duration_ms)
    } else {
        corrected.max(0)
    }
}

fn player_is_playing(player: &Value) -> bool {
    let speed = first_f64(player, &["playback_speed"]);
    if matches!(speed, Some(speed) if speed <= 0.0) {
        return false;
    }

    let is_paused = first_bool(player, &["is_paused", "paused"]);
    if matches!(is_paused, Some(true)) {
        return false;
    }

    if let Some(is_playing) = first_bool(player, &["is_playing", "playing"]) {
        return is_playing;
    }

    if matches!(is_paused, Some(false)) {
        return true;
    }

    if let Some(speed) = speed {
        return speed > 0.0;
    }
    false
}

fn extract_volume(raw: &Value) -> Option<f64> {
    raw.pointer("/devices")
        .and_then(Value::as_object)
        .and_then(|devices| active_device_id(raw).and_then(|id| devices.get(&id)))
        .and_then(|device| first_f64(device, &["volume", "volume_percent"]))
        .map(|volume| if volume > 1.0 { volume / 100.0 } else { volume })
}

fn active_device_id(raw: &Value) -> Option<String> {
    first_string(raw, &["active_device_id"])
        .or_else(|| string_at(raw, &["player_state", "device", "id"]))
        .or_else(|| string_at(raw, &["player_state", "device_id"]))
}

fn is_rate_limited(error: &DaemonError) -> bool {
    matches!(error, DaemonError::HttpStatus { status: 429, .. })
}

fn first_string(raw: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| raw.get(*key))
        .and_then(|value| value.as_str().map(str::to_owned))
}

fn first_bool(raw: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter().find_map(|key| {
        let value = raw.get(*key)?;
        value
            .as_bool()
            .or_else(|| value.as_i64().map(|value| value != 0))
            .or_else(|| {
                value
                    .as_str()
                    .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
                        "true" | "1" | "yes" => Some(true),
                        "false" | "0" | "no" => Some(false),
                        _ => None,
                    })
            })
    })
}

fn first_i64(raw: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| raw.get(*key)).and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
    })
}

fn first_duration_ms(raw: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        let value = raw.get(*key)?;
        let duration = value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))?;
        Some(if key.ends_with("_ms") {
            duration
        } else {
            normalize_second_precision_duration_ms(duration)
        })
    })
}

fn normalize_second_precision_duration_ms(duration: i64) -> i64 {
    if duration > 0 && duration < 10_000 {
        duration.saturating_mul(1_000)
    } else {
        duration
    }
}

fn first_f64(raw: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| raw.get(*key)).and_then(|value| {
        value
            .as_f64()
            .or_else(|| value.as_i64().map(|value| value as f64))
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
    })
}

fn string_at(raw: &Value, path: &[&str]) -> Option<String> {
    let mut current = raw;
    for segment in path {
        current = current.get(*segment)?;
    }
    current.as_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_player_state_with_metadata_fallbacks() {
        let now = chrono::Utc::now().timestamp_millis();
        let raw = json!({
            "active_device_id": "device-a",
            "devices": {
                "device-a": { "volume_percent": 42 }
            },
            "player_state": {
                "is_paused": false,
                "timestamp": now - 1_000,
                "position_as_of_timestamp": 10_000,
                "duration": 180_000,
                "track": {
                    "gid": "00000000000000000000000000000001",
                    "metadata": {
                        "title": "Counting Stars",
                        "artist_name": "OneRepublic",
                        "album_title": "Native",
                        "image_url": "https://image.test/cover.jpg"
                    }
                }
            }
        });

        let snapshot = map_connect_state_response(&raw).expect("snapshot");
        assert!(snapshot.state.is_playing);
        assert_eq!(
            snapshot.state.track_uri,
            "spotify:track:0000000000000000000001"
        );
        assert_eq!(snapshot.state.track_name, "Counting Stars");
        assert_eq!(snapshot.state.artist_name, "OneRepublic");
        assert_eq!(snapshot.state.album_name, "Native");
        assert_eq!(snapshot.state.album_art_url, "https://image.test/cover.jpg");
        assert_eq!(snapshot.state.duration_ms, 180_000);
        assert!(snapshot.state.position_ms >= 10_000);
        assert_eq!(snapshot.state.volume, 0.42);
        assert_eq!(snapshot.active_device_id.as_deref(), Some("device-a"));
        assert_eq!(
            snapshot.track.as_ref().unwrap().id,
            "0000000000000000000001"
        );
        assert_eq!(snapshot.track.as_ref().unwrap().duration_ms, 180_000);
    }

    #[test]
    fn returns_none_without_player_track() {
        assert!(map_connect_state_response(&json!({ "player_state": {} })).is_none());
    }

    #[test]
    fn maps_web_player_response_when_connect_state_is_unavailable() {
        let raw = json!({
            "device": {
                "id": "web-device",
                "volume_percent": 37
            },
            "progress_ms": 42_000,
            "is_playing": true,
            "item": {
                "id": "4uLU6hMCjMI75M1A2tKUQC",
                "uri": "spotify:track:4uLU6hMCjMI75M1A2tKUQC",
                "name": "Never Gonna Give You Up",
                "duration_ms": 213_000,
                "album": {
                    "name": "Whenever You Need Somebody",
                    "images": [
                        { "url": "https://image.test/rick.jpg", "width": 640, "height": 640 }
                    ]
                },
                "artists": [
                    { "id": "0gxyHStUsqpMadRV0Di1Qt", "uri": "spotify:artist:0gxyHStUsqpMadRV0Di1Qt", "name": "Rick Astley" }
                ]
            }
        });

        let snapshot = map_web_player_response(&raw).expect("snapshot");
        assert!(snapshot.state.is_playing);
        assert_eq!(
            snapshot.state.track_uri,
            "spotify:track:4uLU6hMCjMI75M1A2tKUQC"
        );
        assert_eq!(snapshot.state.track_name, "Never Gonna Give You Up");
        assert_eq!(snapshot.state.artist_name, "Rick Astley");
        assert_eq!(snapshot.state.album_name, "Whenever You Need Somebody");
        assert_eq!(snapshot.state.album_art_url, "https://image.test/rick.jpg");
        assert_eq!(snapshot.state.position_ms, 42_000);
        assert_eq!(snapshot.state.duration_ms, 213_000);
        assert_eq!(snapshot.state.volume, 0.37);
        assert_eq!(snapshot.active_device_id.as_deref(), Some("web-device"));
    }

    #[test]
    fn paused_connect_state_does_not_advance_from_old_timestamp() {
        let raw = json!({
            "player_state": {
                "is_paused": true,
                "timestamp": chrono::Utc::now().timestamp_millis() - 10_000,
                "position_as_of_timestamp": 12_345,
                "duration": 180_000,
                "track": {
                    "gid": "00000000000000000000000000000001",
                    "metadata": {
                        "title": "Paused Song",
                        "artist_name": "Artist"
                    }
                }
            }
        });

        let snapshot = map_connect_state_response(&raw).expect("snapshot");

        assert!(!snapshot.state.is_playing);
        assert_eq!(snapshot.state.position_ms, 12_345);
    }

    #[test]
    fn playback_speed_zero_overrides_stale_connect_playing_flags() {
        let raw = json!({
            "player_state": {
                "is_playing": true,
                "is_paused": false,
                "playback_speed": 0,
                "timestamp": chrono::Utc::now().timestamp_millis() - 10_000,
                "position_as_of_timestamp": 12_345,
                "duration": 180_000,
                "track": {
                    "gid": "00000000000000000000000000000001",
                    "metadata": {
                        "title": "Paused Song",
                        "artist_name": "Artist"
                    }
                }
            }
        });

        let snapshot = map_connect_state_response(&raw).expect("snapshot");

        assert!(!snapshot.state.is_playing);
        assert_eq!(snapshot.state.position_ms, 12_345);
    }

    #[test]
    fn playback_speed_zero_overrides_stale_web_player_flags() {
        let raw = json!({
            "timestamp": chrono::Utc::now().timestamp_millis() - 10_000,
            "progress_ms": 42_000,
            "is_playing": true,
            "playback_speed": 0,
            "item": {
                "id": "4uLU6hMCjMI75M1A2tKUQC",
                "uri": "spotify:track:4uLU6hMCjMI75M1A2tKUQC",
                "name": "Paused Web Song",
                "duration_ms": 213_000,
                "artists": [{ "name": "Rick Astley" }]
            }
        });

        let snapshot = map_web_player_response(&raw).expect("snapshot");

        assert!(!snapshot.state.is_playing);
        assert_eq!(snapshot.state.position_ms, 42_000);
    }

    #[test]
    fn web_player_position_uses_timestamp_when_playing() {
        let raw = json!({
            "timestamp": chrono::Utc::now().timestamp_millis() - 1_000,
            "progress_ms": 42_000,
            "is_playing": true,
            "item": {
                "id": "4uLU6hMCjMI75M1A2tKUQC",
                "uri": "spotify:track:4uLU6hMCjMI75M1A2tKUQC",
                "name": "Never Gonna Give You Up",
                "duration_ms": 213_000,
                "artists": [{ "name": "Rick Astley" }]
            }
        });

        let snapshot = map_web_player_response(&raw).expect("snapshot");

        assert!(snapshot.state.is_playing);
        assert!(
            (42_900..=43_300).contains(&snapshot.state.position_ms),
            "timestamp-corrected position should include elapsed time, got {}",
            snapshot.state.position_ms
        );
    }

    #[test]
    fn playing_position_keeps_seeked_progress_when_timestamp_is_stale() {
        let raw = json!({
            "timestamp": chrono::Utc::now().timestamp_millis() - 30_000,
            "progress_ms": 42_000,
            "is_playing": true,
            "item": {
                "id": "4uLU6hMCjMI75M1A2tKUQC",
                "uri": "spotify:track:4uLU6hMCjMI75M1A2tKUQC",
                "name": "Seeked Song",
                "duration_ms": 213_000,
                "artists": [{ "name": "Rick Astley" }]
            }
        });

        let snapshot = map_web_player_response(&raw).expect("snapshot");

        assert!(snapshot.state.is_playing);
        assert!(
            (42_000..=42_100).contains(&snapshot.state.position_ms),
            "stale timestamp after seek should not move fresh progress, got {}",
            snapshot.state.position_ms
        );
    }

    #[test]
    fn web_player_paused_position_ignores_old_timestamp() {
        let raw = json!({
            "timestamp": chrono::Utc::now().timestamp_millis() - 10_000,
            "progress_ms": 42_000,
            "is_playing": false,
            "item": {
                "id": "4uLU6hMCjMI75M1A2tKUQC",
                "uri": "spotify:track:4uLU6hMCjMI75M1A2tKUQC",
                "name": "Never Gonna Give You Up",
                "duration_ms": 213_000,
                "artists": [{ "name": "Rick Astley" }]
            }
        });

        let snapshot = map_web_player_response(&raw).expect("snapshot");

        assert!(!snapshot.state.is_playing);
        assert_eq!(snapshot.state.position_ms, 42_000);
    }

    #[test]
    fn stale_cached_playing_snapshot_is_frozen() {
        let state = PlaybackFallbackState {
            connect_state_next_allowed: Instant::now(),
            web_player_next_allowed: Instant::now(),
            last_web_player_snapshot: Some(CachedPlaybackSnapshot {
                snapshot: ConnectPlaybackSnapshot {
                    state: PlaybackState {
                        is_playing: true,
                        track_uri: "spotify:track:test".into(),
                        track_name: "Test".into(),
                        artist_name: String::new(),
                        album_name: String::new(),
                        album_art_url: String::new(),
                        position_ms: 1_000,
                        duration_ms: 120_000,
                        volume: 1.0,
                        player_status: "ready".into(),
                        playback_source: PLAYBACK_SOURCE_WEB_API.into(),
                    },
                    track: None,
                    active_device_id: None,
                },
                observed_at: Instant::now()
                    - MAX_CACHED_PLAYING_EXTRAPOLATION
                    - Duration::from_secs(5),
            }),
        };

        let snapshot = state
            .cached_web_player_snapshot(Instant::now())
            .expect("snapshot");

        assert!(!snapshot.state.is_playing);
        assert!(
            (4_000..=4_100).contains(&snapshot.state.position_ms),
            "stale cached fallback should cap extrapolated position, got {}",
            snapshot.state.position_ms
        );
    }

    #[test]
    fn builds_spotify_web_observer_contract() {
        let playback_device_id =
            playback_device_id_from_seed("7754b2fb-76ad-40ed-bca2-814f0d0f5617");
        assert_eq!(
            playback_device_id,
            "95153d0c1877b62b5e346a0da0d3b517811824a0"
        );
        assert_eq!(
            observer_device_id(&playback_device_id),
            "hobs_95153d0c1877b62b5e346a0da0d3b517811"
        );

        let event = json!({
            "type": "message",
            "uri": "hm://pusher/v1/connections/fallback-id",
            "headers": {
                "Spotify-Connection-Id": "header-connection-id"
            }
        });
        assert_eq!(
            dealer_connection_id_from_event(&event).as_deref(),
            Some("header-connection-id")
        );

        assert_eq!(
            connect_state_observer_body(),
            json!({
                "member_type": "CONNECT_STATE",
                "device": {
                    "device_info": {
                        "capabilities": {
                            "can_be_player": false,
                            "hidden": true,
                            "needs_full_player_state": true
                        }
                    }
                }
            })
        );
    }
}
