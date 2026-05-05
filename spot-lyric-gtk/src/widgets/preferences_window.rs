//! Adwaita preferences window — the program's main GUI surface.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc as std_mpsc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib::clone;
use gtk::prelude::*;
use gtk::{gio, glib};
use tokio::sync::mpsc;

use crate::bridge::Command;
use crate::config;
use crate::dbus::types::{AuthSnapshot, LyricsSettings};
use crate::dialogs::{auth_dialog, lyrics_match_dialog};
use crate::utils::{rgba_from_hex, rgba_to_hex};

mod imp {
    use super::*;

    pub struct PreferencesWindow {
        pub cmd_tx: RefCell<Option<mpsc::UnboundedSender<Command>>>,
        pub gsettings: RefCell<Option<gio::Settings>>,

        pub banner: RefCell<Option<adw::Banner>>,
        pub toast_overlay: RefCell<Option<adw::ToastOverlay>>,

        // Account
        pub account_status_row: RefCell<Option<adw::ActionRow>>,
        pub account_status_dot: RefCell<Option<gtk::Box>>,
        pub profile_combo: RefCell<Option<adw::ComboRow>>,

        // Lyrics
        pub provider_combo: RefCell<Option<adw::ComboRow>>,
        pub offset_spin: RefCell<Option<adw::SpinRow>>,
        pub manual_match_button: RefCell<Option<gtk::Button>>,
        pub manual_match_row: RefCell<Option<adw::ActionRow>>,

        // Display
        pub font_button: RefCell<Option<gtk::FontDialogButton>>,

        pub last_track_uri: RefCell<String>,
        pub last_track_label: RefCell<String>,
        pub last_auth: RefCell<Option<AuthSnapshot>>,
        pub last_settings: RefCell<Option<LyricsSettings>>,
        pub suppress_signals: Cell<bool>,
    }

    impl Default for PreferencesWindow {
        fn default() -> Self {
            Self {
                cmd_tx: RefCell::new(None),
                gsettings: RefCell::new(None),
                banner: RefCell::new(None),
                toast_overlay: RefCell::new(None),
                account_status_row: RefCell::new(None),
                account_status_dot: RefCell::new(None),
                profile_combo: RefCell::new(None),
                provider_combo: RefCell::new(None),
                offset_spin: RefCell::new(None),
                manual_match_button: RefCell::new(None),
                manual_match_row: RefCell::new(None),
                font_button: RefCell::new(None),
                last_track_uri: RefCell::new(String::new()),
                last_track_label: RefCell::new(String::new()),
                last_auth: RefCell::new(None),
                last_settings: RefCell::new(None),
                suppress_signals: Cell::new(false),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PreferencesWindow {
        const NAME: &'static str = "SpotLyricPreferencesWindow";
        type Type = super::PreferencesWindow;
        type ParentType = adw::PreferencesWindow;
    }

    impl ObjectImpl for PreferencesWindow {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.build_ui();
            obj.bind_settings();

            // Hide instead of destroy so the tray can re-present us.
            obj.connect_close_request(|win| {
                win.set_visible(false);
                glib::Propagation::Stop
            });
        }
    }

    impl WidgetImpl for PreferencesWindow {}
    impl WindowImpl for PreferencesWindow {}
    impl AdwWindowImpl for PreferencesWindow {}
    impl PreferencesWindowImpl for PreferencesWindow {}
}

glib::wrapper! {
    pub struct PreferencesWindow(ObjectSubclass<imp::PreferencesWindow>)
        @extends adw::PreferencesWindow, adw::Window, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible,
                    gtk::Buildable, gtk::Native, gtk::Root;
}

impl PreferencesWindow {
    pub fn new(app: &impl IsA<gtk::Application>) -> Self {
        glib::Object::builder()
            .property("application", app)
            .property("title", "Spot-Lyric")
            .property("default-width", 620)
            .property("default-height", 720)
            .build()
    }

    pub fn attach_bridge(&self, cmd_tx: mpsc::UnboundedSender<Command>) {
        *self.imp().cmd_tx.borrow_mut() = Some(cmd_tx);
    }

    fn send(&self, cmd: Command) {
        if let Some(tx) = self.imp().cmd_tx.borrow().as_ref() {
            let _ = tx.send(cmd);
        }
    }

