use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use tokio::{
    fs,
    sync::{broadcast, Mutex, Notify},
    task::JoinHandle,
};

use crate::{
    config::AppPaths,
    domain::{LyricsDomain, PlaybackDomain},
    error::Result,
    lyrics_external::{netease::NeteaseLyricsClient, qq::QqMusicLyricsClient},
    playback_sources::mpris::MprisPlaybackSource,
    spotify::{
        auth_service::{AuthService, AuthServiceOptions},
        connect_state::ConnectStateClient,
        discovery::{DiscoveryService, ProtocolRegistry},
        lyrics_api::LyricsClient,
        transport::SpotifyTransport,
    },
    storage::{CookieStore, Database, DeviceStore, LyricsStore, PlaybackStore},
    types::{
        AuthSnapshot, LyricsCandidate, LyricsPayload, LyricsSettings, PlaybackSettings,
        PlaybackState,
    },
};

#[derive(Debug)]
struct InnerState {
    shutdown_requested: AtomicBool,
    shutdown_notify: Notify,
}

#[derive(Clone)]
pub struct AppState {
    auth: Arc<AuthService>,
    inner: Arc<InnerState>,
    lyrics_domain: LyricsDomain,
    paths: Arc<AppPaths>,
    playback_domain: PlaybackDomain,
    playback_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    playback_settings_events: broadcast::Sender<PlaybackSettings>,
    playback_store: PlaybackStore,
    settings_events: broadcast::Sender<LyricsSettings>,
    shutdown_events: broadcast::Sender<()>,
}

impl AppState {
    pub async fn bootstrap(data_dir_override: Option<PathBuf>) -> Result<Self> {
        let paths = AppPaths::resolve(data_dir_override)?;
        fs::create_dir_all(&paths.root_dir).await?;

        let database = Database::open(&paths.sqlite_path).await?;
        let device_store = DeviceStore::new(database.clone());
        let cookie_store = CookieStore::new(database.clone());
        let playback_store = PlaybackStore::new(database.clone());
        let lyrics_store = LyricsStore::new(database);
        let device_id = device_store.get_or_create_device_id().await?;

        let protocol = ProtocolRegistry::default();
        let http_client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
            .timeout(Duration::from_secs(15))
            .build()?;
        let discovery = DiscoveryService::new(http_client.clone(), protocol.clone());
        match tokio::time::timeout(
            Duration::from_secs(3),
            discovery.get_spclient_base_url(false),
        )
        .await
        {
            Ok(Ok(spclient_base)) => protocol.set_spclient_base(&spclient_base)?,
            Ok(Err(error)) => {
                tracing::warn!(%error, "spotify endpoint discovery failed; using default spclient base")
            }
            Err(_) => {
                tracing::warn!("spotify endpoint discovery timed out; using default spclient base")
            }
        }

        let auth = Arc::new(
            AuthService::new(AuthServiceOptions {
                cookie_store,
                device_store,
                client: http_client.clone(),
                protocol: protocol.clone(),
                secrets_remote_url: None,
                open_spotify_head_url: None,
            })
            .await?,
        );
        let transport = SpotifyTransport::new(auth.clone(), http_client.clone(), protocol.clone());
        let connect = ConnectStateClient::new(transport.clone(), protocol.clone(), device_id);
        let playback_domain = PlaybackDomain::new(
            connect,
            MprisPlaybackSource::new(),
            playback_store.clone(),
        );
        let spotify_lyrics = LyricsClient::new(transport, protocol);
        let lyrics_domain = LyricsDomain::new(
            playback_domain.clone(),
            lyrics_store,
            spotify_lyrics,
            NeteaseLyricsClient::new(http_client.clone()),
            QqMusicLyricsClient::new(http_client),
        );
        let (settings_events, _) = broadcast::channel(64);
        let (playback_settings_events, _) = broadcast::channel(64);
        let (shutdown_events, _) = broadcast::channel(8);

        let state = Self {
            auth,
            inner: Arc::new(InnerState {
                shutdown_requested: AtomicBool::new(false),
                shutdown_notify: Notify::new(),
            }),
            lyrics_domain,
            paths: Arc::new(paths),
            playback_domain,
            playback_task: Arc::new(Mutex::new(None)),
            playback_settings_events,
            playback_store,
            settings_events,
            shutdown_events,
        };
        state.spawn_playback_loop().await;
        Ok(state)
    }

    pub fn paths(&self) -> Arc<AppPaths> {
        self.paths.clone()
    }

