use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::sync::{broadcast, RwLock};

use crate::{
    error::Result,
    playback_sources::mpris::MprisPlaybackSource,
    spotify::connect_state::{ConnectPlaybackSnapshot, ConnectStateClient},
    storage::PlaybackStore,
    types::{
        playback::{
            normalize_preferred_playback_source, playback_source_order, PLAYBACK_SOURCE_CONNECT_STATE,
            PLAYBACK_SOURCE_DEALER, PLAYBACK_SOURCE_ERROR, PLAYBACK_SOURCE_IDLE,
            PLAYBACK_SOURCE_MPRIS, PLAYBACK_SOURCE_WEB_API, PLAYBACK_SOURCE_WEB_API_CACHE,
        },
        PlaybackState, TrackInfo,
    },
};

#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackSnapshot {
    pub state: PlaybackState,
    pub track: Option<TrackInfo>,
    pub active_device_id: Option<String>,
}

#[derive(Debug, Clone)]
struct TimelineState {
    duration_ms: i64,
    is_playing: bool,
    observed_at: Instant,
    position_ms: i64,
    track_uri: String,
}

impl TimelineState {
    fn from_state(state: &PlaybackState, observed_at: Instant) -> Self {
        Self {
            duration_ms: state.duration_ms,
            is_playing: state.is_playing,
            observed_at,
            position_ms: state.position_ms,
            track_uri: state.track_uri.clone(),
        }
    }

    fn estimate_position_ms(&self) -> i64 {
        let mut position_ms = self.position_ms;
        if self.is_playing {
            let elapsed_ms = self
                .observed_at
                .elapsed()
                .as_millis()
                .min(i64::MAX as u128) as i64;
            position_ms = position_ms.saturating_add(elapsed_ms);
        }

        if self.duration_ms > 0 {
            position_ms.clamp(0, self.duration_ms)
        } else {
            position_ms.max(0)
        }
    }
}

#[derive(Clone)]
pub struct PlaybackDomain {
    active_device_id: Arc<RwLock<Option<String>>>,
    connect: ConnectStateClient,
    consecutive_failures: Arc<RwLock<u32>>,
    last_track: Arc<RwLock<Option<TrackInfo>>>,
    mpris: MprisPlaybackSource,
    notifier: broadcast::Sender<PlaybackState>,
    playback_store: PlaybackStore,
    preferred_playback_source: Arc<RwLock<String>>,
    state: Arc<RwLock<PlaybackState>>,
    timeline: Arc<RwLock<Option<TimelineState>>>,
}

