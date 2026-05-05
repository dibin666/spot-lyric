//! Application-wide constants.

/// GApplication / GSettings / GResource ID
pub const APP_ID: &str = "cn.spotlyric.Gtk";

/// In-process backend D-Bus identity (kept stable for GTK bridge proxies)
pub const DAEMON_BUS_NAME: &str = "cn.spotlyric.Daemon";

/// UI tick rate for client-side playback position interpolation.
pub const POSITION_TICK_MS: u64 = 40;

/// How frequently the bridge polls for connection state when disconnected.
pub const RECONNECT_INTERVAL_MS: u64 = 3_000;
