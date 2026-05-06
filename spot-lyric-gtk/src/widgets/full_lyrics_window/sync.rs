use adw::subclass::prelude::*;
use gtk::glib;
use gtk::prelude::*;

use crate::config;
use crate::dbus::types::{LyricsPayload, PlaybackState};

use super::layout::active_lyric_index;
use super::rows::{build_line_row, track_label};
use super::FullLyricsWindow;

impl FullLyricsWindow {
    pub(super) fn start_clock(&self) {
        let weak = self.downgrade();
        let id = glib::timeout_add_local(tick_duration(), move || {
            let Some(win) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            win.update_active_line(win.imp().clock.estimate());
            glib::ControlFlow::Continue
        });
        self.imp().clock_tick_id.set(Some(id));
    }

    pub fn apply_playback(&self, state: &PlaybackState) {
        let label = track_label(state);
        self.update_track_label(&label);
        if self.imp().current_track_uri.borrow().as_str() != state.track_uri.as_str() {
            *self.imp().current_track_uri.borrow_mut() = state.track_uri.clone();
            self.imp().clock.reset();
            self.set_lyrics(&LyricsPayload::default());
        }
        let position_ms =
            self.imp()
                .clock
                .snapshot(state.position_ms, state.duration_ms, state.is_playing);
        tracing::debug!(
            target: "spot_lyric_gtk::timeline",
            surface = "full",
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
        *self.imp().lyrics.borrow_mut() = payload.lines.clone();
        self.imp().active_index.set(None);
        self.rebuild_rows();
        self.update_payload_subtitle(payload);
        self.update_active_line(self.imp().clock.estimate());
    }

    pub fn current_track_uri(&self) -> String {
        self.imp().current_track_uri.borrow().clone()
    }

    fn update_track_label(&self, label: &str) {
        *self.imp().current_track_label.borrow_mut() = label.to_string();
        if let Some(title) = self.imp().title_label.borrow().as_ref() {
            title.set_text(if label.is_empty() {
                "完整歌词"
            } else {
                label
            });
        }
    }

    fn rebuild_rows(&self) {
        let imp = self.imp();
        if let Some(box_) = imp.lines_box.borrow().as_ref() {
            while let Some(child) = box_.first_child() {
                box_.remove(&child);
            }
            let rows: Vec<_> = imp.lyrics.borrow().iter().map(build_line_row).collect();
            for row in &rows {
                box_.append(row);
            }
            *imp.rows.borrow_mut() = rows;
        }
        self.set_stack_page();
        self.update_empty_label();
    }

    fn set_stack_page(&self) {
        let Some(stack) = self.imp().stack.borrow().as_ref().cloned() else {
            return;
        };
        let page = if self.imp().lyrics.borrow().is_empty() {
            "empty"
        } else {
            "lyrics"
        };
        stack.set_visible_child_name(page);
    }

    fn update_active_line(&self, position_ms: i64) {
        let starts: Vec<i64> = self
            .imp()
            .lyrics
            .borrow()
            .iter()
            .map(|line| line.start_time_ms)
            .collect();
        let new_index = active_lyric_index(&starts, position_ms);
        let previous_index = self.imp().active_index.get();
        if new_index == previous_index {
            return;
        }
        tracing::debug!(
            target: "spot_lyric_gtk::timeline",
            surface = "full",
            position_ms,
            previous_index = ?previous_index,
            new_index = ?new_index,
            "active lyric line changed"
        );
        self.imp().active_index.set(new_index);
        self.update_row_classes(new_index);
        if let Some(idx) = new_index {
            self.scroll_to_index_soon(idx);
        }
    }

    fn update_row_classes(&self, new_index: Option<usize>) {
        for (idx, row) in self.imp().rows.borrow().iter().enumerate() {
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
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(win) = weak.upgrade() {
                win.scroll_to_index(idx);
            }
        });
    }

    fn scroll_to_index(&self, idx: usize) {
        let imp = self.imp();
        let Some(scrolled) = imp.scrolled.borrow().as_ref().cloned() else {
            return;
        };
        let Some(row) = imp.rows.borrow().get(idx).cloned() else {
            return;
        };
        let Some(lines_box) = imp.lines_box.borrow().as_ref().cloned() else {
            return;
        };
        let Some(bounds) = row.compute_bounds(&lines_box) else {
            return;
        };
        let adj = scrolled.vadjustment();
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
        if let Some(label) = self.imp().subtitle_label.borrow().as_ref() {
            label.set_text(&subtitle);
        }
    }

    fn update_empty_label(&self) {
        let track = self.imp().current_track_label.borrow().clone();
        let text = if track.is_empty() {
            "暂无歌词\n播放歌曲或点击手动匹配预览后显示完整歌词".to_string()
        } else {
            format!("{track}\n暂无歌词，点击手动匹配预览可在此查看完整歌词")
        };
        if let Some(label) = self.imp().empty_label.borrow().as_ref() {
            label.set_text(&text);
        }
    }
}

fn tick_duration() -> std::time::Duration {
    std::time::Duration::from_millis(config::POSITION_TICK_MS)
}
