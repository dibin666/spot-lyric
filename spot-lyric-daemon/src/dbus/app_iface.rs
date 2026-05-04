use zbus::{fdo, interface};

use crate::app_state::AppState;

#[derive(Clone)]
pub struct AppIface {
    state: AppState,
}

impl AppIface {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[interface(name = "cn.spotlyric.App")]
impl AppIface {
    async fn quit(&self) -> fdo::Result<()> {
        self.state.request_shutdown();
        Ok(())
    }
}
