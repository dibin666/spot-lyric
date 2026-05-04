use std::{collections::HashMap, time::Duration};

use serde::de::DeserializeOwned;

use crate::{
    error::{DaemonError, Result},
    types::{LyricsLine, LyricsPayload, LyricsWord},
};

pub mod netease;
pub mod qq;

const DEFAULT_TIMEOUT_MS: u64 = 12_000;
const LEADING_CREDITS_PATTERNS: &[&str] = &[
    "作词",
    "作曲",
    "编曲",
    "词",
    "曲",
    "制作人",
    "监制",
    "lyrics",
    "composer",
    "arranger",
    "producer",
];
const MAX_RETRIES: usize = 2;
const MAX_TRANSLATION_GAP_MS: i64 = 800;
pub const DEFAULT_TIMING_OFFSET_MS: i32 = 0;
const MAX_TIMING_OFFSET_MS: i32 = 5_000;

#[derive(Debug, Clone)]
pub struct RequestTextOptions {
    pub body: Option<String>,
    pub headers: HashMap<String, String>,
    pub method: reqwest::Method,
    pub url: String,
}

pub fn apply_timing_offset(payload: &LyricsPayload, offset_ms: i32) -> LyricsPayload {
    let normalized_offset = normalize_timing_offset_ms(offset_ms) as i64;
    if normalized_offset == 0
        || payload.source == "spotify"
        || payload.sync_type == "unsynced"
        || payload.lines.is_empty()
    {
        return payload.clone();
    }

    LyricsPayload {
        lines: payload
            .lines
            .iter()
            .map(|line| LyricsLine {
                text: line.text.clone(),
                translated_text: line.translated_text.clone(),
                start_time_ms: line.start_time_ms + normalized_offset,
                end_time_ms: line.end_time_ms + normalized_offset,
                words: line
                    .words
                    .iter()
                    .map(|word| LyricsWord {
                        text: word.text.clone(),
                        start_time_ms: word.start_time_ms + normalized_offset,
                        end_time_ms: word.end_time_ms + normalized_offset,
                    })
                    .collect(),
            })
            .collect(),
        ..payload.clone()
    }
}

pub fn attach_translated_lyrics(
    payload: &LyricsPayload,
    translated_lines: &[LyricsLine],
) -> LyricsPayload {
    let anchor_index = payload
        .lines
        .iter()
        .position(|line| !is_leading_credits_line(line.text.as_str()));

    LyricsPayload {
        lines: payload
            .lines
            .iter()
            .enumerate()
            .map(|(index, line)| LyricsLine {
                translated_text: anchor_index
                    .filter(|anchor| {
                        index >= *anchor && !is_leading_credits_line(line.text.as_str())
                    })
                    .and_then(|_| {
                        find_translated_line(line, translated_lines)
                            .map(|translated| translated.text.clone())
                    }),
                ..line.clone()
            })
            .collect(),
        ..payload.clone()
    }
}

pub fn has_usable_lyrics(payload: &LyricsPayload) -> bool {
    payload
        .lines
        .iter()
        .any(|line| !line.text.trim().is_empty())
}

pub fn normalize_search_query(value: &str) -> String {
    value
        .chars()
        .map(|character: char| {
            if character.is_alphanumeric() || character.is_whitespace() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn normalize_timing_offset_ms(value: i32) -> i32 {
    value.clamp(-MAX_TIMING_OFFSET_MS, MAX_TIMING_OFFSET_MS)
}

pub fn parse_lrc_lyrics(input: &str) -> Vec<LyricsLine> {
    let mut parsed = Vec::new();
    let mut offset_ms = 0i64;

    for raw_line in input
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
    {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(parsed_offset) = parse_offset_metadata(line) {
            offset_ms = parsed_offset;
            continue;
        }
        if is_metadata_line(line) {
            continue;
        }

        let timestamps = extract_timestamps(line);
        if timestamps.is_empty() {
            continue;
        }
        let text_start = timestamps.last().map(|(end, _)| *end).unwrap_or(0);
        let text = line[text_start..].trim();
        if text.is_empty() || text == "//" {
            continue;
        }

        for (_, start_time_ms) in timestamps {
            parsed.push(LyricsLine {
                text: text.to_string(),
                translated_text: None,
                start_time_ms,
                end_time_ms: start_time_ms,
                words: Vec::new(),
            });
        }
    }

    parsed.sort_by_key(|line| line.start_time_ms);
    let finalized = finalize_line_end_times(parsed);
    if offset_ms == 0 {
        finalized
    } else {
        finalized
            .into_iter()
            .map(|line| LyricsLine {
                start_time_ms: line.start_time_ms - offset_ms,
                end_time_ms: line.end_time_ms - offset_ms,
                ..line
            })
            .collect()
    }
}

pub async fn request_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    options: RequestTextOptions,
) -> Result<T> {
    let text = request_text(client, options, 0).await?;
    Ok(serde_json::from_str(&text)?)
}

pub async fn request_raw_text(
    client: &reqwest::Client,
    options: RequestTextOptions,
) -> Result<String> {
    request_text(client, options, 0).await
}

fn extract_timestamps(line: &str) -> Vec<(usize, i64)> {
    let mut timestamps = Vec::new();
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'[' {
            index += 1;
            continue;
        }
        let Some(end_rel) = line[index..].find(']') else {
            break;
        };
        let end = index + end_rel;
        let content = &line[index + 1..end];
        if let Some(timestamp) = parse_timestamp(content) {
            timestamps.push((end + 1, timestamp));
        }
        index = end + 1;
    }
    timestamps
}

fn finalize_line_end_times(lines: Vec<LyricsLine>) -> Vec<LyricsLine> {
    let mut finalized = lines;
    for index in 0..finalized.len() {
        if finalized[index].end_time_ms <= finalized[index].start_time_ms {
            let next_start = finalized
                .get(index + 1)
                .map(|line| line.start_time_ms)
                .unwrap_or(finalized[index].start_time_ms);
            finalized[index].end_time_ms = next_start;
        }
    }
    finalized
}

fn find_translated_line<'a>(
    line: &LyricsLine,
    translated_lines: &'a [LyricsLine],
) -> Option<&'a LyricsLine> {
    translated_lines.iter().find(|translated| {
        (translated.start_time_ms - line.start_time_ms).abs() <= MAX_TRANSLATION_GAP_MS
    })
}

