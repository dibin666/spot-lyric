use std::collections::HashMap;

use base64::Engine as _;
use serde_json::Value;

use crate::{
    error::Result,
    types::{LyricsPayload, StoredLyricsCandidate},
};

use super::{
    attach_translated_lyrics, parse_lrc_lyrics, request_json, request_raw_text, RequestTextOptions,
};

#[derive(Clone)]
pub struct QqMusicLyricsClient {
    client: reqwest::Client,
}

impl QqMusicLyricsClient {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    pub async fn lookup_track(
        &self,
        id: &str,
        mid: Option<&str>,
    ) -> Result<Option<StoredLyricsCandidate>> {
        let body = reqwest::Url::parse_with_params(
            "https://dummy.local/",
            [
                (
                    if mid.is_some() { "songmid" } else { "songid" },
                    mid.unwrap_or(id),
                ),
                ("callback", "getOneSongInfoCallback"),
                ("format", "jsonp"),
                ("g_tk", "5381"),
                ("hostUin", "0"),
                ("jsonpCallback", "getOneSongInfoCallback"),
                ("loginUin", "0"),
                ("needNewCode", "0"),
                ("notice", "0"),
                ("outCharset", "utf8"),
                ("platform", "yqq"),
                ("tpl", "yqq_song_detail"),
            ],
        )
        .expect("valid params")
        .query()
        .unwrap_or_default()
        .to_string();
        let response = parse_jsonp(
            request_raw_text(
                &self.client,
                RequestTextOptions {
                    body: Some(body),
                    headers: detail_headers(),
                    method: reqwest::Method::POST,
                    url: "https://c.y.qq.com/v8/fcg-bin/fcg_play_single_song.fcg".into(),
                },
            )
            .await?
            .as_str(),
        )?;
        Ok(
            song_candidates(response.get("data").and_then(Value::as_array))
                .into_iter()
                .next(),
        )
    }

    pub async fn search_tracks(&self, query: &str) -> Result<Vec<StoredLyricsCandidate>> {
        let normalized_query = super::normalize_search_query(query);
        if normalized_query.is_empty() {
            return Ok(Vec::new());
        }

        let response: Value = request_json(
            &self.client,
            RequestTextOptions {
                body: Some(
                    serde_json::json!({
                        "req_1": {
                            "method": "DoSearchForQQMusicDesktop",
                            "module": "music.search.SearchCgiService",
                            "param": {
                                "num_per_page": 20,
                                "page_num": 1,
                                "query": normalized_query,
                                "search_type": 0,
                            }
                        }
                    })
                    .to_string(),
                ),
                headers: search_headers(),
                method: reqwest::Method::POST,
                url: "https://u.y.qq.com/cgi-bin/musicu.fcg".into(),
            },
        )
        .await?;
        Ok(song_candidates(
            response
                .pointer("/req_1/data/body/song/list")
                .and_then(Value::as_array),
        ))
    }

    pub async fn get_lyrics(
        &self,
        candidate: &StoredLyricsCandidate,
        track_uri: Option<&str>,
        track_id: Option<&str>,
    ) -> Result<Option<LyricsPayload>> {
        if candidate.mid.is_none() {
            return Ok(None);
        }

        let response: Value = request_json(
            &self.client,
            RequestTextOptions {
                body: Some(build_play_lyric_info_request(candidate)),
                headers: search_headers(),
                method: reqwest::Method::POST,
                url: "https://u.y.qq.com/cgi-bin/musicu.fcg".into(),
            },
        )
        .await?;
        let lines = parse_lrc_lyrics(
            decode_base64_lyric(
                response
                    .pointer("/req_1/data/lyric")
                    .and_then(Value::as_str),
            )
            .as_str(),
        );
        if lines.is_empty() {
            return Ok(None);
        }

        let payload = LyricsPayload {
            track_uri: track_uri.map(str::to_owned),
            track_id: track_id.map(str::to_owned),
            language: None,
            provider: Some("QQ Music".into()),
            source: "qq".into(),
            sync_type: "line".into(),
            lines,
        };
        let translated = parse_lrc_lyrics(
            decode_base64_lyric(
                response
                    .pointer("/req_1/data/trans")
                    .and_then(Value::as_str),
            )
            .as_str(),
        );
        Ok(Some(attach_translated_lyrics(&payload, &translated)))
    }
}

