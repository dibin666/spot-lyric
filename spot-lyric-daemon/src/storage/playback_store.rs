use rusqlite::{params, OptionalExtension};

use crate::{
    error::{DaemonError, Result},
    types::playback::{
        default_preferred_playback_source, normalize_preferred_playback_source, PlaybackSettings,
    },
};

use super::database::Database;

const KEY_PREFERRED_PLAYBACK_SOURCE: &str = "playback.preferred-source";

#[derive(Debug, Clone)]
pub struct PlaybackStore {
    database: Database,
}

impl PlaybackStore {
    pub fn new(database: Database) -> Self {
        Self { database }
    }

    pub async fn get_settings(&self) -> Result<PlaybackSettings> {
        Ok(PlaybackSettings {
            preferred_playback_source: self.get_preferred_playback_source().await?,
        })
    }

    pub async fn get_preferred_playback_source(&self) -> Result<String> {
        self.database
            .with_connection(|connection| {
                let value = connection
                    .query_row(
                        "SELECT value FROM settings WHERE key = ?1",
                        params![KEY_PREFERRED_PLAYBACK_SOURCE],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?;

                Ok(value
                    .as_deref()
                    .and_then(normalize_preferred_playback_source)
                    .unwrap_or_else(default_preferred_playback_source))
            })
            .await
    }

    pub async fn set_preferred_playback_source(&self, source: &str) -> Result<PlaybackSettings> {
        let source = normalize_preferred_playback_source(source).ok_or_else(|| {
            DaemonError::InvalidArgument(format!("invalid playback source preference: {source}"))
        })?;
        self.database
            .with_connection(move |connection| {
                connection.execute(
                    "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![KEY_PREFERRED_PLAYBACK_SOURCE, source],
                )?;
                Ok(())
            })
            .await?;
        self.get_settings().await
    }
}
