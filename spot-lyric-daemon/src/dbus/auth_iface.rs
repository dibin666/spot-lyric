use zbus::{fdo, interface, object_server::SignalEmitter};

use crate::{
    app_state::AppState,
    dbus::{to_fdo_error, to_json_reply},
};

#[derive(Clone)]
pub struct AuthIface {
    state: AppState,
}

impl AuthIface {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[interface(name = "cn.spotlyric.Auth")]
impl AuthIface {
    async fn get_snapshot(&self) -> fdo::Result<String> {
        let snapshot = self.state.auth_snapshot().await;
        to_json_reply(&snapshot)
    }

    async fn import_cookie_file(&self, path: &str) -> fdo::Result<String> {
        let snapshot = self
            .state
            .import_cookie_file(path)
            .await
            .map_err(to_fdo_error)?;
        to_json_reply(&snapshot)
    }

    async fn import_cookie_string(&self, cookie: &str) -> fdo::Result<String> {
        let snapshot = self
            .state
            .import_cookie_string(cookie)
            .await
            .map_err(to_fdo_error)?;
        to_json_reply(&snapshot)
    }

    async fn refresh(&self) -> fdo::Result<String> {
        let snapshot = self.state.refresh_auth().await.map_err(to_fdo_error)?;
        to_json_reply(&snapshot)
    }

    async fn clear_cookie(&self) -> fdo::Result<()> {
        self.state.clear_cookie().await.map_err(to_fdo_error)
    }

    #[zbus(signal)]
    pub async fn snapshot_changed(emitter: &SignalEmitter<'_>, snapshot: &str) -> zbus::Result<()>;
}
