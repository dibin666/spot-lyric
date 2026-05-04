//! Updates pushed from the D-Bus worker thread back to the GTK main thread.

use crate::dbus::types::{
    AuthSnapshot, LyricsCandidate, LyricsPayload, LyricsSettings, PlaybackState,
};

#[derive(Debug, Clone)]
pub enum UiUpdate {
    Connected,
    Disconnected(String),

    PlaybackStateChanged(PlaybackState),

    LyricsLoaded {
        track_uri: String,
        payload: LyricsPayload,
    },
    LyricsLoadFailed {
        track_uri: String,
        error: String,
    },
    LyricsMatchResults(Vec<LyricsCandidate>),
    LyricsPreview(LyricsPayload),
    LyricsMatchSaved {
        track_uri: String,
    },
    LyricsSettingsLoaded(LyricsSettings),

    AuthSnapshotLoaded(AuthSnapshot),
    AuthSnapshotChanged(AuthSnapshot),

    Error(String),
    Toast(String),
}
