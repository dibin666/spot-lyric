use gtk::prelude::*;

use crate::dbus::types::{LyricsLine, PlaybackState};

pub fn build_line_row(line: &LyricsLine) -> gtk::Box {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(3)
        .css_classes(["full-lyrics-line"])
        .build();
    row.append(&primary_label(line));
    if let Some(label) = translation_label(line) {
        row.append(&label);
    }
    row
}

pub fn track_label(state: &PlaybackState) -> String {
    if state.track_name.is_empty() {
        String::new()
    } else if state.artist_name.is_empty() {
        state.track_name.clone()
    } else {
        format!("{} — {}", state.track_name, state.artist_name)
    }
}

fn primary_label(line: &LyricsLine) -> gtk::Label {
    let text = if line.text.trim().is_empty() {
        "♪"
    } else {
        line.text.trim()
    };
    gtk::Label::builder()
        .label(text)
        .css_classes(["full-lyrics-text"])
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .build()
}

fn translation_label(line: &LyricsLine) -> Option<gtk::Label> {
    let text = line.translated_text.as_deref()?.trim();
    if text.is_empty() {
        return None;
    }
    Some(
        gtk::Label::builder()
            .label(text)
            .css_classes(["full-lyrics-translation"])
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .build(),
    )
}
