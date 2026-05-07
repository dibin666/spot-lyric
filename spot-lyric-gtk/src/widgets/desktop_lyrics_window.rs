//! Always-on-top, transparent, draggable, lockable desktop lyrics overlay.
//!
//! On X11 we explicitly drive EWMH state ourselves (see `platform::x11`)
//! because GTK4 dropped `set_keep_above` and provides no built-in way to
//! keep a borderless toplevel above other windows.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::subclass::prelude::*;
use gdk4 as gdk;
use gtk::pango;
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::config;
use crate::dbus::types::{LyricsLine, LyricsPayload, PlaybackState};
use crate::platform::{MonitorGeometry, X11Helper};
use crate::utils::{font_css_from_description, hex_to_rgb, position_clock::CellClock};

const APP_INSTANCE: &str = "spot-lyric-gtk";

mod imp {
    use super::*;

    pub struct DesktopLyricsWindow {
        pub container: RefCell<Option<gtk::Box>>,
        pub active_label: RefCell<Option<gtk::Label>>,
        pub next_label: RefCell<Option<gtk::Label>>,

        pub lyrics: RefCell<Vec<LyricsLine>>,
        pub active_index: Cell<Option<usize>>,
        pub current_track_uri: RefCell<String>,
        pub current_track_label: RefCell<String>,

        pub clock: Rc<CellClock>,
        pub clock_tick_id: Cell<Option<glib::SourceId>>,

        pub locked: Cell<bool>,
        pub show_translation: Cell<bool>,
        pub line_mode_dual: Cell<bool>,

        pub css_provider: RefCell<Option<gtk::CssProvider>>,
        pub settings: RefCell<Option<gio::Settings>>,
        pub settings_handler: RefCell<Option<glib::SignalHandlerId>>,

        pub x11: RefCell<Option<X11Helper>>,
        pub xid: Cell<Option<u32>>,
    }

    impl Default for DesktopLyricsWindow {
        fn default() -> Self {
            Self {
                container: RefCell::new(None),
                active_label: RefCell::new(None),
                next_label: RefCell::new(None),
                lyrics: RefCell::new(Vec::new()),
                active_index: Cell::new(None),
                current_track_uri: RefCell::new(String::new()),
                current_track_label: RefCell::new(String::new()),
                clock: Rc::new(CellClock::new()),
                clock_tick_id: Cell::new(None),
                locked: Cell::new(true),
                show_translation: Cell::new(true),
                line_mode_dual: Cell::new(true),
                css_provider: RefCell::new(None),
                settings: RefCell::new(None),
                settings_handler: RefCell::new(None),
                x11: RefCell::new(None),
                xid: Cell::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DesktopLyricsWindow {
        const NAME: &'static str = "SpotLyricDesktopLyricsWindow";
        type Type = super::DesktopLyricsWindow;
        type ParentType = gtk::Window;
    }

    impl ObjectImpl for DesktopLyricsWindow {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.setup_window();
            obj.build_ui();
            obj.bind_settings();
            obj.start_clock();

            // X11 atoms applied once the surface exists.
            let weak = obj.downgrade();
            obj.connect_realize(move |_| {
                if let Some(win) = weak.upgrade() {
                    win.realize_x11();
                }
            });

            // Hide instead of close to keep the overlay reachable from tray.
            obj.connect_close_request(|win| {
                win.set_visible(false);
                if let Some(settings) = win.imp().settings.borrow().as_ref() {
                    let _ = settings.set_boolean("desktop-lyrics-enabled", false);
                }
                glib::Propagation::Stop
            });
        }

        fn dispose(&self) {
            if let Some(id) = self.clock_tick_id.take() {
                id.remove();
            }
            if let (Some(handler), Some(settings)) = (
                self.settings_handler.take(),
                self.settings.borrow().as_ref(),
            ) {
                settings.disconnect(handler);
            }
        }
    }

    impl WidgetImpl for DesktopLyricsWindow {
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            // Window resize invalidates the X11 SHAPE input rectangle.
            // Re-apply it so that the input region always covers the
            // full window area after a resize.
            if !self.locked.get() {
                self.obj().reapply_lock_state_soon();
            }
        }
    }
    impl WindowImpl for DesktopLyricsWindow {}
}

glib::wrapper! {
    pub struct DesktopLyricsWindow(ObjectSubclass<imp::DesktopLyricsWindow>)
        @extends gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Native;
}

impl DesktopLyricsWindow {
    pub fn new(app: &impl IsA<gtk::Application>) -> Self {
        glib::Object::builder().property("application", app).build()
    }

