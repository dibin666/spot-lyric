//! Generated zbus proxies for the integrated backend service.
//!
//! Service name / object path / method signatures match
//! `backend-integration.md` §3 verbatim.

use zbus::Connection;

// Each interface lives in its own module so generated argument structs do
// not collide (e.g. `StateChangedArgs` would otherwise clash between
// Auth.SnapshotChanged and Playback.StateChanged).

pub mod auth {
    use zbus::proxy;

    #[proxy(
        interface = "cn.spotlyric.Auth",
        default_service = "cn.spotlyric.Daemon",
        default_path = "/cn/spotlyric/Daemon"
    )]
    pub trait SpotLyricAuth {
        async fn get_snapshot(&self) -> zbus::Result<String>;
        async fn import_cookie_file(&self, path: &str) -> zbus::Result<String>;
        async fn import_cookie_string(&self, cookie: &str) -> zbus::Result<String>;
        async fn refresh(&self) -> zbus::Result<String>;
        async fn clear_cookie(&self) -> zbus::Result<()>;

        #[zbus(signal)]
        async fn snapshot_changed(&self, snapshot: &str) -> zbus::Result<()>;
    }
}

pub mod playback {
    use super::super::types::PlaybackState;
    use zbus::proxy;

    #[proxy(
        interface = "cn.spotlyric.Playback",
        default_service = "cn.spotlyric.Daemon",
        default_path = "/cn/spotlyric/Daemon"
    )]
    pub trait SpotLyricPlayback {
        async fn get_state(&self) -> zbus::Result<PlaybackState>;
        async fn get_settings(&self) -> zbus::Result<String>;
        async fn set_preferred_playback_source(&self, source: &str) -> zbus::Result<()>;
        async fn toggle_playing(&self) -> zbus::Result<()>;
        async fn skip_next(&self) -> zbus::Result<()>;
        async fn skip_previous(&self) -> zbus::Result<()>;

        #[zbus(signal)]
        async fn state_changed(&self, state: PlaybackState) -> zbus::Result<()>;

        #[zbus(signal)]
        async fn settings_changed(&self, settings: &str) -> zbus::Result<()>;
    }
}

pub mod lyrics {
    use zbus::proxy;

    #[proxy(
        interface = "cn.spotlyric.Lyrics",
        default_service = "cn.spotlyric.Daemon",
        default_path = "/cn/spotlyric/Daemon"
    )]
    pub trait SpotLyricLyrics {
        async fn get_track_lyrics(&self, track_uri: &str) -> zbus::Result<String>;
        async fn search_manual_matches(&self, query: &str) -> zbus::Result<String>;
        async fn preview_manual_match(&self, candidate_id: &str) -> zbus::Result<String>;
        async fn save_manual_match(&self, track_uri: &str, candidate_id: &str) -> zbus::Result<()>;
        async fn get_settings(&self) -> zbus::Result<String>;
        async fn set_preferred_provider(&self, provider: &str) -> zbus::Result<()>;
        async fn set_timing_offset_ms(&self, offset_ms: i32) -> zbus::Result<()>;

        #[zbus(signal)]
        async fn settings_changed(&self, settings: &str) -> zbus::Result<()>;
    }
}

// Re-exports
pub use auth::SpotLyricAuthProxy;
pub use lyrics::SpotLyricLyricsProxy;
pub use playback::SpotLyricPlaybackProxy;

/// Bundle of every proxy connected to the daemon.
#[derive(Clone)]
pub struct DaemonClient {
    pub auth: SpotLyricAuthProxy<'static>,
    pub playback: SpotLyricPlaybackProxy<'static>,
    pub lyrics: SpotLyricLyricsProxy<'static>,
}

impl DaemonClient {
    pub async fn connect() -> zbus::Result<Self> {
        let conn = Connection::session().await?;
        Ok(Self {
            auth: SpotLyricAuthProxy::new(&conn).await?,
            playback: SpotLyricPlaybackProxy::new(&conn).await?,
            lyrics: SpotLyricLyricsProxy::new(&conn).await?,
        })
    }
}
