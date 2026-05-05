//! D-Bus bridge: owns a private tokio runtime on a worker thread, exposes
//! a `Command` channel inwards and a `UiUpdate` channel outwards.

use std::sync::{mpsc as std_mpsc, Arc};
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

use super::commands::Command;
use super::updates::UiUpdate;
use crate::backend_runtime::BackendRuntime;
use crate::config;
use crate::dbus::client::DaemonClient;
use crate::dbus::types::{
    AuthSnapshot, LyricsCandidate, LyricsPayload, LyricsSettings, PlaybackState,
};

/// Public handle returned to the GTK side.
pub struct Bridge {
    pub cmd_tx: mpsc::UnboundedSender<Command>,
}

impl Bridge {
    /// Spawn the worker thread. Returns a command channel and a `std::mpsc`
    /// receiver of UI updates suitable for polling from the glib main loop.
    pub fn start(backend_runtime: BackendRuntime) -> (Self, std_mpsc::Receiver<UiUpdate>) {
        let (ui_tx, ui_rx) = std_mpsc::channel::<UiUpdate>();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Command>();

        std::thread::Builder::new()
            .name("spot-lyric-bridge".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("tokio runtime for bridge");
                runtime.block_on(run_bridge(ui_tx, cmd_rx, backend_runtime));
            })
            .expect("spawn bridge thread");

        (Self { cmd_tx }, ui_rx)
    }

    pub fn send(&self, cmd: Command) {
        if let Err(error) = self.cmd_tx.send(cmd) {
            error!("Bridge command channel closed: {error}");
        }
    }
}

// ─── Worker ──────────────────────────────────────────────────────────────────

type UiSender = std_mpsc::Sender<UiUpdate>;

#[derive(Default)]
struct Holdings {
    /// Used by the lyrics loader to remember which track triggered a load
    /// (so a stale lyrics response after a track switch is ignored).
    last_loaded_uri: RwLock<Option<String>>,
}

async fn run_bridge(
    ui_tx: UiSender,
    mut cmd_rx: mpsc::UnboundedReceiver<Command>,
    backend_runtime: BackendRuntime,
) {
    let holdings = Arc::new(Holdings::default());

    // Reconnect loop: repeatedly attempt to connect; once connected, run the
    // command/signal pump until the connection drops, then go back to retry.
    loop {
        if let Err(error) = backend_runtime.ensure_running() {
            let message = format!("integrated backend unavailable: {error}");
            warn!("{message}");
            let _ = ui_tx.send(UiUpdate::Disconnected(message));
            if !wait_for_reconnect(&mut cmd_rx).await {
                return;
            }
            continue;
        }
        match DaemonClient::connect().await {
            Ok(client) => {
                let _ = ui_tx.send(UiUpdate::Connected);
                info!(
                    "Connected to integrated backend over {}",
                    config::DAEMON_BUS_NAME
                );

                // Spawn signal listeners (they hold weak refs to the client)
                spawn_auth_signal(client.clone(), ui_tx.clone());
                spawn_playback_signal(client.clone(), ui_tx.clone());
                spawn_lyrics_signal(client.clone(), ui_tx.clone());

                // Push initial snapshots to the UI
                fetch_initial_state(&client, &ui_tx).await;

                // Run command loop until the channel closes or a fatal error occurs.
                if let Err(error) = run_command_loop(&client, &ui_tx, &mut cmd_rx, &holdings).await
                {
                    warn!("Bridge command loop ended: {error}");
                    let _ = ui_tx.send(UiUpdate::Disconnected(error));
                } else {
                    debug!("Bridge command channel closed; exiting worker thread");
                    return;
                }
            }
            Err(error) => {
                let message = format!("D-Bus connect failed: {error}");
                debug!("{message}");
                let _ = ui_tx.send(UiUpdate::Disconnected(message));
            }
        }

        // Wait for either an explicit Reconnect command or a timeout, then retry.
        if !wait_for_reconnect(&mut cmd_rx).await {
            return;
        }
    }
}