    fn build_ui(&self) {
        let settings = gio::Settings::new(config::APP_ID);
        *self.imp().gsettings.borrow_mut() = Some(settings.clone());

        let banner = adw::Banner::builder()
            .revealed(false)
            .button_label("重试")
            .build();
        let weak = self.downgrade();
        banner.connect_button_clicked(move |_| {
            if let Some(win) = weak.upgrade() {
                win.send(Command::Reconnect);
            }
        });

        // adw::PreferencesWindow doesn't expose an explicit content slot; we
        // use the built-in `add` for pages and rely on a top banner via
        // `set_search_enabled(false)` + custom child.
        // Adwaita 1.5 added `add_toast` directly on PreferencesWindow.
        // We keep the banner separate by putting it inside a wrapper page.

        self.add(&self.build_account_page());
        self.add(&self.build_lyrics_page(&settings));
        self.add(&self.build_display_page(&settings));
        self.add(&self.build_about_page());

        *self.imp().banner.borrow_mut() = Some(banner);
    }

    fn bind_settings(&self) {
        let imp = self.imp();
        let Some(settings) = imp.gsettings.borrow().as_ref().cloned() else {
            return;
        };

        // Two-way bind the offset spin row to GSettings + send to daemon.
        if let Some(spin) = imp.offset_spin.borrow().as_ref() {
            settings.bind("timing-offset-ms", spin, "value").build();
            let weak = self.downgrade();
            spin.connect_changed(move |row| {
                let Some(win) = weak.upgrade() else { return };
                if win.imp().suppress_signals.get() {
                    return;
                }
                win.send(Command::SetTimingOffsetMs(row.value() as i32));
            });
        }
    }

    fn build_account_page(&self) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder()
            .title("账号")
            .icon_name("avatar-default-symbolic")
            .build();

        let group = adw::PreferencesGroup::builder()
            .title("Spotify 账号")
            .description("使用 Cookie 登录 Spotify 网页版以获取播放状态")
            .build();

        let dot = gtk::Box::builder()
            .css_classes(["spotlyric-status-dot", "idle"])
            .valign(gtk::Align::Center)
            .build();

        let status_row = adw::ActionRow::builder()
            .title("未登录")
            .subtitle("尚未导入 Cookie")
            .build();
        status_row.add_prefix(&dot);
        group.add(&status_row);

        let import_file_btn = gtk::Button::builder()
            .label("导入 cookies.txt 文件…")
            .css_classes(["suggested-action"])
            .build();
        let weak = self.downgrade();
        import_file_btn.connect_clicked(move |_| {
            if let Some(win) = weak.upgrade() {
                win.show_cookie_file_chooser();
            }
        });

        let paste_btn = gtk::Button::builder().label("粘贴 Cookie 字符串…").build();
        let weak = self.downgrade();
        paste_btn.connect_clicked(move |_| {
            if let Some(win) = weak.upgrade() {
                let tx = win
                    .imp()
                    .cmd_tx
                    .borrow()
                    .as_ref()
                    .cloned()
                    .expect("bridge attached");
                auth_dialog::show_paste_dialog(win.upcast_ref::<gtk::Widget>(), tx);
            }
        });

        let refresh_btn = gtk::Button::builder().label("刷新 Token").build();
        let weak = self.downgrade();
        refresh_btn.connect_clicked(move |_| {
            if let Some(win) = weak.upgrade() {
                win.send(Command::RefreshAuth);
            }
        });

        let clear_btn = gtk::Button::builder()
            .label("清除登录")
            .css_classes(["destructive-action"])
            .build();
        let weak = self.downgrade();
        clear_btn.connect_clicked(move |_| {
            if let Some(win) = weak.upgrade() {
                win.send(Command::ClearCookie);
            }
        });

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .halign(gtk::Align::End)
            .margin_top(4)
            .build();
        buttons.append(&import_file_btn);
        buttons.append(&paste_btn);
        buttons.append(&refresh_btn);
        buttons.append(&clear_btn);

        let buttons_row = adw::ActionRow::builder().activatable(false).build();
        buttons_row.add_suffix(&buttons);
        group.add(&buttons_row);

        page.add(&group);

        let imp = self.imp();
        *imp.account_status_row.borrow_mut() = Some(status_row);
        *imp.account_status_dot.borrow_mut() = Some(dot);

        page
    }

