use futures_util::StreamExt;
use tokio::task::JoinHandle;
use zbus::{connection::Builder, fdo::DBusProxy};

use crate::{
    app_state::AppState,
    config::{DBUS_BUS_NAME, DBUS_OBJECT_PATH},
    dbus::{
        app_iface::AppIface, auth_iface::AuthIface, lyrics_iface::LyricsIface,
        playback_iface::PlaybackIface, to_json_reply,
    },
    error::Result,
};

pub struct DbusRuntime {
    _connection: zbus::Connection,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for DbusRuntime {
    fn drop(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

pub async fn serve_dbus(state: AppState) -> Result<DbusRuntime> {
    let connection = Builder::session()?
        .name(DBUS_BUS_NAME)?
        .serve_at(DBUS_OBJECT_PATH, AuthIface::new(state.clone()))?
        .serve_at(DBUS_OBJECT_PATH, PlaybackIface::new(state.clone()))?
        .serve_at(DBUS_OBJECT_PATH, LyricsIface::new(state.clone()))?
        .serve_at(DBUS_OBJECT_PATH, AppIface::new(state.clone()))?
        .build()
        .await?;

    tracing::info!(
        bus_name = DBUS_BUS_NAME,
        object_path = DBUS_OBJECT_PATH,
        "registered D-Bus interfaces"
    );

    let tasks = vec![
        spawn_auth_signal_pump(connection.clone(), state.clone()),
        spawn_playback_signal_pump(connection.clone(), state.clone()),
        spawn_lyrics_signal_pump(connection.clone(), state.clone()),
        spawn_name_watchdog(connection.clone(), state),
    ];

    Ok(DbusRuntime {
        _connection: connection,
        tasks,
    })
}

fn spawn_auth_signal_pump(connection: zbus::Connection, state: AppState) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut receiver = state.subscribe_auth();
        loop {
            tokio::select! {
                _ = state.wait_for_shutdown() => break,
                result = receiver.recv() => {
                    let Ok(snapshot) = result else { continue; };
                    let Ok(payload) = to_json_reply(&snapshot) else { continue; };
                    if let Ok(iface_ref) = connection.object_server().interface::<_, AuthIface>(DBUS_OBJECT_PATH).await {
                        let emitter = iface_ref.signal_emitter();
                        let _ = AuthIface::snapshot_changed(&emitter, &payload).await;
                    }
                }
            }
        }
    })
}

fn spawn_playback_signal_pump(connection: zbus::Connection, state: AppState) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut receiver = state.subscribe_playback();
        loop {
            tokio::select! {
                _ = state.wait_for_shutdown() => break,
                result = receiver.recv() => {
                    let Ok(playback) = result else { continue; };
                    if let Ok(iface_ref) = connection.object_server().interface::<_, PlaybackIface>(DBUS_OBJECT_PATH).await {
                        let emitter = iface_ref.signal_emitter();
                        let _ = PlaybackIface::state_changed(&emitter, &playback).await;
                    }
                }
            }
        }
    })
}

fn spawn_lyrics_signal_pump(connection: zbus::Connection, state: AppState) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut receiver = state.subscribe_lyrics_settings();
        loop {
            tokio::select! {
                _ = state.wait_for_shutdown() => break,
                result = receiver.recv() => {
                    let Ok(settings) = result else { continue; };
                    let Ok(payload) = to_json_reply(&settings) else { continue; };
                    if let Ok(iface_ref) = connection.object_server().interface::<_, LyricsIface>(DBUS_OBJECT_PATH).await {
                        let emitter = iface_ref.signal_emitter();
                        let _ = LyricsIface::settings_changed(&emitter, &payload).await;
                    }
                }
            }
        }
    })
}

fn spawn_name_watchdog(connection: zbus::Connection, state: AppState) -> JoinHandle<()> {
    tokio::spawn(async move {
        let Ok(proxy) = DBusProxy::new(&connection).await else {
            tracing::error!("failed to create org.freedesktop.DBus proxy for name watchdog");
            state.request_shutdown();
            return;
        };
        let Some(unique_name) = connection.unique_name().map(|name| name.to_string()) else {
            tracing::error!("D-Bus service connection is missing a unique name");
            state.request_shutdown();
            return;
        };
        let mut owner_changes = match proxy.receive_name_owner_changed().await {
            Ok(stream) => stream,
            Err(error) => {
                tracing::error!(%error, "failed to subscribe to D-Bus name-owner changes");
                state.request_shutdown();
                return;
            }
        };

        while let Some(signal) = owner_changes.next().await {
            match signal.args() {
                Ok(args) => {
                    if args.name.as_str() != DBUS_BUS_NAME {
                        continue;
                    }
                    let new_owner = args
                        .new_owner
                        .as_ref()
                        .map(|owner| owner.as_str())
                        .unwrap_or_default();
                    if new_owner == unique_name {
                        continue;
                    }
                    tracing::warn!(
                        old_owner = args.old_owner.as_ref().map(|owner| owner.as_str()).unwrap_or_default(),
                        new_owner,
                        expected_owner = %unique_name,
                        bus_name = DBUS_BUS_NAME,
                        "lost D-Bus name ownership; shutting down"
                    );
                    state.request_shutdown();
                    return;
                }
                Err(error) => {
                    tracing::error!(%error, "failed to decode D-Bus name-owner change signal");
                    state.request_shutdown();
                    return;
                }
            }
        }

        tracing::warn!(
            bus_name = DBUS_BUS_NAME,
            "D-Bus name-owner change stream ended"
        );
        state.request_shutdown();
    })
}