/// Sleep until either:
///   - a timer elapses (auto-retry), or
///   - the UI sends `Command::Reconnect`, or
///   - the command channel closes (returns false → worker should exit).
async fn wait_for_reconnect(cmd_rx: &mut mpsc::UnboundedReceiver<Command>) -> bool {
    loop {
        let timer = tokio::time::sleep(Duration::from_millis(config::RECONNECT_INTERVAL_MS));
        tokio::pin!(timer);

        tokio::select! {
            _ = &mut timer => return true,
            cmd = cmd_rx.recv() => match cmd {
                None => return false,
                Some(Command::Reconnect) => return true,
                // Drop other commands that arrive while disconnected.
                Some(other) => debug!("Dropping command while disconnected: {other:?}"),
            }
        }
    }
}

async fn run_command_loop(
    client: &DaemonClient,
    ui_tx: &UiSender,
    cmd_rx: &mut mpsc::UnboundedReceiver<Command>,
    holdings: &Arc<Holdings>,
) -> Result<(), String> {
    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            Command::Reconnect => continue, // already connected; no-op

            Command::TogglePlaying => zbus_call(client.playback.toggle_playing(), ui_tx).await,
            Command::SkipNext => zbus_call(client.playback.skip_next(), ui_tx).await,
            Command::SkipPrevious => zbus_call(client.playback.skip_previous(), ui_tx).await,

            Command::LoadLyrics { track_uri } => {
                *holdings.last_loaded_uri.write().await = Some(track_uri.clone());
                load_lyrics(client, ui_tx, holdings, track_uri).await;
            }
            Command::SearchLyricsMatches { query } => {
                search_lyrics_matches(client, ui_tx, query).await;
            }
            Command::PreviewLyricsMatch { candidate_id } => {
                preview_lyrics_match(client, ui_tx, candidate_id).await;
            }
            Command::SaveLyricsMatch {
                track_uri,
                candidate_id,
            } => save_lyrics_match(client, ui_tx, track_uri, candidate_id).await,
            Command::SetPreferredProvider(provider) => {
                match client.lyrics.set_preferred_provider(&provider).await {
                    Ok(_) => debug!("preferred provider set to {provider}"),
                    Err(error) => report_error(ui_tx, error.to_string()),
                }
            }
            Command::SetTimingOffsetMs(offset) => {
                match client.lyrics.set_timing_offset_ms(offset).await {
                    Ok(_) => debug!("timing offset set to {offset} ms"),
                    Err(error) => report_error(ui_tx, error.to_string()),
                }
            }
            Command::LoadLyricsSettings => load_lyrics_settings(client, ui_tx).await,

            Command::LoadAuthSnapshot => load_auth_snapshot(client, ui_tx).await,
            Command::ImportCookieFile(path) => import_cookie_file(client, ui_tx, path).await,
            Command::ImportCookieString(cookie) => {
                import_cookie_string(client, ui_tx, cookie).await
            }
            Command::RefreshAuth => refresh_auth(client, ui_tx).await,
            Command::ClearCookie => clear_cookie(client, ui_tx).await,
        }
    }
    Ok(())
}

// ─── Signal listeners ───────────────────────────────────────────────────────

fn spawn_auth_signal(client: DaemonClient, ui_tx: UiSender) {
    tokio::spawn(async move {
        match client.auth.receive_snapshot_changed().await {
            Ok(mut stream) => {
                while let Some(signal) = stream.next().await {
                    match signal.args() {
                        Ok(args) => match serde_json::from_str::<AuthSnapshot>(args.snapshot) {
                            Ok(snapshot) => {
                                let _ = ui_tx.send(UiUpdate::AuthSnapshotChanged(snapshot));
                            }
                            Err(error) => warn!("Auth snapshot parse failed: {error}"),
                        },
                        Err(error) => warn!("Auth signal arg error: {error}"),
                    }
                }
                warn!("Auth signal stream ended");
                let _ = ui_tx.send(UiUpdate::Disconnected("Auth signal closed".into()));
            }
            Err(error) => error!("Auth signal subscribe failed: {error}"),
        }
    });
}

fn spawn_playback_signal(client: DaemonClient, ui_tx: UiSender) {
    tokio::spawn(async move {
        match client.playback.receive_state_changed().await {
            Ok(mut stream) => {
                while let Some(signal) = stream.next().await {
                    match signal.args() {
                        Ok(args) => {
                            let _ = ui_tx.send(UiUpdate::PlaybackStateChanged(args.state));
                        }
                        Err(error) => warn!("Playback signal arg error: {error}"),
                    }
                }
                warn!("Playback signal stream ended");
                let _ = ui_tx.send(UiUpdate::Disconnected("Playback signal closed".into()));
            }
            Err(error) => error!("Playback signal subscribe failed: {error}"),
        }
    });
}