    pub fn request_shutdown(&self) {
        if !self.inner.shutdown_requested.swap(true, Ordering::SeqCst) {
            let _ = self.shutdown_events.send(());
            self.inner.shutdown_notify.notify_waiters();
        }
    }

    pub async fn wait_for_shutdown(&self) {
        if self.inner.shutdown_requested.load(Ordering::SeqCst) {
            return;
        }
        self.inner.shutdown_notify.notified().await;
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.request_shutdown();
        if let Some(task) = self.playback_task.lock().await.take() {
            task.await?;
        }
        Ok(())
    }

    pub fn subscribe_auth(&self) -> broadcast::Receiver<AuthSnapshot> {
        self.auth.subscribe()
    }

    pub fn subscribe_playback(&self) -> broadcast::Receiver<PlaybackState> {
        self.playback_domain.subscribe()
    }

    pub fn subscribe_lyrics_settings(&self) -> broadcast::Receiver<LyricsSettings> {
        self.settings_events.subscribe()
    }

    pub fn subscribe_playback_settings(&self) -> broadcast::Receiver<PlaybackSettings> {
        self.playback_settings_events.subscribe()
    }

    pub async fn auth_snapshot(&self) -> AuthSnapshot {
        self.auth.get_snapshot().await
    }

    pub async fn import_cookie_file(&self, path: &str) -> Result<AuthSnapshot> {
        self.auth.import_cookie_file(path).await
    }

    pub async fn import_cookie_string(&self, cookie: &str) -> Result<AuthSnapshot> {
        self.auth.import_cookie_string(cookie).await
    }

    pub async fn refresh_auth(&self) -> Result<AuthSnapshot> {
        self.auth.refresh().await
    }

    pub async fn clear_cookie(&self) -> Result<()> {
        self.auth.clear_cookie().await
    }

    pub async fn playback_state(&self) -> PlaybackState {
        self.playback_domain.get_state().await
    }

    pub async fn playback_settings(&self) -> Result<PlaybackSettings> {
        self.playback_store.get_settings().await
    }

    pub async fn toggle_playing(&self) -> Result<()> {
        self.playback_domain.toggle_playing().await
    }

    pub async fn skip_next(&self) -> Result<()> {
        self.playback_domain.skip_next().await
    }

    pub async fn skip_previous(&self) -> Result<()> {
        self.playback_domain.skip_previous().await
    }

    pub async fn get_track_lyrics(&self, track_uri: &str) -> Result<LyricsPayload> {
        self.lyrics_domain.get_track_lyrics(track_uri).await
    }

    pub async fn search_manual_matches(&self, query: &str) -> Result<Vec<LyricsCandidate>> {
        self.lyrics_domain.search_manual_matches(query).await
    }

    pub async fn preview_manual_match(&self, candidate_id: &str) -> Result<LyricsPayload> {
        self.lyrics_domain
            .preview_manual_match(None, candidate_id)
            .await
    }

    pub async fn save_manual_match(
        &self,
        track_uri: &str,
        candidate_id: &str,
    ) -> Result<LyricsSettings> {
        let settings = self
            .lyrics_domain
            .save_manual_match(track_uri, candidate_id)
            .await?;
        let _ = self.settings_events.send(settings.clone());
        Ok(settings)
    }

    pub async fn lyrics_settings(&self) -> Result<LyricsSettings> {
        self.lyrics_domain.get_settings(None).await
    }

    pub async fn set_preferred_provider(&self, provider: &str) -> Result<LyricsSettings> {
        let settings = self.lyrics_domain.set_preferred_provider(provider).await?;
        let _ = self.settings_events.send(settings.clone());
        Ok(settings)
    }

    pub async fn set_timing_offset_ms(&self, offset_ms: i32) -> Result<LyricsSettings> {
        let settings = self.lyrics_domain.set_timing_offset_ms(offset_ms).await?;
        let _ = self.settings_events.send(settings.clone());
        Ok(settings)
    }

    pub async fn set_preferred_playback_source(&self, source: &str) -> Result<PlaybackSettings> {
        let settings = self.playback_store.set_preferred_playback_source(source).await?;
        self.playback_domain
            .set_preferred_playback_source(&settings.preferred_playback_source)
            .await;
        let _ = self.playback_settings_events.send(settings.clone());
        Ok(settings)
    }

    async fn spawn_playback_loop(&self) {
        let playback = Arc::new(self.playback_domain.clone());
        let shutdown = self.shutdown_events.subscribe();
        let task = tokio::spawn(playback.run(shutdown));
        *self.playback_task.lock().await = Some(task);
    }
}
