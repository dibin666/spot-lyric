use crate::bridge::Command;
use crate::config;
use ksni::{
    menu::{CheckmarkItem, StandardItem, SubMenu},
    Icon, MenuItem, Tray,
};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

const APP_ICON_RELATIVE_PATH: &str = "scalable/apps/cn.spotlyric.Gtk.svg";
const TRAY_ICON_THEME_ENV: &str = "SPOT_LYRIC_ICON_THEME_PATH";
const ICON_REFRESH_DELAYS_MS: [u64; 3] = [300, 1_200, 3_000];

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

fn local_icon_theme_path() -> String {
    if let Ok(path) = std::env::var(TRAY_ICON_THEME_ENV) {
        if !path.is_empty() && Path::new(&path).exists() {
            return path;
        }
    }

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let target_dir = manifest_dir.join("target");
    let running_from_target = std::env::current_exe()
        .map(|exe| exe.starts_with(&target_dir))
        .unwrap_or(false);
    if !running_from_target {
        return String::new();
    }

    let source_icon_theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/icons");
    if source_icon_theme.join(APP_ICON_RELATIVE_PATH).is_file() {
        return source_icon_theme.to_string_lossy().into_owned();
    }

    String::new()
}

fn tray_icon_pixmaps(generation: u8) -> Vec<Icon> {
    vec![
        draw_tray_icon(64, generation),
        draw_tray_icon(32, generation),
        draw_tray_icon(22, generation),
    ]
}

fn draw_tray_icon(size: i32, generation: u8) -> Icon {
    let side = size.max(1) as usize;
    let mut data = vec![0; side * side * 4];
    data[1] = generation;

    for y in 0..size {
        for x in 0..size {
            if in_rounded_rect(x, y, 1, 1, size - 1, size - 1, (size / 5).max(2)) {
                let t = y as f32 / (size - 1).max(1) as f32;
                let r = lerp(59, 29, t);
                let g = lerp(130, 78, t);
                let b = lerp(246, 216, t);
                put_pixel(&mut data, side, x, y, 255, r, g, b);
            }
        }
    }

    let bubble_left = size * 5 / 32;
    let bubble_top = size * 10 / 32;
    let bubble_right = size * 27 / 32;
    let bubble_bottom = size * 23 / 32;
    let bubble_radius = (size * 2 / 32).max(2);
    for y in bubble_top..bubble_bottom {
        for x in bubble_left..bubble_right {
            if in_rounded_rect(
                x,
                y,
                bubble_left,
                bubble_top,
                bubble_right,
                bubble_bottom,
                bubble_radius,
            ) || in_tail(x, y, size)
            {
                put_pixel(&mut data, side, x, y, 245, 255, 255, 255);
            }
        }
    }

    draw_round_bar(
        &mut data,
        side,
        size * 8 / 32,
        size * 13 / 32,
        size * 14 / 32,
        (size / 14).max(2),
    );
    draw_round_bar(
        &mut data,
        side,
        size * 8 / 32,
        size * 16 / 32,
        size * 11 / 32,
        (size / 18).max(1),
    );
    draw_round_bar(
        &mut data,
        side,
        size * 8 / 32,
        size * 19 / 32,
        size * 9 / 32,
        (size / 18).max(1),
    );
    draw_note(&mut data, side, size);

    Icon {
        width: size,
        height: size,
        data,
    }
}

fn lerp(start: u8, end: u8, t: f32) -> u8 {
    (start as f32 + (end as f32 - start as f32) * t).round() as u8
}

fn put_pixel(data: &mut [u8], side: usize, x: i32, y: i32, a: u8, r: u8, g: u8, b: u8) {
    if x < 0 || y < 0 || x as usize >= side || y as usize >= side {
        return;
    }
    let idx = ((y as usize * side) + x as usize) * 4;
    data[idx] = a;
    data[idx + 1] = r;
    data[idx + 2] = g;
    data[idx + 3] = b;
}

fn in_rounded_rect(
    x: i32,
    y: i32,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    radius: i32,
) -> bool {
    if x < left || x >= right || y < top || y >= bottom {
        return false;
    }

    let radius = radius.max(1);
    let cx = if x < left + radius {
        left + radius
    } else if x >= right - radius {
        right - radius - 1
    } else {
        x
    };
    let cy = if y < top + radius {
        top + radius
    } else if y >= bottom - radius {
        bottom - radius - 1
    } else {
        y
    };

    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= radius * radius
}