fn spawn_lyrics_signal(client: DaemonClient, ui_tx: UiSender) {
    tokio::spawn(async move {
        match client.lyrics.receive_settings_changed().await {
            Ok(mut stream) => {
                while let Some(signal) = stream.next().await {
                    match signal.args() {
                        Ok(args) => match serde_json::from_str::<LyricsSettings>(args.settings) {
                            Ok(settings) => {
                                let _ = ui_tx.send(UiUpdate::LyricsSettingsLoaded(settings));
                            }
                            Err(error) => warn!("Lyrics settings parse failed: {error}"),
                        },
                        Err(error) => warn!("Lyrics signal arg error: {error}"),
                    }
                }
            }
            Err(error) => error!("Lyrics signal subscribe failed: {error}"),
        }
    });
}

// ─── Initial state ──────────────────────────────────────────────────────────

async fn fetch_initial_state(client: &DaemonClient, ui_tx: &UiSender) {
    load_auth_snapshot(client, ui_tx).await;

    match client.playback.get_state().await {
        Ok(state) => {
            let _ = ui_tx.send(UiUpdate::PlaybackStateChanged(state));
        }
        Err(error) => warn!("Initial playback state: {error}"),
    }

    load_lyrics_settings(client, ui_tx).await;
}

// ─── Per-command handlers ───────────────────────────────────────────────────

async fn zbus_call(future: impl std::future::Future<Output = zbus::Result<()>>, ui_tx: &UiSender) {
    if let Err(error) = future.await {
        report_error(ui_tx, error.to_string());
    }
}

async fn load_lyrics(
    client: &DaemonClient,
    ui_tx: &UiSender,
    holdings: &Arc<Holdings>,
    track_uri: String,
) {
    match client.lyrics.get_track_lyrics(&track_uri).await {
        Ok(json) => match serde_json::from_str::<LyricsPayload>(&json) {
            Ok(payload) => {
                let still_current = holdings
                    .last_loaded_uri
                    .read()
                    .await
                    .as_deref()
                    .map(|current| current == track_uri.as_str())
                    .unwrap_or(true);
                if still_current {
                    let _ = ui_tx.send(UiUpdate::LyricsLoaded { track_uri, payload });
                }
            }
            Err(error) => {
                let _ = ui_tx.send(UiUpdate::LyricsLoadFailed {
                    track_uri,
                    error: format!("parse: {error}"),
                });
            }
        },
        Err(error) => {
            let _ = ui_tx.send(UiUpdate::LyricsLoadFailed {
                track_uri,
                error: error.to_string(),
            });
        }
    }
}

async fn search_lyrics_matches(client: &DaemonClient, ui_tx: &UiSender, query: String) {
    match client.lyrics.search_manual_matches(&query).await {
        Ok(json) => match serde_json::from_str::<Vec<LyricsCandidate>>(&json) {
            Ok(candidates) => {
                let _ = ui_tx.send(UiUpdate::LyricsMatchResults(candidates));
            }
            Err(error) => report_error(ui_tx, format!("parse candidates: {error}")),
        },
        Err(error) => report_error(ui_tx, error.to_string()),
    }
}

async fn preview_lyrics_match(client: &DaemonClient, ui_tx: &UiSender, candidate_id: String) {
    match client.lyrics.preview_manual_match(&candidate_id).await {
        Ok(json) => match serde_json::from_str::<LyricsPayload>(&json) {
            Ok(payload) => {
                let _ = ui_tx.send(UiUpdate::LyricsPreview(payload));
            }
            Err(error) => report_error(ui_tx, format!("parse preview: {error}")),
        },
        Err(error) => report_error(ui_tx, error.to_string()),
    }
}

