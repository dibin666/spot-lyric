pub mod status_notifier;

pub use status_notifier::{
    apply_desktop_settings_to_state, apply_lyrics_provider_to_state, StatusNotifierTray,
    TrayAction, TrayHandle, TrayState,
};
