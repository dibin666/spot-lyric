use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::{
    error::{DaemonError, Result},
    types::AuthProfile,
};

use super::database::Database;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CookieProfileState {
    pub active_profile_id: Option<String>,
    pub profiles: Vec<AuthProfile>,
}

#[derive(Debug, Clone)]
pub struct CookieStore {
    database: Database,
}

impl CookieStore {
    pub fn new(database: Database) -> Self {
        Self { database }
    }

    pub async fn active_cookie_text(&self) -> Result<Option<String>> {
        self.database
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT cookie_data FROM cookie_profiles WHERE is_active = 1 LIMIT 1",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(Into::into)
            })
            .await
    }

    pub async fn clear_cookie(&self) -> Result<CookieProfileState> {
        self.database
            .with_connection(|connection| {
                connection.execute("UPDATE cookie_profiles SET is_active = 0", [])?;
                Ok(())
            })
            .await?;
        self.list_profiles().await
    }

    pub async fn import_cookie_text(
        &self,
        label: Option<&str>,
        cookie_text: &str,
    ) -> Result<CookieProfileState> {
        let cookie_text = cookie_text.trim().to_string();
        if cookie_text.is_empty() {
            return Err(DaemonError::InvalidArgument(
                "cookie text is required".into(),
            ));
        }

        let label = label.map(str::to_owned);
        self.database
            .with_connection(move |connection| {
                let now = now_millis();
                let existing = connection
                    .query_row(
                        "SELECT id, label, created_at FROM cookie_profiles WHERE cookie_data = ?1 LIMIT 1",
                        params![cookie_text],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, i64>(2)?,
                            ))
                        },
                    )
                    .optional()?;

                let profile_count: i64 = connection.query_row("SELECT COUNT(*) FROM cookie_profiles", [], |row| row.get(0))?;
                let fallback = format!("Cookie {}", profile_count + 1);
                let normalized_label = normalize_label(label.as_deref(), fallback.as_str());

                let (profile_id, created_at) = if let Some((profile_id, _existing_label, created_at)) = existing {
                    connection.execute(
                        "UPDATE cookie_profiles SET label = ?1, cookie_data = ?2, updated_at = ?3 WHERE id = ?4",
                        params![normalized_label, cookie_text, now, profile_id],
                    )?;
                    (profile_id, created_at)
                } else {
                    let profile_id = Uuid::new_v4().to_string();
                    connection.execute(
                        "INSERT INTO cookie_profiles (id, label, cookie_data, is_active, created_at, updated_at) VALUES (?1, ?2, ?3, 0, ?4, ?5)",
                        params![profile_id, normalized_label, cookie_text, now, now],
                    )?;
                    (profile_id, now)
                };

                set_active_profile(connection, &profile_id)?;
                Ok((profile_id, created_at))
            })
            .await?;

        self.list_profiles().await
    }

    pub async fn list_profiles(&self) -> Result<CookieProfileState> {
        self.database
            .with_connection(|connection| {
                let active_profile_id = connection
                    .query_row(
                        "SELECT id FROM cookie_profiles WHERE is_active = 1 LIMIT 1",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?;

                let mut statement = connection.prepare(
                    "SELECT id, label, created_at, updated_at FROM cookie_profiles ORDER BY is_active DESC, updated_at DESC, created_at DESC",
                )?;
                let profiles = statement
                    .query_map([], |row| {
                        Ok(AuthProfile {
                            id: row.get(0)?,
                            label: row.get(1)?,
                            created_at: row.get(2)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;

                Ok(CookieProfileState {
                    active_profile_id,
                    profiles,
                })
            })
            .await
    }

    pub async fn switch_active_profile(&self, profile_id: &str) -> Result<CookieProfileState> {
        let profile_id = profile_id.to_string();
        self.database
            .with_connection(move |connection| {
                let exists = connection
                    .query_row(
                        "SELECT 1 FROM cookie_profiles WHERE id = ?1 LIMIT 1",
                        params![profile_id],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?
                    .is_some();
                if !exists {
                    return Err(DaemonError::InvalidArgument(format!(
                        "unknown cookie profile: {profile_id}"
                    )));
                }
                connection.execute(
                    "UPDATE cookie_profiles SET updated_at = ?1 WHERE id = ?2",
                    params![now_millis(), profile_id.clone()],
                )?;
                set_active_profile(connection, &profile_id)?;
                Ok(())
            })
            .await?;
        self.list_profiles().await
    }
}

fn normalize_label(label: Option<&str>, fallback: &str) -> String {
    let trimmed = label.unwrap_or_default().trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn set_active_profile(connection: &mut rusqlite::Connection, profile_id: &str) -> Result<()> {
    let transaction = connection.transaction()?;
    transaction.execute("UPDATE cookie_profiles SET is_active = 0", [])?;
    transaction.execute(
        "UPDATE cookie_profiles SET is_active = 1 WHERE id = ?1",
        params![profile_id],
    )?;
    transaction.commit()?;
    Ok(())
}