    fn setup_window(&self) {
        self.set_title(Some("Spot-Lyric"));
        self.set_decorated(false);
        self.set_resizable(false);
        self.set_deletable(false);
        self.add_css_class("desktop-lyrics-window");

        // Ensure we own a 32-bit visual when a compositor is around so that
        // `background: transparent` actually composes the windows below.
        // GTK4 picks the right visual automatically when the CSS background
        // is transparent; nothing to do explicitly.

        let settings = gio::Settings::new(config::APP_ID);
        let width = settings.int("desktop-lyrics-width").max(320);
        self.set_default_size(width, -1);
        self.set_size_request(width, -1);
        *self.imp().settings.borrow_mut() = Some(settings);
    }

    fn build_ui(&self) {
        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .css_classes(["desktop-lyrics-container"])
            .tooltip_text("关闭锁定后，可拖动系统窗口栏移动")
            .build();

        let active_label = gtk::Label::builder()
            .label("")
            .css_classes(["desktop-lyrics-active"])
            .wrap(true)
            .wrap_mode(pango::WrapMode::WordChar)
            .justify(gtk::Justification::Center)
            .halign(gtk::Align::Fill)
            .hexpand(true)
            .build();

        let next_label = gtk::Label::builder()
            .label("")
            .css_classes(["desktop-lyrics-next"])
            .wrap(true)
            .wrap_mode(pango::WrapMode::WordChar)
            .justify(gtk::Justification::Center)
            .halign(gtk::Align::Fill)
            .hexpand(true)
            .build();

        container.append(&active_label);
        container.append(&next_label);

        self.set_child(Some(&container));

        let imp = self.imp();
        *imp.container.borrow_mut() = Some(container);
        *imp.active_label.borrow_mut() = Some(active_label);
        *imp.next_label.borrow_mut() = Some(next_label);

        self.apply_style();
    }

    fn bind_settings(&self) {
        let imp = self.imp();
        let settings = imp
            .settings
            .borrow()
            .as_ref()
            .expect("settings created in setup_window")
            .clone();

        // React to any desktop-lyrics-* change.
        let weak = self.downgrade();
        let handler = settings.connect_changed(None, move |_, key| {
            let Some(win) = weak.upgrade() else { return };
            if !key.starts_with("desktop-lyrics-") {
                return;
            }
            match key {
                "desktop-lyrics-locked" => {
                    let locked = win
                        .imp()
                        .settings
                        .borrow()
                        .as_ref()
                        .map(|s| s.boolean("desktop-lyrics-locked"))
                        .unwrap_or(true);
                    win.apply_lock_state(locked);
                }
                "desktop-lyrics-enabled" => {
                    let enabled = win
                        .imp()
                        .settings
                        .borrow()
                        .as_ref()
                        .map(|s| s.boolean("desktop-lyrics-enabled"))
                        .unwrap_or(true);
                    if enabled {
                        win.show_window();
                    } else {
                        win.set_visible(false);
                    }
                }
                "desktop-lyrics-width" => {
                    if let Some(s) = win.imp().settings.borrow().as_ref() {
                        let w = s.int("desktop-lyrics-width").max(320);
                        win.set_default_size(w, -1);
                        win.set_size_request(w, -1);
                    }
                    win.restore_position();
                    win.apply_style();
                }
                "desktop-lyrics-x" | "desktop-lyrics-y" | "desktop-lyrics-bottom-margin" => {
                    win.restore_position();
                    win.apply_style();
                }
                "desktop-lyrics-show-translation" => {
                    let value = win
                        .imp()
                        .settings
                        .borrow()
                        .as_ref()
                        .map(|s| s.boolean("desktop-lyrics-show-translation"))
                        .unwrap_or(true);
                    win.imp().show_translation.set(value);
                    // Force a re-render of the active line.
                    let pos = win.imp().clock.estimate();
                    win.imp().active_index.set(None);
                    win.update_active_line(pos);
                }
                "desktop-lyrics-line-mode" => {
                    let dual = win
                        .imp()
                        .settings
                        .borrow()
                        .as_ref()
                        .map(|s| s.string("desktop-lyrics-line-mode").as_str() == "dual")
                        .unwrap_or(true);
                    win.imp().line_mode_dual.set(dual);
                    if let Some(label) = win.imp().next_label.borrow().as_ref() {
                        label.set_visible(dual);
                    }
                }
                _ => win.apply_style(),
            }
        });

        // Initial state from settings.
        imp.locked.set(settings.boolean("desktop-lyrics-locked"));
        imp.show_translation
            .set(settings.boolean("desktop-lyrics-show-translation"));
        let dual = settings.string("desktop-lyrics-line-mode").as_str() == "dual";
        imp.line_mode_dual.set(dual);
        if let Some(label) = imp.next_label.borrow().as_ref() {
            label.set_visible(dual);
        }
        *imp.settings_handler.borrow_mut() = Some(handler);
    }

