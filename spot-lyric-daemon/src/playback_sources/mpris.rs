use std::{collections::HashMap, convert::TryFrom, sync::Arc};

use tokio::sync::Mutex;
use zbus::{zvariant::OwnedValue, Connection, Proxy};

use crate::{
    error::Result,
    spotify::connect_state::ConnectPlaybackSnapshot,
    types::{
        playback::PLAYBACK_SOURCE_MPRIS, Artist, ImageResource, PlaybackState, TrackInfo,
    },
    util::spotify::{extract_track_id, to_hex_track_id},
};

const MPRIS_SERVICE: &str = "org.mpris.MediaPlayer2.spotify";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const MPRIS_PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";

#[derive(Clone, Default)]
pub struct MprisPlaybackSource {
    connection: Arc<Mutex<Option<Connection>>>,
}

impl MprisPlaybackSource {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn fetch_state(&self) -> Result<Option<ConnectPlaybackSnapshot>> {
        let connection = self.connection().await?;
        let proxy = match Proxy::new(
            &connection,
            MPRIS_SERVICE,
            MPRIS_PATH,
            MPRIS_PLAYER_INTERFACE,
        )
        .await
        {
            Ok(proxy) => proxy,
            Err(error) if is_service_unavailable(&error) => return Ok(None),
            Err(error) => return Err(error.into()),
        };

        let playback_status: String = match proxy.get_property("PlaybackStatus").await {
            Ok(status) => status,
            Err(error) if is_service_unavailable(&error) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let metadata: HashMap<String, OwnedValue> = match proxy.get_property("Metadata").await {
            Ok(metadata) => metadata,
            Err(error) if is_service_unavailable(&error) => return Ok(None),
            Err(error) => return Err(error.into()),
        };

        let track = map_track(&metadata);
        let track_uri = track
            .as_ref()
            .and_then(|track| track.uri.clone())
            .or_else(|| metadata_string(&metadata, "xesam:url"))
            .unwrap_or_default();
        if track_uri.is_empty() && track.is_none() {
            return Ok(None);
        }

        let duration_ms = track.as_ref().map(|track| track.duration_ms).unwrap_or_default();
        let mut position_ms = match proxy.get_property::<i64>("Position").await {
            Ok(position_us) => micros_to_millis(position_us),
            Err(_) => 0,
        };
        if duration_ms > 0 {
            position_ms = position_ms.clamp(0, duration_ms);
        } else {
            position_ms = position_ms.max(0);
        }

        let volume = proxy
            .get_property::<f64>("Volume")
            .await
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        let is_playing = playback_status.eq_ignore_ascii_case("playing");

        let state = PlaybackState {
            is_playing,
            track_uri,
            track_name: track
                .as_ref()
                .map(|track| track.name.clone())
                .unwrap_or_default(),
            artist_name: track
                .as_ref()
                .map(|track| {
                    track
                        .artists
                        .iter()
                        .map(|artist| artist.name.as_str())
                        .filter(|name| !name.is_empty())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default(),
            album_name: track
                .as_ref()
                .and_then(|track| track.album_name.clone())
                .unwrap_or_default(),
            album_art_url: track
                .as_ref()
                .and_then(|track| track.images.first().map(|image| image.url.clone()))
                .unwrap_or_default(),
            position_ms,
            duration_ms,
            volume,
            player_status: "ready".into(),
            playback_source: PLAYBACK_SOURCE_MPRIS.into(),
        };

        Ok(Some(ConnectPlaybackSnapshot {
            state,
            track,
            active_device_id: None,
        }))
    }

    async fn connection(&self) -> Result<Connection> {
        let mut guard = self.connection.lock().await;
        if let Some(connection) = guard.as_ref() {
            return Ok(connection.clone());
        }

        let connection = Connection::session().await?;
        *guard = Some(connection.clone());
        Ok(connection)
    }
}

fn is_service_unavailable(error: &zbus::Error) -> bool {
    let text = error.to_string();
    text.contains("NameHasNoOwner") || text.contains("ServiceUnknown")
}

fn map_track(metadata: &HashMap<String, OwnedValue>) -> Option<TrackInfo> {
    let raw_uri = metadata_string(metadata, "xesam:url")
        .or_else(|| metadata_string(metadata, "mpris:trackid"));
    let title = metadata_string(metadata, "xesam:title").unwrap_or_default();
    let album_name = metadata_string(metadata, "xesam:album");
    let artists = metadata_string_list(metadata, "xesam:artist")
        .into_iter()
        .map(|name| Artist {
            id: None,
            uri: None,
            name,
            images: Vec::new(),
        })
        .collect::<Vec<_>>();
    let image_url = metadata_string(metadata, "mpris:artUrl");
    let duration_ms = metadata_i64(metadata, "mpris:length")
        .map(micros_to_millis)
        .unwrap_or_default()
        .max(0);

    if raw_uri.as_deref().unwrap_or_default().is_empty() && title.trim().is_empty() {
        return None;
    }

    let id = raw_uri
        .as_deref()
        .map(extract_track_id)
        .filter(|id| !id.is_empty() && to_hex_track_id(id.as_str()).is_some())
        .unwrap_or_default();
    let uri = if id.is_empty() {
        raw_uri
    } else {
        Some(format!("spotify:track:{id}"))
    };
    let images = image_url
        .map(|url| {
            vec![ImageResource {
                url,
                width: None,
                height: None,
            }]
        })
        .unwrap_or_default();

    Some(TrackInfo {
        added_at: None,
        id: id.clone(),
        hex_id: if id.is_empty() {
            None
        } else {
            to_hex_track_id(&id)
        },
        uri,
        name: title,
        album_name,
        album_id: None,
        album_uri: None,
        artists,
        duration_ms,
        explicit: false,
        playable: true,
        preview_url: None,
        images,
    })
}

fn metadata_string(metadata: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    let value = metadata.get(key)?.clone();
    String::try_from(value)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn metadata_string_list(metadata: &HashMap<String, OwnedValue>, key: &str) -> Vec<String> {
    metadata
        .get(key)
        .cloned()
        .and_then(|value| Vec::<String>::try_from(value).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn metadata_i64(metadata: &HashMap<String, OwnedValue>, key: &str) -> Option<i64> {
    let value = metadata.get(key)?.clone();
    i64::try_from(value.clone())
        .ok()
        .or_else(|| u64::try_from(value).ok().and_then(|value| i64::try_from(value).ok()))
}

fn micros_to_millis(value: i64) -> i64 {
    value.saturating_div(1_000)
}
