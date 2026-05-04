# Spot-Lyric 后端集成方案

> 本文是给**实现 `spot-lyric-daemon`** 的工程师/AI 看的。前端（`spot-lyric-gtk`）已按 `frontend-ui-design.md` 写好；本文档定义两件事：
>
> 1. **如何从参考工程 `~/work/sporify-client/spol-daemon` 抽取所需逻辑**（到行号级）
> 2. **D-Bus 接口契约**（前端拿这个 spec 直接写代理；后端按这个 spec 实现服务）
>
> **强约束**：D-Bus 名称、接口、对象路径、方法签名、信号字段在前端代码里都已经定死；后端必须**精确**按本文实现，否则前端不工作。

---

## 1. 总体职责

后端独立进程 `spot-lyric-daemon`：

1. **Spotify 网页版逆向认证**：从用户提供的 cookie（含 `sp_dc`）走 TOTP 协议拿到 `access_token` + `client_token`。
2. **拉取播放状态**：通过 Spotify Web 内部 API（不走 OAuth 公共 API），实时（≤2 秒延迟）汇报当前曲目、位置、是否在播。
3. **歌词搜索 + 自动匹配**：用 NetEase Cloud Music 与 QQ Music 两个公开 API 搜索，按用户偏好 provider 排序，使用打分匹配最优候选。
4. **手动匹配 + 持久化**：用户在前端搜索 / 选定候选 → 保存绑定到本地 SQLite，后续这首歌直接走该绑定。
5. **设置存储**：preferred provider、timing offset、saved match。
6. **DBus 服务暴露**：见 §3。

---

## 2. 工程结构（建议）

```
spot-lyric-daemon/
├── Cargo.toml
├── src/
│   ├── main.rs                       # 进程入口 + DBus 注册
│   ├── lib.rs
│   ├── config.rs                     # 路径常量、轮询间隔等
│   ├── error.rs                      # DaemonError + Result
│   │
│   ├── spotify/
│   │   ├── mod.rs
│   │   ├── auth_service.rs           # ★ 直接抽自参考工程
│   │   ├── transport.rs              # ★ 直接抽自参考工程
│   │   ├── discovery.rs              # ★ 直接抽自参考工程
│   │   ├── pathfinder.rs             # ★ 直接抽自参考工程
│   │   ├── connect_state.rs          # ☆ 简化：拉 connect-state JSON
│   │   ├── lyrics_api.rs             # ★ 直接抽自参考工程
│   │   └── secrets.rs                # 内嵌 secret dict
│   │
│   ├── lyrics_external/
│   │   ├── mod.rs                    # ★ 直接抽自参考工程
│   │   ├── netease.rs                # ★ 直接抽自参考工程
│   │   └── qq.rs                     # ★ 直接抽自参考工程
│   │
│   ├── domain/
│   │   ├── mod.rs
│   │   ├── auth_domain.rs
│   │   ├── playback_domain.rs        # 轮询 connect-state，发布快照
│   │   ├── lyrics_domain.rs          # ★ 改造自参考工程
│   │   └── track_match.rs            # ★ 直接抽自 util/track_match.rs
│   │
│   ├── storage/
│   │   ├── mod.rs
│   │   ├── database.rs               # rusqlite 连接池
│   │   ├── cookie_store.rs           # ★ 直接抽自参考工程
│   │   ├── device_store.rs           # ★ 直接抽自参考工程
│   │   └── lyrics_store.rs           # 简化：只保留 saved_match + settings
│   │
│   ├── dbus/
│   │   ├── mod.rs
│   │   ├── server.rs                 # 绑定 cn.spotlyric.Daemon 总线名
│   │   ├── auth_iface.rs             # cn.spotlyric.Auth
│   │   ├── playback_iface.rs         # cn.spotlyric.Playback
│   │   ├── lyrics_iface.rs           # cn.spotlyric.Lyrics
│   │   └── app_iface.rs              # cn.spotlyric.App
│   │
│   └── types/
│       ├── mod.rs
│       ├── auth.rs                   # AuthSnapshot
│       ├── playback.rs               # PlaybackState
│       └── lyrics.rs                 # LyricsPayload, LyricsCandidate, ...
│
└── data/
    └── cn.spotlyric.Daemon.service.in    # systemd user unit
```

