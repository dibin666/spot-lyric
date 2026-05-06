use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

use crate::backend_runtime::BackendRuntime;
use crate::bridge;
use crate::config;
use crate::tray;
use crate::widgets::{
    desktop_lyrics_window::DesktopLyricsWindow, full_lyrics_window::FullLyricsWindow,
    preferences_window::PreferencesWindow,
};

fn present_preferences_window(prefs: &PreferencesWindow) {
    prefs.set_visible(true);
    prefs.present();
}

fn sync_tray_desktop_settings(
    settings: &gio::Settings,
    tray_state: &std::sync::Arc<std::sync::Mutex<tray::TrayState>>,
    tray_handle: &Rc<RefCell<Option<tray::TrayHandle>>>,
) {
    {
        let mut state = tray_state.lock().unwrap();
        tray::apply_desktop_settings_to_state(
            &mut state,
            settings.boolean("desktop-lyrics-enabled"),
            settings.boolean("desktop-lyrics-locked"),
        );
    }
    if let Some(handle) = tray_handle.borrow().as_ref() {
        handle.refresh();
    }
}

mod imp {
    use super::*;

    pub struct SpotLyricApplication {
        pub preferences_window: RefCell<Option<PreferencesWindow>>,
        pub desktop_lyrics: RefCell<Option<DesktopLyricsWindow>>,
        pub full_lyrics: RefCell<Option<FullLyricsWindow>>,
        pub tray_state: std::sync::Arc<std::sync::Mutex<tray::TrayState>>,
        pub tray_handle: Rc<RefCell<Option<tray::TrayHandle>>>,
        pub backend_runtime: RefCell<BackendRuntime>,
        pub hold_guard: RefCell<Option<gio::ApplicationHoldGuard>>,
        pub desktop_settings: RefCell<Option<gio::Settings>>,
        pub desktop_settings_handler: RefCell<Option<glib::SignalHandlerId>>,
    }

