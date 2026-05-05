# Spot-Lyric 前端 UI 设计方案

> **范围**：本文档定义 GTK 单进程应用（`spot-lyric-gtk`）的视觉设计、组件结构、交互、构建与配置。后端运行在同一进程内；前端桥接层仍通过本进程拥有的 D-Bus 服务拉取/订阅业务状态。

---

## 1. 总览

### 1.1 产品形态
- 一个**驻留托盘**（StatusNotifierItem）的 GTK4 + libadwaita 桌面应用。
- 主窗口是**设置中心**（Adwaita PreferencesWindow），可以随时关闭，关闭后程序进入纯托盘状态。
- 一个**始终置顶**的桌面歌词浮层窗（无边框、透明、可拖动、可锁定为点击穿透），独立于主窗口生命周期。
- 设计语言：**GNOME / GTK 原生风格**（libadwaita 默认主题），不模拟 macOS / Windows / 自定义视觉，最大化与 GNOME / KDE / XFCE 桌面环境的一致性。

### 1.2 目标平台
- **X11 一等公民**。所有"始终置顶 / 点击穿透 / 拖动 / 工作区无关位置"全部走 X11 EWMH 协议（`_NET_WM_STATE_ABOVE`、`SHAPE input region`、`XMoveWindow`），不依赖任何 Wayland / layer-shell 协议。
- 在没有合成器（picom / compton / mutter）时仍能用，仅"半透明"会退化为不透明背景。

### 1.3 与内置后端的边界
- **前端只持有 UI 状态和用户偏好**（GSettings 中的字体、颜色、窗口位置等）。
- **内置后端持有播放状态、登录态、歌词数据**。前端 bridge 通过 D-Bus proxy 拉取/订阅，保持 UI 与业务逻辑解耦。
- 内置后端启动失败时，前端进入“降级模式”：托盘菜单中只可改样式 / 看 about / 退出，主窗口顶部显示连接错误条。

---

## 2. 工程结构

```
spot-lyric-gtk/
├── Cargo.toml
├── build.rs
├── data/
│   ├── cn.spotlyric.Gtk.gschema.xml      ← 用户偏好（字体/颜色/位置）
│   ├── cn.spotlyric.Gtk.desktop          ← 启动器条目
│   ├── cn.spotlyric.Gtk.metainfo.xml     ← AppStream 元数据
│   └── icons/scalable/apps/
│       └── cn.spotlyric.Gtk.svg          ← 应用图标(也用于托盘)
├── resources/
│   ├── resources.gresource.xml
│   └── style.css                         ← 自定义 CSS 覆盖
└── src/
    ├── main.rs                           ← 入口 + 资源注册 + 日志
    ├── backend_runtime.rs                ← 内置后端线程 + Tokio runtime 生命周期
    ├── application.rs                    ← SpotLyricApplication (adw::Application)
    ├── config.rs                         ← APP_ID / 常量
    ├── bridge/                           ← UI ↔ 内置后端 D-Bus 桥
    │   ├── mod.rs
    │   ├── controller.rs                 ← tokio 运行时 + 命令循环
    │   ├── commands.rs                   ← Command enum
    │   └── updates.rs                    ← UiUpdate enum
    ├── dbus/                             ← 本进程后端 D-Bus 客户端代理
    │   ├── mod.rs
    │   ├── client.rs                     ← #[zbus::proxy] 定义
    │   └── types.rs                      ← 跨 D-Bus 序列化类型
    ├── widgets/
    │   ├── mod.rs
    │   ├── preferences_window.rs         ← 主设置窗
    │   ├── desktop_lyrics_window.rs      ← 桌面歌词浮层
    │   ├── lyrics_match_dialog.rs        ← 手动匹配对话框
    │   ├── auth_dialog.rs                ← Cookie 导入对话框
    │   └── color_button_row.rs           ← Adwaita PreferencesRow + ColorButton
    ├── tray/
    │   ├── mod.rs
    │   └── status_notifier.rs            ← ksni 实现
    ├── platform/
    │   ├── mod.rs
    │   └── x11.rs                        ← x11rb: keep-above / shape / move
    └── utils/
        ├── mod.rs
        ├── position_clock.rs             ← 客户端播放位置插值
        └── lrc_format.rs                 ← 一些 LRC 派生工具
```

---

## 3. 依赖

### 3.1 Cargo.toml