async fn save_lyrics_match(
    client: &DaemonClient,
    ui_tx: &UiSender,
    track_uri: String,
    candidate_id: String,
) {
    match client
        .lyrics
        .save_manual_match(&track_uri, &candidate_id)
        .await
    {
        Ok(_) => {
            let _ = ui_tx.send(UiUpdate::LyricsMatchSaved {
                track_uri: track_uri.clone(),
            });
            // After saving, ask daemon for fresh lyrics so the overlay updates.
            // Reuse the same path so the holdings guard tracks the request.
            let _ = Arc::new(track_uri.clone()); // no-op: keep variable lifetime
            match client.lyrics.get_track_lyrics(&track_uri).await {
                Ok(json) => {
                    if let Ok(payload) = serde_json::from_str::<LyricsPayload>(&json) {
                        let _ = ui_tx.send(UiUpdate::LyricsLoaded { track_uri, payload });
                    }
                }
                Err(error) => warn!("Re-fetch lyrics after save failed: {error}"),
            }
        }
        Err(error) => report_error(ui_tx, error.to_string()),
    }
}

async fn load_lyrics_settings(client: &DaemonClient, ui_tx: &UiSender) {
    match client.lyrics.get_settings().await {
        Ok(json) => match serde_json::from_str::<LyricsSettings>(&json) {
            Ok(settings) => {
                let _ = ui_tx.send(UiUpdate::LyricsSettingsLoaded(settings));
            }
            Err(error) => report_error(ui_tx, format!("parse lyrics settings: {error}")),
        },
        Err(error) => report_error(ui_tx, error.to_string()),
    }
}

async fn load_auth_snapshot(client: &DaemonClient, ui_tx: &UiSender) {
    match client.auth.get_snapshot().await {
        Ok(json) => match serde_json::from_str::<AuthSnapshot>(&json) {
            Ok(snapshot) => {
                let _ = ui_tx.send(UiUpdate::AuthSnapshotLoaded(snapshot));
            }
            Err(error) => report_error(ui_tx, format!("parse auth snapshot: {error}")),
        },
        Err(error) => report_error(ui_tx, error.to_string()),
    }
}

async fn import_cookie_file(client: &DaemonClient, ui_tx: &UiSender, path: String) {
    match client.auth.import_cookie_file(&path).await {
        Ok(json) => match serde_json::from_str::<AuthSnapshot>(&json) {
            Ok(snapshot) => {
                let _ = ui_tx.send(UiUpdate::AuthSnapshotLoaded(snapshot));
                let _ = ui_tx.send(UiUpdate::Toast("Cookie 已导入".into()));
            }
            Err(error) => report_error(ui_tx, format!("parse auth snapshot: {error}")),
        },
        Err(error) => report_error(ui_tx, error.to_string()),
    }
}

async fn import_cookie_string(client: &DaemonClient, ui_tx: &UiSender, cookie: String) {
    match client.auth.import_cookie_string(&cookie).await {
        Ok(json) => match serde_json::from_str::<AuthSnapshot>(&json) {
            Ok(snapshot) => {
                let _ = ui_tx.send(UiUpdate::AuthSnapshotLoaded(snapshot));
                let _ = ui_tx.send(UiUpdate::Toast("Cookie 已导入".into()));
            }
            Err(error) => report_error(ui_tx, format!("parse auth snapshot: {error}")),
        },
        Err(error) => report_error(ui_tx, error.to_string()),
    }
}

async fn refresh_auth(client: &DaemonClient, ui_tx: &UiSender) {
    match client.auth.refresh().await {
        Ok(json) => match serde_json::from_str::<AuthSnapshot>(&json) {
            Ok(snapshot) => {
                let _ = ui_tx.send(UiUpdate::AuthSnapshotLoaded(snapshot));
            }
            Err(error) => report_error(ui_tx, format!("parse auth snapshot: {error}")),
        },
        Err(error) => report_error(ui_tx, error.to_string()),
    }
}

async fn clear_cookie(client: &DaemonClient, ui_tx: &UiSender) {
    if let Err(error) = client.auth.clear_cookie().await {
        report_error(ui_tx, error.to_string());
        return;
    }
    let _ = ui_tx.send(UiUpdate::Toast("已清除登录".into()));
    load_auth_snapshot(client, ui_tx).await;
}

fn report_error(ui_tx: &UiSender, message: String) {
    warn!("Bridge error: {message}");
    let _ = ui_tx.send(UiUpdate::Error(message));
}

// Unused but kept to keep compiler-checked PlaybackState shape close to the
// daemon contract.
#[allow(dead_code)]
fn ensure_playback_state_shape(state: PlaybackState) -> PlaybackState {
    state
}
