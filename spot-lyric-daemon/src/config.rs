use std::path::PathBuf;

use crate::error::{DaemonError, Result};

pub const APP_NAME: &str = "spot-lyric";
pub const DBUS_BUS_NAME: &str = "cn.spotlyric.Daemon";
pub const DBUS_OBJECT_PATH: &str = "/cn/spotlyric/Daemon";
pub const POLL_INTERVAL_MS: u64 = 2_000;
pub const SQLITE_FILE: &str = "spot-lyric.db";

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub root_dir: PathBuf,
    pub sqlite_path: PathBuf,
}

impl AppPaths {
    pub fn resolve(data_dir_override: Option<PathBuf>) -> Result<Self> {
        let root_dir = match data_dir_override {
            Some(path) => path,
            None => dirs::data_local_dir()
                .ok_or(DaemonError::MissingConfigDir)?
                .join(APP_NAME),
        };

        Ok(Self {
            sqlite_path: root_dir.join(SQLITE_FILE),
            root_dir,
        })
    }
}