    /// Build a per-window CssProvider from the user's settings and attach it
    /// to the default display. Re-runs whenever any styling key changes.
    fn apply_style(&self) {
        let imp = self.imp();
        let Some(settings) = imp.settings.borrow().as_ref().cloned() else {
            return;
        };

        let font = settings.string("desktop-lyrics-font").to_string();
        let active_color = settings.string("desktop-lyrics-active-color").to_string();
        let inactive_color = settings.string("desktop-lyrics-inactive-color").to_string();
        let stroke_color = settings.string("desktop-lyrics-stroke-color").to_string();
        let stroke_width = settings.int("desktop-lyrics-stroke-width").max(0);
        let bg_color = settings.string("desktop-lyrics-bg-color").to_string();
        let bg_opacity = settings.double("desktop-lyrics-bg-opacity").clamp(0.0, 1.0);
        let line_height = settings
            .double("desktop-lyrics-line-height")
            .clamp(0.9, 3.0);

        let (br, bg, bb) = hex_to_rgb(&bg_color);

        let font_active_css = font_css_from_description(&font, 1.0);
        let font_next_css = font_css_from_description(&font, 0.75);

        // Rough pixel-stroke approximation via 8-direction text-shadow.
        let stroke_shadow = if stroke_width > 0 {
            let sw = stroke_width;
            format!(
                "text-shadow:
                  -{sw}px -{sw}px 0 {sc}, {sw}px -{sw}px 0 {sc},
                  -{sw}px  {sw}px 0 {sc}, {sw}px  {sw}px 0 {sc},
                  0 -{sw}px 0 {sc}, 0  {sw}px 0 {sc},
                  -{sw}px 0 0 {sc}, {sw}px 0 0 {sc};",
                sc = stroke_color
            )
        } else {
            String::new()
        };

        let css = format!(
            ".desktop-lyrics-active {{
                {font_active_css}
                color: {active_color};
                line-height: {line_height};
                {stroke_shadow}
            }}
            .desktop-lyrics-translation {{
                {font_next_css}
                color: alpha({active_color}, 0.85);
            }}
            .desktop-lyrics-next {{
                {font_next_css}
                color: {inactive_color};
                line-height: {line_height};
                {stroke_shadow}
            }}
            .desktop-lyrics-container {{
                background-color: rgba({br}, {bg}, {bb}, {bg_opacity});
            }}",
        );

        let display = match gdk::Display::default() {
            Some(d) => d,
            None => return,
        };

        if let Some(old) = imp.css_provider.borrow().as_ref() {
            gtk::style_context_remove_provider_for_display(&display, old);
        }

