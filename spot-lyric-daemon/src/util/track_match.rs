use std::cmp::Ordering;

use crate::{
    types::{StoredLyricsCandidate, TrackInfo},
    util::spotify::{hex_to_base62, is_hex_track_id, resolve_track_identity, TrackIdentity},
};

const AUTO_MATCH_DURATION_TOLERANCE_MS: i64 = 1_000;
const COMMENT_MATCH_DURATION_TOLERANCE_MS: i64 = 12_000;
const MAX_AUTO_MATCH_QUERIES: usize = 6;
const MAX_COMMENT_MATCH_QUERIES: usize = 10;
const VERSION_SUFFIX_HINTS: &[&str] = &[
    "acoustic",
    "bonus",
    "clean",
    "cover",
    "demo",
    "deluxe",
    "edit",
    "english ver",
    "explicit",
    "from ",
    "instrumental",
    "japanese ver",
    "karaoke",
    "live",
    "mix",
    "mono",
    "remaster",
    "remastered",
    "romanized",
    "sped up",
    "stereo",
    "tv size",
    "ver",
    "version",
    "伴奏",
    "加速版",
    "原唱",
    "现场",
    "翻唱",
];

pub fn saved_match_track_context(track_uri: Option<&str>) -> (Option<&str>, Option<String>) {
    let track_uri = track_uri
        .map(str::trim)
        .filter(|track_uri| !track_uri.is_empty());
    let track_id = track_uri.and_then(|track_uri| {
        let TrackIdentity { id, hex_id } = resolve_track_identity(None, Some(track_uri), None);
        let track_id = id.filter(|track_id| !track_id.is_empty() && !is_hex_track_id(track_id));
        hex_id.and_then(|hex_id| track_id.or_else(|| hex_to_base62(hex_id.as_str())))
    });
    (track_uri, track_id)
}

