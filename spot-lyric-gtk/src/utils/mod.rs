pub mod position_clock;

pub use position_clock::{CellClock, PositionClock};

use gtk::gdk;

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