`★` = 几乎可以直接复制粘贴的文件
`☆` = 需要重写（参考工程用 Chrome+CDP，我们用 connect-state 走纯 HTTP）

---

## 3. D-Bus 契约（**强制**）

| 项 | 值 |
|---|---|
| Bus | session |
| Service name | `cn.spotlyric.Daemon` |
| Object path | `/cn/spotlyric/Daemon` |

> 前端代理已按这个写在 `spot-lyric-gtk/src/dbus/client.rs`，**任何字段重命名都会让前端编译不过 / 解析错误**。

### 3.1 接口 `cn.spotlyric.Auth`

**方法**：

```text
GetSnapshot()                   → s   (JSON of AuthSnapshot)
ImportCookieFile(path:s)        → s   (JSON of AuthSnapshot)
ImportCookieString(cookie:s)    → s   (JSON of AuthSnapshot)
Refresh()                       → s   (JSON of AuthSnapshot)
ClearCookie()                   → ()
```

**信号**：

```text
SnapshotChanged(snapshot:s)     # JSON of AuthSnapshot —— 任何 status / cookie / token 变化都发
```

**`AuthSnapshot` JSON 结构**：

```json
{
  "device_id": "string",
  "client_id": "string|null",
  "access_token_expires_at": 1730000000000 | null,
  "client_token_expires_at": 1730000000000 | null,
  "active_profile_id": "string|null",
  "has_cookie": true,
  "profiles": [
    { "id": "string", "label": "string", "created_at": 1730000000000 }
  ],
  "status": "idle" | "refreshing" | "ready" | "error",
  "error": "string|null"
}
```

### 3.2 接口 `cn.spotlyric.Playback`

**方法**：

```text
GetState()                      → a{sv}   (PlaybackState dict, 见下)
TogglePlaying()                 → ()
SkipNext()                      → ()
SkipPrevious()                  → ()
```

**信号**：

```text
StateChanged(state:a{sv})       # 每次有变化时推
```

**`PlaybackState`（zbus dict signature `a{sv}`）**：

```rust
#[derive(SerializeDict, DeserializeDict, Type, Clone, PartialEq, Default)]
#[zvariant(signature = "a{sv}")]
pub struct PlaybackState {
    pub is_playing:    bool,    // 是否正在播放（true=播放中）
    pub track_uri:     String,  // "spotify:track:..."；无曲目时为 ""
    pub track_name:    String,
    pub artist_name:   String,  // 多个艺人用 ", " 拼
    pub album_name:    String,
    pub album_art_url: String,
    pub position_ms:   i64,     // 当前进度
    pub duration_ms:   i64,     // 总时长；未知=0
    pub volume:        f64,     // 0.0..1.0
    pub player_status: String,  // "idle"|"connecting"|"ready"|"error"
}
```

**前端依赖的不变量**：
- 信号至少在以下情形下推：曲目变化、播放/暂停切换、用户跳转 seek（前端会 reset 客户端时钟）。
- 在播放中也建议每 1-2 秒推一次（用于客户端时钟矫正、补偿插值漂移）。
- 没有播放时（用户未打开 Spotify），`track_uri = ""` 且 `is_playing = false`。

### 3.3 接口 `cn.spotlyric.Lyrics`

**方法**：

```text
GetTrackLyrics(track_uri:s)            → s   (JSON of LyricsPayload)
SearchManualMatches(query:s)           → s   (JSON of [LyricsCandidate])
PreviewManualMatch(candidate_id:s)     → s   (JSON of LyricsPayload)
SaveManualMatch(track_uri:s,
                candidate_id:s)        → ()
GetSettings()                          → s   (JSON of LyricsSettings)
SetPreferredProvider(provider:s)       → ()  # "netease" | "qq"
SetTimingOffsetMs(offset_ms:i)         → ()
```