fn in_tail(x: i32, y: i32, size: i32) -> bool {
    let tail_top = size * 21 / 32;
    let tail_bottom = size * 27 / 32;
    let tail_left = size * 14 / 32;
    let tail_right = size * 19 / 32;
    y >= tail_top
        && y < tail_bottom
        && x >= tail_left
        && x < tail_right
        && x - tail_left <= tail_bottom - y
}

fn draw_round_bar(data: &mut [u8], side: usize, x: i32, y: i32, width: i32, height: i32) {
    for yy in y..(y + height) {
        for xx in x..(x + width) {
            if in_rounded_rect(xx, yy, x, y, x + width, y + height, (height / 2).max(1)) {
                put_pixel(data, side, xx, yy, 255, 29, 78, 216);
            }
        }
    }
}

fn draw_note(data: &mut [u8], side: usize, size: i32) {
    let blue = (255, 29, 78, 216);
    let stem_x = size * 23 / 32;
    let stem_y = size * 12 / 32;
    let stem_w = (size / 14).max(2);
    let stem_h = size * 9 / 32;

    for y in stem_y..(stem_y + stem_h) {
        for x in stem_x..(stem_x + stem_w) {
            put_pixel(data, side, x, y, blue.0, blue.1, blue.2, blue.3);
        }
    }

    let head_cx = stem_x - size * 2 / 32;
    let head_cy = stem_y + stem_h;
    let rx = (size * 5 / 64).max(2);
    let ry = (size * 4 / 64).max(2);
    for y in (head_cy - ry)..=(head_cy + ry) {
        for x in (head_cx - rx)..=(head_cx + rx) {
            let dx = (x - head_cx) as f32 / rx as f32;
            let dy = (y - head_cy) as f32 / ry as f32;
            if dx * dx + dy * dy <= 1.0 {
                put_pixel(data, side, x, y, blue.0, blue.1, blue.2, blue.3);
            }
        }
    }

    for y in stem_y..(stem_y + size * 3 / 32) {
        let row_w = (size * 6 / 32) - (y - stem_y);
        for x in stem_x..(stem_x + row_w.max(1)) {
            put_pixel(data, side, x, y, blue.0, blue.1, blue.2, blue.3);
        }
    }
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
        self.handle.update(|tray| tray.bump_icon_generation());
    }
}

pub struct StatusNotifierTray {
    cmd_tx: mpsc::UnboundedSender<Command>,
    state: Arc<Mutex<TrayState>>,
    action_tx: mpsc::UnboundedSender<TrayAction>,
    icon_generation: u8,
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
            icon_generation: 0,
        };

        let svc = ksni::TrayService::new(tray);
        let handle = svc.handle();
        *handle_out.borrow_mut() = Some(TrayHandle {
            handle: handle.clone(),
        });

        std::thread::spawn(move || {
            svc.spawn();
        });

        let icon_refresh_handle = handle.clone();
        std::thread::spawn(move || {
            for delay_ms in ICON_REFRESH_DELAYS_MS {
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                icon_refresh_handle.update(|tray| tray.bump_icon_generation());
            }
        });
    }

    fn bump_icon_generation(&mut self) {
        self.icon_generation = self.icon_generation.wrapping_add(1);
    }
}

impl Tray for StatusNotifierTray {
    fn id(&self) -> String {
        config::APP_ID.to_string()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.action_tx.send(TrayAction::Preferences);
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        let _ = self.action_tx.send(TrayAction::Preferences);
    }

    fn icon_name(&self) -> String {
        config::APP_ID.to_string()
    }

    fn icon_theme_path(&self) -> String {
        local_icon_theme_path()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        tray_icon_pixmaps(self.icon_generation)
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
            icon_pixmap: tray_icon_pixmaps(self.icon_generation),
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

    #[test]
    fn tray_icon_pixmaps_are_valid_argb_buffers() {
        let pixmaps = tray_icon_pixmaps(0);

        assert!(!pixmaps.is_empty());
        for icon in pixmaps {
            assert!(icon.width > 0);
            assert!(icon.height > 0);
            assert_eq!(icon.data.len(), (icon.width * icon.height * 4) as usize);
            assert!(icon.data.chunks_exact(4).any(|pixel| pixel[0] > 0));
        }
    }

    #[test]
    fn source_icon_theme_path_exists_in_development() {
        let path = local_icon_theme_path();

        assert!(!path.is_empty());
        assert!(Path::new(&path).exists());
    }

    #[test]
    fn tray_icon_generation_changes_invisible_pixel_for_new_icon_signal() {
        let first = tray_icon_pixmaps(1);
        let second = tray_icon_pixmaps(2);

        assert_eq!(first[0].data[0], 0);
        assert_eq!(second[0].data[0], 0);
        assert_ne!(first[0].data, second[0].data);
    }
}
