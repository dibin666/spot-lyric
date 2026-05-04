use std::collections::HashMap;
use std::time::Duration;

use serde_json::Value;

use crate::{
    error::{DaemonError, Result},
    types::{LyricsPayload, StoredLyricsCandidate},
};

use super::{attach_translated_lyrics, parse_lrc_lyrics, request_json, RequestTextOptions};

const NETEASE_API_MAX_ATTEMPTS: usize = 3;
const NETEASE_SEARCH_LIMIT: &str = "50";
const RETRYABLE_NETEASE_CODES: &[i64] = &[405, 509];

#[derive(Clone)]
pub struct NeteaseLyricsClient {
    client: reqwest::Client,
}

impl NeteaseLyricsClient {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    pub async fn lookup_track(&self, id: &str) -> Result<Option<StoredLyricsCandidate>> {
        let mut url =
            reqwest::Url::parse("https://music.163.com/api/song/detail/").expect("valid url");
        url.query_pairs_mut()
            .append_pair("id", id)
            .append_pair("ids", format!("[{id}]").as_str());
        let response = self
            .request_success_json(
                RequestTextOptions {
                    body: None,
                    headers: default_headers(),
                    method: reqwest::Method::GET,
                    url: url.to_string(),
                },
                "load NetEase track detail",
            )
            .await?;
        Ok(
            song_candidates(response.get("songs").and_then(Value::as_array))
                .into_iter()
                .next(),
        )
    }

    pub async fn search_tracks(&self, query: &str) -> Result<Vec<StoredLyricsCandidate>> {
        let normalized_query = super::normalize_search_query(query);
        if normalized_query.is_empty() {
            return Ok(Vec::new());
        }

        let mut url =
            reqwest::Url::parse("https://music.163.com/api/search/get/web").expect("valid url");
        url.query_pairs_mut()
            .append_pair("csrf_token", "")
            .append_pair("s", normalized_query.as_str())
            .append_pair("type", "1")
            .append_pair("offset", "0")
            .append_pair("total", "true")
            .append_pair("limit", NETEASE_SEARCH_LIMIT);
        let response = self
            .request_success_json(
                RequestTextOptions {
                    body: None,
                    headers: default_headers(),
                    method: reqwest::Method::GET,
                    url: url.to_string(),
                },
                "search NetEase tracks",
            )
            .await?;
        Ok(song_candidates(
            response.pointer("/result/songs").and_then(Value::as_array),
        ))
    }

    pub async fn get_lyrics(
        &self,
        candidate: &StoredLyricsCandidate,
        track_uri: Option<&str>,
        track_id: Option<&str>,
    ) -> Result<Option<LyricsPayload>> {
        let mut url =
            reqwest::Url::parse("https://music.163.com/api/song/lyric").expect("valid url");
        url.query_pairs_mut()
            .append_pair("id", candidate.id.as_str())
            .append_pair("lv", "-1")
            .append_pair("kv", "-1")
            .append_pair("tv", "-1");
        let response = self
            .request_success_json(
                RequestTextOptions {
                    body: None,
                    headers: default_headers(),
                    method: reqwest::Method::GET,
                    url: url.to_string(),
                },
                "load NetEase lyrics",
            )
            .await?;
        let lines = parse_lrc_lyrics(
            response
                .pointer("/lrc/lyric")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        if lines.is_empty() {
            return Ok(None);
        }

        let payload = LyricsPayload {
            track_uri: track_uri.map(str::to_owned),
            track_id: track_id.map(str::to_owned),
            language: None,
            provider: Some("Netease Cloud Music".into()),
            source: "netease".into(),
            sync_type: "line".into(),
            lines,
        };
        let translated = parse_lrc_lyrics(
            response
                .pointer("/tlyric/lyric")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        Ok(Some(attach_translated_lyrics(&payload, &translated)))
    }

    async fn request_success_json(
        &self,
        options: RequestTextOptions,
        context: &'static str,
    ) -> Result<Value> {
        for attempt in 0..NETEASE_API_MAX_ATTEMPTS {
            let response: Value = request_json(&self.client, options.clone()).await?;
            match response.get("code").and_then(Value::as_i64) {
                Some(200) | None => return Ok(response),
                Some(code)
                    if RETRYABLE_NETEASE_CODES.contains(&code)
                        && attempt + 1 < NETEASE_API_MAX_ATTEMPTS =>
                {
                    let backoff_ms = 350 * (attempt as u64 + 1);
                    tracing::warn!(
                        code,
                        attempt = attempt + 1,
                        backoff_ms,
                        query_url = %options.url,
                        "NetEase API throttled while trying to {context}; retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                }
                Some(code) => {
                    return Err(DaemonError::InvalidResponse(format!(
                        "NetEase API failed to {context}: code {code}"
                    )));
                }
            }
        }

        Err(DaemonError::InvalidResponse(format!(
            "NetEase API failed to {context}: exhausted retries"
        )))
    }
}

fn default_headers() -> HashMap<String, String> {
    HashMap::from([
        ("referer".into(), "https://music.163.com/".into()),
        ("user-agent".into(), "Mozilla/5.0".into()),
    ])
}

fn map_song(song: &Value) -> Option<StoredLyricsCandidate> {
    let id = song
        .get("id")?
        .as_i64()
        .or_else(|| song.get("id")?.as_str()?.parse::<i64>().ok())?;
    let title = song.get("name")?.as_str()?.to_string();
    let album = song
        .pointer("/album/name")
        .and_then(Value::as_str)
        .or_else(|| song.pointer("/al/name").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();
    let artists = song
        .get("artists")
        .and_then(Value::as_array)
        .or_else(|| song.get("ar").and_then(Value::as_array))
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|artist| {
            artist
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    Some(StoredLyricsCandidate {
        album,
        artists,
        duration_ms: song
            .get("duration")
            .and_then(Value::as_i64)
            .or_else(|| song.get("dt").and_then(Value::as_i64)),
        id: id.to_string(),
        mid: None,
        provider: "netease".into(),
        score: None,
        title,
    })
}

fn song_candidates(songs: Option<&Vec<Value>>) -> Vec<StoredLyricsCandidate> {
    songs
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(map_song)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_netease_codes_include_search_throttling() {
        assert!(RETRYABLE_NETEASE_CODES.contains(&405));
        assert!(RETRYABLE_NETEASE_CODES.contains(&509));
        assert!(!RETRYABLE_NETEASE_CODES.contains(&200));
    }

    #[test]
    fn netease_search_limit_is_expanded_for_match_recall() {
        assert_eq!(NETEASE_SEARCH_LIMIT, "50");
    }
}