**信号**：

```text
SettingsChanged(settings:s)            # JSON of LyricsSettings
```

**`LyricsPayload`**（与参考工程 `types/lyrics.rs` 完全一致）：

```json
{
  "track_uri": "spotify:track:...|null",
  "track_id":  "string|null",
  "language":  "string|null",
  "provider":  "Netease Cloud Music | QQ Music | null",
  "source":    "netease | qq | spotify",
  "sync_type": "line | word | unsynced",
  "lines": [
    {
      "text": "string",
      "translated_text": "string|null",
      "start_time_ms": 1234,
      "end_time_ms":   5678,
      "words": [
        { "text":"string", "start_time_ms":1234, "end_time_ms":1300 }
      ]
    }
  ]
}
```

**`LyricsCandidate`**：

```json
{
  "candidate_id": "base64 of {provider, id, mid?}",
  "title":   "string",
  "album":   "string",
  "artists": ["string"],
  "duration_ms": 234567 | null,
  "provider":    "netease | qq",
  "score":       8.4 | null
}
```

**`LyricsSettings`**：

```json
{
  "lyrics_timing_offset_ms": 0,
  "preferred_provider": "netease | qq",
  "saved_match": { /* StoredLyricsCandidate or null */ } | null
}
```

### 3.4 接口 `cn.spotlyric.App`

```text
Quit()                           → ()
```

调用后 daemon 立即开始优雅退出（保存状态、断开 WebSocket、释放 cookie）。

---

## 4. 抽取自参考工程的逻辑（按文件清单）

> 路径相对于 `~/work/sporify-client/spol-daemon/src/`。

### 4.1 直接复制（基本不改）

| 参考文件 | 目标位置 | 说明 |
|---|---|---|
| `lyrics_external/mod.rs` | `lyrics_external/mod.rs` | LRC 解析、normalize、attach_translated_lyrics、apply_timing_offset 全部需要 |
| `lyrics_external/netease.rs` | `lyrics_external/netease.rs` | NetEase 搜索/详情/lyric 客户端 |
| `lyrics_external/qq.rs` | `lyrics_external/qq.rs` | QQ Music 搜索/详情/lyric 客户端 |
| `util/track_match.rs` | `domain/track_match.rs` | 打分匹配 |
| `util/spotify.rs` | `util/spotify.rs` | base62 / hex_id 工具 |
| `util/convert.rs` | `util/convert.rs` | candidate_id 编解码、provider 偏好枚举 |
| `spotify/auth_service.rs` | `spotify/auth_service.rs` | TOTP + cookie + access_token + client_token |
| `spotify/transport.rs` | `spotify/transport.rs` | 带认证 / 重试 / 限流的 HTTP 客户端 |
| `spotify/discovery.rs` | `spotify/discovery.rs` | apresolve（dealer + spclient endpoints） |
| `spotify/pathfinder.rs` | `spotify/pathfinder.rs` | 协议常量 + Pathfinder hash 表 |
| `spotify/lyrics_api.rs` | `spotify/lyrics_api.rs` | Spotify color-lyrics 兜底 |
| `storage/cookie_store.rs` | `storage/cookie_store.rs` | profile 存储 |
| `storage/device_store.rs` | `storage/device_store.rs` | device_id 持久化 |
| `storage/database.rs` | `storage/database.rs` | rusqlite 连接 |

### 4.2 改造（需要精简）

#### `domain/lyrics_domain.rs` ← `domain/lyrics_domain.rs:1-426`

参考实现是完整的（带 spotify color-lyrics 兜底、saved_match、preview/save/search、settings）。可以**整体复制**，删除两个无关引用：

