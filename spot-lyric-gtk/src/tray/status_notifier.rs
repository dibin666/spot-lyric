use crate::bridge::Command;
use crate::config;
use gtk::glib;
use ksni::{
    menu::{CheckmarkItem, StandardItem, SubMenu},
    MenuItem, Tray,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[derive(Default, Clone)]
pub struct TrayState {
    pub connected: bool,
    pub is_playing: bool,
    pub now_playing: Option<String>,
    pub desktop_lyrics_enabled: bool,
    pub desktop_lyrics_locked: bool,
    pub preferred_provider: String,
}

pub fn apply_desktop_settings_to_state(state: &mut TrayState, enabled: bool, locked: bool) {
    state.desktop_lyrics_enabled = enabled;
    state.desktop_lyrics_locked = locked;
}

pub fn apply_lyrics_provider_to_state(state: &mut TrayState, provider: &str) {
    state.preferred_provider = if provider == "qq" { "qq" } else { "netease" }.into();
}

pub enum TrayAction {
    ToggleLyrics,
    ToggleLock,
    Preferences,
    MatchLyrics,
    Quit,
    SetProvider(String),
}

#[derive(Clone)]
pub struct TrayHandle {
    handle: ksni::Handle<StatusNotifierTray>,
}

impl TrayHandle {
    pub fn refresh(&self) {
        self.handle.update(|_| {});
    }
}

pub struct StatusNotifierTray {
    cmd_tx: mpsc::UnboundedSender<Command>,
    state: Arc<Mutex<TrayState>>,
    action_tx: mpsc::UnboundedSender<TrayAction>,
}

impl StatusNotifierTray {
    pub fn spawn(
        action_tx: mpsc::UnboundedSender<TrayAction>,
        cmd_tx: mpsc::UnboundedSender<Command>,
        state: Arc<Mutex<TrayState>>,
        handle_out: Rc<RefCell<Option<TrayHandle>>>,
    ) {
        let tray = StatusNotifierTray {
            cmd_tx,
            state,
            action_tx,
        };

        let svc = ksni::TrayService::new(tray);
        let handle = svc.handle();
        *handle_out.borrow_mut() = Some(TrayHandle {
            handle: handle.clone(),
        });

        std::thread::spawn(move || {
            svc.spawn();
        });
    }
}

impl Tray for StatusNotifierTray {
    fn icon_name(&self) -> String {
        config::APP_ID.to_string()
    }

    fn title(&self) -> String {
        "Spot-Lyric".to_string()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let state = self.state.lock().unwrap();
        let desc = if !state.connected {
            "后端未运行".to_string()
        } else if let Some(ref np) = state.now_playing {
            if state.is_playing {
                np.clone()
            } else {
                format!("{} (paused)", np)
            }
        } else {
            "Spot-Lyric".to_string()
        };

        ksni::ToolTip {
            icon_name: config::APP_ID.to_string(),
            title: "Spot-Lyric".to_string(),
            description: desc,
            icon_pixmap: Vec::new(),
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let state = self.state.lock().unwrap();
        let mut menu = Vec::new();

        let tx = self.action_tx.clone();
        menu.push(MenuItem::Checkmark(CheckmarkItem {
            label: "显示桌面歌词".into(),
            checked: state.desktop_lyrics_enabled,
            activate: Box::new(move |_| {
                let _ = tx.send(TrayAction::ToggleLyrics);
            }),
            ..Default::default()
        }));

        let tx = self.action_tx.clone();
        menu.push(MenuItem::Checkmark(CheckmarkItem {
            label: "锁定（点击穿透）".into(),
            checked: state.desktop_lyrics_locked,
            activate: Box::new(move |_| {
                let _ = tx.send(TrayAction::ToggleLock);
            }),
            ..Default::default()
        }));

        menu.push(MenuItem::Separator);

        let np_label = if let Some(ref np) = state.now_playing {
            format!("当前播放：{}", np)
        } else {
            "当前没有播放".to_string()
        };

        menu.push(MenuItem::Standard(StandardItem {
            label: np_label,
            enabled: false,
            ..Default::default()
        }));

        menu.push(MenuItem::Separator);

        let tx = self.action_tx.clone();
        menu.push(MenuItem::Standard(StandardItem {
            label: "打开偏好设置…".into(),
            activate: Box::new(move |_| {
                let _ = tx.send(TrayAction::Preferences);
            }),
            ..Default::default()
        }));

        let tx = self.action_tx.clone();
        menu.push(MenuItem::Standard(StandardItem {
            label: "手动匹配歌词…".into(),
            activate: Box::new(move |_| {
                let _ = tx.send(TrayAction::MatchLyrics);
            }),
            ..Default::default()
        }));

        let mut sources = Vec::new();
        let preferred = state.preferred_provider.clone();

        let tx = self.cmd_tx.clone();
        sources.push(MenuItem::Standard(StandardItem {
            label: (if preferred == "netease" {
                "• 网易云音乐"
            } else {
                "  网易云音乐"
            })
            .into(),
            activate: Box::new(move |_| {
                let _ = tx.send(Command::SetPreferredProvider("netease".into()));
            }),
            ..Default::default()
        }));
        let tx = self.cmd_tx.clone();
        sources.push(MenuItem::Standard(StandardItem {
            label: (if preferred == "qq" {
                "• QQ 音乐"
            } else {
                "  QQ 音乐"
            })
            .into(),
            activate: Box::new(move |_| {
                let _ = tx.send(Command::SetPreferredProvider("qq".into()));
            }),
            ..Default::default()
        }));

        menu.push(MenuItem::SubMenu(SubMenu {
            label: "歌词源".into(),
            submenu: sources,
            ..Default::default()
        }));

        menu.push(MenuItem::Separator);

        let tx = self.action_tx.clone();
        menu.push(MenuItem::Standard(StandardItem {
            label: "退出".into(),
            activate: Box::new(move |_| {
                let _ = tx.send(TrayAction::Quit);
            }),
            ..Default::default()
        }));

        menu
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_state_tracks_desktop_settings() {
        let mut state = TrayState::default();

        apply_desktop_settings_to_state(&mut state, true, false);

        assert!(state.desktop_lyrics_enabled);
        assert!(!state.desktop_lyrics_locked);
    }

    #[test]
    fn tray_state_normalizes_provider() {
        let mut state = TrayState::default();

        apply_lyrics_provider_to_state(&mut state, "qq");
        assert_eq!(state.preferred_provider, "qq");

        apply_lyrics_provider_to_state(&mut state, "invalid");
        assert_eq!(state.preferred_provider, "netease");
    }
}
