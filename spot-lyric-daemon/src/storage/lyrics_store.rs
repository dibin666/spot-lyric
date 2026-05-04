use std::str::FromStr;

use rusqlite::{params, OptionalExtension};

use crate::{
    types::{LyricsSettings, SavedLyricsMatch, StoredLyricsCandidate},
    util::convert::LyricsProviderPreference,
};

use super::database::Database;
use crate::error::Result;

const DEFAULT_PREFERRED_PROVIDER: &str = "netease";
const DEFAULT_TIMING_OFFSET_MS: i32 = 0;
const MAX_TIMING_OFFSET_MS: i32 = 5_000;
const KEY_PREFERRED_PROVIDER: &str = "preferred-provider";
const KEY_TIMING_OFFSET_MS: &str = "timing-offset-ms";

#[derive(Debug, Clone)]
pub struct LyricsStore {
    database: Database,
}

impl LyricsStore {
    pub fn new(database: Database) -> Self {
        Self { database }
    }

    pub async fn get_saved_match(
        &self,
        spotify_track_id: &str,
    ) -> Result<Option<SavedLyricsMatch>> {
        let spotify_track_id = spotify_track_id.to_string();
        self.database
            .with_connection(move |connection| {
                connection
                    .query_row(
                        "SELECT candidate_json, created_at, updated_at FROM lyrics_matches WHERE spotify_track_id = ?1",
                        params![spotify_track_id],
                        |row| {
                            let candidate_json: String = row.get(0)?;
                            let candidate: StoredLyricsCandidate = serde_json::from_str(&candidate_json).map_err(to_sql_error)?;
                            Ok(SavedLyricsMatch {
                                spotify_track_id: spotify_track_id.clone(),
                                candidate,
                                created_at: row.get(1)?,
                                updated_at: row.get(2)?,
                            })
                        },
                    )
                    .optional()
                    .map_err(Into::into)
            })
            .await
    }

    pub async fn get_settings(&self) -> Result<LyricsSettings> {
        let preferred_provider = self.get_preferred_provider().await?;
        let lyrics_timing_offset_ms = self.get_timing_offset_ms().await?;
        Ok(LyricsSettings {
            lyrics_timing_offset_ms,
            preferred_provider,
            saved_match: None,
        })
    }

    pub async fn get_preferred_provider(&self) -> Result<String> {
        self.database
            .with_connection(|connection| {
                let value = connection
                    .query_row(
                        "SELECT value FROM settings WHERE key = ?1",
                        params![KEY_PREFERRED_PROVIDER],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?;

                let provider = value
                    .and_then(|stored| {
                        LyricsProviderPreference::from_str(&stored)
                            .ok()
                            .map(|mode| mode.to_string())
                    })
                    .unwrap_or_else(|| DEFAULT_PREFERRED_PROVIDER.to_string());
                Ok(provider)
            })
            .await
    }

    pub async fn get_timing_offset_ms(&self) -> Result<i32> {
        self.database
            .with_connection(|connection| {
                let value = connection
                    .query_row(
                        "SELECT value FROM settings WHERE key = ?1",
                        params![KEY_TIMING_OFFSET_MS],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?;
                let parsed = value
                    .and_then(|stored| stored.parse::<i32>().ok())
                    .map(normalize_timing_offset_ms)
                    .unwrap_or(DEFAULT_TIMING_OFFSET_MS);
                Ok(parsed)
            })
            .await
    }

    pub async fn save_track_match(
        &self,
        spotify_track_id: &str,
        candidate: &StoredLyricsCandidate,
    ) -> Result<SavedLyricsMatch> {
        let spotify_track_id = spotify_track_id.to_string();
        let candidate = candidate.clone();
        self.database
            .with_connection(move |connection| {
                let now = now_millis();
                let candidate_json = serde_json::to_string(&candidate).map_err(to_sql_error)?;
                let created_at = connection
                    .query_row(
                        "SELECT created_at FROM lyrics_matches WHERE spotify_track_id = ?1",
                        params![spotify_track_id],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?
                    .unwrap_or(now);

                connection.execute(
                    r#"
                    INSERT INTO lyrics_matches (spotify_track_id, candidate_json, created_at, updated_at)
                    VALUES (?1, ?2, ?3, ?4)
                    ON CONFLICT(spotify_track_id) DO UPDATE SET
                        candidate_json = excluded.candidate_json,
                        updated_at = excluded.updated_at
                    "#,
                    params![spotify_track_id, candidate_json, created_at, now],
                )?;

                Ok(SavedLyricsMatch {
                    spotify_track_id,
                    candidate,
                    created_at,
                    updated_at: now,
                })
            })
            .await
    }

    pub async fn set_preferred_provider(&self, provider: &str) -> Result<LyricsSettings> {
        let provider = LyricsProviderPreference::from_str(provider)?.to_string();
        self.database
            .with_connection(move |connection| {
                connection.execute(
                    "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![KEY_PREFERRED_PROVIDER, provider],
                )?;
                Ok(())
            })
            .await?;
        self.get_settings().await
    }

    pub async fn set_timing_offset_ms(&self, value: i32) -> Result<LyricsSettings> {
        let value = normalize_timing_offset_ms(value).to_string();
        self.database
            .with_connection(move |connection| {
                connection.execute(
                    "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![KEY_TIMING_OFFSET_MS, value],
                )?;
                Ok(())
            })
            .await?;
        self.get_settings().await
    }
}

fn normalize_timing_offset_ms(value: i32) -> i32 {
    value.clamp(-MAX_TIMING_OFFSET_MS, MAX_TIMING_OFFSET_MS)
}

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn to_sql_error(error: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}