- `track_domain` 引用：参考工程用它从 spotify metadata 拉曲目信息。本工程改成接收前端传来的 `track_uri` 后，从**playback 当前快照**中读 `track.name / artists / album_name / duration_ms`（playback domain 自己持有最近一次的 track info），不需要单独的 track_domain。
- `comments_*`：删，本工程不做评论功能。

简化后函数签名保持不变：
```rust
pub async fn get_track_lyrics(&self, track_uri: &str) -> Result<LyricsPayload>;
pub async fn preview_manual_match(&self, track_uri: Option<&str>, candidate_id: &str) -> Result<LyricsPayload>;
pub async fn save_manual_match(&self, track_uri: &str, candidate_id: &str) -> Result<LyricsSettings>;
pub async fn search_manual_matches(&self, query: &str) -> Result<Vec<LyricsCandidate>>;
pub async fn set_preferred_provider(&self, provider: &str) -> Result<LyricsSettings>;
pub async fn set_timing_offset_ms(&self, offset_ms: i32) -> Result<LyricsSettings>;
pub async fn get_settings(&self, track_uri: Option<&str>) -> Result<LyricsSettings>;
```

#### `storage/lyrics_store.rs`

只需要这些列：
- `lyrics_settings` 单行表：`preferred_provider TEXT`, `lyrics_timing_offset_ms INTEGER`
- `saved_lyrics_match`：`spotify_track_id TEXT PK`, `provider TEXT`, `id TEXT`, `mid TEXT|NULL`, `title TEXT`, `album TEXT`, `artists TEXT (JSON)`, `duration_ms INTEGER|NULL`, `created_at INTEGER`, `updated_at INTEGER`

可以从参考工程的 `lyrics_store.rs` 删掉所有 cache 表（`color_lyrics_cache` 等）—— 缓存层对本工程不必要，访问 NetEase/QQ 已经足够快。

### 4.3 ★ 重写：播放状态拉取

**这是与参考工程最大的差异**。参考工程通过启动 Xvfb + Chrome + CDP，注入 JS 嗅探 Spotify Web Player 内部状态。这套方案功能强但启动慢、依赖重（Chromium、Xvfb、Widevine）。

**本工程改用 Spotify 官方 connect-state HTTP API**：

#### 4.3.1 概念

Spotify Web Player 在线时会作为一个"connect device"出现在用户的设备列表里。通过 `/connect-state/v1/devices/hobs_<device_id>` GET，可以**直接拿到当前设备列表 + 当前活跃设备的播放状态**。这个端点在参考工程的 `pathfinder.rs:62` 已经声明了（`connect_state_path`），但参考工程没用它，因为它要把自己作为可控 device。

我们不需要做"可控 device"——我们只读用户其它设备（手机、桌面 Spotify、网页）的播放状态即可。

#### 4.3.2 实现要点（`spotify/connect_state.rs`）

```rust
pub struct ConnectStateClient {
    transport: SpotifyTransport,
    protocol:  ProtocolRegistry,
    device_id: String,
}

#[derive(Deserialize)]
struct ConnectStateResponse {
    pub player_state: Option<PlayerStateView>,
    pub active_device_id: Option<String>,
    // ...
}

#[derive(Deserialize)]
struct PlayerStateView {
    pub is_playing:   bool,
    pub is_paused:    bool,
    pub timestamp:    i64,         // 服务器时间戳
    pub position_as_of_timestamp: i64,
    pub duration:     i64,
    pub track: Option<TrackView>,
    // ...
}

impl ConnectStateClient {
    pub async fn fetch_state(&self) -> Result<Option<PlaybackSnapshot>> {
        let url = self.protocol.build_spclient_url(
            &format!("/connect-state/v1/devices/hobs_{}", self.device_id)
        )?;
        let raw: serde_json::Value =
            self.transport.put_json(url, json!({
                "member_type": "CONNECT_STATE",
                "device": { "device_info": minimal_device_info() }
            }), None).await?;
        Ok(map_player_state(&raw))
    }
}
```

