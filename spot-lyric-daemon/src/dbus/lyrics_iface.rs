use zbus::{fdo, interface, object_server::SignalEmitter};

use crate::{
    app_state::AppState,
    dbus::{to_fdo_error, to_json_reply},
};

#[derive(Clone)]
pub struct LyricsIface {
    state: AppState,
}

impl LyricsIface {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[interface(name = "cn.spotlyric.Lyrics")]
impl LyricsIface {
    async fn get_track_lyrics(&self, track_uri: &str) -> fdo::Result<String> {
        let payload = self
            .state
            .get_track_lyrics(track_uri)
            .await
            .map_err(to_fdo_error)?;
        to_json_reply(&payload)
    }

    async fn search_manual_matches(&self, query: &str) -> fdo::Result<String> {
        let results = self
            .state
            .search_manual_matches(query)
            .await
            .map_err(to_fdo_error)?;
        to_json_reply(&results)
    }

    async fn preview_manual_match(&self, candidate_id: &str) -> fdo::Result<String> {
        let payload = self
            .state
            .preview_manual_match(candidate_id)
            .await
            .map_err(to_fdo_error)?;
        to_json_reply(&payload)
    }

    async fn save_manual_match(&self, track_uri: &str, candidate_id: &str) -> fdo::Result<()> {
        self.state
            .save_manual_match(track_uri, candidate_id)
            .await
            .map_err(to_fdo_error)?;
        Ok(())
    }

    async fn get_settings(&self) -> fdo::Result<String> {
        let settings = self.state.lyrics_settings().await.map_err(to_fdo_error)?;
        to_json_reply(&settings)
    }

    async fn set_preferred_provider(&self, provider: &str) -> fdo::Result<()> {
        self.state
            .set_preferred_provider(provider)
            .await
            .map_err(to_fdo_error)?;
        Ok(())
    }

    async fn set_timing_offset_ms(&self, offset_ms: i32) -> fdo::Result<()> {
        self.state
            .set_timing_offset_ms(offset_ms)
            .await
            .map_err(to_fdo_error)?;
        Ok(())
    }

    #[zbus(signal)]
    pub async fn settings_changed(emitter: &SignalEmitter<'_>, settings: &str) -> zbus::Result<()>;
}