```toml
[package]
name = "spot-lyric-gtk"
version = "0.1.0"
edition = "2021"
rust-version = "1.80"

[dependencies]
gtk         = { package = "gtk4", version = "0.9", features = ["v4_14"] }
adw         = { package = "libadwaita", version = "0.7", features = ["v1_5"] }
glib        = "0.20"
gio         = "0.20"
gdk4        = "0.9"
gdk4-x11    = "0.9"
cairo-rs    = { package = "cairo-rs", version = "0.20", features = ["v1_16"] }
zbus        = { version = "5", features = ["tokio"] }
tokio       = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time"] }
serde       = { version = "1", features = ["derive"] }
serde_json  = "1"
tracing     = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
once_cell   = "1"
futures-util = "0.3"
ksni        = "0.2"
x11rb       = { version = "0.13", features = ["allow-unsafe-code", "shape"] }
anyhow      = "1"

[build-dependencies]
glib-build-tools = "0.20"
```

> 注：`x11rb` 用 pure-rust 实现 X11 协议，不需要链接 libxcb-shape。

### 3.2 系统包（运行时）
- `gtk4 ≥ 4.14`、`libadwaita ≥ 1.5`
- 一个 StatusNotifierWatcher 实现：
  - KDE / Cinnamon / MATE / XFCE / Pantheon 自带
  - GNOME 需要装 [`AppIndicator and KStatusNotifierItem Support`](https://extensions.gnome.org/extension/615/appindicator-support/) 扩展
  - 否则托盘退化为不可见（需要文档警告用户）
- 一个 X11 合成器（picom / compton / mutter / kwin_x11）以获得真透明，否则降级为整块背景色。

---

## 4. 应用生命周期

### 4.1 入口（`main.rs`）
1. 安装 panic hook，将 backtrace 打到 stderr。
2. 初始化 `tracing_subscriber`（默认 `spot_lyric_gtk=info`，受 `RUST_LOG` 控制）。
3. 注册 GResource `spot_lyric.gresource`。
4. 创建 `BackendRuntime`，注入 `SpotLyricApplication`，调用 `app.run()`。

### 4.2 `SpotLyricApplication`（`application.rs`）
- 派生自 `adw::Application`，APP_ID = `cn.spotlyric.Gtk`。
- `startup` 钩子：
  1. 加载 `style.css` 到 default display 的 `StyleProvider`（`USER` 优先级）。
  2. 启动 `bridge::Controller`（D-Bus 桥），由它确保 `BackendRuntime` 已启动并返回 `cmd_tx` 与 `ui_rx`。
  3. 启动 `tray::StatusNotifierTray`，与 `cmd_tx`、`ui_rx` 共享通道（见 §8）。
  4. **`app.hold()` 手动持有引用**，让所有 GTK 窗口都关闭后进程仍存活（等托盘菜单触发 Quit 才走 `app.release()` + `app.quit()`）。
- `activate` 钩子：
  1. 创建并 `present()` 主 `PreferencesWindow`（如果还没有）。
  2. 创建 `DesktopLyricsWindow`，根据 `desktop-lyrics-enabled` 决定是否 show。
- 把 `ui_rx` 接到 glib `MainContext` 上：每帧轮询，将 `UiUpdate` 派发到对应的 widget。

### 4.3 状态机
```
Application Startup
   │
   ├─ Bridge::start → tokio thread
   │     │
   │     └─ DBus connect:
   │           ┌─ Ok  → UiUpdate::Connected, fetch initial state
   │           └─ Err → UiUpdate::Disconnected(reason)
   │
   ├─ Tray spawn (ksni thread, 自带 tokio runtime)
   │
   └─ activate → show main window + desktop lyrics window (if enabled)
```

---

## 5. D-Bus 桥（`bridge/`）

### 5.1 设计原则
- 一个**专用 OS 线程**承载 `tokio::runtime::Builder::new_current_thread()`，避免污染 GTK 主循环。
- UI → 桥：`tokio::sync::mpsc::UnboundedSender<Command>`（Command 是 enum，所有变体见下表）。
- 桥 → UI：`std::sync::mpsc::Sender<UiUpdate>`（用 std 通道是为了能从 glib 主循环里 `try_recv` 而无需 await）。
- 信号订阅在桥线程内自动起 `tokio::spawn`，把 D-Bus 信号转换成 `UiUpdate` 后丢回 UI。

### 5.2 Command 枚举（`bridge/commands.rs`）

```rust
pub enum Command {
    // ─── 播放控制（透传到 daemon） ───
    TogglePlaying,
    SkipNext,
    SkipPrevious,

    // ─── 歌词 ───
    LoadLyrics { track_uri: String },
    SearchLyricsMatches { query: String },
    PreviewLyricsMatch { candidate_id: String },
    SaveLyricsMatch { track_uri: String, candidate_id: String },
    SetPreferredProvider(String),     // "netease" | "qq"
    SetTimingOffsetMs(i32),
    LoadLyricsSettings,

    // ─── 鉴权 ───
    LoadAuthSnapshot,
    ImportCookieFile(String),
    ImportCookieString(String),
    RefreshAuth,
    ClearCookie,

    // ─── 应用 ───
    Quit,                              // 让 daemon 也退出
    Reconnect,                         // 用户在错误条上手动重连
}
```

### 5.3 UiUpdate 枚举（`bridge/updates.rs`）

```rust
pub enum UiUpdate {
    Connected,
    Disconnected(String),

    PlaybackStateChanged(PlaybackState),
    LyricsLoaded { track_uri: String, payload: LyricsPayload },
    LyricsLoadFailed { track_uri: String, error: String },

    LyricsMatchResults(Vec<LyricsCandidate>),
    LyricsPreview(LyricsPayload),

    AuthSnapshotLoaded(AuthSnapshot),
    AuthSnapshotChanged(AuthSnapshot),

    LyricsSettingsLoaded(LyricsSettings),

    Error(String),                     // 通用 toast
}
```

### 5.4 D-Bus 代理（`dbus/client.rs`）
所有方法都对应本进程拥有的 `cn.spotlyric.Daemon` 服务下的接口（详见 `backend-integration.md` §3）。前端不感知后端是否跨进程，只调用 proxy。每个接口一个独立 `mod`，避免 `StateChangedArgs` 等生成名冲突。

```rust
pub struct DaemonClient {
    pub auth:     AuthProxy<'static>,
    pub playback: PlaybackProxy<'static>,
    pub lyrics:   LyricsProxy<'static>,
    pub app:      AppProxy<'static>,
}
```

---

## 6. 主设置窗：`PreferencesWindow`

### 6.1 整体
- 基类 `adw::PreferencesWindow`（默认带 ViewSwitcher + 多页面）。
- 默认大小 `560×680`，`destroy_with_parent = false`。
- **关闭按钮 = 隐藏**：`connect_close_request` 返回 `Propagation::Stop` 并 `set_visible(false)`。
- 支持通过托盘 / app action 重新 present。

### 6.2 页面布局

#### 页面 A — 账户（`account`，图标 `avatar-default-symbolic`）
- `PreferencesGroup "Spotify 账号"`：
  - `ActionRow`：标题 = 当前登录用户 / "未登录"；副标题 = `device_id` 前 8 位；suffix = 状态色点 + 文本（idle / refreshing / ready / error）。
  - 按钮组：`导入 cookies.txt 文件…`（FileChooser）、`粘贴 cookie 字符串…`（Adwaita Dialog 内置 TextView）、`刷新 token`、`清除登录`。
- `PreferencesGroup "Profile 切换"`：从 `AuthSnapshot.profiles` 渲染 `ComboRow`。
- 错误条（`AdwBanner`）：当 `auth.status == "error"` 时弹出红色 banner 并显示 `error` 字段。

#### 页面 B — 歌词源（`lyrics`，图标 `microphone-sensitivity-high-symbolic`）
- `PreferencesGroup "歌词源"`：
  - `ComboRow`：歌词源优先级（"网易云音乐" / "QQ 音乐"），绑定 `preferred-provider`。
  - `SpinRow`：歌词时间偏移 `-5000 .. 5000 ms`，单位 ms，绑定 `timing-offset-ms`。
  - `ActionRow`：手动匹配（按钮"打开匹配对话框"，行为见 §9）。需要传入当前播放的 `track_uri`，未在播放时按钮置灰，副标题"开始播放后才能手动匹配"。
- `PreferencesGroup "翻译"`：
  - `SwitchRow`：显示翻译副歌词（绑定 `desktop-lyrics-show-translation`）。
  - `SwitchRow`：双行模式（当前+下一行）/ 单行模式（绑定 `desktop-lyrics-line-mode`）。

#### 页面 C — 显示（`display`，图标 `applications-graphics-symbolic`）
- `PreferencesGroup "字体"`：
  - `ActionRow + FontDialogButton`：字体（family + size + weight + style 一次性选）。绑定 `desktop-lyrics-font`（Pango 字符串如 `"HarmonyOS Sans SC Bold 32"`）。
  - `SpinRow`：行间距倍数 `1.0 .. 2.5`，step 0.1（绑定 `desktop-lyrics-line-height`）。
- `PreferencesGroup "颜色"`：
  - `ColorRow`（自定义，包 `gtk::ColorDialogButton`）：
    - 高亮行颜色（`desktop-lyrics-active-color`）
    - 非高亮行颜色（`desktop-lyrics-inactive-color`）
    - 描边颜色（`desktop-lyrics-stroke-color`）
  - `SpinRow`：描边宽度 `0..4 px`（绑定 `desktop-lyrics-stroke-width`）。
  - `SpinRow`：背景透明度 `0.00 .. 1.00`，step 0.05（绑定 `desktop-lyrics-bg-opacity`）。
- `PreferencesGroup "窗口"`：
  - `SwitchRow`：启动时显示桌面歌词（`desktop-lyrics-enabled`）。
  - `SwitchRow`：默认锁定（点击穿透）（`desktop-lyrics-locked`）。
  - `ButtonRow`："重置位置到屏幕底部居中" — 清空 `desktop-lyrics-x` / `-y`，下次开窗用默认位置。
  - `SpinRow`：窗口宽度 `400 .. 1920`（`desktop-lyrics-width`）。

#### 页面 D — 关于（`about`，图标 `help-about-symbolic`）
- 用 `adw::AboutWindow` 风格嵌入页面：版本、开源协议、致谢（NetEase / QQ Music API 提供方、libadwaita、ksni 等）。

### 6.3 错误条
- 顶部 `AdwBanner`，三种状态：
  - **后台守护进程未连接** — 红色，按钮"重试"（发 `Command::Reconnect`）。
  - **登录已过期** — 黄色，按钮"重新导入 Cookie"（打开 §7 对话框）。
  - **歌词加载失败** — 蓝色，按钮"打开手动匹配"。

---

## 7. Cookie 导入对话框（`auth_dialog.rs`）
- `adw::Dialog`，`content_width=520, content_height=420`。
- Toolbar 上 `HeaderBar`，左 = "返回"，右 = "保存"（默认禁用）。
- 主体两个 Tab（`AdwViewSwitcherBar` + `AdwViewStack`）：
  1. **从文件导入**：`gtk::FileChooserDialog`（filter: `*.txt`），文件路径显示在 EntryRow，旁边一个"重新选择"按钮。
  2. **粘贴文本**：`gtk::TextView` + `gtk::ScrolledWindow`，提示文本"将浏览器开发者工具中的整段 cookie 字符串粘贴到此处（必须包含 sp_dc 字段）"。
- 按"保存"时：
  - 从文件 → 发 `ImportCookieFile(path)`
  - 从文本 → 发 `ImportCookieString(text)`
- 收到 `AuthSnapshotLoaded` 后关闭对话框；收到 `Error(msg)` 在底部 `AdwToastOverlay` 弹错误 toast 不关。

---

## 8. 托盘图标（`tray/status_notifier.rs`）

### 8.1 实现
基于 `ksni` crate（StatusNotifierItem D-Bus 协议）。
- 单独的 OS 线程：`std::thread::spawn` → `ksni::TrayService::spawn()` 内置 tokio runtime。
- 与 GTK 通信：复用 `bridge` 的 `cmd_tx`，但 UI 状态（now-playing 文本、是否锁定）由 GTK 主线程通过 `Arc<Mutex<TrayState>>` 推给 ksni 线程，每次 push 后调 `handle.update(|tray| { ... })` 触发 SNI 重绘。

### 8.2 图标 / 文本
- Icon name: `cn.spotlyric.Gtk`（系统主题里的 SVG），fallback `audio-x-generic-symbolic`。
- Title: `Spot-Lyric`
- Tooltip:
  - 已连接 + 在播：`{track} — {artist}`
  - 已连接 + 暂停：`{track} (paused)`
  - 未连接：`后端未运行`

### 8.3 菜单
```
☑  显示桌面歌词                 ← 切换 visible
☑  锁定（点击穿透）             ← 切换 locked
─────
当前播放：{track} — {artist}     ← 不可点击 disable item
─────
打开偏好设置…                   ← 拉起主窗
手动匹配歌词…                   ← 拉起匹配对话框
歌词源 ▶
   • 网易云音乐
   • QQ 音乐
─────
退出
```
两个 checkable item 的勾选态由 GTK 主线程的偏好实时同步（监听 GSettings 的 `desktop-lyrics-enabled` / `desktop-lyrics-locked`）。

### 8.4 GNOME 兼容
- 启动时检测 `XDG_CURRENT_DESKTOP=GNOME` 且 GNOME Shell 进程存在 → 检查 `org.kde.StatusNotifierWatcher` D-Bus 名字是否注册。
- 如果未注册 → 偏好窗顶部 banner 提示"GNOME 默认不支持托盘图标，请启用 AppIndicator 扩展或保持主窗口运行"。

---

## 9. 手动匹配对话框（`lyrics_match_dialog.rs`）

### 9.1 触发
- 偏好"歌词"页面的按钮
- 托盘"手动匹配歌词…"
- 错误条"打开手动匹配"

### 9.2 布局
基于 `adw::Dialog`（500×620）。
```
┌── HeaderBar ────────────────────────────────┐
│  ←  手动匹配歌词                             │
└──────────────────────────────────────────────┘
当前曲目: {track} — {artist}
[ 搜索 NetEase / QQ 链接或关键词       ] [搜索]
─────────────────────────────────────────────
List (boxed-list):
┌─────────────────────────────────────────────┐
│ {title}                                  [▶] [⤓] │
│   {artist} · {album} · {duration}            │
└─────────────────────────────────────────────┘
... (重复)
```
- 搜索框默认填充当前曲目 `name + " " + artist`。
- 两个按钮分别是"预览"（`PreviewLyricsMatch`，结果通过 `LyricsPreview` 推到桌面歌词浮层临时展示）和"保存"（`SaveLyricsMatch`，保存后立即重新拉取歌词并关闭对话框）。
- 列表来自 `UiUpdate::LyricsMatchResults`。

---

## 10. 桌面歌词浮层（`desktop_lyrics_window.rs`）

> 本组件是整个程序的灵魂。要做到的事：始终置顶、透明背景、可拖动、可锁定（点击穿透）、自定义字体颜色、X11 兼容。

### 10.1 构造
- 基类 `gtk::Window`（不要 `adw::Window`，因为我们要全控制装饰）。
- 属性：
  - `decorated = false`
  - `resizable = false`
  - `deletable = false`
  - `default_size = (settings.width, -1)`（高度跟随内容）
  - 加 CSS class `desktop-lyrics-window`

### 10.2 内容树
```
GtkWindow.desktop-lyrics-window
└── GtkBox vertical .desktop-lyrics-container        ← 圆角 + 半透明背景
    ├── GtkLabel #active   .desktop-lyrics-active    ← 当前行
    │     可能两行: 原文 + 翻译（用 \n 分隔）
    └── GtkLabel #next     .desktop-lyrics-next      ← 下一行（可选）
```

### 10.3 GResource style.css 关键片段
```css
.desktop-lyrics-window {
    background: transparent;
}

.desktop-lyrics-container {
    border-radius: 16px;
    padding: 14px 32px;
    transition: background-color 200ms ease;
}

.desktop-lyrics-active {
    font-weight: 700;
    transition: color 250ms ease;
}

.desktop-lyrics-next {
    font-weight: 400;
    opacity: 0.6;
}
```
**用户字体 / 颜色 / 描边走运行时生成的 CSS Provider**（每次 GSettings 变更时 remove + add）：
```rust
let css = format!(
    ".desktop-lyrics-active {{
        font: {font};
        color: {active_color};
        text-shadow:
            -{sw}px -{sw}px 0 {stroke},  {sw}px -{sw}px 0 {stroke},
            -{sw}px  {sw}px 0 {stroke},  {sw}px  {sw}px 0 {stroke},
                  0 -{sw}px 0 {stroke},        0  {sw}px 0 {stroke},
            -{sw}px      0 0 {stroke},  {sw}px      0  0 {stroke};
    }}
    .desktop-lyrics-next {{
        font: {font_next};
        color: {inactive_color};
    }}
    .desktop-lyrics-container {{
        background-color: rgba({r},{g},{b}, {bg_opacity});
    }}",
    font = font_pango_str,
    font_next = font_pango_str_smaller_75pct,
    active_color = settings.active_color,
    inactive_color = settings.inactive_color,
    stroke = settings.stroke_color,
    sw = settings.stroke_width,
    r = bg_rgb.r, g = bg_rgb.g, b = bg_rgb.b,
    bg_opacity = settings.bg_opacity,
);
```

### 10.4 X11 平台层（`platform/x11.rs`）

公共 API：
```rust
pub struct X11Helper { conn: x11rb::rust_connection::RustConnection, screen: usize }

impl X11Helper {
    pub fn new() -> Result<Self>;

    /// 设置窗口为 _NET_WM_STATE_ABOVE + _NET_WM_STATE_SKIP_TASKBAR + SKIP_PAGER
    /// + _NET_WM_WINDOW_TYPE_UTILITY
    pub fn make_overlay(&self, xid: u32) -> Result<()>;

    /// 让窗口对鼠标事件透明（点击穿透）。region=None 恢复正常。
    pub fn set_input_passthrough(&self, xid: u32, passthrough: bool) -> Result<()>;

    /// 移动窗口到屏幕坐标 (x, y)。
    pub fn move_window(&self, xid: u32, x: i32, y: i32) -> Result<()>;

    /// 获取屏幕几何（用于"重置到底部居中"）。
    pub fn primary_monitor_geometry(&self) -> Result<MonitorGeometry>;
}
```

实现要点：
- `make_overlay`: 用 `change_property` 设置 `_NET_WM_WINDOW_TYPE = _NET_WM_WINDOW_TYPE_UTILITY`，再发送 `_NET_WM_STATE` ClientMessage（`action=_NET_WM_STATE_ADD`、各 atom）到 root window，`event_mask = SUBSTRUCTURE_NOTIFY | SUBSTRUCTURE_REDIRECT`。
- `set_input_passthrough`:
  - true → `xshape::rectangles(xid, ShapeKind::Input, ClipOrdering::Unsorted, 0,0, &[])`（空矩形列表 → 输入区域为空 → 鼠标穿透）
  - false → `xshape::mask(xid, ShapeKind::Input, 0,0, x11rb::NONE)`（恢复默认）
- `move_window`: `configure_window(xid, &ConfigureWindowAux::new().x(x).y(y))`。

### 10.5 GTK 端集成
```rust
fn realize_x11(&self) {
    let surface = self.surface().expect("realized");
    let xid = surface
        .downcast_ref::<gdk4_x11::X11Surface>()
        .expect("X11 only")
        .xid() as u32;

    let helper = X11Helper::new().expect("X server");
    helper.make_overlay(xid).expect("set above");
    self.imp().x11.set(Some((helper, xid)));
    self.apply_lock_state();      // 把保存的 locked 状态应用到 input shape
    self.restore_position();      // 用保存的 x/y 调 move_window
}
```
注意：`xid()` 必须在 `realize` 之后再读，构造时 surface 还不存在。

### 10.6 拖动
- `gtk::GestureDrag` 监听 `drag-update`。
- 锁定状态下不允许拖：`if self.imp().locked.get() { return; }`。
- 解锁状态下：拖动时累计 `(dx, dy)` → 调 `helper.move_window(xid, base_x + dx, base_y + dy)`。
- `drag-end`：把当前位置 (x, y) 写到 GSettings `desktop-lyrics-x` / `-y`。
- 第一次启动（GSettings 中位置 = -1）：调用 `primary_monitor_geometry()`，算出底部居中坐标 `(monitor.width-w)/2, monitor.height - h - bottom_margin)`。

### 10.7 锁定切换
- 公开 `pub fn set_locked(&self, locked: bool)`：
  1. `imp.locked.set(locked)`；
  2. `helper.set_input_passthrough(xid, locked)`；
  3. 触发 `notify::locked` 信号给托盘 / 偏好同步 UI。
- 解锁状态会显示一个细的"拖动指示条"在底部（CSS 类 `.drag-handle`，4×40px 圆角白色 30% 不透明），锁定时隐藏。

### 10.8 歌词渲染
- 公开两个方法：
  ```rust
  pub fn set_lyrics(&self, payload: &LyricsPayload);
  pub fn set_position(&self, position_ms: i64);
  ```
- `set_lyrics`：把 `lines` 存到 `RefCell<Vec<LyricsLine>>`，重置 `active_index = None`、清空标签。
- `set_position`：
  - 二分查找当前行索引（`rposition(start_time_ms <= position_ms)`）。
  - 索引未变 → return。
  - 索引变了：
    - active label = `line.text` + 翻译（如开启 + 存在 → `\n` + translated_text）
    - next label（仅 dual 模式可见）= 下一行 text
- 客户端插值（`utils/position_clock.rs`）：
  - 收到 `PlaybackStateChanged` 时记录 `(baseline_position, baseline_at, is_playing)`。
  - 以 `glib::timeout_add_local(Duration::from_millis(40), ...)` 跑 25Hz tick：
    `current = baseline_position + (now - baseline_at) * is_playing as i64`
    截断到 `[0, duration_ms]`。
- 当前曲目无歌词或 `payload.sync_type == "unsynced"` 时，active label 显示 `♪ {track} — {artist}`，next label 隐藏。

### 10.9 显示 / 隐藏 / 退出
- `pub fn show()`：`set_visible(true) + present()`。如果 X11 已 realize，重新应用 keep-above（某些 WM 在 unmap → map 时会丢 ABOVE）。
- `pub fn hide()`：`set_visible(false)`。
- 监听 GSettings `desktop-lyrics-enabled` 变化：true → show、false → hide。
- 监听 `close-request`：拦截、隐藏代替关闭、`set_enabled(false)`（让托盘菜单 ☑ 也同步）。

---

## 11. GSettings Schema（`data/cn.spotlyric.Gtk.gschema.xml`）

```xml
<?xml version="1.0" encoding="UTF-8"?>
<schemalist gettext-domain="spot-lyric">
  <schema id="cn.spotlyric.Gtk" path="/cn/spotlyric/Gtk/">

    <!-- 主窗口 -->
    <key name="window-width" type="i"><default>560</default></key>
    <key name="window-height" type="i"><default>680</default></key>

    <!-- 桌面歌词 -->
    <key name="desktop-lyrics-enabled" type="b"><default>true</default></key>
    <key name="desktop-lyrics-locked"  type="b"><default>true</default></key>

    <key name="desktop-lyrics-x" type="i"><default>-1</default></key>
    <key name="desktop-lyrics-y" type="i"><default>-1</default></key>
    <key name="desktop-lyrics-width" type="i"><default>900</default></key>
    <key name="desktop-lyrics-bottom-margin" type="i"><default>80</default></key>

    <key name="desktop-lyrics-font" type="s">
      <default>'HarmonyOS Sans SC Bold 32'</default>
    </key>
    <key name="desktop-lyrics-line-height" type="d"><default>1.4</default></key>

    <key name="desktop-lyrics-active-color"   type="s"><default>'#5fd9ff'</default></key>
    <key name="desktop-lyrics-inactive-color" type="s"><default>'#ffffff'</default></key>
    <key name="desktop-lyrics-stroke-color"   type="s"><default>'#000000'</default></key>
    <key name="desktop-lyrics-stroke-width"   type="i"><default>2</default></key>
    <key name="desktop-lyrics-bg-opacity"     type="d"><default>0.5</default></key>
    <key name="desktop-lyrics-line-mode" type="s">
      <choices><choice value="single"/><choice value="dual"/></choices>
      <default>'dual'</default>
    </key>
    <key name="desktop-lyrics-show-translation" type="b"><default>true</default></key>

    <!-- 歌词源（仅缓存最近一次同步过来的偏好；权威来源是 daemon） -->
    <key name="preferred-provider" type="s">
      <choices><choice value="netease"/><choice value="qq"/></choices>
      <default>'netease'</default>
    </key>
    <key name="timing-offset-ms" type="i"><default>0</default></key>
  </schema>
</schemalist>
```

---

## 12. 资源 / 图标

- `data/icons/scalable/apps/cn.spotlyric.Gtk.svg`：一个简单的"音符 + 文字气泡"组合，使用 GNOME Adwaita 风格调色（轮廓 + 单色填充）。
- 安装时通过 `glib-build-tools::compile_resources!` 把 `style.css` 打入二进制。
- `cn.spotlyric.Gtk.desktop` 中 `Categories=AudioVideo;Audio;Player;`，`Keywords=lyrics;spotify;karaoke`，`Icon=cn.spotlyric.Gtk`。

---

## 13. 快捷键

绑定到 `app.*` action：
| 动作 | 加速键 | 行为 |
|---|---|---|
| `app.preferences` | Ctrl+, | 显示主设置窗 |
| `app.toggle-lyrics` | Ctrl+Shift+L | 显示/隐藏桌面歌词 |
| `app.toggle-lock` | Ctrl+Shift+K | 切换锁定 |
| `app.match-lyrics` | Ctrl+Shift+M | 打开手动匹配 |
| `app.quit` | Ctrl+Q | 退出 |

> **注意**：这些快捷键只在 GTK 窗口聚焦时有效。全局快捷键超出本期范围。

---

## 14. CSS 类清单

| Class | 用途 |
|---|---|
| `.desktop-lyrics-window`        | 桌面歌词根窗（透明背景） |
| `.desktop-lyrics-container`     | 歌词容器盒（圆角+半透明） |
| `.desktop-lyrics-active`        | 当前行 |
| `.desktop-lyrics-next`          | 下一行 |
| `.desktop-lyrics-translation`   | 翻译副歌词（在 active label 第二行） |
| `.drag-handle`                  | 解锁时显示的小拖动条 |
| `.banner-error` / `.banner-warn`/ `.banner-info` | 主窗口错误条样式 |

---

## 15. 错误处理 / 降级

| 场景 | UI 反馈 |
|---|---|
| 内置后端启动失败 | 主窗 banner 红色“未连接到内置后端”，提供“重试”按钮，托盘 tooltip 提示“后端未运行” |
| cookie 过期 | 黄色 banner，提供"重新导入" |
| 歌词 404 | 桌面歌词显示"♪ {track} — {artist}"，主窗弹 toast"未找到此曲歌词，可手动匹配" |
| 歌词加载报错 | 错误 toast，桌面歌词保持上一首/空 |
| X11 协议错误 | 启动时弹 GTK MessageDialog "无法初始化 X11 扩展，桌面歌词功能将不可用"，但程序继续允许打开主设置窗 |
| 没有 SNI 实现 | banner "您的桌面环境未启用托盘图标支持..." |

---

## 16. 测试

### 16.1 单元测试
- `utils/position_clock.rs`：插值算法在 paused / playing 切换、负偏移、超界（>duration）下的输出。
- `bridge/controller.rs`：用 fake D-Bus proxy（trait + mock impl）覆盖：
  - 收到 `PlaybackStateChanged` 时正确生成 `UiUpdate::PlaybackStateChanged`
  - `LoadLyrics` 走整个流程：调 proxy → 解析 JSON → 推 `UiUpdate::LyricsLoaded`

### 16.2 集成测试
- 启动一个 mock daemon（zbus session bus 上注册 `cn.spotlyric.Daemon`），断言 GTK 端在 connect 后 1s 内收到 `Connected`。
- 发出一条 `state_changed` 信号，断言 `DesktopLyricsWindow` 的 active label 在 100ms 内更新到对应行（用 `glib::idle_add_local` 探针）。

### 16.3 手动验收清单
- [ ] X11 + GNOME（启用 AppIndicator）：托盘可见 / 菜单可点 / 锁定切换工作
- [ ] X11 + KDE：同上
- [ ] X11 + XFCE：同上
- [ ] X11 不带合成器：背景退化为不透明，但功能正常
- [ ] 锁定后用鼠标点歌词区域不穿透 → 锁定后再点应能穿透到下层窗口
- [ ] 解锁拖动后位置持久化，重启程序自动恢复
- [ ] 字体改成 `Noto Sans CJK SC Bold 24` 后立即生效，不重启
- [ ] 切歌词源（NetEase ↔ QQ）后下一首自动用新源

---

## 17. 构建 / 安装

### 17.1 build.rs
```rust
fn main() {
    glib_build_tools::compile_resources(
        &["resources"],
        "resources/resources.gresource.xml",
        "spot_lyric.gresource",
    );
}
```

### 17.2 资源描述
```xml
<?xml version="1.0" encoding="UTF-8"?>
<gresources>
  <gresource prefix="/cn/spotlyric/Gtk">
    <file alias="style.css" compressed="true">style.css</file>
  </gresource>
</gresources>
```

### 17.3 安装命令（README 文档中提供）
```bash
cargo build --release
install -Dm755 target/release/spot-lyric-gtk /usr/local/bin/spot-lyric-gtk
install -Dm644 data/cn.spotlyric.Gtk.desktop \
        /usr/local/share/applications/cn.spotlyric.Gtk.desktop
install -Dm644 data/icons/scalable/apps/cn.spotlyric.Gtk.svg \
        /usr/local/share/icons/hicolor/scalable/apps/cn.spotlyric.Gtk.svg
glib-compile-schemas \
        --targetdir=/usr/local/share/glib-2.0/schemas \
        data/
update-desktop-database  /usr/local/share/applications
gtk-update-icon-cache   /usr/local/share/icons/hicolor
```

### 17.4 开发期运行
```bash
GSETTINGS_SCHEMA_DIR=$(pwd)/data \
RUST_LOG=spot_lyric_gtk=debug \
cargo run --release
```

---

## 18. 风险 / 已知限制

| 项 | 说明 | 缓解 |
|---|---|---|
| GNOME 默认无托盘 | 用户没装 AppIndicator 扩展 → 托盘不显示 | 启动时检测并 banner 提示，主窗口提供"最小化为托盘"开关 |
| 无合成器 | 透明背景失败、CSS 阴影效果丢失 | 退化为不透明背景色，文档说明 |
| `_NET_WM_STATE_ABOVE` 被忽略 | 某些极简 WM（dwm / i3 默认）不识别 EWMH | 文档列出推荐 WM 列表 |
| 中文字体 | 系统未装 HarmonyOS Sans / Noto CJK 时显示豆腐块 | 默认字体回退栈 `'HarmonyOS Sans SC', 'Noto Sans CJK SC', 'Source Han Sans SC', sans-serif` |
| 多显示器 | 不同 DPI / 不同分辨率 | 用 `primary_monitor` + 保存绝对坐标；在 v1 不做跨屏自动归位 |

---

## 19. 后续 / Out of scope（本期不做）

- 全局快捷键（需要 X11 grab）
- 歌词卡拉OK级别的 word-by-word 高亮（后端可提供 word level，但前端先按行级实现，留 hook）
- 歌词导出 / 截图
- 多语言界面（先纯中文 UI，字符串集中在 `i18n.rs` 中以备后续 gettext 化）

---

**Done.** 实现按本文 §2 工程结构对应文件展开即可。
