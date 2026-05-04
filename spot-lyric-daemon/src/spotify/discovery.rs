use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use serde::Deserialize;

use crate::error::{DaemonError, Result};

pub const CACHE_TTL_MS: i64 = 15 * 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathfinderQueryName {
    ProfileAttributes,
    FetchEntitiesForRecentlyPlayed,
    FetchPlaylistContents,
    FetchPlaylistMetadata,
    Home,
    LibraryV3,
    FetchLibraryTracks,
    BrowseAll,
    SearchGenres,
    SearchTopResultsList,
    SearchDesktop,
    SearchSuggestions,
    QueryArtistOverview,
    QueryArtistDiscographyAlbums,
    QueryArtistDiscographySingles,
    GetAlbum,
}

#[derive(Debug, Clone)]
pub struct ProtocolConstants {
    pub app_version: String,
    pub app_platform: String,
    pub accept_language: String,
    pub totp_version: u32,
    pub pathfinder_url: String,
    pub open_spotify_token_url: String,
    pub client_token_url: String,
    pub apresolve_url: String,
    pub spclient_base_url: String,
    pub remote_config_path: String,
    pub track_playback_devices_path: String,
    pub connect_state_path: String,
}

impl Default for ProtocolConstants {
    fn default() -> Self {
        Self {
            app_version: "1.2.86.129.gc7526625".into(),
            app_platform: "WebPlayer".into(),
            accept_language: "zh-CN".into(),
            totp_version: 61,
            pathfinder_url: "https://api-partner.spotify.com/pathfinder/v2/query".into(),
            open_spotify_token_url: "https://open.spotify.com/api/token".into(),
            client_token_url: "https://clienttoken.spotify.com/v1/clienttoken".into(),
            apresolve_url: "https://apresolve.spotify.com/?type=dealer-g2&type=spclient".into(),
            spclient_base_url: "https://spclient.wg.spotify.com".into(),
            remote_config_path: "/remote-config-resolver/v3/configuration".into(),
            track_playback_devices_path: "/track-playback/v1/devices".into(),
            connect_state_path: "/connect-state/v1/devices".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PathVersions {
    pub metadata_track: String,
    pub rootlist: String,
    pub recently_played: String,
    pub collection_contains: String,
    pub color_lyrics: String,
}

impl Default for PathVersions {
    fn default() -> Self {
        Self {
            metadata_track: "/metadata/4/track".into(),
            rootlist: "/playlist/v2".into(),
            recently_played: "/recently-played/v3".into(),
            collection_contains: "/collection/v2/contains".into(),
            color_lyrics: "/color-lyrics/v2/track".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct RawDiscoveryResponse {
    #[serde(rename = "dealer-g2")]
    dealer_g2: Option<Vec<String>>,
    spclient: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverySnapshot {
    pub dealer_g2: Vec<String>,
    pub spclient: Vec<String>,
    pub fetched_at: i64,
}

#[derive(Debug, Clone)]
pub struct ProtocolRegistry {
    pub constants: ProtocolConstants,
    pub path_versions: PathVersions,
    pathfinder_hashes: Arc<Mutex<HashMap<PathfinderQueryName, String>>>,
    spclient_base: Arc<Mutex<String>>,
}

impl Default for ProtocolRegistry {
    fn default() -> Self {
        Self {
            constants: ProtocolConstants::default(),
            path_versions: PathVersions::default(),
            pathfinder_hashes: Arc::new(Mutex::new(default_pathfinder_hashes())),
            spclient_base: Arc::new(Mutex::new(ProtocolConstants::default().spclient_base_url)),
        }
    }
}

impl ProtocolRegistry {
    pub fn build_spclient_url(&self, path: &str) -> Result<String> {
        let base = self
            .spclient_base
            .lock()
            .map_err(|_| DaemonError::Poisoned("protocol spclient base".into()))?
            .clone();
        Ok(format!("{}{}", base.trim_end_matches('/'), path))
    }

    pub fn get_hash(&self, query_name: PathfinderQueryName) -> Result<String> {
        self.pathfinder_hashes
            .lock()
            .map_err(|_| DaemonError::Poisoned("protocol pathfinder hashes".into()))?
            .get(&query_name)
            .cloned()
            .ok_or_else(|| {
                DaemonError::InvalidArgument(format!("missing hash for query {query_name:?}"))
            })
    }

    pub fn set_spclient_base(&self, url: &str) -> Result<()> {
        *self
            .spclient_base
            .lock()
            .map_err(|_| DaemonError::Poisoned("protocol spclient base".into()))? =
            url.trim_end_matches('/').to_string();
        Ok(())
    }

    pub fn update_hash(&self, query_name: PathfinderQueryName, hash: &str) -> Result<()> {
        self.pathfinder_hashes
            .lock()
            .map_err(|_| DaemonError::Poisoned("protocol pathfinder hashes".into()))?
            .insert(query_name, hash.to_string());
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct DiscoveryService {
    client: reqwest::Client,
    protocol: ProtocolRegistry,
    cached: Arc<tokio::sync::RwLock<Option<DiscoverySnapshot>>>,
}

impl DiscoveryService {
    pub fn new(client: reqwest::Client, protocol: ProtocolRegistry) -> Self {
        Self {
            client,
            protocol,
            cached: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }

    pub async fn get_endpoints(&self, force_refresh: bool) -> Result<DiscoverySnapshot> {
        if !force_refresh {
            if let Some(snapshot) = self.cached.read().await.clone() {
                if now_millis() - snapshot.fetched_at < CACHE_TTL_MS {
                    return Ok(snapshot);
                }
            }
        }

        let payload = self
            .client
            .get(&self.protocol.constants.apresolve_url)
            .header("accept", "application/json")
            .send()
            .await?;

        if !payload.status().is_success() {
            return Err(DaemonError::HttpStatus {
                status: payload.status().as_u16(),
                method: "GET".into(),
                url: self.protocol.constants.apresolve_url.clone(),
                response_text: payload.text().await.unwrap_or_default(),
            });
        }

        let decoded: RawDiscoveryResponse = payload.json().await?;
        let snapshot = DiscoverySnapshot {
            dealer_g2: decoded.dealer_g2.unwrap_or_default(),
            spclient: decoded.spclient.unwrap_or_default(),
            fetched_at: now_millis(),
        };

        if let Some(base) = snapshot.spclient.first() {
            self.protocol
                .set_spclient_base(format!("https://{}", base).as_str())?;
        }

        *self.cached.write().await = Some(snapshot.clone());
        Ok(snapshot)
    }

    pub async fn get_dealer_url(&self, force_refresh: bool) -> Result<String> {
        let snapshot = self.get_endpoints(force_refresh).await?;
        let endpoint = snapshot
            .dealer_g2
            .first()
            .ok_or_else(|| DaemonError::InvalidResponse("dealer-g2 not resolved".into()))?;
        Ok(format!("wss://{}/", endpoint.trim_end_matches('/')))
    }

    pub async fn get_spclient_base_url(&self, force_refresh: bool) -> Result<String> {
        let snapshot = self.get_endpoints(force_refresh).await?;
        let endpoint = snapshot
            .spclient
            .first()
            .ok_or_else(|| DaemonError::InvalidResponse("spclient not resolved".into()))?;
        Ok(format!("https://{}", endpoint.trim_end_matches('/')))
    }
}

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn default_pathfinder_hashes() -> HashMap<PathfinderQueryName, String> {
    HashMap::from([
        (
            PathfinderQueryName::ProfileAttributes,
            "53bcb064f6cd18c23f752bc324a791194d20df612d8e1239c735144ab0399ced".into(),
        ),
        (
            PathfinderQueryName::FetchEntitiesForRecentlyPlayed,
            "5bb408450626d595cb24363104b612e14f9b966430f599121696e8996ea03794".into(),
        ),
        (
            PathfinderQueryName::FetchPlaylistContents,
            "9c53fb83f35c6a177be88bf1b67cb080b853e86b576ed174216faa8f9164fc8f".into(),
        ),
        (
            PathfinderQueryName::FetchPlaylistMetadata,
            "9c53fb83f35c6a177be88bf1b67cb080b853e86b576ed174216faa8f9164fc8f".into(),
        ),
        (
            PathfinderQueryName::Home,
            "23e37f2e58d82d567f27080101d36609009d8c3676457b1086cb0acc55b72a5d".into(),
        ),
        (
            PathfinderQueryName::LibraryV3,
            "973e511ca44261fda7eebac8b653155e7caee3675abb4fb110cc1b8c78b091c3".into(),
        ),
        (
            PathfinderQueryName::FetchLibraryTracks,
            "087278b20b743578a6262c2b0b4bcd20d879c503cc359a2285baf083ef944240".into(),
        ),
        (
            PathfinderQueryName::BrowseAll,
            "dbd8b55e09a58afc52eab438bc228ba28fd72ac2f2148c6c26354980e4579001".into(),
        ),
        (
            PathfinderQueryName::SearchGenres,
            "9e1c0e056c46239dd1956ea915b988913c87c04ce3dadccdb537774490266f46".into(),
        ),
        (
            PathfinderQueryName::SearchTopResultsList,
            "18af0867c6022d717995f873cc27c0e85f6105f404f0035429d1107864b9d220".into(),
        ),
        (
            PathfinderQueryName::SearchDesktop,
            "3c9d3f60dac5dea3876b6db3f534192b1c1d90032c4233c1bbaba526db41eb31".into(),
        ),
        (
            PathfinderQueryName::SearchSuggestions,
            "9fe3ad78e43a1684b3a9fabc741c5928928d4d30d7d8fd7fd193c7ebb4a544f4".into(),
        ),
        (
            PathfinderQueryName::QueryArtistOverview,
            "dd14c6043d8127b56c5acbe534f6b3c58714f0c26bc6ad41776079ed52833a8f".into(),
        ),
        (
            PathfinderQueryName::QueryArtistDiscographyAlbums,
            "5e07d323febb57b4a56a42abbf781490e58764aa45feb6e3dc0591564fc56599".into(),
        ),
        (
            PathfinderQueryName::QueryArtistDiscographySingles,
            "5e07d323febb57b4a56a42abbf781490e58764aa45feb6e3dc0591564fc56599".into(),
        ),
        (
            PathfinderQueryName::GetAlbum,
            "b9bfabef66ed756e5e13f68a942deb60bd4125ec1f1be8cc42769dc0259b4b10".into(),
        ),
    ])
}
