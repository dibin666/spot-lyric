use serde_json::Value;

use crate::error::Result;

use super::{discovery::ProtocolRegistry, transport::SpotifyTransport};

#[derive(Clone)]
pub struct LyricsClient {
    protocol: ProtocolRegistry,
    transport: SpotifyTransport,
}

impl LyricsClient {
    pub fn new(transport: SpotifyTransport, protocol: ProtocolRegistry) -> Self {
        Self {
            protocol,
            transport,
        }
    }

    pub async fn get_color_lyrics(&self, track_id: &str) -> Result<Value> {
        self.transport
            .get_json(
                self.protocol.build_spclient_url(
                    format!(
                        "{}/{}",
                        self.protocol.path_versions.color_lyrics,
                        url::form_urlencoded::byte_serialize(track_id.as_bytes())
                            .collect::<String>()
                    )
                    .as_str(),
                )?,
                None,
            )
            .await
    }
}
