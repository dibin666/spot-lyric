use crate::types::ImageResource;

const BASE62_CHARS: &str = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
const LOCAL_URI_PREFIX: &str = "spotify:local:";
const TRACK_URI_PREFIX: &str = "spotify:track:";
const SPOTIFY_MPRIS_TRACK_PATH_PREFIX: &str = "/com/spotify/track/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackIdentity {
    pub id: Option<String>,
    pub hex_id: Option<String>,
}

/// Convert a 32-char hex ID to a Spotify base62 ID (22 chars).
pub fn hex_to_base62(hex: &str) -> Option<String> {
    if !is_hex_track_id(hex) {
        return None;
    }
    let mut value = u128::from_str_radix(hex, 16).ok()?;
    if value == 0 {
        return Some("0".repeat(22));
    }
    let mut chars = Vec::with_capacity(22);
    let base62_bytes = BASE62_CHARS.as_bytes();
    while value > 0 {
        chars.push(base62_bytes[(value % 62) as usize] as char);
        value /= 62;
    }
    // Pad to 22 chars
    while chars.len() < 22 {
        chars.push('0');
    }
    chars.reverse();
    Some(chars.into_iter().collect())
}

pub fn extract_track_id(value: &str) -> String {
    let value = value.trim();
    if let Some(track_id) = value.strip_prefix(TRACK_URI_PREFIX) {
        return track_id.to_string();
    }
    if let Some(track_id) = value.strip_prefix(SPOTIFY_MPRIS_TRACK_PATH_PREFIX) {
        return track_id.to_string();
    }
    if let Ok(url) = url::Url::parse(value) {
        if let Some(track_id) = spotify_track_id_from_url(&url) {
            return track_id;
        }
    }
    value.to_string()
}

fn spotify_track_id_from_url(url: &url::Url) -> Option<String> {
    let host = url.host_str()?.trim();
    if !host.eq_ignore_ascii_case("open.spotify.com")
        && !host.eq_ignore_ascii_case("play.spotify.com")
    {
        return None;
    }

    let segments: Vec<_> = url.path_segments()?.collect();
    let track_index = segments.iter().position(|segment| *segment == "track")?;
    let track_id = segments.get(track_index + 1)?.trim();
    if track_id.is_empty() {
        None
    } else {
        Some(track_id.to_string())
    }
}

pub fn is_local_track_uri(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.starts_with(LOCAL_URI_PREFIX))
}

pub fn is_hex_track_id(value: &str) -> bool {
    value.len() == 32 && value.chars().all(|character| character.is_ascii_hexdigit())
}

pub fn to_hex_track_id(value: &str) -> Option<String> {
    let track_id = extract_track_id(value);
    if track_id.is_empty() {
        return None;
    }
    if is_hex_track_id(track_id.as_str()) {
        return Some(track_id.to_ascii_lowercase());
    }

    let mut numeric_value = 0u128;
    for character in track_id.chars() {
        let index = BASE62_CHARS.find(character)? as u128;
        numeric_value = numeric_value.saturating_mul(62).saturating_add(index);
    }
    Some(format!("{numeric_value:032x}"))
}

pub fn resolve_track_identity(
    id: Option<&str>,
    uri: Option<&str>,
    hex_id: Option<&str>,
) -> TrackIdentity {
    if is_local_track_uri(uri) {
        return TrackIdentity {
            id: uri.map(str::to_owned),
            hex_id: None,
        };
    }

    let id = uri.map(extract_track_id).or_else(|| {
        id.filter(|value| !is_hex_track_id(value))
            .map(str::to_owned)
    });
    let hex_from_id = id
        .as_deref()
        .filter(|value| is_hex_track_id(value))
        .map(|value| value.to_ascii_lowercase());
    let hex_id = hex_id
        .map(|value| value.to_ascii_lowercase())
        .or_else(|| id.as_deref().and_then(to_hex_track_id))
        .or(hex_from_id);

    TrackIdentity { id, hex_id }
}

pub fn sort_images_by_quality(mut images: Vec<ImageResource>) -> Vec<ImageResource> {
    images.sort_by(|left, right| {
        let left_size =
            (left.width.unwrap_or_default() as u64) * (left.height.unwrap_or_default() as u64);
        let right_size =
            (right.width.unwrap_or_default() as u64) * (right.height.unwrap_or_default() as u64);
        right_size
            .cmp(&left_size)
            .then_with(|| right.url.cmp(&left.url))
    });
    images
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_track_id_from_spotify_uri() {
        assert_eq!(
            extract_track_id("spotify:track:4uLU6hMCjMI75M1A2tKUQC"),
            "4uLU6hMCjMI75M1A2tKUQC"
        );
    }

    #[test]
    fn extracts_track_id_from_open_spotify_url() {
        assert_eq!(
            extract_track_id("https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC?si=test"),
            "4uLU6hMCjMI75M1A2tKUQC"
        );
    }

    #[test]
    fn extracts_track_id_from_mpris_track_path() {
        assert_eq!(
            extract_track_id("/com/spotify/track/4uLU6hMCjMI75M1A2tKUQC"),
            "4uLU6hMCjMI75M1A2tKUQC"
        );
    }

    #[test]
    fn resolves_identity_from_open_spotify_url() {
        let identity = resolve_track_identity(
            None,
            Some("https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC?si=test"),
            None,
        );

        assert_eq!(identity.id.as_deref(), Some("4uLU6hMCjMI75M1A2tKUQC"));
        assert_eq!(identity.hex_id, to_hex_track_id("4uLU6hMCjMI75M1A2tKUQC"));
    }
}