    fn build_lyrics_page(&self, settings: &gio::Settings) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder()
            .title("歌词")
            .icon_name("microphone-sensitivity-high-symbolic")
            .build();

        let provider_group = adw::PreferencesGroup::builder().title("歌词源").build();

        let model = gtk::StringList::new(&["网易云音乐", "QQ 音乐"]);
        let provider_combo = adw::ComboRow::builder()
            .title("优先歌词源")
            .subtitle("自动匹配时优先尝试的来源")
            .model(&model)
            .build();
        let initial_provider = settings.string("preferred-provider").to_string();
        provider_combo.set_selected(if initial_provider == "qq" { 1 } else { 0 });
        let weak = self.downgrade();
        provider_combo.connect_selected_notify(move |row| {
            let Some(win) = weak.upgrade() else { return };
            if win.imp().suppress_signals.get() {
                return;
            }
            let value = if row.selected() == 1 { "qq" } else { "netease" };
            if let Some(s) = win.imp().gsettings.borrow().as_ref() {
                let _ = s.set_string("preferred-provider", value);
            }
            win.send(Command::SetPreferredProvider(value.into()));
        });
        provider_group.add(&provider_combo);

        let offset_spin = adw::SpinRow::with_range(-5000.0, 5000.0, 50.0);
        offset_spin.set_title("时间偏移 (ms)");
        offset_spin.set_subtitle("正值让歌词更早出现，负值则更晚");
        offset_spin.set_value(settings.int("timing-offset-ms") as f64);
        provider_group.add(&offset_spin);

        let translation = adw::SwitchRow::builder()
            .title("显示翻译副歌词")
            .subtitle("当歌词源提供翻译时显示")
            .build();
        settings
            .bind("desktop-lyrics-show-translation", &translation, "active")
            .build();
        provider_group.add(&translation);

        let line_mode_model = gtk::StringList::new(&["仅当前行", "当前 + 下一行"]);
        let line_mode = adw::ComboRow::builder()
            .title("显示模式")
            .model(&line_mode_model)
            .build();
        line_mode.set_selected(
            if settings.string("desktop-lyrics-line-mode").as_str() == "dual" {
                1
            } else {
                0
            },
        );
        let s_clone = settings.clone();
        line_mode.connect_selected_notify(move |row| {
            let value = if row.selected() == 1 {
                "dual"
            } else {
                "single"
            };
            let _ = s_clone.set_string("desktop-lyrics-line-mode", value);
        });
        provider_group.add(&line_mode);

        page.add(&provider_group);

        let match_group = adw::PreferencesGroup::builder().title("手动匹配").build();
        let manual_row = adw::ActionRow::builder()
            .title("打开手动匹配对话框")
            .subtitle("当自动匹配不准时使用")
            .activatable(true)
            .build();
        let manual_btn = gtk::Button::builder()
            .label("打开…")
            .valign(gtk::Align::Center)
            .css_classes(["suggested-action"])
            .build();
        manual_row.add_suffix(&manual_btn);
        manual_row.set_sensitive(false);
        manual_btn.set_sensitive(false);

        let weak = self.downgrade();
        let open_match = move || {
            let Some(win) = weak.upgrade() else { return };
            let _ = win.open_manual_match_dialog();
        };
        let cb = open_match.clone();
        manual_btn.connect_clicked(move |_| cb());
        let cb = open_match.clone();
        manual_row.connect_activated(move |_| cb());
        match_group.add(&manual_row);

        page.add(&match_group);

        let imp = self.imp();
        *imp.provider_combo.borrow_mut() = Some(provider_combo);
        *imp.offset_spin.borrow_mut() = Some(offset_spin);
        *imp.manual_match_button.borrow_mut() = Some(manual_btn);
        *imp.manual_match_row.borrow_mut() = Some(manual_row);

        page
    }

    fn build_display_page(&self, settings: &gio::Settings) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder()
            .title("显示")
            .icon_name("applications-graphics-symbolic")
            .build();

        // ── 字体 ──
        let font_group = adw::PreferencesGroup::builder().title("字体").build();

