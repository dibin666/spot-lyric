use zbus::{fdo, interface, object_server::SignalEmitter};

use crate::{
    app_state::AppState,
    dbus::{to_fdo_error, to_json_reply},
    types::PlaybackState,
};

#[derive(Clone)]
pub struct PlaybackIface {
    state: AppState,
}

impl PlaybackIface {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[interface(name = "cn.spotlyric.Playback")]
impl PlaybackIface {
    async fn get_state(&self) -> fdo::Result<PlaybackState> {
        Ok(self.state.playback_state().await)
    }

    async fn get_settings(&self) -> fdo::Result<String> {
        let settings = self.state.playback_settings().await.map_err(to_fdo_error)?;
        to_json_reply(&settings)
    }

    async fn set_preferred_playback_source(&self, source: &str) -> fdo::Result<()> {
        self.state
            .set_preferred_playback_source(source)
            .await
            .map_err(to_fdo_error)?;
        Ok(())
    }

    async fn toggle_playing(&self) -> fdo::Result<()> {
        self.state.toggle_playing().await.map_err(to_fdo_error)
    }

    async fn skip_next(&self) -> fdo::Result<()> {
        self.state.skip_next().await.map_err(to_fdo_error)
    }

    async fn skip_previous(&self) -> fdo::Result<()> {
        self.state.skip_previous().await.map_err(to_fdo_error)
    }

    #[zbus(signal)]
    pub async fn state_changed(
        emitter: &SignalEmitter<'_>,
        state: &PlaybackState,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn settings_changed(
        emitter: &SignalEmitter<'_>,
        settings: &str,
    ) -> zbus::Result<()>;
}
