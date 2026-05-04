use std::path::PathBuf;

use spot_lyric_daemon::{app_state::AppState, config, dbus::server::serve_dbus, Result};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "spot_lyric_daemon=info,zbus=warn".into()),
        )
        .init();

    let data_dir_override = parse_data_dir_override()?;
    let state = AppState::bootstrap(data_dir_override).await?;
    let paths = state.paths();
    tracing::info!(
        app_name = config::APP_NAME,
        dbus_bus_name = config::DBUS_BUS_NAME,
        dbus_object_path = config::DBUS_OBJECT_PATH,
        poll_interval_ms = config::POLL_INTERVAL_MS,
        sqlite_path = %paths.sqlite_path.display(),
        "starting spot-lyric daemon",
    );

    let _dbus_runtime = serve_dbus(state.clone()).await?;
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    tokio::select! {
        signal = tokio::signal::ctrl_c() => {
            signal?;
            tracing::info!("received SIGINT");
            state.request_shutdown();
        }
        _ = sigterm.recv() => {
            tracing::info!("received SIGTERM");
            state.request_shutdown();
        }
        _ = state.wait_for_shutdown() => {
            tracing::info!("shutdown requested");
        }
    }

    state.shutdown().await?;
    tracing::info!("daemon shutdown complete");
    Ok(())
}

fn parse_data_dir_override() -> Result<Option<PathBuf>> {
    let mut args = std::env::args_os().skip(1);
    let mut data_dir = None;
    while let Some(arg) = args.next() {
        if arg == "--data-dir" {
            let value = args.next().ok_or_else(|| {
                spot_lyric_daemon::DaemonError::InvalidArgument("--data-dir requires a path".into())
            })?;
            data_dir = Some(PathBuf::from(value));
        } else {
            return Err(spot_lyric_daemon::DaemonError::InvalidArgument(format!(
                "unknown argument: {}",
                arg.to_string_lossy()
            )));
        }
    }
    Ok(data_dir)
}