        let font_dialog = gtk::FontDialog::builder().title("选择歌词字体").build();
        let font_button = gtk::FontDialogButton::builder()
            .dialog(&font_dialog)
            .level(gtk::FontLevel::Font)
            .valign(gtk::Align::Center)
            .build();
        let initial_font = settings.string("desktop-lyrics-font").to_string();
        font_button.set_font_desc(&gtk::pango::FontDescription::from_string(&initial_font));
        let s_clone = settings.clone();
        font_button.connect_font_desc_notify(move |btn| {
            if let Some(desc) = btn.font_desc() {
                let _ = s_clone.set_string("desktop-lyrics-font", &desc.to_str());
            }
        });
        let font_row = adw::ActionRow::builder().title("歌词字体").build();
        font_row.add_suffix(&font_button);
        font_group.add(&font_row);

        let line_height = adw::SpinRow::with_range(1.0, 2.5, 0.05);
        line_height.set_title("行高倍数");
        line_height.set_value(settings.double("desktop-lyrics-line-height"));
        line_height.set_digits(2);
        let s_clone = settings.clone();
        line_height.connect_changed(move |row| {
            let _ = s_clone.set_double("desktop-lyrics-line-height", row.value());
        });
        font_group.add(&line_height);

        page.add(&font_group);

        // ── 颜色 ──
        let color_group = adw::PreferencesGroup::builder().title("颜色").build();

        color_group.add(&Self::build_color_row(
            settings,
            "高亮行颜色",
            "desktop-lyrics-active-color",
        ));
        color_group.add(&Self::build_color_row(
            settings,
            "其他行颜色",
            "desktop-lyrics-inactive-color",
        ));
        color_group.add(&Self::build_color_row(
            settings,
            "描边颜色",
            "desktop-lyrics-stroke-color",
        ));

        let stroke_width = adw::SpinRow::with_range(0.0, 4.0, 1.0);
        stroke_width.set_title("描边宽度 (px)");
        stroke_width.set_value(settings.int("desktop-lyrics-stroke-width") as f64);
        let s_clone = settings.clone();
        stroke_width.connect_changed(move |row| {
            let _ = s_clone.set_int("desktop-lyrics-stroke-width", row.value() as i32);
        });
        color_group.add(&stroke_width);

        color_group.add(&Self::build_color_row(
            settings,
            "背景颜色",
            "desktop-lyrics-bg-color",
        ));

        let bg_opacity = adw::SpinRow::with_range(0.0, 1.0, 0.05);
        bg_opacity.set_title("背景不透明度");
        bg_opacity.set_value(settings.double("desktop-lyrics-bg-opacity"));
        bg_opacity.set_digits(2);
        let s_clone = settings.clone();
        bg_opacity.connect_changed(move |row| {
            let _ = s_clone.set_double("desktop-lyrics-bg-opacity", row.value());
        });
        color_group.add(&bg_opacity);

        page.add(&color_group);

        // ── 窗口 ──
        let window_group = adw::PreferencesGroup::builder().title("浮层窗口").build();

        let enabled = adw::SwitchRow::builder().title("显示桌面歌词").build();
        settings
            .bind("desktop-lyrics-enabled", &enabled, "active")
            .build();
        window_group.add(&enabled);

        let locked = adw::SwitchRow::builder()
            .title("锁定（点击穿透）")
            .subtitle("锁定后鼠标点击会穿透到下方窗口")
            .build();
        settings
            .bind("desktop-lyrics-locked", &locked, "active")
            .build();
        window_group.add(&locked);

        let width = adw::SpinRow::with_range(400.0, 1920.0, 20.0);
        width.set_title("浮层宽度 (px)");
        width.set_value(settings.int("desktop-lyrics-width") as f64);
        let s_clone = settings.clone();
        width.connect_changed(move |row| {
            let _ = s_clone.set_int("desktop-lyrics-width", row.value() as i32);
        });
        window_group.add(&width);

        let reset_row = adw::ActionRow::builder()
            .title("重置位置")
            .subtitle("将浮层重新放到屏幕底部居中")
            .build();
        let reset_btn = gtk::Button::builder()
            .label("重置")
            .valign(gtk::Align::Center)
            .build();
        let s_clone = settings.clone();
        reset_btn.connect_clicked(move |_| {
            let _ = s_clone.set_int("desktop-lyrics-x", -1);
            let _ = s_clone.set_int("desktop-lyrics-y", -1);
        });
        reset_row.add_suffix(&reset_btn);
        window_group.add(&reset_row);

        page.add(&window_group);