注意：
- **endpoint 是 PUT 不是 GET**——你需要"宣布"自己作为一个 listener，服务器才返回完整状态。device_info 提供最小集（type=COMPUTER、name="spot-lyric"、capabilities.gaia_eq_connect_id=true、is_observable=true）即可。
- 上述端点的具体协议在参考工程的 `proto/connect_state.proto` 中声明（仅 schema，不强制 use）。本工程纯走 JSON 路径，不需要 protobuf。

#### 4.3.3 实时更新

两种途径，**两者并存**：

1. **HTTP 轮询**：每 2 秒调一次 `fetch_state`。代价低，足够流畅（前端会做客户端插值）。
2. **Dealer WebSocket**：参考工程的 `spotify/dealer.rs` 给了一个空壳。完整实现是连接 `wss://dealer.../?access_token=...`，订阅 `playlist`、`playback` 等 message channel。当用户在手机/Web 切歌时，dealer 会推送 `cluster_update` 等消息，可以**立刻拉一次** `fetch_state`，达到 <500ms 延迟。

> 推荐先做 HTTP 轮询，简单可靠；dealer 作为 v2 优化。前端代码已经按 1-2 秒延迟设计客户端时钟，纯轮询完全够用。

#### 4.3.4 PlaybackDomain（`domain/playback_domain.rs`）

```rust
pub struct PlaybackDomain {
    state:      Arc<RwLock<PlaybackState>>,
    last_track: Arc<RwLock<Option<TrackInfo>>>,    // 给 lyrics_domain 用
    notifier:   broadcast::Sender<PlaybackState>,
    connect:    ConnectStateClient,
}

impl PlaybackDomain {
    pub async fn run(self: Arc<Self>) {
        let mut interval = tokio::time::interval(Duration::from_millis(2_000));
        loop {
            interval.tick().await;
            match self.connect.fetch_state().await {
                Ok(Some(snapshot)) => self.publish(snapshot).await,
                Ok(None) => self.publish_idle().await,
                Err(error) => {
                    tracing::warn!(?error, "connect-state poll failed");
                    // 不更新 state；前端时钟会继续插值；如果连续 N 次失败再切换到 player_status="error"
                }
            }
        }
    }
}
```

`publish`：写入 `state` + `last_track` + 走 `notifier.send`，DBus 层订阅 `notifier` 把变化转成信号。

### 4.4 简化的拷贝：`config.rs`

```rust
pub const APP_NAME: &str = "spot-lyric";
pub const DBUS_NAME: &str = "cn.spotlyric.Daemon";
pub const DBUS_PATH: &str = "/cn/spotlyric/Daemon";
pub const POLL_INTERVAL_MS: u64 = 2_000;
pub fn data_dir() -> PathBuf { dirs::data_local_dir().unwrap().join("spot-lyric") }
pub fn cookie_path() -> PathBuf { data_dir().join("cookie.txt") }
pub fn db_path() -> PathBuf { data_dir().join("spot-lyric.db") }
```

---

## 5. 关键算法详解

### 5.1 自动匹配（已经在 `track_match.rs` 里了，复用即可）

输入：当前 `TrackInfo { name, artists, album_name, duration_ms }` + `preferred_provider: "netease"|"qq"`

步骤：
1. `build_search_queries(track)` → 6 条候选查询字符串（artist+title 组合 + 版本剥离）
2. 对每条查询并发调 `netease.search_tracks` + `qq.search_tracks`
3. `dedupe_candidates` 去重
4. `rank_candidates_for_track(track, all, Some(preferred))`：
   - 时长过滤：`|duration - candidate.duration| > 3000ms` 直接淘汰
   - 打分：`title × 1.0 + artists × 1.0 + album × 0.4 + duration × 2.0`
   - 排序：偏好 provider 优先，然后按分数降
