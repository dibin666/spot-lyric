use zbus::{fdo, interface, object_server::SignalEmitter};

use crate::{app_state::AppState, dbus::to_fdo_error, types::PlaybackState};

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
}