pub fn build_search_queries(track: &TrackInfo) -> Vec<String> {
    build_search_queries_with_limit(track, MAX_AUTO_MATCH_QUERIES, false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoMatchStrategy {
    TitleDuration,
    TitleArtistDuration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoMatchQueryGroup {
    pub strategy: AutoMatchStrategy,
    pub queries: Vec<String>,
}

pub fn build_auto_match_query_groups(track: &TrackInfo) -> Vec<AutoMatchQueryGroup> {
    let title_variants = title_search_variants(track.name.as_str(), false);
    if title_variants.is_empty() {
        return Vec::new();
    }

    let mut title_queries = Vec::new();
    for title in &title_variants {
        push_query_variant(&mut title_queries, title);
    }

    let mut title_artist_queries = Vec::new();
    let artist_queries = auto_match_artist_queries(track);
    for title in &title_variants {
        for artist in &artist_queries {
            push_query_variant(
                &mut title_artist_queries,
                format!("{title} {artist}").as_str(),
            );
        }
    }

    vec![
        AutoMatchQueryGroup {
            strategy: AutoMatchStrategy::TitleDuration,
            queries: title_queries,
        },
        AutoMatchQueryGroup {
            strategy: AutoMatchStrategy::TitleArtistDuration,
            queries: title_artist_queries,
        },
    ]
}

pub fn build_comment_search_queries(track: &TrackInfo) -> Vec<String> {
    build_search_queries_with_limit(track, MAX_COMMENT_MATCH_QUERIES, true)
}

fn build_search_queries_with_limit(
    track: &TrackInfo,
    max_queries: usize,
    include_original_version_variant: bool,
) -> Vec<String> {
    let title_variants =
        title_search_variants(track.name.as_str(), include_original_version_variant);
    if title_variants.is_empty() {
        return Vec::new();
    }

    let primary_artist_query = normalize_search_request_query(
        track
            .artists
            .first()
            .map(|artist| artist.name.as_str())
            .unwrap_or_default(),
    );
    let artist_query = normalize_search_request_query(
        track
            .artists
            .iter()
            .map(|artist| artist.name.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .as_str(),
    );
    let album_query =
        normalize_search_request_query(track.album_name.as_deref().unwrap_or_default());
    let mut queries = Vec::new();
    for title in title_variants {
        for query in [
            format!("{title} {primary_artist_query}"),
            format!("{title} {artist_query}"),
            format!("{title} {album_query}"),
            title.clone(),
        ] {
            let normalized = normalize_search_request_query(query.as_str());
            if !normalized.is_empty() && !queries.contains(&normalized) {
                queries.push(normalized);
                if queries.len() >= max_queries {
                    return queries;
                }
            }
        }
    }
    queries
}

pub fn dedupe_candidates(candidates: Vec<StoredLyricsCandidate>) -> Vec<StoredLyricsCandidate> {
    let mut seen = std::collections::HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| {
            seen.insert(format!(
                "{}:{}:{}",
                candidate.provider,
                candidate.id,
                candidate.mid.as_deref().unwrap_or_default()
            ))
        })
        .collect()
}

pub fn rank_candidates_for_track(
    track: &TrackInfo,
    candidates: Vec<StoredLyricsCandidate>,
    preferred_provider: Option<&str>,
) -> Vec<StoredLyricsCandidate> {
    rank_candidates_for_track_with_options(
        track,
        candidates,
        preferred_provider,
        TrackMatchOptions {
            duration_tolerance_ms: AUTO_MATCH_DURATION_TOLERANCE_MS,
            filter_duration_mismatch: true,
        },
    )
}

pub fn rank_auto_match_candidates_for_track(
    track: &TrackInfo,
    candidates: Vec<StoredLyricsCandidate>,
    preferred_provider: Option<&str>,
    strategy: AutoMatchStrategy,
) -> Vec<StoredLyricsCandidate> {
    let mut ranked: Vec<_> = candidates
        .into_iter()
        .filter_map(|candidate| {
            strict_auto_match_rank(track, &candidate, preferred_provider, strategy)
                .map(|rank| (rank, candidate))
        })
        .collect();

    ranked.sort_by(|left, right| left.0.cmp(&right.0));
    ranked.into_iter().map(|(_, candidate)| candidate).collect()
}

pub fn rank_comment_candidates_for_track(
    track: &TrackInfo,
    candidates: Vec<StoredLyricsCandidate>,
    preferred_provider: Option<&str>,
) -> Vec<StoredLyricsCandidate> {
    rank_candidates_for_track_with_options(
        track,
        candidates,
        preferred_provider,
        TrackMatchOptions {
            duration_tolerance_ms: COMMENT_MATCH_DURATION_TOLERANCE_MS,
            filter_duration_mismatch: false,
        },
    )
}

fn rank_candidates_for_track_with_options(
    track: &TrackInfo,
    candidates: Vec<StoredLyricsCandidate>,
    preferred_provider: Option<&str>,
    options: TrackMatchOptions,
) -> Vec<StoredLyricsCandidate> {
    let mut ranked: Vec<_> = candidates
        .into_iter()
        .filter(|candidate| {
            !options.filter_duration_mismatch
                || is_duration_match_within_tolerance(
                    track.duration_ms,
                    candidate.duration_ms.unwrap_or_default(),
                    options.duration_tolerance_ms,
                )
        })
        .map(|candidate| {
            let score = score_match(track, &candidate, options.duration_tolerance_ms);
            (
                preferred_provider.is_some_and(|provider| candidate.provider == provider),
                score,
                candidate,
            )
        })
        .collect();
    ranked.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| right.1.partial_cmp(&left.1).unwrap_or(Ordering::Equal))
    });
    ranked
        .into_iter()
        .map(|(_, _, candidate)| candidate)
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct TrackMatchOptions {
    duration_tolerance_ms: i64,
    filter_duration_mismatch: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct StrictAutoMatchRank {
    preferred_provider_rank: u8,
    duration_delta_ms: i64,
    provider_score_rank: i64,
    title: String,
    id: String,
}

impl Eq for StrictAutoMatchRank {}

impl Ord for StrictAutoMatchRank {
    fn cmp(&self, other: &Self) -> Ordering {
        self.preferred_provider_rank
            .cmp(&other.preferred_provider_rank)
            .then_with(|| self.duration_delta_ms.cmp(&other.duration_delta_ms))
            .then_with(|| self.provider_score_rank.cmp(&other.provider_score_rank))
            .then_with(|| self.title.cmp(&other.title))
            .then_with(|| self.id.cmp(&other.id))
    }
}

impl PartialOrd for StrictAutoMatchRank {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn strict_auto_match_rank(
    track: &TrackInfo,
    candidate: &StoredLyricsCandidate,
    preferred_provider: Option<&str>,
    strategy: AutoMatchStrategy,
) -> Option<StrictAutoMatchRank> {
    let duration_ms = candidate.duration_ms?;
    let duration_delta_ms = strict_duration_delta(track.duration_ms, duration_ms)?;
    if !title_matches_track(track.name.as_str(), candidate.title.as_str()) {
        return None;
    }
    if strategy == AutoMatchStrategy::TitleArtistDuration
        && !artist_matches_track(track, candidate.artists.as_slice())
    {
        return None;
    }

    Some(StrictAutoMatchRank {
        preferred_provider_rank: if preferred_provider
            .is_some_and(|provider| candidate.provider.as_str() == provider)
        {
            0
        } else {
            1
        },
        duration_delta_ms,
        provider_score_rank: provider_score_rank(candidate.score),
        title: candidate.title.clone(),
        id: candidate.id.clone(),
    })
}

fn strict_duration_delta(expected: i64, actual: i64) -> Option<i64> {
    if expected <= 0 || actual <= 0 {
        return None;
    }
    let delta = candidate_duration_delta_ms(expected, actual)?;
    if delta <= AUTO_MATCH_DURATION_TOLERANCE_MS {
        Some(delta)
    } else {
        None
    }
}

fn candidate_duration_delta_ms(expected_ms: i64, actual: i64) -> Option<i64> {
    if actual <= 0 {
        return None;
    }

    if expected_ms >= 30_000 && actual < 10_000 {
        duration_delta_to_second_precision_interval(expected_ms, actual)
    } else if actual % 1_000 == 0 {
        duration_delta_to_second_precision_interval(expected_ms, actual / 1_000)
    } else {
        Some((expected_ms - actual).abs())
    }
}

fn duration_delta_to_second_precision_interval(
    expected_ms: i64,
    actual_seconds: i64,
) -> Option<i64> {
    let start = actual_seconds.checked_mul(1_000)?;
    let end = start.checked_add(999)?;
    if expected_ms < start {
        Some(start - expected_ms)
    } else if expected_ms > end {
        Some(expected_ms - end)
    } else {
        Some(0)
    }
}

fn provider_score_rank(score: Option<f64>) -> i64 {
    score
        .filter(|score| score.is_finite())
        .map(|score| (score * -1_000_000.0).round() as i64)
        .unwrap_or(i64::MAX)
}

fn title_matches_track(expected: &str, actual: &str) -> bool {
    let expected_variants = normalized_title_match_variants(expected);
    let actual_variants = normalized_title_match_variants(actual);
    expected_variants
        .iter()
        .any(|expected| actual_variants.iter().any(|actual| actual == expected))
}

fn artist_matches_track(track: &TrackInfo, actual_artists: &[String]) -> bool {
    let expected: std::collections::HashSet<_> = track
        .artists
        .iter()
        .flat_map(|artist| normalized_artist_match_variants(artist.name.as_str()))
        .collect();
    if expected.is_empty() {
        return false;
    }

    actual_artists
        .iter()
        .flat_map(|artist| normalized_artist_match_variants(artist.as_str()))
        .any(|artist| expected.contains(&artist))
}

fn normalized_title_match_variants(title: &str) -> Vec<String> {
    let mut variants = Vec::new();
    for variant in title_search_variants(title, true) {
        push_normalized_match_variant(&mut variants, variant.as_str());
    }
    variants
}

fn normalized_artist_match_variants(artist: &str) -> Vec<String> {
    let mut variants = Vec::new();
    push_normalized_match_variant(&mut variants, artist);
    let lower = artist.to_lowercase();
    for separator in [
        " feat. ",
        " feat ",
        " ft. ",
        " ft ",
        " featuring ",
        " with ",
        " x ",
        " & ",
        " and ",
        ",",
        "，",
        "、",
        "/",
        "／",
        ";",
        "；",
        "|",
    ] {
        for part in lower.split(separator) {
            push_normalized_match_variant(&mut variants, part);
        }
    }
    variants
}

fn push_normalized_match_variant(variants: &mut Vec<String>, value: &str) {
    let normalized = normalize_match_text(value);
    if !normalized.is_empty() && !variants.contains(&normalized) {
        variants.push(normalized);
    }
}

fn normalize_match_text(value: &str) -> String {
    normalize_search_request_query(value).to_lowercase()
}

fn is_duration_match_within_tolerance(expected: i64, actual: i64, tolerance_ms: i64) -> bool {
    if expected == 0 || actual == 0 {
        return true;
    }
    (expected - actual).abs() <= tolerance_ms
}

fn score_match(
    track: &TrackInfo,
    candidate: &StoredLyricsCandidate,
    duration_tolerance_ms: i64,
) -> f64 {
    score_text(
        &strip_feat(track.name.as_str()),
        &strip_feat(candidate.title.as_str()),
    ) + score_artists(
        &track
            .artists
            .iter()
            .map(|artist| artist.name.clone())
            .collect::<Vec<_>>(),
        &candidate.artists,
    ) + score_text(
        track.album_name.as_deref().unwrap_or_default(),
        candidate.album.as_str(),
    ) * 0.4
        + score_duration(
            track.duration_ms,
            candidate.duration_ms.unwrap_or_default(),
            duration_tolerance_ms,
        ) * 2.0
}

fn score_text(expected: &str, actual: &str) -> f64 {
    let left = crate::lyrics_external::normalize_search_query(expected).to_lowercase();
    let right = crate::lyrics_external::normalize_search_query(actual).to_lowercase();
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    if left == right {
        return 10.0;
    }
    if left.contains(&right) || right.contains(&left) {
        return 8.0;
    }
    let left_tokens: std::collections::HashSet<_> = left.split_whitespace().collect();
    let right_tokens: std::collections::HashSet<_> = right.split_whitespace().collect();
    let overlap = left_tokens
        .iter()
        .filter(|token| right_tokens.contains(**token))
        .count();
    if overlap == 0 {
        0.0
    } else {
        (overlap as f64 / left_tokens.len().max(right_tokens.len()) as f64) * 7.0
    }
}

fn score_artists(expected: &[String], actual: &[String]) -> f64 {
    let left: std::collections::HashSet<_> = expected
        .iter()
        .map(|value| crate::lyrics_external::normalize_search_query(value).to_lowercase())
        .filter(|value| !value.is_empty())
        .collect();
    let right: std::collections::HashSet<_> = actual
        .iter()
        .map(|value| crate::lyrics_external::normalize_search_query(value).to_lowercase())
        .filter(|value| !value.is_empty())
        .collect();
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let overlap = left.iter().filter(|artist| right.contains(*artist)).count();
    if overlap == 0 {
        0.0
    } else {
        (overlap as f64 / left.len().max(right.len()) as f64) * 10.0
    }
}

fn score_duration(expected: i64, actual: i64, tolerance_ms: i64) -> f64 {
    if expected == 0 || actual == 0 {
        return 0.0;
    }
    let delta = (expected - actual).abs();
    if delta > tolerance_ms {
        return 0.0;
    }
    10.0 - ((delta as f64 / tolerance_ms as f64) * 8.0)
}

fn strip_feat(title: &str) -> String {
    let lower = title.to_lowercase();
    if let Some(index) = lower.find("(feat.") {
        return title[..index].trim().to_string();
    }
    if let Some(index) = lower.find("（feat.") {
        return title[..index].trim().to_string();
    }
    if let Some(index) = lower.find(" - feat.") {
        return title[..index].trim().to_string();
    }
    title.trim().to_string()
}

fn title_search_variants(title: &str, include_original_version_variant: bool) -> Vec<String> {
    let mut variants = Vec::new();
    let featured_stripped = strip_feat(title);
    let version_stripped = strip_trailing_version_suffixes(featured_stripped.as_str());
    push_query_variant(&mut variants, version_stripped.as_str());
    if include_original_version_variant && version_stripped != featured_stripped {
        push_query_variant(&mut variants, featured_stripped.as_str());
    }

    for alias in split_dual_title_aliases(version_stripped.as_str()) {
        push_query_variant(&mut variants, alias.as_str());
    }

    variants
}

fn strip_trailing_version_suffixes(title: &str) -> String {
    let mut current = title.trim().to_string();

    loop {
        let stripped = strip_one_trailing_suffix(current.as_str());
        if stripped == current {
            return current;
        }
        current = stripped;
    }
}

fn strip_one_trailing_suffix(title: &str) -> String {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    for (open, close) in [('(', ')'), ('[', ']'), ('（', '）'), ('【', '】')] {
        if trimmed.ends_with(close) {
            let prefix = &trimmed[..trimmed.len() - close.len_utf8()];
            if let Some(index) = prefix.rfind(open) {
                let suffix =
                    trimmed[index + open.len_utf8()..trimmed.len() - close.len_utf8()].trim();
                if looks_like_version_suffix(suffix) {
                    return trimmed[..index].trim().to_string();
                }
            }
        }
    }

    for separator in [" - ", " – ", " — ", " ~ "] {
        if let Some((head, tail)) = trimmed.rsplit_once(separator) {
            if looks_like_version_suffix(tail) {
                return head.trim().to_string();
            }
        }
    }

    trimmed.to_string()
}

fn looks_like_version_suffix(value: &str) -> bool {
    let normalized = normalize_search_request_query(value).to_lowercase();
    if normalized.is_empty() {
        return false;
    }

    VERSION_SUFFIX_HINTS
        .iter()
        .any(|hint| normalized.contains(hint))
}

fn split_dual_title_aliases(title: &str) -> Vec<String> {
    let mut aliases = Vec::new();
    for separator in [" / ", " | ", " · ", "：", ": ", " - ", " – ", " — ", " ~ "] {
        if let Some((left, right)) = title.split_once(separator) {
            let left = normalize_search_request_query(left);
            let right = normalize_search_request_query(right);
            if left.is_empty() || right.is_empty() || left == right {
                continue;
            }
            push_query_variant(&mut aliases, left.as_str());
            push_query_variant(&mut aliases, right.as_str());
        }
    }
    aliases
}

fn push_query_variant(variants: &mut Vec<String>, value: &str) {
    let normalized = normalize_search_request_query(value);
    if !normalized.is_empty() && !variants.contains(&normalized) {
        variants.push(normalized);
    }
}

fn auto_match_artist_queries(track: &TrackInfo) -> Vec<String> {
    let mut artists = Vec::new();
    if let Some(primary) = track.artists.first() {
        push_query_variant(&mut artists, primary.name.as_str());
    }

    let all_artists = track
        .artists
        .iter()
        .map(|artist| artist.name.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    push_query_variant(&mut artists, all_artists.as_str());

    artists
}

fn normalize_search_request_query(value: &str) -> String {
    crate::lyrics_external::normalize_search_query(value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Artist;

    fn sample_track() -> TrackInfo {
        TrackInfo {
            added_at: None,
            id: "track-1".into(),
            hex_id: Some("00000000000000000000000000000001".into()),
            uri: Some("spotify:track:track-1".into()),
            name: "Counting Stars (feat. Demo)".into(),
            album_name: Some("Native".into()),
            album_id: None,
            album_uri: None,
            artists: vec![Artist {
                id: None,
                uri: None,
                name: "OneRepublic".into(),
                images: Vec::new(),
            }],
            duration_ms: 256_946,
            explicit: false,
            playable: true,
            preview_url: None,
            images: Vec::new(),
        }
    }

    #[test]
    fn saved_match_track_context_preserves_spotify_track_identity() {
        let track_uri = "spotify:track:2TpxZ7JUBn3uw46aR7qd6V";
        let (resolved_uri, track_id) = saved_match_track_context(Some(track_uri));

        assert_eq!(resolved_uri, Some(track_uri));
        assert_eq!(track_id.as_deref(), Some("2TpxZ7JUBn3uw46aR7qd6V"));
    }

    #[test]
    fn saved_match_track_context_does_not_invent_spotify_id_for_local_tracks() {
        let track_uri = "spotify:local:artist:album:track:123";
        let (resolved_uri, track_id) = saved_match_track_context(Some(track_uri));

        assert_eq!(resolved_uri, Some(track_uri));
        assert_eq!(track_id, None);
    }

    #[test]
    fn build_search_queries_matches_existing_fallback_order() {
        assert_eq!(
            build_search_queries(&sample_track()),
            vec![
                "Counting Stars OneRepublic".to_string(),
                "Counting Stars Native".to_string(),
                "Counting Stars".to_string(),
            ],
        );
    }

    #[test]
    fn build_search_queries_adds_stripped_version_variant() {
        let mut track = sample_track();
        track.name = "アイドル (English ver.)".into();
        track.artists[0].name = "YOASOBI".into();

        assert_eq!(
            build_search_queries(&track),
            vec![
                "アイドル YOASOBI".to_string(),
                "アイドル Native".to_string(),
                "アイドル".to_string(),
            ],
        );
    }

    #[test]
    fn build_auto_match_query_groups_uses_title_then_title_artist() {
        let groups = build_auto_match_query_groups(&sample_track());

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].strategy, AutoMatchStrategy::TitleDuration);
        assert_eq!(groups[0].queries, vec!["Counting Stars".to_string()]);
        assert_eq!(groups[1].strategy, AutoMatchStrategy::TitleArtistDuration);
        assert_eq!(
            groups[1].queries,
            vec!["Counting Stars OneRepublic".to_string()]
        );
    }

    #[test]
    fn build_comment_search_queries_keeps_more_fallback_variants() {
        let mut track = sample_track();
        track.name = "神っぽいな / God-ish (English ver.)".into();
        track.artists[0].name = "PinocchioP".into();

        let queries = build_comment_search_queries(&track);
        assert!(queries
            .iter()
            .any(|query| query == "神っぽいな God ish PinocchioP"));
        assert!(queries.iter().any(|query| query == "神っぽいな PinocchioP"));
        assert!(queries.len() >= 4);
    }

    #[test]
    fn strip_trailing_version_suffixes_only_removes_known_release_suffixes() {
        assert_eq!(
            strip_trailing_version_suffixes("Song Title (Live at Tokyo Dome)"),
            "Song Title"
        );
        assert_eq!(strip_trailing_version_suffixes("God-Ish"), "God-Ish");
    }

    #[test]
    fn rank_candidates_for_track_prefers_close_title_artist_and_duration() {
        let ranked = rank_candidates_for_track(
            &sample_track(),
            vec![StoredLyricsCandidate {
                album: "Native".into(),
                artists: vec!["OneRepublic".into()],
                duration_ms: Some(256_900),
                id: "123".into(),
                mid: Some("qqmid".into()),
                provider: "qq".into(),
                score: None,
                title: "Counting Stars".into(),
            }],
            Some("qq"),
        );
        assert_eq!(ranked.len(), 1);
    }

    #[test]
    fn rank_auto_match_candidates_keeps_exact_title_with_one_second_duration_tolerance() {
        let track = sample_track();
        let ranked = rank_auto_match_candidates_for_track(
            &track,
            vec![
                StoredLyricsCandidate {
                    album: "Native".into(),
                    artists: vec!["Different Artist".into()],
                    duration_ms: Some(track.duration_ms + 1_000),
                    id: "kept".into(),
                    mid: None,
                    provider: "netease".into(),
                    score: None,
                    title: "Counting Stars".into(),
                },
                StoredLyricsCandidate {
                    album: "Native".into(),
                    artists: vec!["OneRepublic".into()],
                    duration_ms: Some(track.duration_ms + 1_001),
                    id: "rejected-duration".into(),
                    mid: None,
                    provider: "netease".into(),
                    score: None,
                    title: "Counting Stars".into(),
                },
                StoredLyricsCandidate {
                    album: "Native".into(),
                    artists: vec!["OneRepublic".into()],
                    duration_ms: Some(track.duration_ms),
                    id: "rejected-title".into(),
                    mid: None,
                    provider: "netease".into(),
                    score: None,
                    title: "Counting Stars Live".into(),
                },
            ],
            Some("netease"),
            AutoMatchStrategy::TitleDuration,
        );

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].id, "kept");
    }

    #[test]
    fn rank_auto_match_title_artist_strategy_requires_exact_artist() {
        let track = sample_track();
        let ranked = rank_auto_match_candidates_for_track(
            &track,
            vec![
                StoredLyricsCandidate {
                    album: "Native".into(),
                    artists: vec!["Different Artist".into()],
                    duration_ms: Some(track.duration_ms),
                    id: "rejected-artist".into(),
                    mid: None,
                    provider: "netease".into(),
                    score: None,
                    title: "Counting Stars".into(),
                },
                StoredLyricsCandidate {
                    album: "Native".into(),
                    artists: vec!["OneRepublic".into()],
                    duration_ms: Some(track.duration_ms),
                    id: "kept".into(),
                    mid: None,
                    provider: "qq".into(),
                    score: None,
                    title: "Counting Stars".into(),
                },
            ],
            Some("qq"),
            AutoMatchStrategy::TitleArtistDuration,
        );

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].id, "kept");
    }

    #[test]
    fn rank_auto_match_accepts_exact_dual_title_alias() {
        let mut track = sample_track();
        track.name = "God-ish".into();
        track.artists[0].name = "PinocchioP".into();

        let ranked = rank_auto_match_candidates_for_track(
            &track,
            vec![StoredLyricsCandidate {
                album: String::new(),
                artists: vec!["PinocchioP".into()],
                duration_ms: Some(track.duration_ms),
                id: "dual-title".into(),
                mid: None,
                provider: "netease".into(),
                score: None,
                title: "神っぽいな / God-ish".into(),
            }],
            Some("netease"),
            AutoMatchStrategy::TitleArtistDuration,
        );

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].id, "dual-title");
    }

    #[test]
    fn rank_auto_match_rejects_unknown_durations() {
        let ranked = rank_auto_match_candidates_for_track(
            &sample_track(),
            vec![StoredLyricsCandidate {
                album: "Native".into(),
                artists: vec!["OneRepublic".into()],
                duration_ms: None,
                id: "unknown-duration".into(),
                mid: None,
                provider: "netease".into(),
                score: None,
                title: "Counting Stars".into(),
            }],
            Some("netease"),
            AutoMatchStrategy::TitleDuration,
        );

        assert!(ranked.is_empty());
    }

    #[test]
    fn rank_auto_match_accepts_provider_duration_reported_in_seconds() {
        let track = sample_track();
        let ranked = rank_auto_match_candidates_for_track(
            &track,
            vec![StoredLyricsCandidate {
                album: "Native".into(),
                artists: vec!["OneRepublic".into()],
                duration_ms: Some(257),
                id: "seconds-duration".into(),
                mid: None,
                provider: "netease".into(),
                score: None,
                title: "Counting Stars".into(),
            }],
            Some("netease"),
            AutoMatchStrategy::TitleArtistDuration,
        );

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].id, "seconds-duration");
    }

    #[test]
    fn rank_auto_match_accepts_second_precision_duration_floor() {
        let mut track = sample_track();
        track.duration_ms = 257_999;
        let ranked = rank_auto_match_candidates_for_track(
            &track,
            vec![StoredLyricsCandidate {
                album: "Native".into(),
                artists: vec!["OneRepublic".into()],
                duration_ms: Some(256_000),
                id: "second-floor-duration".into(),
                mid: None,
                provider: "qq".into(),
                score: None,
                title: "Counting Stars".into(),
            }],
            Some("qq"),
            AutoMatchStrategy::TitleArtistDuration,
        );

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].id, "second-floor-duration");
    }

    #[test]
    fn rank_comment_candidates_keeps_close_title_even_if_duration_is_not_lyrics_exact() {
        let track = sample_track();
        let ranked = rank_comment_candidates_for_track(
            &track,
            vec![StoredLyricsCandidate {
                album: "Native".into(),
                artists: vec!["OneRepublic".into()],
                duration_ms: Some(track.duration_ms + 8_000),
                id: "comment-candidate".into(),
                mid: None,
                provider: "netease".into(),
                score: None,
                title: "Counting Stars".into(),
            }],
            Some("netease"),
        );

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].id, "comment-candidate");
    }
}