        let imp = self.imp();
        *imp.font_button.borrow_mut() = Some(font_button);

        page
    }

    fn build_color_row(settings: &gio::Settings, title: &str, key: &str) -> adw::ActionRow {
        let row = adw::ActionRow::builder().title(title).build();
        let dialog = gtk::ColorDialog::builder()
            .title(title)
            .with_alpha(false)
            .build();
        let button = gtk::ColorDialogButton::builder()
            .dialog(&dialog)
            .valign(gtk::Align::Center)
            .build();
        button.set_rgba(&rgba_from_hex(&settings.string(key)));
        let s_clone = settings.clone();
        let key_owned = key.to_string();
        button.connect_rgba_notify(move |btn| {
            let hex = rgba_to_hex(&btn.rgba());
            let _ = s_clone.set_string(&key_owned, &hex);
        });
        row.add_suffix(&button);
        row
    }

    fn build_about_page(&self) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder()
            .title("关于")
            .icon_name("help-about-symbolic")
            .build();

        let group = adw::PreferencesGroup::builder().title("Spot-Lyric").build();
        let row = adw::ActionRow::builder()
            .title("Spot-Lyric")
            .subtitle("X11 桌面歌词 · GTK4 + libadwaita")
            .build();
        group.add(&row);

        let version_row = adw::ActionRow::builder()
            .title("版本")
            .subtitle(env!("CARGO_PKG_VERSION"))
            .css_classes(["spotlyric-mono"])
            .build();
        group.add(&version_row);

        let license_row = adw::ActionRow::builder()
            .title("协议")
            .subtitle("MIT")
            .build();
        group.add(&license_row);

        let credits_row = adw::ActionRow::builder()
            .title("致谢")
            .subtitle("基于 NetEase Cloud Music / QQ Music 公开 API · libadwaita · ksni")
            .build();
        group.add(&credits_row);

        page.add(&group);
        page
    }

    fn show_cookie_file_chooser(&self) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Cookie 文件"));
        filter.add_pattern("*.txt");
        filter.add_pattern("*.cookie");
        filter.add_mime_type("text/plain");

        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);

        let dialog = gtk::FileDialog::builder()
            .title("选择 cookies.txt")
            .filters(&filters)
            .modal(true)
            .build();

        let weak = self.downgrade();
        dialog.open(
            Some(self.upcast_ref::<gtk::Window>()),
            None::<&gio::Cancellable>,
            move |result| match result {
                Ok(file) => {
                    if let Some(win) = weak.upgrade() {
                        if let Some(path) = file.path() {
                            win.send(Command::ImportCookieFile(
                                path.to_string_lossy().to_string(),
                            ));
                        }
                    }
                }
                Err(err) if err.matches(gtk::DialogError::Dismissed) => {}
                Err(err) => {
                    if let Some(win) = weak.upgrade() {
                        win.show_toast(&format!("打开文件失败：{err}"));
                    }
                }
            },
        );
    }

    pub fn show_toast(&self, message: &str) {
        let toast = adw::Toast::new(message);
        self.add_toast(toast);
    }

    pub fn show_banner(&self, message: &str, button_label: Option<&str>) {
        if let Some(banner) = self.imp().banner.borrow().as_ref() {
            banner.set_title(message);
            banner.set_button_label(button_label);
            banner.set_revealed(true);
        }
    }

    pub fn hide_banner(&self) {
        if let Some(banner) = self.imp().banner.borrow().as_ref() {
            banner.set_revealed(false);
        }
    }

    pub fn apply_auth_snapshot(&self, snapshot: &AuthSnapshot) {
        let imp = self.imp();
        *imp.last_auth.borrow_mut() = Some(snapshot.clone());

        let (status_class, title, subtitle) = match snapshot.status.as_str() {
            "ready" => (
                "ready",
                "已登录".to_string(),
                snapshot
                    .active_profile_id
                    .clone()
                    .or_else(|| snapshot.client_id.clone())
                    .unwrap_or_else(|| "Spotify Web".into()),
            ),
            "refreshing" => ("refreshing", "正在刷新…".to_string(), "请稍候".to_string()),
            "error" => (
                "error",
                "登录已过期".to_string(),
                snapshot.error.clone().unwrap_or_else(|| "未知错误".into()),
            ),
            _ => (
                "idle",
                "未登录".to_string(),
                "点击右侧按钮导入 Cookie".to_string(),
            ),
        };

        if let Some(row) = imp.account_status_row.borrow().as_ref() {
            row.set_title(&title);
            row.set_subtitle(&subtitle);
        }
        if let Some(dot) = imp.account_status_dot.borrow().as_ref() {
            for c in ["idle", "refreshing", "ready", "error"] {
                dot.remove_css_class(c);
            }
            dot.add_css_class(status_class);
        }

        match snapshot.status.as_str() {
            "error" => self.show_banner("登录已过期，请重新导入 Cookie", Some("重新导入")),
            _ => self.hide_banner(),
        }
    }

    pub fn apply_lyrics_settings(&self, settings: &LyricsSettings) {
        let imp = self.imp();
        *imp.last_settings.borrow_mut() = Some(settings.clone());

        imp.suppress_signals.set(true);
        if let Some(combo) = imp.provider_combo.borrow().as_ref() {
            combo.set_selected(if settings.preferred_provider == "qq" {
                1
            } else {
                0
            });
        }
        if let Some(spin) = imp.offset_spin.borrow().as_ref() {
            spin.set_value(settings.lyrics_timing_offset_ms as f64);
        }
        if let Some(s) = imp.gsettings.borrow().as_ref() {
            let _ = s.set_string(
                "preferred-provider",
                if settings.preferred_provider == "qq" {
                    "qq"
                } else {
                    "netease"
                },
            );
            let _ = s.set_int("timing-offset-ms", settings.lyrics_timing_offset_ms);
        }
        imp.suppress_signals.set(false);
    }

    pub fn open_manual_match_dialog(&self) -> bool {
        let track_uri = self.imp().last_track_uri.borrow().clone();
        if !manual_match_available(track_uri.as_str()) {
            return false;
        }
        let track_label = self.imp().last_track_label.borrow().clone();
        let Some(tx) = self.imp().cmd_tx.borrow().as_ref().cloned() else {
            return false;
        };
        lyrics_match_dialog::show(
            self.upcast_ref::<gtk::Widget>(),
            &track_label,
            &track_uri,
            tx,
        );
        true
    }

    pub fn apply_playback(&self, track_uri: &str, track_name: &str, artist_name: &str) {
        let imp = self.imp();
        *imp.last_track_uri.borrow_mut() = track_uri.to_string();
        let label = if track_name.is_empty() {
            String::new()
        } else if artist_name.is_empty() {
            track_name.to_string()
        } else {
            format!("{track_name} — {artist_name}")
        };
        *imp.last_track_label.borrow_mut() = label;

        let available = manual_match_available(track_uri);
        if let Some(btn) = imp.manual_match_button.borrow().as_ref() {
            btn.set_sensitive(available);
        }
        if let Some(row) = imp.manual_match_row.borrow().as_ref() {
            row.set_sensitive(available);
        }
    }

    pub fn set_connected(&self, connected: bool) {
        if connected {
            self.hide_banner();
        } else {
            self.show_banner("未连接到内置后端", Some("重试"));
        }
    }
}

