use std::str::FromStr;

use serde_json::Value;

use crate::{
    domain::PlaybackDomain,
    error::{DaemonError, Result},
    lyrics_external::{apply_timing_offset, has_usable_lyrics, normalize_search_query},
    lyrics_external::{netease::NeteaseLyricsClient, qq::QqMusicLyricsClient},
    spotify::lyrics_api::LyricsClient,
    storage::LyricsStore,
    types::{
        LyricsCandidate, LyricsLine, LyricsPayload, LyricsSettings, LyricsWord, SavedLyricsMatch,
        StoredLyricsCandidate, TrackInfo,
    },
    util::{
        convert::{decode_candidate_id, to_public_lyrics_candidate, LyricsProviderPreference},
        track_match::{
            build_search_queries, dedupe_candidates, rank_candidates_for_track,
            saved_match_track_context,
        },
    },
};

#[derive(Clone)]
pub struct LyricsDomain {
    lyrics_store: LyricsStore,
    netease: NeteaseLyricsClient,
    playback: PlaybackDomain,
    qq: QqMusicLyricsClient,
    spotify_lyrics: LyricsClient,
}

impl LyricsDomain {
    pub fn new(
        playback: PlaybackDomain,
        lyrics_store: LyricsStore,
        spotify_lyrics: LyricsClient,
        netease: NeteaseLyricsClient,
        qq: QqMusicLyricsClient,
    ) -> Self {
        Self {
            lyrics_store,
            netease,
            playback,
            qq,
            spotify_lyrics,
        }
    }

    pub async fn get_track_lyrics(&self, track_uri: &str) -> Result<LyricsPayload> {
        let settings = self.get_settings(Some(track_uri)).await?;
        let (_, fallback_track_id) = saved_match_track_context(Some(track_uri));

        if let Some(saved_match) = settings.saved_match.as_ref() {
            let payload = self
                .get_lyrics_for_candidate(
                    Some(track_uri),
                    fallback_track_id.as_deref(),
                    &saved_match.candidate,
                )
                .await?;
            let payload = apply_timing_offset(&payload, settings.lyrics_timing_offset_ms);
            if has_usable_lyrics(&payload) {
                return Ok(payload);
            }
        }

        if let Some(track) = self.playback.current_track_for_uri(track_uri).await {
            let ordered_candidates = self
                .search_candidates_for_track(&track, settings.preferred_provider.as_str())
                .await?;
            for candidate in ordered_candidates {
                let payload = self
                    .get_lyrics_for_candidate(Some(track_uri), Some(track.id.as_str()), &candidate)
                    .await?;
                let payload = apply_timing_offset(&payload, settings.lyrics_timing_offset_ms);
                if has_usable_lyrics(&payload) {
                    return Ok(payload);
                }
            }

            return self
                .spotify_or_empty(Some(track_uri), Some(track.id.as_str()))
                .await;
        }

        self.spotify_or_empty(Some(track_uri), fallback_track_id.as_deref())
            .await
    }

    pub async fn get_settings(&self, track_uri: Option<&str>) -> Result<LyricsSettings> {
        let mut settings = self.lyrics_store.get_settings().await?;
        settings.saved_match = self.get_saved_match_for_track_uri(track_uri).await?;
        Ok(settings)
    }

    pub async fn preview_manual_match(
        &self,
        track_uri: Option<&str>,
        candidate_id: &str,
    ) -> Result<LyricsPayload> {
        let candidate = decode_candidate_id(candidate_id)?;
        let settings = self.lyrics_store.get_settings().await?;
        let (track_uri, track_id) = saved_match_track_context(track_uri);
        let payload = self
            .get_lyrics_for_candidate(track_uri, track_id.as_deref(), &candidate)
            .await?;
        Ok(apply_timing_offset(
            &payload,
            settings.lyrics_timing_offset_ms,
        ))
    }

    pub async fn save_manual_match(
        &self,
        track_uri: &str,
        candidate_id: &str,
    ) -> Result<LyricsSettings> {
        let candidate = decode_candidate_id(candidate_id)?;
        let (_, track_id) = saved_match_track_context(Some(track_uri));
        let track_id = track_id.ok_or_else(|| {
            DaemonError::InvalidArgument(format!("could not derive track id from {track_uri}"))
        })?;
        self.lyrics_store
            .save_track_match(track_id.as_str(), &candidate)
            .await?;
        self.get_settings(Some(track_uri)).await
    }

    pub async fn search_manual_matches(&self, query: &str) -> Result<Vec<LyricsCandidate>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let netease_ref = parse_netease_link(query);
        let qq_ref = parse_qq_link(query);
        let normalized_query = normalize_search_query(query);

