//! Commands sent from GTK widgets to the D-Bus worker thread.

#[derive(Debug, Clone)]
pub enum Command {
    // ── Connection
    Reconnect,

    // ── Playback control (forwarded to daemon)
    TogglePlaying,
    SkipNext,
    SkipPrevious,

    // ── Lyrics
    LoadLyrics {
        track_uri: String,
    },
    SearchLyricsMatches {
        query: String,
    },
    PreviewLyricsMatch {
        candidate_id: String,
    },
    SaveLyricsMatch {
        track_uri: String,
        candidate_id: String,
    },
    SetPreferredProvider(String),
    SetTimingOffsetMs(i32),
    LoadLyricsSettings,

    // ── Auth
    LoadAuthSnapshot,
    ImportCookieFile(String),
    ImportCookieString(String),
    RefreshAuth,
    ClearCookie,

    // ── Lifecycle
    QuitDaemon,
}
