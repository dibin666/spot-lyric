use std::{sync::Arc, time::Duration};

use tokio::sync::{broadcast, RwLock};

use crate::{
    error::Result,
    spotify::connect_state::{ConnectPlaybackSnapshot, ConnectStateClient},
    types::{PlaybackState, TrackInfo},
};

#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackSnapshot {
    pub state: PlaybackState,
    pub track: Option<TrackInfo>,
    pub active_device_id: Option<String>,
}

#[derive(Clone)]
pub struct PlaybackDomain {
    active_device_id: Arc<RwLock<Option<String>>>,
    connect: ConnectStateClient,
    consecutive_failures: Arc<RwLock<u32>>,
    last_track: Arc<RwLock<Option<TrackInfo>>>,
    notifier: broadcast::Sender<PlaybackState>,
    state: Arc<RwLock<PlaybackState>>,
}

impl PlaybackDomain {
    pub fn new(connect: ConnectStateClient) -> Self {
        let (notifier, _) = broadcast::channel(64);
        Self {
            active_device_id: Arc::new(RwLock::new(None)),
            connect,
            consecutive_failures: Arc::new(RwLock::new(0)),
            last_track: Arc::new(RwLock::new(None)),
            notifier,
            state: Arc::new(RwLock::new(idle_state())),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PlaybackState> {
        self.notifier.subscribe()
    }

    pub async fn get_state(&self) -> PlaybackState {
        self.state.read().await.clone()
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

    pub async fn run(self: Arc<Self>, mut shutdown: tokio::sync::broadcast::Receiver<()>) {
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
        match self.connect.fetch_state().await? {
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

    async fn publish_snapshot(&self, snapshot: ConnectPlaybackSnapshot) {
        *self.active_device_id.write().await = snapshot.active_device_id.clone();
        *self.last_track.write().await = snapshot.track.clone();
        *self.state.write().await = snapshot.state.clone();
        let _ = self.notifier.send(snapshot.state);
    }

    async fn publish_idle(&self) {
        *self.active_device_id.write().await = None;
        *self.last_track.write().await = None;
        let idle = idle_state();
        *self.state.write().await = idle.clone();
        let _ = self.notifier.send(idle);
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
            let _ = self.notifier.send(state.clone());
        }
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
    }
}