        let (netease, qq) = tokio::try_join!(
            async {
                if let Some(id) = netease_ref.as_deref() {
                    Ok(self
                        .netease
                        .lookup_track(id)
                        .await?
                        .into_iter()
                        .collect::<Vec<_>>())
                } else {
                    self.netease.search_tracks(normalized_query.as_str()).await
                }
            },
            async {
                if let Some((id, mid)) = qq_ref.as_ref() {
                    Ok(self
                        .qq
                        .lookup_track(id, mid.as_deref())
                        .await?
                        .into_iter()
                        .collect::<Vec<_>>())
                } else {
                    self.qq.search_tracks(normalized_query.as_str()).await
                }
            },
        )?;

        dedupe_candidates(netease.into_iter().chain(qq).collect())
            .into_iter()
            .map(|candidate| to_public_lyrics_candidate(&candidate))
            .collect()
    }

    pub async fn set_preferred_provider(&self, provider: &str) -> Result<LyricsSettings> {
        let provider = LyricsProviderPreference::from_str(provider)?.to_string();
        self.lyrics_store.set_preferred_provider(&provider).await
    }

    pub async fn set_timing_offset_ms(&self, offset_ms: i32) -> Result<LyricsSettings> {
        self.lyrics_store.set_timing_offset_ms(offset_ms).await
    }

    async fn get_saved_match_for_track_uri(
        &self,
        track_uri: Option<&str>,
    ) -> Result<Option<SavedLyricsMatch>> {
        let (_, track_id) = saved_match_track_context(track_uri);
        match track_id {
            Some(track_id) => self.lyrics_store.get_saved_match(track_id.as_str()).await,
            None => Ok(None),
        }
    }

    async fn get_lyrics_for_candidate(
        &self,
        track_uri: Option<&str>,
        track_id: Option<&str>,
        candidate: &StoredLyricsCandidate,
    ) -> Result<LyricsPayload> {
        let payload = match candidate.provider.as_str() {
            "netease" => {
                self.netease
                    .get_lyrics(candidate, track_uri, track_id)
                    .await?
            }
            "qq" => self.qq.get_lyrics(candidate, track_uri, track_id).await?,
            other => {
                return Err(DaemonError::InvalidArgument(format!(
                    "unsupported lyrics provider: {other}"
                )));
            }
        };

        Ok(payload.unwrap_or_else(|| {
            empty_lyrics_payload(track_uri, track_id, candidate.provider.as_str())
        }))
    }

    async fn search_candidates_for_track(
        &self,
        track: &TrackInfo,
        preferred_provider: &str,
    ) -> Result<Vec<StoredLyricsCandidate>> {
        let queries = build_search_queries(track);
        if queries.is_empty() {
            return Ok(Vec::new());
        }

        let (netease, qq) = tokio::try_join!(
            search_provider_all_queries(&self.netease, &queries),
            search_provider_all_queries(&self.qq, &queries),
        )?;
        let all = dedupe_candidates(netease.into_iter().chain(qq).collect());
        let preferred = if preferred_provider == "qq" {
            "qq"
        } else {
            "netease"
        };

        Ok(rank_candidates_for_track(track, all, Some(preferred)))
    }

    async fn spotify_or_empty(
        &self,
        track_uri: Option<&str>,
        track_id: Option<&str>,
    ) -> Result<LyricsPayload> {
        let Some(track_id) = track_id.filter(|track_id| !track_id.trim().is_empty()) else {
            return Ok(empty_lyrics_payload(track_uri, None, "spotify"));
        };

        match self.spotify_lyrics.get_color_lyrics(track_id).await {
            Ok(raw) => Ok(map_spotify_lyrics_payload(track_uri, Some(track_id), &raw)),
            Err(DaemonError::HttpStatus { status: 404, .. }) => {
                Ok(empty_lyrics_payload(track_uri, Some(track_id), "spotify"))
            }
            Err(error) => Err(error),
        }
    }
}

fn empty_lyrics_payload(
    track_uri: Option<&str>,
    track_id: Option<&str>,
    source: &str,
) -> LyricsPayload {
    LyricsPayload {
        track_uri: track_uri.map(str::to_owned),
        track_id: track_id.map(str::to_owned),
        language: None,
        provider: None,
        source: source.into(),
        sync_type: "unsynced".into(),
        lines: Vec::new(),
    }
}