        let provider = gtk::CssProvider::new();
        provider.load_from_string(&css);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
        *imp.css_provider.borrow_mut() = Some(provider);
        if !imp.locked.get() {
            self.reapply_lock_state_soon();
        }
    }

    fn realize_x11(&self) {
        let imp = self.imp();

        let surface = match self.surface() {
            Some(surface) => surface,
            None => return,
        };

        // We only support X11 in v1; on Wayland this downcast fails and the
        // overlay degrades to an ordinary undecorated window.
        let Some(x11_surface) = surface.downcast_ref::<gdk4_x11::X11Surface>() else {
            tracing::warn!(
                "Not an X11 surface — overlay features (keep-above, click-through) disabled"
            );
            return;
        };

        let xid = x11_surface.xid() as u32;
        imp.xid.set(Some(xid));

        let helper = match X11Helper::connect() {
            Ok(helper) => helper,
            Err(error) => {
                tracing::warn!("X11 helper init failed: {error}");
                return;
            }
        };

        if let Err(error) = helper.make_overlay(xid, config::APP_ID, APP_INSTANCE) {
            tracing::warn!("make_overlay failed: {error}");
        }

        *imp.x11.borrow_mut() = Some(helper);

        // Apply saved position (or default to bottom-center).
        self.restore_position();
        // Apply lock state (input passthrough).
        self.apply_lock_state(imp.locked.get());
        self.reapply_lock_state_soon();
    }

    fn restore_position(&self) {
        let imp = self.imp();
        let (Some(helper), Some(xid)) = (imp.x11.borrow().as_ref().cloned(), imp.xid.get()) else {
            return;
        };
        let settings = match imp.settings.borrow().as_ref().cloned() {
            Some(s) => s,
            None => return,
        };

        let saved_x = settings.int("desktop-lyrics-x");
        let saved_y = settings.int("desktop-lyrics-y");

        if has_saved_position(saved_x, saved_y) {
            let _ = helper.move_window(xid, saved_x, saved_y);
            return;
        }

        if let Ok(geometry) = helper.primary_monitor_geometry() {
            let bottom_margin = settings.int("desktop-lyrics-bottom-margin").max(0);
            let w = self
                .default_width()
                .max(settings.int("desktop-lyrics-width"));
            let h = self.height().max(120);
            let (x, y) = default_overlay_position(geometry, w, h, bottom_margin);
            let _ = helper.move_window(xid, x, y);
        }
    }

    /// Push an absolute lock state down to the X11 input shape.
    pub fn apply_lock_state(&self, locked: bool) {
        let imp = self.imp();
        imp.locked.set(locked);
        self.set_decorated(desktop_system_titlebar_visible(locked));
        if let (Some(helper), Some(xid)) = (imp.x11.borrow().as_ref().cloned(), imp.xid.get()) {
            if let Err(error) = helper.set_input_passthrough(xid, locked) {
                tracing::warn!("set_input_passthrough failed: {error}");
            }
        }
        self.set_cursor_from_name(None);
        if let Some(container) = imp.container.borrow().as_ref() {
            container.set_cursor_from_name(None);
        }
        if let Some(label) = imp.active_label.borrow().as_ref() {
            label.set_cursor_from_name(None);
        }
        if let Some(label) = imp.next_label.borrow().as_ref() {
            label.set_cursor_from_name(None);
        }
    }

    /// Toggle and persist the lock state.
    pub fn toggle_lock(&self) {
        let new_state = !self.imp().locked.get();
        if let Some(settings) = self.imp().settings.borrow().as_ref() {
            let _ = settings.set_boolean("desktop-lyrics-locked", new_state);
        }
        // bind_settings() will pick up the change and call apply_lock_state.
    }

    pub fn is_locked(&self) -> bool {
        self.imp().locked.get()
    }

    fn reapply_lock_state_soon(&self) {
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            let Some(win) = weak.upgrade() else { return };
            win.apply_lock_state(win.imp().locked.get());
        });
    }

    fn start_clock(&self) {
        let weak = self.downgrade();
        let id = glib::timeout_add_local(
            std::time::Duration::from_millis(config::POSITION_TICK_MS),
            move || {
                if let Some(win) = weak.upgrade() {
                    let pos = win.imp().clock.estimate();
                    win.update_active_line(pos);
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                }
            },
        );
        self.imp().clock_tick_id.set(Some(id));
    }

    // ─── Public API ────────────────────────────────────────────────────────

    pub fn show_window(&self) {
        self.set_visible(true);
        self.present();
        // Re-apply EWMH hints — some WMs drop them on unmap → map.
        if let (Some(helper), Some(xid)) = (
            self.imp().x11.borrow().as_ref().cloned(),
            self.imp().xid.get(),
        ) {
            let _ = helper.make_overlay(xid, config::APP_ID, APP_INSTANCE);
            let locked = self.imp().locked.get();
            let _ = helper.set_input_passthrough(xid, locked);
        }
        self.reapply_lock_state_soon();

        // Delayed re-apply: some reparenting WMs (Cinnamon/Muffin, KWin)
        // create their frame window asynchronously after the map request.
        // Without this the frame window may inherit an empty input shape from
        // a previous locked state, blocking pointer events and making drag
        // unreliable.
        let weak = self.downgrade();
        glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
            let Some(win) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            win.apply_lock_state(win.imp().locked.get());
            glib::ControlFlow::Break
        });
    }

    pub fn hide_window(&self) {
        self.set_visible(false);
    }

    /// Apply a new playback state (from D-Bus). Resets the position clock.
    pub fn apply_playback(&self, state: &PlaybackState) {
        let imp = self.imp();

        let label = if state.track_name.is_empty() {
            "♪ —".to_string()
        } else if state.artist_name.is_empty() {
            format!("♪ {}", state.track_name)
        } else {
            format!("♪ {} — {}", state.track_name, state.artist_name)
        };
        *imp.current_track_label.borrow_mut() = label.clone();

        // Track changed? Reset lyrics + remember new uri.
        let changed = imp.current_track_uri.borrow().as_str() != state.track_uri.as_str();
        if changed {
            *imp.current_track_uri.borrow_mut() = state.track_uri.clone();
            imp.clock.reset();
            self.set_lyrics(&LyricsPayload::default());
        }

        // Reset clock so insertion happens at the daemon's reported position.
        let position_ms =
            imp.clock
                .snapshot(state.position_ms, state.duration_ms, state.is_playing);
        tracing::debug!(
            target: "spot_lyric_gtk::timeline",
            surface = "desktop",
            raw_position_ms = state.position_ms,
            estimated_position_ms = position_ms,
            is_playing = state.is_playing,
            track_uri = %state.track_uri,
            "playback snapshot applied"
        );

        // If we have no lyrics, fall back to track label on the active line.
        if imp.lyrics.borrow().is_empty() {
            if let Some(active) = imp.active_label.borrow().as_ref() {
                active.set_text(&label);
            }
            if let Some(next) = imp.next_label.borrow().as_ref() {
                next.set_text("");
            }
        } else {
            // Render the daemon snapshot immediately so pause/resume/seek events
            // do not wait for the next interpolation tick.
            self.update_active_line(position_ms);
        }
    }

    /// Apply a fresh lyrics payload (line-synced) for the *current* track.
    pub fn set_lyrics(&self, payload: &LyricsPayload) {
        let imp = self.imp();
        *imp.lyrics.borrow_mut() = payload.lines.clone();
        imp.active_index.set(None);

        if let Some(label) = imp.active_label.borrow().as_ref() {
            label.set_text(&imp.current_track_label.borrow());
        }
        if let Some(label) = imp.next_label.borrow().as_ref() {
            label.set_text("");
        }

        // Force a render based on the current clock estimate.
        let pos = imp.clock.estimate();
        self.update_active_line(pos);
    }

    pub fn current_track_uri(&self) -> String {
        self.imp().current_track_uri.borrow().clone()
    }

    fn update_active_line(&self, position_ms: i64) {
        let imp = self.imp();
        let lines = imp.lyrics.borrow();
        if lines.is_empty() {
            return;
        }

        let new_index = lines
            .iter()
            .rposition(|line| line.start_time_ms <= position_ms);

        let previous_index = imp.active_index.get();
        if new_index == previous_index {
            return;
        }
        tracing::debug!(
            target: "spot_lyric_gtk::timeline",
            surface = "desktop",
            position_ms,
            previous_index = ?previous_index,
            new_index = ?new_index,
            line_start_ms = new_index.and_then(|idx| lines.get(idx).map(|line| line.start_time_ms)),
            next_start_ms = new_index.and_then(|idx| lines.get(idx + 1).map(|line| line.start_time_ms)),
            "active lyric line changed"
        );
        imp.active_index.set(new_index);

        let show_translation = imp.show_translation.get();
        let dual = imp.line_mode_dual.get();

        if let Some(idx) = new_index {
            if let Some(label) = imp.active_label.borrow().as_ref() {
                let line = &lines[idx];
                let markup = build_active_markup(line, show_translation);
                label.set_markup(&markup);
            }
            if let Some(label) = imp.next_label.borrow().as_ref() {
                if dual {
                    let next = lines
                        .get(idx + 1)
                        .map(|line| line.text.clone())
                        .unwrap_or_default();
                    label.set_text(&next);
                } else {
                    label.set_text("");
                }
            }
        } else {
            // Position before the first line — keep the track label.
            if let Some(label) = imp.active_label.borrow().as_ref() {
                label.set_text(&imp.current_track_label.borrow());
            }
            if let Some(label) = imp.next_label.borrow().as_ref() {
                label.set_text("");
            }
        }
    }
}