impl PlaybackDomain {
    pub fn new(
        connect: ConnectStateClient,
        mpris: MprisPlaybackSource,
        playback_store: PlaybackStore,
    ) -> Self {
        let (notifier, _) = broadcast::channel(64);
        Self {
            active_device_id: Arc::new(RwLock::new(None)),
            connect,
            consecutive_failures: Arc::new(RwLock::new(0)),
            last_track: Arc::new(RwLock::new(None)),
            mpris,
            notifier,
            playback_store,
            preferred_playback_source: Arc::new(RwLock::new("auto".into())),
            state: Arc::new(RwLock::new(idle_state())),
            timeline: Arc::new(RwLock::new(None)),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PlaybackState> {
        self.notifier.subscribe()
    }

    pub async fn get_state(&self) -> PlaybackState {
        let state = self.state.read().await.clone();
        self.estimate_state(state).await
    }

    pub async fn current_track_for_uri(&self, track_uri: &str) -> Option<TrackInfo> {
        let normalized = track_uri.trim();
        self.last_track.read().await.clone().filter(|track| {
            normalized.is_empty()
                || track
                    .uri
                    .as_deref()
                    .map(|uri| uri == normalized)
                    .unwrap_or(false)
        })
    }

    pub async fn set_preferred_playback_source(&self, source: &str) {
        if let Some(source) = normalize_preferred_playback_source(source) {
            *self.preferred_playback_source.write().await = source;
        }
    }

    pub async fn run(self: Arc<Self>, mut shutdown: tokio::sync::broadcast::Receiver<()>) {
        if let Ok(source) = self.playback_store.get_preferred_playback_source().await {
            self.set_preferred_playback_source(&source).await;
        }

        let mut interval =
            tokio::time::interval(Duration::from_millis(crate::config::POLL_INTERVAL_MS));
        loop {
            tokio::select! {
                _ = shutdown.recv() => break,
                _ = interval.tick() => {
                    if let Err(error) = self.refresh_once().await {
                        tracing::warn!(%error, "connect-state poll failed");
                        self.record_poll_failure().await;
                    }
                }
            }
        }
    }

    pub async fn refresh_once(&self) -> Result<()> {
        match self.fetch_snapshot().await? {
            Some(snapshot) => self.publish_snapshot(snapshot).await,
            None => self.publish_idle().await,
        }
        *self.consecutive_failures.write().await = 0;
        Ok(())
    }

    pub async fn toggle_playing(&self) -> Result<()> {
        let state = self.get_state().await;
        let active_device_id = self.active_device_id.read().await.clone();
        if state.is_playing {
            self.connect.pause(active_device_id.as_deref()).await?;
        } else {
            self.connect.play(active_device_id.as_deref()).await?;
        }
        self.refresh_once().await
    }

    pub async fn skip_next(&self) -> Result<()> {
        let active_device_id = self.active_device_id.read().await.clone();
        self.connect.skip_next(active_device_id.as_deref()).await?;
        self.refresh_once().await
    }

    pub async fn skip_previous(&self) -> Result<()> {
        let active_device_id = self.active_device_id.read().await.clone();
        self.connect
            .skip_previous(active_device_id.as_deref())
            .await?;
        self.refresh_once().await
    }

    async fn fetch_snapshot(&self) -> Result<Option<ConnectPlaybackSnapshot>> {
        let preferred = self.preferred_playback_source.read().await.clone();
        for source in playback_source_order(&preferred) {
            let snapshot = match source {
                PLAYBACK_SOURCE_MPRIS => self.mpris.fetch_state().await?,
                PLAYBACK_SOURCE_DEALER
                | PLAYBACK_SOURCE_CONNECT_STATE
                | PLAYBACK_SOURCE_WEB_API => self.connect.fetch_source(source).await?,
                _ => None,
            };
            if snapshot.is_some() {
                return Ok(snapshot);
            }
        }

        Ok(None)
    }

    async fn publish_snapshot(&self, snapshot: ConnectPlaybackSnapshot) {
        if !self.should_accept_snapshot(&snapshot.state).await {
            return;
        }

        let observed_at = Instant::now();
        *self.active_device_id.write().await = snapshot.active_device_id.clone();
        *self.last_track.write().await = snapshot.track.clone();
        *self.state.write().await = snapshot.state.clone();
        *self.timeline.write().await = Some(TimelineState::from_state(&snapshot.state, observed_at));
        let _ = self.notifier.send(snapshot.state);
    }

    async fn publish_idle(&self) {
        *self.active_device_id.write().await = None;
        *self.last_track.write().await = None;
        *self.timeline.write().await = None;
        let idle = idle_state();
        *self.state.write().await = idle.clone();
        let _ = self.notifier.send(idle);
    }

    async fn estimate_state(&self, mut state: PlaybackState) -> PlaybackState {
        let timeline = self.timeline.read().await.clone();
        let Some(timeline) = timeline else {
            return state;
        };
        if timeline.track_uri == state.track_uri && state.player_status == "ready" {
            state.position_ms = timeline.estimate_position_ms();
        }
        state
    }

    async fn should_accept_snapshot(&self, incoming: &PlaybackState) -> bool {
        let current = self.get_state().await;
        if current.track_uri.is_empty()
            || incoming.track_uri.is_empty()
            || current.player_status != "ready"
            || incoming.player_status != "ready"
        {
            return true;
        }
        if current.track_uri != incoming.track_uri {
            return true;
        }
        if !current.is_playing || !incoming.is_playing || current.is_playing != incoming.is_playing {
            return true;
        }

        incoming.position_ms + rewind_tolerance_ms(&incoming.playback_source) >= current.position_ms
    }

    async fn record_poll_failure(&self) {
        let mut failures = self.consecutive_failures.write().await;
        *failures += 1;
        if *failures < 3 {
            return;
        }

        let mut state = self.state.write().await;
        if state.player_status != "error" {
            state.player_status = "error".into();
            state.playback_source = PLAYBACK_SOURCE_ERROR.into();
            let _ = self.notifier.send(state.clone());
        }
    }
}

fn rewind_tolerance_ms(source: &str) -> i64 {
    match source {
        PLAYBACK_SOURCE_MPRIS => 3_000,
        PLAYBACK_SOURCE_DEALER => 2_000,
        PLAYBACK_SOURCE_CONNECT_STATE => 1_500,
        PLAYBACK_SOURCE_WEB_API | PLAYBACK_SOURCE_WEB_API_CACHE => 1_000,
        _ => 1_500,
    }
}

fn idle_state() -> PlaybackState {
    PlaybackState {
        is_playing: false,
        track_uri: String::new(),
        track_name: String::new(),
        artist_name: String::new(),
        album_name: String::new(),
        album_art_url: String::new(),
        position_ms: 0,
        duration_ms: 0,
        volume: 1.0,
        player_status: "idle".into(),
        playback_source: PLAYBACK_SOURCE_IDLE.into(),
    }
}
