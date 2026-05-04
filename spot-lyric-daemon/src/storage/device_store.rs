use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::error::Result;

use super::database::Database;

const KEY_DEVICE_ID: &str = "device-id";
const KEY_USER_ID: &str = "user-id";
const KEY_USER_DISPLAY_NAME: &str = "user-display-name";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceIdentity {
    pub device_id: String,
    pub user_id: Option<String>,
    pub user_display_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DeviceStore {
    database: Database,
}

impl DeviceStore {
    pub fn new(database: Database) -> Self {
        Self { database }
    }

    pub async fn get_identity(&self) -> Result<DeviceIdentity> {
        let device_id = self.get_or_create_device_id().await?;
        let (user_id, user_display_name) = self
            .database
            .with_connection(|connection| {
                let user_id = read_value(connection, KEY_USER_ID)?;
                let user_display_name = read_value(connection, KEY_USER_DISPLAY_NAME)?;
                Ok((user_id, user_display_name))
            })
            .await?;

        Ok(DeviceIdentity {
            device_id,
            user_id,
            user_display_name,
        })
    }

    pub async fn get_or_create_device_id(&self) -> Result<String> {
        self.database
            .with_connection(|connection| {
                if let Some(existing) = read_value(connection, KEY_DEVICE_ID)? {
                    return Ok(existing);
                }

                let device_id = Uuid::new_v4().to_string();
                write_value(connection, KEY_DEVICE_ID, Some(device_id.as_str()))?;
                Ok(device_id)
            })
            .await
    }

    pub async fn set_user_identity(
        &self,
        user_id: Option<&str>,
        user_display_name: Option<&str>,
    ) -> Result<DeviceIdentity> {
        let user_id = user_id.map(str::to_owned);
        let user_display_name = user_display_name.map(str::to_owned);
        self.database
            .with_connection(move |connection| {
                write_value(connection, KEY_USER_ID, user_id.as_deref())?;
                write_value(
                    connection,
                    KEY_USER_DISPLAY_NAME,
                    user_display_name.as_deref(),
                )?;
                Ok(())
            })
            .await?;
        self.get_identity().await
    }
}

fn read_value(
    connection: &mut rusqlite::Connection,
    key: &str,
) -> rusqlite::Result<Option<String>> {
    connection
        .query_row(
            "SELECT value FROM device_identity WHERE key = ?1 LIMIT 1",
            params![key],
            |row| row.get::<_, String>(0),
        )
        .optional()
}

fn write_value(
    connection: &mut rusqlite::Connection,
    key: &str,
    value: Option<&str>,
) -> rusqlite::Result<()> {
    if let Some(value) = value {
        connection.execute(
            "INSERT INTO device_identity (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
    } else {
        connection.execute("DELETE FROM device_identity WHERE key = ?1", params![key])?;
    }
    Ok(())
}
