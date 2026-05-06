//! Embedded lyrics preview page for the main application window.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::config;
use crate::dbus::types::{LyricsLine, LyricsPayload, PlaybackState};
use crate::utils::position_clock::CellClock;

#[derive(Clone)]
pub struct LyricsPreviewPage {
    inner: Rc<LyricsPreviewPageInner>,
}

struct LyricsPreviewPageInner {
    page: adw::PreferencesPage,
    title_label: gtk::Label,
    subtitle_label: gtk::Label,
    stack: gtk::Stack,
    empty_label: gtk::Label,
    scrolled: gtk::ScrolledWindow,
    lines_box: gtk::Box,
    rows: RefCell<Vec<gtk::Box>>,
    lyrics: RefCell<Vec<LyricsLine>>,
    active_index: Cell<Option<usize>>,
    current_track_uri: RefCell<String>,
    current_track_label: RefCell<String>,
    clock: CellClock,
    clock_tick_id: Cell<Option<glib::SourceId>>,
}

impl Drop for LyricsPreviewPageInner {
    fn drop(&mut self) {
        if let Some(id) = self.clock_tick_id.take() {
            id.remove();
        }
    }
}

impl LyricsPreviewPage {
    pub fn new() -> Self {
        let inner = Rc::new(build_inner());
        let page = Self { inner };
        page.start_clock();
        page
    }

    pub fn widget(&self) -> adw::PreferencesPage {
        self.inner.page.clone()
    }

    pub fn apply_playback(&self, state: &PlaybackState) {
        let label = track_label(state);
        self.update_track_label(&label);
        if self.inner.current_track_uri.borrow().as_str() != state.track_uri.as_str() {
            *self.inner.current_track_uri.borrow_mut() = state.track_uri.clone();
            self.inner.clock.reset();
            self.set_lyrics(&LyricsPayload::default());
        }
        let position_ms =
            self.inner
                .clock
                .snapshot(state.position_ms, state.duration_ms, state.is_playing);
        tracing::debug!(
            target: "spot_lyric_gtk::timeline",
            surface = "preview",
            raw_position_ms = state.position_ms,
            estimated_position_ms = position_ms,
            is_playing = state.is_playing,
            track_uri = %state.track_uri,
            "playback snapshot applied"
        );
        self.update_empty_label();
        self.update_active_line(position_ms);
    }

    pub fn set_lyrics(&self, payload: &LyricsPayload) {
        *self.inner.lyrics.borrow_mut() = payload.lines.clone();
        self.inner.active_index.set(None);
        self.rebuild_rows();
        self.update_payload_subtitle(payload);
        self.update_active_line(self.inner.clock.estimate());
    }

    pub fn current_track_uri(&self) -> String {
        self.inner.current_track_uri.borrow().clone()
    }

    fn start_clock(&self) {
        let weak = Rc::downgrade(&self.inner);
        let id = glib::timeout_add_local(tick_duration(), move || {
            let Some(inner) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let page = LyricsPreviewPage { inner };
            page.update_active_line(page.inner.clock.estimate());
            glib::ControlFlow::Continue
        });
        self.inner.clock_tick_id.set(Some(id));
    }

    fn update_track_label(&self, label: &str) {
        *self.inner.current_track_label.borrow_mut() = label.to_string();
        self.inner.title_label.set_text(if label.is_empty() {
            "歌词预览"
        } else {
            label
        });
    }

    fn rebuild_rows(&self) {
        while let Some(child) = self.inner.lines_box.first_child() {
            self.inner.lines_box.remove(&child);
        }
        let rows: Vec<_> = self
            .inner
            .lyrics
            .borrow()
            .iter()
            .map(build_line_row)
            .collect();
        for row in &rows {
            self.inner.lines_box.append(row);
        }
        *self.inner.rows.borrow_mut() = rows;
        self.set_stack_page();
        self.update_empty_label();
    }

    fn set_stack_page(&self) {
        let page = if self.inner.lyrics.borrow().is_empty() {
            "empty"
        } else {
            "lyrics"
        };
        self.inner.stack.set_visible_child_name(page);
    }

    fn update_active_line(&self, position_ms: i64) {
        let starts: Vec<i64> = self
            .inner
            .lyrics
            .borrow()
            .iter()
            .map(|line| line.start_time_ms)
            .collect();
        let new_index = active_lyric_index(&starts, position_ms);
        let previous_index = self.inner.active_index.get();
        if new_index == previous_index {
            return;
        }
        tracing::debug!(
            target: "spot_lyric_gtk::timeline",
            surface = "preview",
            position_ms,
            previous_index = ?previous_index,
            new_index = ?new_index,
            "active lyric line changed"
        );
        self.inner.active_index.set(new_index);
        self.update_row_classes(new_index);
        if let Some(idx) = new_index {
            self.scroll_to_index_soon(idx);
        }
    }

    fn update_row_classes(&self, new_index: Option<usize>) {
        for (idx, row) in self.inner.rows.borrow().iter().enumerate() {
            row.remove_css_class("active");
            row.remove_css_class("past");
            if Some(idx) == new_index {
                row.add_css_class("active");
            } else if new_index.map(|active| idx < active).unwrap_or(false) {
                row.add_css_class("past");
            }
        }
    }

    fn scroll_to_index_soon(&self, idx: usize) {
        let weak = Rc::downgrade(&self.inner);
        glib::idle_add_local_once(move || {
            if let Some(inner) = weak.upgrade() {
                LyricsPreviewPage { inner }.scroll_to_index(idx);
            }
        });
    }

