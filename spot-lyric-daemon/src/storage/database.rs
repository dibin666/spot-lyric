use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rusqlite::Connection;

use crate::error::{DaemonError, Result};

#[derive(Debug, Clone)]
pub struct Database {
    connection: Arc<Mutex<Connection>>,
    path: Arc<PathBuf>,
}

impl Database {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let open_path = path.clone();
        let connection = tokio::task::spawn_blocking(move || -> Result<Connection> {
            if let Some(parent) = open_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let mut connection = Connection::open(&open_path)?;
            initialize_schema(&mut connection)?;
            Ok(connection)
        })
        .await??;

        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            path: Arc::new(path),
        })
    }

    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub async fn with_connection<T, F>(&self, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let connection = self.connection.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle
                    .spawn_blocking(move || {
                        let mut guard = connection
                            .lock()
                            .map_err(|_| DaemonError::Poisoned("sqlite connection mutex".into()))?;
                        operation(&mut guard)
                    })
                    .await?
            }
            Err(_) => {
                let mut guard = connection
                    .lock()
                    .map_err(|_| DaemonError::Poisoned("sqlite connection mutex".into()))?;
                operation(&mut guard)
            }
        }
    }
}

fn initialize_schema(connection: &mut Connection) -> Result<()> {
    connection.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA foreign_keys = ON;
        PRAGMA busy_timeout = 5000;

        CREATE TABLE IF NOT EXISTS lyrics_matches (
            spotify_track_id TEXT PRIMARY KEY,
            candidate_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS comment_matches (
            spotify_track_id TEXT PRIMARY KEY,
            candidate_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS cookie_profiles (
            id TEXT PRIMARY KEY,
            label TEXT NOT NULL,
            cookie_data TEXT NOT NULL,
            is_active INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS device_identity (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS track_durations (
            uri TEXT PRIMARY KEY,
            duration_ms INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        "#,
    )?;

    Ok(())
}