// ─── helper: bridge UI updates from std::mpsc into preference window calls ──

pub fn install_ui_dispatcher(
    window: &PreferencesWindow,
    rx: std_mpsc::Receiver<crate::bridge::UiUpdate>,
    desktop: crate::widgets::desktop_lyrics_window::DesktopLyricsWindow,
    tray_state: std::sync::Arc<std::sync::Mutex<crate::tray::TrayState>>,
    tray_handle: Rc<std::cell::RefCell<Option<crate::tray::TrayHandle>>>,
) {
    use crate::bridge::UiUpdate;
    let win_weak = window.downgrade();
    let rx = std::cell::RefCell::new(rx);
    glib::source::idle_add_local(
        clone!(@strong desktop, @strong tray_state, @strong tray_handle => move || {
            // Drain quickly per idle tick.
            while let Ok(update) = rx.borrow_mut().try_recv() {
                let Some(win) = win_weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                match update {
                    UiUpdate::Connected => {
                        win.set_connected(true);
                        win.show_toast("已连接到 daemon");
                        win.send(Command::LoadAuthSnapshot);
                        win.send(Command::LoadLyricsSettings);
                        tray_state.lock().unwrap().connected = true;
                        if let Some(handle) = tray_handle.borrow().as_ref() {
                            handle.refresh();
                        }
                    }
                    UiUpdate::Disconnected(reason) => {
                        win.set_connected(false);
                        tracing::debug!("daemon disconnected: {reason}");
                        let mut st = tray_state.lock().unwrap();
                        st.connected = false;
                        st.now_playing = None;
                        drop(st);
                        if let Some(handle) = tray_handle.borrow().as_ref() {
                            handle.refresh();
                        }
                    }
                    UiUpdate::PlaybackStateChanged(state) => {
                        let previous_track_uri = desktop.current_track_uri();
                        win.apply_playback(&state.track_uri, &state.track_name, &state.artist_name);
                        desktop.apply_playback(&state);
                        if let Some(track_uri) = lyrics_load_request_for_playback(
                            previous_track_uri.as_str(),
                            state.track_uri.as_str(),
                        ) {
                            win.send(Command::LoadLyrics { track_uri });
                        }
                        {
                            let mut state_ref = tray_state.lock().unwrap();
                            state_ref.is_playing = state.is_playing;
                            state_ref.now_playing = if state.track_uri.is_empty() {
                                None
                            } else if state.artist_name.is_empty() {
                                Some(state.track_name.clone())
                            } else {
                                Some(format!("{} — {}", state.track_name, state.artist_name))
                            };
                        }
                        if let Some(handle) = tray_handle.borrow().as_ref() {
                            handle.refresh();
                        }
                    }
                    UiUpdate::LyricsLoaded { track_uri, payload } => {
                        if track_uri == desktop.current_track_uri() {
                            desktop.set_lyrics(&payload);
                        }
                    }
                    UiUpdate::LyricsLoadFailed { track_uri: _, error } => {
                        win.show_toast(&format!("歌词加载失败：{error}"));
                    }
                    UiUpdate::LyricsMatchResults(candidates) => {
                        crate::dialogs::lyrics_match_dialog::dispatch_results(candidates);
                    }
                    UiUpdate::LyricsPreview(payload) => {
                        desktop.set_lyrics(&payload);
                    }
                    UiUpdate::LyricsMatchSaved { track_uri: _ } => {
                        win.show_toast("已保存匹配，正在重新加载歌词…");
                    }
                    UiUpdate::LyricsSettingsLoaded(settings) => {
                        win.apply_lyrics_settings(&settings);
                        {
                            let mut state_ref = tray_state.lock().unwrap();
                            crate::tray::apply_lyrics_provider_to_state(
                                &mut state_ref,
                                settings.preferred_provider.as_str(),
                            );
                        }
                        if let Some(handle) = tray_handle.borrow().as_ref() {
                            handle.refresh();
                        }
                    }
                    UiUpdate::AuthSnapshotLoaded(snapshot)
                    | UiUpdate::AuthSnapshotChanged(snapshot) => {
                        win.apply_auth_snapshot(&snapshot);
                    }
                    UiUpdate::Error(message) => {
                        win.show_toast(&message);
                    }
                    UiUpdate::Toast(message) => {
                        win.show_toast(&message);
                    }
                }
            }
            glib::ControlFlow::Continue
        }),
    );
}

fn lyrics_load_request_for_playback(
    previous_track_uri: &str,
    next_track_uri: &str,
) -> Option<String> {
    if next_track_uri.is_empty() || previous_track_uri == next_track_uri {
        None
    } else {
        Some(next_track_uri.to_string())
    }
}

fn manual_match_available(track_uri: &str) -> bool {
    !track_uri.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lyrics_loads_only_for_new_non_empty_track() {
        assert_eq!(
            lyrics_load_request_for_playback("", "spotify:track:first"),
            Some("spotify:track:first".to_string())
        );
        assert_eq!(
            lyrics_load_request_for_playback("spotify:track:first", "spotify:track:first"),
            None
        );
        assert_eq!(
            lyrics_load_request_for_playback("spotify:track:first", ""),
            None
        );
        assert_eq!(
            lyrics_load_request_for_playback("spotify:track:first", "spotify:track:second"),
            Some("spotify:track:second".to_string())
        );
    }

    #[test]
    fn manual_match_is_available_for_current_track_only() {
        assert!(manual_match_available("spotify:track:abc"));
        assert!(!manual_match_available(""));
        assert!(!manual_match_available("   "));
    }
}
