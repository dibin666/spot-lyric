pub mod position_clock;

pub use position_clock::{CellClock, PositionClock};

use gtk::gdk;

pub const FONT_WEIGHT_OPTIONS: [&str; 3] = ["常规", "中等", "粗体"];

pub const FONT_SIZE_OPTIONS: [i32; 14] = [18, 20, 22, 24, 26, 28, 30, 32, 36, 40, 44, 48, 56, 64];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFontDescription {
    pub family: String,
    pub weight_index: usize,
    pub size_index: usize,
}

impl Default for ParsedFontDescription {
    fn default() -> Self {
        Self {
            family: "Sans".into(),
            weight_index: 2,
            size_index: nearest_font_size_index(32),
        }
    }
}

pub fn parse_font_description(description: &str) -> ParsedFontDescription {
    let mut parsed = ParsedFontDescription::default();
    let mut prefix = description.trim();

    if let Some((without_size, size)) = split_trailing_size(prefix) {
        parsed.size_index = nearest_font_size_index(size);
        prefix = without_size.trim_end();
    }

    let (family, weight_index) = split_trailing_weight(prefix);
    parsed.weight_index = weight_index;
    parsed.family = normalized_font_family(family);

    parsed
}

pub fn font_description_from_parts(family: &str, weight_index: usize, size_index: usize) -> String {
    let family = normalized_font_family(family);
    let weight = pango_weight_suffix(weight_index);
    let size = FONT_SIZE_OPTIONS.get(size_index).copied().unwrap_or(32);

    if weight.is_empty() {
        format!("{family} {size}")
    } else {
        format!("{family} {weight} {size}")
    }
}

pub fn font_family_matches(left: &str, right: &str) -> bool {
    normalized_font_family(left).eq_ignore_ascii_case(&normalized_font_family(right))
}

pub fn font_css_from_description(description: &str, size_scale: f64) -> String {
    let parsed = parse_font_description(description);
    let size = FONT_SIZE_OPTIONS
        .get(parsed.size_index)
        .copied()
        .unwrap_or(32);
    let scaled_size = ((size as f64) * size_scale).round().clamp(8.0, 96.0) as i32;
    let css_weight = css_font_weight(parsed.weight_index);

    format!(
        "font-family: {}; font-size: {scaled_size}px; font-weight: {css_weight};",
        css_font_family_list(&parsed.family),
    )
}

/// Convert a hex string `#rrggbb` to an `(r, g, b)` 0..255 triple.
/// Returns black on failure.
pub fn hex_to_rgb(hex: &str) -> (u8, u8, u8) {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return (0, 0, 0);
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
    (r, g, b)
}

/// Convert a `gdk::RGBA` to a `#rrggbb` string (alpha ignored — we store
/// opacity separately so users can keep one color and tweak transparency).
pub fn rgba_to_hex(rgba: &gdk::RGBA) -> String {
    let r = (rgba.red() * 255.0).round().clamp(0.0, 255.0) as u8;
    let g = (rgba.green() * 255.0).round().clamp(0.0, 255.0) as u8;
    let b = (rgba.blue() * 255.0).round().clamp(0.0, 255.0) as u8;
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Parse `#rrggbb` (alpha = 1.0) into an `RGBA`.
pub fn rgba_from_hex(hex: &str) -> gdk::RGBA {
    let (r, g, b) = hex_to_rgb(hex);
    gdk::RGBA::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
}

/// Format a millisecond duration as `M:SS` (or `H:MM:SS` past one hour).
pub fn format_duration_ms(ms: i64) -> String {
    if ms <= 0 {
        return "0:00".into();
    }
    let secs = ms / 1000;
    let h = secs / 3600;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn split_trailing_size(input: &str) -> Option<(&str, i32)> {
    let (prefix, suffix) = input.rsplit_once(' ')?;
    let size = suffix.parse::<f64>().ok()?.round() as i32;
    Some((prefix, size))
}

fn normalized_font_family(family: &str) -> String {
    let trimmed = family.trim().trim_matches('"').trim_matches('\'').trim();
    if trimmed.is_empty() {
        "Sans".into()
    } else {
        trimmed.to_string()
    }
}

fn split_trailing_weight(input: &str) -> (&str, usize) {
    for (suffix, index) in [
        (" Semibold", 2),
        (" SemiBold", 2),
        (" DemiBold", 2),
        (" Heavy", 2),
        (" Black", 2),
        (" Bold", 2),
        (" Medium", 1),
        (" Regular", 0),
        (" Normal", 0),
    ] {
        if let Some(prefix) = input.strip_suffix(suffix) {
            return (prefix, index);
        }
    }

    (input, 0)
}

fn nearest_font_size_index(size: i32) -> usize {
    FONT_SIZE_OPTIONS
        .iter()
        .enumerate()
        .min_by_key(|(_, candidate)| (*candidate - size).abs())
        .map(|(index, _)| index)
        .unwrap_or(7)
}

fn pango_weight_suffix(index: usize) -> &'static str {
    match index {
        1 => "Medium",
        2 => "Bold",
        _ => "",
    }
}

fn css_font_weight(index: usize) -> i32 {
    match index {
        1 => 500,
        2 => 700,
        _ => 400,
    }
}

fn css_font_family_list(family: &str) -> String {
    if family.eq_ignore_ascii_case("sans") || family.eq_ignore_ascii_case("sans-serif") {
        "sans-serif".into()
    } else {
        format!("{}, sans-serif", css_quote(family))
    }
}

fn css_quote(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_font_description() {
        let parsed = parse_font_description("HarmonyOS Sans SC Bold 32");

        assert_eq!(parsed.family, "HarmonyOS Sans SC");
        assert_eq!(parsed.weight_index, 2);
        assert_eq!(FONT_SIZE_OPTIONS[parsed.size_index], 32);
    }

    #[test]
    fn formats_selected_font_description() {
        assert_eq!(
            font_description_from_parts("Noto Sans CJK SC", 2, nearest_font_size_index(24)),
            "Noto Sans CJK SC Bold 24"
        );
    }

    #[test]
    fn builds_valid_css_font_properties() {
        let css = font_css_from_description("Noto Sans CJK SC Bold 24", 0.75);

        assert!(css.contains("font-family: \"Noto Sans CJK SC\", sans-serif;"));
        assert!(css.contains("font-size: 18px;"));
        assert!(css.contains("font-weight: 700;"));
    }
}