5. 依次拉每个候选的歌词，第一个有内容的返回
6. 都失败 → 兜底 Spotify color-lyrics（参考工程 `lyrics_api.rs`）
7. 还失败 → 返回 `sync_type="unsynced"` 的空 payload，前端会展示"♪ track — artist"

### 5.2 saved_match 优先

`get_track_lyrics(track_uri)` 流程：
```
1. 查 settings.saved_match（按 spotify_track_id）
   有 → 直接拉对应 candidate 的 lyrics → 应用 offset → 返回
2. 否则走自动匹配（5.1）
```

### 5.3 timing offset

`apply_timing_offset(payload, offset_ms)`：
- 偏移量限制在 ±5000 ms（`MAX_TIMING_OFFSET_MS`）。
- spotify 源的歌词不应用偏移（spotify 自己时间戳是准的）。
- unsynced 不应用偏移。
- 其它 source 给每行 / 每词加上 `offset_ms`。

参考工程 `lyrics_external/mod.rs:40-72` 已实现，复制即可。

---

## 6. 进程生命周期 / systemd

### 6.1 `data/cn.spotlyric.Daemon.service.in`

```ini
[Unit]
Description=Spot-Lyric daemon (Spotify lyrics overlay backend)
After=network-online.target

[Service]
Type=dbus
BusName=cn.spotlyric.Daemon
ExecStart=@bindir@/spot-lyric-daemon
Restart=on-failure
RestartSec=3

[Install]
WantedBy=default.target
```

> 启动时用 `dbus-daemon` 自动激活也可以（在 `/usr/share/dbus-1/services/cn.spotlyric.Daemon.service` 里 `Exec=` 指向二进制即可），那样前端在 `connect()` 时会自动起 daemon。

### 6.2 启动流程

```
main()
 ├─ parse_args (--data-dir override)
 ├─ tracing 初始化
 ├─ 打开 SQLite → 跑 migration
 ├─ 创建 reqwest::Client（rustls + cookie 不持久化）
 ├─ DiscoveryService::new + 预热 apresolve
 ├─ ProtocolRegistry::default
 ├─ AuthService::new(...) → 内部 try refresh
 ├─ SpotifyTransport::new(auth, client, protocol)
 ├─ ConnectStateClient::new
 ├─ NeteaseLyricsClient + QqMusicLyricsClient
 ├─ LyricsStore + LyricsDomain
 ├─ PlaybackDomain::spawn(...) → 后台轮询
 ├─ DBus 服务注册：
 │     ├─ cn.spotlyric.Auth
 │     ├─ cn.spotlyric.Playback
 │     ├─ cn.spotlyric.Lyrics
 │     └─ cn.spotlyric.App
 ├─ 注册 SIGINT / SIGTERM 处理器 → 清理流程
 └─ park 主线程
```

### 6.3 退出

收到 `App.Quit()` 或信号：
1. PlaybackDomain 停止轮询。
2. 关闭 dealer WebSocket（如果开了）。
3. SQLite checkpoint。
4. DBus 注销。
5. 进程结束。

---

## 7. 错误处理 / 边界

| 场景 | daemon 行为 | 前端可见 |
|---|---|---|
| 没有 cookie | `auth.status="idle"` | 主窗 banner"未登录" |
| cookie 过期（连续刷新失败 3 次） | `auth.status="error"`, error="Cookie expired" | banner"登录已过期" |
| connect-state 401 / 403 | 触发 auth.force_refresh，重试 1 次 | 没有可见变化 |
| connect-state 持续失败 | `playback.player_status="error"` 但保留最后曲目信息 | 桌面歌词保持不变 |
| 用户没在任何设备播放 | `playback` 推 `is_playing=false, track_uri=""` | 桌面歌词隐藏内容（只显示空 container） |
| NetEase / QQ 双双 timeout | `get_track_lyrics` 返回空 payload (sync_type=unsynced) | 桌面歌词降级为"♪ track" |
| candidate 链接形式输入（用户复制了 NetEase 歌曲 URL） | `lyrics_domain.search_manual_matches` 已支持解析 URL（参考工程 `lyrics_domain.rs:351-385`） | 直接出准确候选 |