fn build_active_markup(line: &LyricsLine, show_translation: bool) -> String {
    let escaped = glib::markup_escape_text(&line.text);
    match line.translated_text.as_deref() {
        Some(trans) if show_translation && !trans.trim().is_empty() => {
            let trans_escaped = glib::markup_escape_text(trans);
            format!("{escaped}\n<span size=\"smaller\">{trans_escaped}</span>")
        }
        _ => escaped.to_string(),
    }
}

fn has_saved_position(x: i32, y: i32) -> bool {
    x != -1 && y != -1
}

fn desktop_system_titlebar_visible(locked: bool) -> bool {
    !locked
}

fn default_overlay_position(
    geometry: MonitorGeometry,
    window_width: i32,
    window_height: i32,
    bottom_margin: i32,
) -> (i32, i32) {
    let width = window_width.max(1);
    let height = window_height.max(1);
    let x = geometry.x + (geometry.width - width) / 2;
    let y = geometry.y + geometry.height - height - bottom_margin.max(0);
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_overlay_position_uses_monitor_origin() {
        let geometry = MonitorGeometry {
            x: 1920,
            y: 0,
            width: 1920,
            height: 1080,
        };

        assert_eq!(
            default_overlay_position(geometry, 900, 120, 80),
            (2430, 880)
        );
    }

    #[test]
    fn unlocked_state_shows_system_titlebar() {
        assert!(desktop_system_titlebar_visible(false));
    }

    #[test]
    fn locked_state_hides_system_titlebar() {
        assert!(!desktop_system_titlebar_visible(true));
    }

    #[test]
    fn saved_position_allows_negative_screen_coordinates() {
        assert!(has_saved_position(-400, 80));
        assert!(!has_saved_position(-1, 80));
    }

    #[test]
    fn active_markup_with_translation_is_valid_pango_markup() {
        let line = LyricsLine {
            text: "キラキラ".into(),
            translated_text: Some("闪耀".into()),
            start_time_ms: 0,
            end_time_ms: 1_000,
            words: Vec::new(),
        };
        let markup = build_active_markup(&line, true);

        assert!(!markup.contains("class="));
        pango::parse_markup(&markup, '\0').expect("valid Pango markup");
    }
}
