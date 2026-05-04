use crate::types::ImageResource;

const BASE62_CHARS: &str = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
const LOCAL_URI_PREFIX: &str = "spotify:local:";
const TRACK_URI_PREFIX: &str = "spotify:track:";

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
    value
        .strip_prefix(TRACK_URI_PREFIX)
        .unwrap_or(value)
        .to_string()
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