    fn scroll_to_index(&self, idx: usize) {
        let Some(row) = self.inner.rows.borrow().get(idx).cloned() else {
            return;
        };
        let Some(bounds) = row.compute_bounds(&self.inner.lines_box) else {
            return;
        };
        let adj = self.inner.scrolled.vadjustment();
        let row_mid = f64::from(bounds.y()) + f64::from(bounds.height()) / 2.0;
        let max_value = (adj.upper() - adj.page_size()).max(adj.lower());
        let target = (row_mid - adj.page_size() * 0.45).clamp(adj.lower(), max_value);
        if (adj.value() - target).abs() > 1.0 {
            adj.set_value(target);
        }
    }

    fn update_payload_subtitle(&self, payload: &LyricsPayload) {
        let provider = payload.provider.as_deref().unwrap_or_default();
        let sync_type = payload.sync_type.as_str();
        let subtitle = match (provider.is_empty(), sync_type.is_empty()) {
            (false, false) => format!("{provider} · {sync_type} · {} 行", payload.lines.len()),
            (false, true) => format!("{provider} · {} 行", payload.lines.len()),
            (true, false) => format!("{sync_type} · {} 行", payload.lines.len()),
            (true, true) => format!("{} 行歌词", payload.lines.len()),
        };
        self.inner.subtitle_label.set_text(&subtitle);
    }

    fn update_empty_label(&self) {
        let track = self.inner.current_track_label.borrow().clone();
        let text = if track.is_empty() {
            "暂无歌词\n播放歌曲或点击手动匹配预览后显示完整歌词".to_string()
        } else {
            format!("{track}\n暂无歌词，点击手动匹配预览可在此查看完整歌词")
        };
        self.inner.empty_label.set_text(&text);
    }
}

impl Default for LyricsPreviewPage {
    fn default() -> Self {
        Self::new()
    }
}

fn build_inner() -> LyricsPreviewPageInner {
    let page = adw::PreferencesPage::builder()
        .title("歌词预览")
        .icon_name("view-list-symbolic")
        .build();
    let group = adw::PreferencesGroup::new();
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .vexpand(true)
        .hexpand(true)
        .css_classes(["lyrics-preview-root"])
        .build();

    let title_label = short_label("歌词预览", "lyrics-preview-title");
    let subtitle_label = short_label("播放歌曲后显示完整歌词", "lyrics-preview-subtitle");
    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(3)
        .margin_start(14)
        .margin_end(14)
        .margin_top(10)
        .build();
    header.append(&title_label);
    header.append(&subtitle_label);
    root.append(&header);

    let stack = gtk::Stack::builder().vexpand(true).hexpand(true).build();
    let empty_label = empty_label("暂无歌词\n播放歌曲或点击手动匹配预览后显示完整歌词");
    let lines_box = lyrics_box();
    let scrolled = scrolled_lines(&lines_box);
    stack.add_named(&empty_label, Some("empty"));
    stack.add_named(&scrolled, Some("lyrics"));
    stack.set_visible_child_name("empty");
    root.append(&stack);
    group.add(&root);
    page.add(&group);

    LyricsPreviewPageInner {
        page,
        title_label,
        subtitle_label,
        stack,
        empty_label,
        scrolled,
        lines_box,
        rows: RefCell::new(Vec::new()),
        lyrics: RefCell::new(Vec::new()),
        active_index: Cell::new(None),
        current_track_uri: RefCell::new(String::new()),
        current_track_label: RefCell::new(String::new()),
        clock: CellClock::new(),
        clock_tick_id: Cell::new(None),
    }
}

fn short_label(text: &str, css_class: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .css_classes([css_class])
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build()
}

fn empty_label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .css_classes(["lyrics-preview-empty"])
        .justify(gtk::Justification::Center)
        .wrap(true)
        .vexpand(true)
        .hexpand(true)
        .build()
}

fn lyrics_box() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(14)
        .margin_end(14)
        .css_classes(["lyrics-preview-list"])
        .build()
}

fn scrolled_lines(lines_box: &gtk::Box) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .child(lines_box)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(420)
        .vexpand(true)
        .hexpand(true)
        .build()
}

fn build_line_row(line: &LyricsLine) -> gtk::Box {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(3)
        .css_classes(["lyrics-preview-line"])
        .build();
    row.append(&primary_label(line));
    if let Some(label) = translation_label(line) {
        row.append(&label);
    }
    row
}

fn primary_label(line: &LyricsLine) -> gtk::Label {
    let text = if line.text.trim().is_empty() {
        "♪"
    } else {
        line.text.trim()
    };
    gtk::Label::builder()
        .label(text)
        .css_classes(["lyrics-preview-text"])
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
            .css_classes(["lyrics-preview-translation"])
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .build(),
    )
}

fn track_label(state: &PlaybackState) -> String {
    if state.track_name.is_empty() {
        String::new()
    } else if state.artist_name.is_empty() {
        state.track_name.clone()
    } else {
        format!("{} — {}", state.track_name, state.artist_name)
    }
}

fn active_lyric_index(line_start_times: &[i64], position_ms: i64) -> Option<usize> {
    line_start_times
        .iter()
        .rposition(|start_time| *start_time <= position_ms)
}

fn tick_duration() -> std::time::Duration {
    std::time::Duration::from_millis(config::POSITION_TICK_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_index_tracks_last_started_line() {
        let starts = [1_000, 2_500, 4_000];

        assert_eq!(active_lyric_index(&starts, 999), None);
        assert_eq!(active_lyric_index(&starts, 1_000), Some(0));
        assert_eq!(active_lyric_index(&starts, 3_000), Some(1));
        assert_eq!(active_lyric_index(&starts, 9_000), Some(2));
    }
}