    impl Default for SpotLyricApplication {
        fn default() -> Self {
            Self {
                preferences_window: RefCell::new(None),
                desktop_lyrics: RefCell::new(None),
                full_lyrics: RefCell::new(None),
                tray_state: std::sync::Arc::new(std::sync::Mutex::new(tray::TrayState::default())),
                tray_handle: Rc::new(RefCell::new(None)),
                backend_runtime: RefCell::new(BackendRuntime::default()),
                hold_guard: RefCell::new(None),
                desktop_settings: RefCell::new(None),
                desktop_settings_handler: RefCell::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SpotLyricApplication {
        const NAME: &'static str = "SpotLyricApplication";
        type Type = super::SpotLyricApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for SpotLyricApplication {}

    impl ApplicationImpl for SpotLyricApplication {
        fn startup(&self) {
            self.parent_startup();
            let app = self.obj();

            // 1. Load style.css
            if let Some(display) = gtk::gdk::Display::default() {
                let provider = gtk::CssProvider::new();
                provider.load_from_resource("/cn/spotlyric/Gtk/style.css");
                gtk::style_context_add_provider_for_display(
                    &display,
                    &provider,
                    gtk::STYLE_PROVIDER_PRIORITY_USER,
                );
            }

            // 2. Start bridge
            let (cmd_tx, ui_rx) = bridge::Bridge::start(self.backend_runtime.borrow().clone());

            // 3. Start tray
            let (action_tx, mut action_rx) = tokio::sync::mpsc::unbounded_channel();
            let tray_state = self.tray_state.clone();
            let tray_handle = self.tray_handle.clone();
            tray::StatusNotifierTray::spawn(
                action_tx,
                cmd_tx.cmd_tx.clone(),
                tray_state.clone(),
                tray_handle.clone(),
            );

            let desktop_settings = gio::Settings::new(config::APP_ID);
            sync_tray_desktop_settings(&desktop_settings, &tray_state, &tray_handle);
            let signal_tray_state = tray_state.clone();
            let signal_tray_handle = tray_handle.clone();
            let handler = desktop_settings.connect_changed(None, move |settings, key| {
                if key == "desktop-lyrics-enabled" || key == "desktop-lyrics-locked" {
                    sync_tray_desktop_settings(settings, &signal_tray_state, &signal_tray_handle);
                }
            });
            *self.desktop_settings.borrow_mut() = Some(desktop_settings);
            *self.desktop_settings_handler.borrow_mut() = Some(handler);

            // Handle tray actions in GTK thread
            let app_clone = app.clone();
            glib::MainContext::default().spawn_local(async move {
                while let Some(action) = action_rx.recv().await {
                    match action {
                        tray::TrayAction::ToggleLyrics => {
                            app_clone.activate_action("toggle-lyrics", None);
                        }
                        tray::TrayAction::ToggleLock => {
                            app_clone.activate_action("toggle-lock", None);
                        }
                        tray::TrayAction::Preferences => {
                            app_clone.activate();
                        }
                        tray::TrayAction::MatchLyrics => {
                            app_clone.activate_action("match-lyrics", None);
                        }
                        tray::TrayAction::Quit => {
                            app_clone.activate_action("quit", None);
                        }
                        tray::TrayAction::SetProvider(_) => {} // handeld by cmd_tx in Tray directly
                    }
                }
            });

            // 4. Hold the app to keep it running when windows are closed
            *self.hold_guard.borrow_mut() = Some(app.hold());

            // 5. Create Preferences Window early to receive bridge events, but don't present yet
            let prefs = PreferencesWindow::new(&*app);
            // 8. Give bridge handle to prefs window
            prefs.attach_bridge(cmd_tx.cmd_tx.clone());

            let desktop = DesktopLyricsWindow::new(&*app);
            let full_lyrics = FullLyricsWindow::new(&*app);
            prefs.attach_full_lyrics_window(full_lyrics.clone());

            // Setup dispatcher
            crate::widgets::preferences_window::install_ui_dispatcher(
                &prefs,
                ui_rx,
                desktop.clone(),
                full_lyrics.clone(),
                self.tray_state.clone(),
                self.tray_handle.clone(),
            );

            // Register actions
            let quit_action = gio::SimpleAction::new("quit", None);
            let app_clone = app.clone();
            quit_action.connect_activate(move |_, _| {
                app_clone.quit();
            });
            app.add_action(&quit_action);
            app.set_accels_for_action("app.quit", &["<Ctrl>Q"]);

            let prefs_action = gio::SimpleAction::new("preferences", None);
            let prefs_clone = prefs.clone();
            prefs_action.connect_activate(move |_, _| {
                present_preferences_window(&prefs_clone);
            });
            app.add_action(&prefs_action);
            app.set_accels_for_action("app.preferences", &["<Ctrl>comma"]);

            let desktop_clone = desktop.clone();
            let toggle_lyrics_action = gio::SimpleAction::new("toggle-lyrics", None);
            toggle_lyrics_action.connect_activate(move |_, _| {
                let settings = gio::Settings::new(config::APP_ID);
                let current = settings.boolean("desktop-lyrics-enabled");
                let _ = settings.set_boolean("desktop-lyrics-enabled", !current);
                if !current {
                    desktop_clone.show_window();
                } else {
                    desktop_clone.hide_window();
                }
            });
            app.add_action(&toggle_lyrics_action);
            app.set_accels_for_action("app.toggle-lyrics", &["<Ctrl><Shift>L"]);

            let desktop_clone = desktop.clone();
            let toggle_lock_action = gio::SimpleAction::new("toggle-lock", None);
            toggle_lock_action.connect_activate(move |_, _| {
                desktop_clone.toggle_lock();
            });
            app.add_action(&toggle_lock_action);
            app.set_accels_for_action("app.toggle-lock", &["<Ctrl><Shift>K"]);

            let prefs_clone = prefs.clone();
            let match_lyrics_action = gio::SimpleAction::new("match-lyrics", None);
            match_lyrics_action.connect_activate(move |_, _| {
                present_preferences_window(&prefs_clone);
                let _ = prefs_clone.open_manual_match_dialog();
            });
            app.add_action(&match_lyrics_action);
            app.set_accels_for_action("app.match-lyrics", &["<Ctrl><Shift>M"]);

            *self.preferences_window.borrow_mut() = Some(prefs);
            *self.desktop_lyrics.borrow_mut() = Some(desktop);
            *self.full_lyrics.borrow_mut() = Some(full_lyrics);
        }

        fn shutdown(&self) {
            self.backend_runtime.borrow().shutdown();
            if let (Some(settings), Some(handler)) = (
                self.desktop_settings.borrow().as_ref(),
                self.desktop_settings_handler.borrow_mut().take(),
            ) {
                settings.disconnect(handler);
            }
            self.desktop_settings.borrow_mut().take();
            self.hold_guard.borrow_mut().take();
            self.parent_shutdown();
        }

        fn activate(&self) {
            self.parent_activate();

            if let Some(prefs) = self.preferences_window.borrow().as_ref() {
                present_preferences_window(prefs);
                if let Some(full) = self.full_lyrics.borrow().as_ref() {
                    full.present_locked_beside(prefs);
                }
            }

            if let Some(desktop) = self.desktop_lyrics.borrow().as_ref() {
                let settings = gio::Settings::new(config::APP_ID);
                if settings.boolean("desktop-lyrics-enabled") {
                    desktop.show_window();
                }
            }
        }
    }

    impl GtkApplicationImpl for SpotLyricApplication {}
    impl AdwApplicationImpl for SpotLyricApplication {}
}

glib::wrapper! {
    pub struct SpotLyricApplication(ObjectSubclass<imp::SpotLyricApplication>)
        @extends gio::Application, gtk::Application, adw::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl SpotLyricApplication {
    pub fn new(backend_runtime: BackendRuntime) -> Self {
        let app: Self = glib::Object::builder()
            .property("application-id", config::APP_ID)
            .property("flags", gio::ApplicationFlags::FLAGS_NONE)
            .build();
        app.imp().backend_runtime.replace(backend_runtime);
        app
    }
}