fn map_spotify_lyrics_payload(
    track_uri: Option<&str>,
    track_id: Option<&str>,
    raw: &Value,
) -> LyricsPayload {
    let lyrics = raw.get("lyrics").unwrap_or(&Value::Null);
    LyricsPayload {
        track_uri: track_uri.map(str::to_owned),
        track_id: track_id.map(str::to_owned),
        language: optional_string_at(lyrics, &["language"]),
        provider: optional_string_at(lyrics, &["provider"]),
        source: "spotify".into(),
        sync_type: match optional_string_at(lyrics, &["syncType"]).as_deref() {
            Some("SYLLABLE_SYNCED") => "word",
            Some("LINE_SYNCED") => "line",
            _ => "unsynced",
        }
        .into(),
        lines: lyrics
            .get("lines")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(map_spotify_line)
            .collect(),
    }
}

fn map_spotify_line(line: Value) -> LyricsLine {
    let words: Vec<LyricsWord> = match line.get("words") {
        Some(Value::Array(words)) => words.iter().cloned().map(map_spotify_word).collect(),
        _ => line
            .get("syllables")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(map_spotify_word)
            .collect(),
    };
    let text = optional_string_at(&line, &["text"])
        .or_else(|| line.get("words").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| {
            words
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>()
                .join("")
        });
    LyricsLine {
        text,
        translated_text: None,
        start_time_ms: parse_i64_string_at(&line, &["startTimeMs"]).unwrap_or_default(),
        end_time_ms: parse_i64_string_at(&line, &["endTimeMs"]).unwrap_or_default(),
        words,
    }
}

fn map_spotify_word(word: Value) -> LyricsWord {
    LyricsWord {
        text: optional_string_at(&word, &["string"])
            .or_else(|| optional_string_at(&word, &["text"]))
            .unwrap_or_default(),
        start_time_ms: parse_i64_string_at(&word, &["startTimeMs"]).unwrap_or_default(),
        end_time_ms: parse_i64_string_at(&word, &["endTimeMs"]).unwrap_or_default(),
    }
}

fn optional_string_at(root: &Value, path: &[&str]) -> Option<String> {
    let mut current = root;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_str().map(str::to_owned)
}

fn parse_i64_string_at(root: &Value, path: &[&str]) -> Option<i64> {
    optional_string_at(root, path)?.parse::<i64>().ok()
}

fn parse_netease_link(input: &str) -> Option<String> {
    extract_numeric_after(input, "song?id=").or_else(|| extract_numeric_after(input, "song/"))
}

fn parse_qq_link(input: &str) -> Option<(String, Option<String>)> {
    extract_alphanumeric_after(input, "songDetail/")
        .or_else(|| extract_alphanumeric_after(input, "song/"))
        .map(|id| (id.clone(), Some(id)))
}

fn extract_numeric_after(input: &str, marker: &str) -> Option<String> {
    let index = input.find(marker)? + marker.len();
    let digits: String = input[index..]
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect();
    if digits.len() >= 3 {
        Some(digits)
    } else {
        None
    }
}

fn extract_alphanumeric_after(input: &str, marker: &str) -> Option<String> {
    let index = input.find(marker)? + marker.len();
    let value: String = input[index..]
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric())
        .collect();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

async fn search_provider_all_queries<'a, P>(
    provider: &'a P,
    queries: &'a [String],
) -> Result<Vec<StoredLyricsCandidate>>
where
    P: LyricsSearchProvider,
{
    let mut all = Vec::new();
    for query in queries {
        if let Ok(results) = provider.search_tracks(query).await {
            all.extend(results);
        }
    }
    Ok(all)
}

trait LyricsSearchProvider {
    fn search_tracks(
        &self,
        query: &str,
    ) -> impl std::future::Future<Output = Result<Vec<StoredLyricsCandidate>>> + Send;
}

impl LyricsSearchProvider for NeteaseLyricsClient {
    fn search_tracks(
        &self,
        query: &str,
    ) -> impl std::future::Future<Output = Result<Vec<StoredLyricsCandidate>>> + Send {
        self.search_tracks(query)
    }
}

impl LyricsSearchProvider for QqMusicLyricsClient {
    fn search_tracks(
        &self,
        query: &str,
    ) -> impl std::future::Future<Output = Result<Vec<StoredLyricsCandidate>>> + Send {
        self.search_tracks(query)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manual_provider_links() {
        assert_eq!(
            parse_netease_link("https://music.163.com/#/song?id=123456"),
            Some("123456".into())
        );
        assert_eq!(
            parse_qq_link("https://y.qq.com/n/ryqq/songDetail/003OUlho2HcRHC"),
            Some(("003OUlho2HcRHC".into(), Some("003OUlho2HcRHC".into())))
        );
    }
}
