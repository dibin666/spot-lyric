//! Secondary window that shows the full lyric timeline for the current track.

mod layout;
mod positioning;
mod rows;
mod sync;
mod ui;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::subclass::prelude::*;
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::dbus::types::LyricsLine;
use crate::utils::position_clock::CellClock;

mod imp {
    use super::*;

    pub struct FullLyricsWindow {
        pub title_label: RefCell<Option<gtk::Label>>,
        pub subtitle_label: RefCell<Option<gtk::Label>>,
        pub stack: RefCell<Option<gtk::Stack>>,
        pub empty_label: RefCell<Option<gtk::Label>>,
        pub scrolled: RefCell<Option<gtk::ScrolledWindow>>,
        pub lines_box: RefCell<Option<gtk::Box>>,
        pub rows: RefCell<Vec<gtk::Box>>,
        pub lyrics: RefCell<Vec<LyricsLine>>,
        pub active_index: Cell<Option<usize>>,
        pub current_track_uri: RefCell<String>,
        pub current_track_label: RefCell<String>,
        pub clock: Rc<CellClock>,
        pub clock_tick_id: Cell<Option<glib::SourceId>>,
        pub lock_tick_id: Cell<Option<glib::SourceId>>,
        pub last_locked_layout: Cell<Option<(i32, i32, i32, i32)>>,
    }

    impl Default for FullLyricsWindow {
        fn default() -> Self {
            Self {
                title_label: RefCell::new(None),
                subtitle_label: RefCell::new(None),
                stack: RefCell::new(None),
                empty_label: RefCell::new(None),
                scrolled: RefCell::new(None),
                lines_box: RefCell::new(None),
                rows: RefCell::new(Vec::new()),
                lyrics: RefCell::new(Vec::new()),
                active_index: Cell::new(None),
                current_track_uri: RefCell::new(String::new()),
                current_track_label: RefCell::new(String::new()),
                clock: Rc::new(CellClock::new()),
                clock_tick_id: Cell::new(None),
                lock_tick_id: Cell::new(None),
                last_locked_layout: Cell::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FullLyricsWindow {
        const NAME: &'static str = "SpotLyricFullLyricsWindow";
        type Type = super::FullLyricsWindow;
        type ParentType = gtk::Window;
    }

    impl ObjectImpl for FullLyricsWindow {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.setup_window();
            obj.build_ui();
            obj.start_clock();
            obj.connect_close_request(|win| {
                win.hide_locked();
                glib::Propagation::Stop
            });
        }

        fn dispose(&self) {
            if let Some(id) = self.clock_tick_id.take() {
                id.remove();
            }
            if let Some(id) = self.lock_tick_id.take() {
                id.remove();
            }
        }
    }

    impl WidgetImpl for FullLyricsWindow {}
    impl WindowImpl for FullLyricsWindow {}
}

glib::wrapper! {
    pub struct FullLyricsWindow(ObjectSubclass<imp::FullLyricsWindow>)
        @extends gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Native;
}

impl FullLyricsWindow {
    pub fn new(app: &impl IsA<gtk::Application>) -> Self {
        glib::Object::builder().property("application", app).build()
    }
}