---

## 8. 测试策略（建议）

### 8.1 单元测试（直接抽自参考工程）

参考工程已经有相当完善的单测：
- `lyrics_external/mod.rs::tests` — LRC 解析 / search query 规范化
- `lyrics_external/netease.rs::tests` — 重试码、search limit
- `lyrics_external/qq.rs::tests` — 缺 mid 时返回 None
- `util/track_match.rs::tests` — 排序、版本剥离、时长容差

**全部应该原样跑通**。把它们复制过来作为最低保障。

### 8.2 集成测试

- **Auth flow**：用一个 `mockito` server 模拟 `/api/token` + `/clienttoken` + `/apresolve`，验证 `AuthService::refresh` 拿到 access_token。
- **Connect-state**：mockito 返回固定 player_state JSON，验证 `PlaybackDomain` 在 2s 内推出 `PlaybackState`。
- **DBus**：起一个内存 `zbus::Connection`（session bus 测试模式），mount Auth/Playback/Lyrics/App 接口，从客户端发 `GetTrackLyrics(uri)`，断言返回的 JSON 字段正确。

---

## 9. 与前端的契约自检清单

实现完后，**逐项验证**这些场景，前端就能正常工作：

- [ ] 启动时 `cn.spotlyric.Daemon` 在 session bus 上注册成功（`busctl --user list | grep spotlyric`）。
- [ ] `dbus-send --session --print-reply --dest=cn.spotlyric.Daemon /cn/spotlyric/Daemon cn.spotlyric.Auth.GetSnapshot` 返回包含 `device_id`、`status` 字段的 JSON 字符串。
- [ ] 导入有效 cookie 后，`Auth.SnapshotChanged` 信号在 5s 内推出 `status="ready"`。
- [ ] 在 Spotify 网页 / 手机播放任意一首歌，`Playback.StateChanged` 在 3s 内推出包含 `track_uri="spotify:track:..."` 的状态。
- [ ] `Lyrics.GetTrackLyrics("spotify:track:...")` 对热门歌曲（如 `2TpxZ7JUBn3uw46aR7qd6V`）返回 `lines.length > 0`。
- [ ] `Lyrics.SearchManualMatches("Counting Stars OneRepublic")` 返回至少 5 条 candidate。
- [ ] `Lyrics.SaveManualMatch(...)` 后再调 `GetTrackLyrics` 用的就是绑定的 candidate（验证：candidate provider == 用户选的）。
- [ ] `Lyrics.SetPreferredProvider("qq")` 后，`SettingsChanged` 信号被推出，且新一首歌的搜索结果 QQ provider 排在前。
- [ ] `App.Quit()` 后 `cn.spotlyric.Daemon` 总线名 5 秒内消失。

---

## 10. 推荐开发顺序

1. **Skeleton + DBus**：先把所有接口 mock 成返回固定数据（`Lyrics.GetTrackLyrics` 返回固定 LRC，`Playback.GetState` 返回固定曲目）。前端连上能跑通界面。
2. **Auth**：从参考工程抽 auth + transport + discovery，能从真实 cookie 拿到 access_token。
3. **Lyrics 离线**：把 NetEase / QQ 客户端、track_match、lyrics_domain 全装上，无视 playback，用前端手动匹配对话框测试。
4. **Connect-state**：实现 `fetch_state` PUT，跑通真实播放状态。
5. **Saved match + persistence**：SQLite 表、settings、saved_match 流。
6. **Dealer WebSocket（可选）**：低延迟切歌。
7. **systemd unit + 自动启动**。

---

**Done.** 后端任何疑问看本文 + 参考工程对应文件即可，无需揣测。