fn build_play_lyric_info_request(candidate: &StoredLyricsCandidate) -> String {
    serde_json::json!({
        "comm": {
            "ct": 24,
            "cv": 4_747_474,
            "format": "json",
            "g_tk": "5381",
            "g_tk_new_20200303": "5381",
            "inCharset": "utf-8",
            "needNewCode": 1,
            "notice": 0,
            "outCharset": "utf-8",
            "platform": "yqq.json",
            "uin": 0,
        },
        "req_1": {
            "method": "GetPlayLyricInfo",
            "module": "music.musichallSong.PlayLyricInfo",
            "param": {
                "qrc": 0,
                "qrc_t": 0,
                "roma": 0,
                "songID": candidate.id.parse::<i64>().unwrap_or_default(),
                "songMID": candidate.mid,
                "trans": 1,
            }
        }
    })
    .to_string()
}

fn decode_base64_lyric(value: Option<&str>) -> String {
    value
        .map(|raw| {
            String::from_utf8(
                base64::engine::general_purpose::STANDARD
                    .decode(raw)
                    .unwrap_or_default(),
            )
            .unwrap_or_default()
        })
        .unwrap_or_default()
}

fn detail_headers() -> HashMap<String, String> {
    let mut headers = qq_headers();
    headers.insert(
        "content-type".into(),
        "application/x-www-form-urlencoded; charset=UTF-8".into(),
    );
    headers
}

fn search_headers() -> HashMap<String, String> {
    let mut headers = qq_headers();
    headers.insert("content-type".into(), "application/json".into());
    headers
}

fn qq_headers() -> HashMap<String, String> {
    HashMap::from([
        ("referer".into(), "https://c.y.qq.com/".into()),
        ("user-agent".into(), "Mozilla/5.0".into()),
    ])
}

fn parse_jsonp(source: &str) -> Result<Value> {
    let start = source.find('(').ok_or_else(|| {
        crate::error::DaemonError::InvalidResponse("invalid QQ JSONP response".into())
    })?;
    let end = source.rfind(')').ok_or_else(|| {
        crate::error::DaemonError::InvalidResponse("invalid QQ JSONP response".into())
    })?;
    Ok(serde_json::from_str(&source[start + 1..end])?)
}

fn map_song(song: &Value) -> Option<StoredLyricsCandidate> {
    let id = song
        .get("id")?
        .as_i64()
        .or_else(|| song.get("id")?.as_str()?.parse::<i64>().ok())?;
    let mid = song.get("mid")?.as_str()?.to_string();
    let title = song.get("title")?.as_str()?.to_string();
    Some(StoredLyricsCandidate {
        album: song
            .pointer("/album/title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        artists: song
            .get("singer")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|artist| {
                artist
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect(),
        duration_ms: song
            .get("interval")
            .and_then(Value::as_i64)
            .map(|seconds| seconds * 1_000),
        id: id.to_string(),
        mid: Some(mid),
        provider: "qq".into(),
        score: None,
        title,
    })
}

fn song_candidates(songs: Option<&Vec<Value>>) -> Vec<StoredLyricsCandidate> {
    let mut results = Vec::new();
    for song in songs.cloned().unwrap_or_default() {
        if let Some(mapped) = map_song(&song) {
            results.push(mapped);
        }
        if let Some(group) = song.get("group").and_then(Value::as_array) {
            results.extend(song_candidates(Some(group)));
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_mid_returns_no_qq_lyrics() {
        let client = QqMusicLyricsClient::new(reqwest::Client::new());
        let candidate = StoredLyricsCandidate {
            album: String::new(),
            artists: Vec::new(),
            duration_ms: None,
            id: "123".into(),
            mid: None,
            provider: "qq".into(),
            score: None,
            title: "Song".into(),
        };

        let payload = client.get_lyrics(&candidate, None, None).await.unwrap();
        assert!(payload.is_none());
    }
}