fn is_leading_credits_line(text: &str) -> bool {
    let normalized = text.trim().to_lowercase();
    LEADING_CREDITS_PATTERNS
        .iter()
        .any(|pattern| normalized.starts_with(&pattern.to_lowercase()))
}

fn is_metadata_line(line: &str) -> bool {
    line.starts_with('{')
        || (line.starts_with('[')
            && line[1..]
                .chars()
                .take_while(|character| *character != ':')
                .all(|character| character.is_ascii_alphabetic()))
}

fn parse_offset_metadata(line: &str) -> Option<i64> {
    let offset = line.strip_prefix("[offset:")?.strip_suffix(']')?;
    offset.parse::<i64>().ok()
}

fn parse_timestamp(value: &str) -> Option<i64> {
    let (minutes, rest) = value.split_once(':')?;
    let (seconds, millis) = match rest.split_once('.') {
        Some((seconds, millis)) => (seconds, millis),
        None => (rest, "0"),
    };
    let millis = format!("{:0<3}", millis)
        .chars()
        .take(3)
        .collect::<String>()
        .parse::<i64>()
        .ok()?;
    Some(minutes.parse::<i64>().ok()? * 60_000 + seconds.parse::<i64>().ok()? * 1_000 + millis)
}

async fn request_text(
    client: &reqwest::Client,
    options: RequestTextOptions,
    attempt: usize,
) -> Result<String> {
    let mut request = client
        .request(options.method.clone(), &options.url)
        .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS));
    for (key, value) in &options.headers {
        request = request.header(key, value);
    }
    if let Some(body) = &options.body {
        request = request.body(body.clone());
    }

    match request.send().await {
        Ok(response) if response.status().is_success() => Ok(response.text().await?),
        Ok(response) => {
            if attempt >= MAX_RETRIES {
                Err(DaemonError::HttpStatus {
                    status: response.status().as_u16(),
                    method: options.method.to_string(),
                    url: options.url,
                    response_text: response.text().await.unwrap_or_default(),
                })
            } else {
                tokio::time::sleep(Duration::from_millis(300 * (attempt as u64 + 1))).await;
                Box::pin(request_text(client, options, attempt + 1)).await
            }
        }
        Err(error) => {
            if attempt >= MAX_RETRIES {
                Err(error.into())
            } else {
                tokio::time::sleep(Duration::from_millis(300 * (attempt as u64 + 1))).await;
                Box::pin(request_text(client, options, attempt + 1)).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lrc_lyrics_keeps_timestamps_and_translation_alignment() {
        let primary = parse_lrc_lyrics("[00:01.00]Line one\n[00:02.00]Line two");
        let translated = parse_lrc_lyrics("[00:01.10]第一行\n[00:02.05]第二行");
        let merged = attach_translated_lyrics(
            &LyricsPayload {
                track_uri: None,
                track_id: Some("track-1".into()),
                language: None,
                provider: None,
                source: "netease".into(),
                sync_type: "line".into(),
                lines: primary,
            },
            &translated,
        );
        assert_eq!(merged.lines[0].translated_text.as_deref(), Some("第一行"));
        assert_eq!(merged.lines[1].translated_text.as_deref(), Some("第二行"));
    }

    #[test]
    fn normalize_search_query_strips_unicode_punctuation() {
        assert_eq!(
            normalize_search_query("神っぽいな／God-ish【English ver.】"),
            "神っぽいな God ish English ver"
        );
    }
}
