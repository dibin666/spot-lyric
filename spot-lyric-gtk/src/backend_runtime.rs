use std::{
    env,
    ffi::OsString,
    path::PathBuf,
    sync::{mpsc as std_mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use spot_lyric_daemon::{app_state::AppState, config as backend_config, dbus::server::serve_dbus};
use tokio::sync::oneshot;
use tracing::{debug, error, info, warn};
#[cfg(test)]
use zbus::{fdo::DBusProxy, names::BusName, Connection};

#[cfg(test)]
use crate::config;

const BACKEND_DATA_DIR_ENV_VAR: &str = "SPOT_LYRIC_DATA_DIR";
const LEGACY_DAEMON_DATA_DIR_ENV_VAR: &str = "SPOT_LYRIC_DAEMON_DATA_DIR";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(8);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

type StartupResult = Result<BackendStarted, String>;

#[derive(Clone, Default)]
pub struct BackendRuntime {
    inner: Arc<Mutex<RuntimeState>>,
}

impl BackendRuntime {
    pub fn ensure_running(&self) -> Result<(), String> {
        self.inner
            .lock()
            .expect("backend runtime lock poisoned")
            .ensure_running()
    }

    pub fn shutdown(&self) {
        self.inner
            .lock()
            .expect("backend runtime lock poisoned")
            .shutdown();
    }
}

#[derive(Default)]
struct RuntimeState {
    backend: Option<BackendThread>,
}

impl RuntimeState {
    fn ensure_running(&mut self) -> Result<(), String> {
        self.reap_finished();

        if let Some(backend) = self.backend.as_mut() {
            return match backend.poll_startup() {
                StartupStatus::Started => Ok(()),
                StartupStatus::Pending => Err("integrated backend is still starting".into()),
                StartupStatus::Failed(error) => {
                    let mut backend = self.backend.take().expect("backend exists");
                    backend.join_if_finished();
                    Err(error)
                }
            };
        }

        let mut backend = BackendThread::spawn()?;
        match backend.wait_for_startup(STARTUP_TIMEOUT) {
            StartupStatus::Started => {
                self.backend = Some(backend);
                Ok(())
            }
            StartupStatus::Pending => {
                let error = format!(
                    "timed out after {} ms waiting for integrated backend startup",
                    STARTUP_TIMEOUT.as_millis()
                );
                self.backend = Some(backend);
                Err(error)
            }
            StartupStatus::Failed(error) => {
                backend.join_if_finished();
                Err(error)
            }
        }
    }

    fn shutdown(&mut self) {
        if let Some(mut backend) = self.backend.take() {
            backend.shutdown();
        }
    }

    fn reap_finished(&mut self) {
        let finished = self
            .backend
            .as_ref()
            .map(BackendThread::is_finished)
            .unwrap_or(false);
        if !finished {
            return;
        }

        let mut backend = self.backend.take().expect("finished backend exists");
        backend.join_if_finished();
        debug!("integrated backend thread exited");
    }
}

struct BackendThread {
    join: Option<thread::JoinHandle<()>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    startup_rx: Option<std_mpsc::Receiver<StartupResult>>,
    started: bool,
}

impl BackendThread {
    fn spawn() -> Result<Self, String> {
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (startup_tx, startup_rx) = std_mpsc::sync_channel(1);
        let join = thread::Builder::new()
            .name("spot-lyric-backend".into())
            .spawn(move || run_backend_thread(shutdown_rx, startup_tx))
            .map_err(|error| format!("spawn integrated backend thread: {error}"))?;

        Ok(Self {
            join: Some(join),
            shutdown_tx: Some(shutdown_tx),
            startup_rx: Some(startup_rx),
            started: false,
        })
    }

    fn poll_startup(&mut self) -> StartupStatus {
        if self.started {
            return StartupStatus::Started;
        }

        let Some(startup_rx) = self.startup_rx.as_ref() else {
            return StartupStatus::Pending;
        };

        match startup_rx.try_recv() {
            Ok(Ok(started)) => {
                self.started = true;
                self.startup_rx = None;
                info!(
                    sqlite_path = %started.sqlite_path.display(),
                    bus_name = backend_config::DBUS_BUS_NAME,
                    "integrated backend is ready"
                );
                StartupStatus::Started
            }
            Ok(Err(error)) => {
                self.startup_rx = None;
                StartupStatus::Failed(error)
            }
            Err(std_mpsc::TryRecvError::Empty) => StartupStatus::Pending,
            Err(std_mpsc::TryRecvError::Disconnected) => {
                self.startup_rx = None;
                StartupStatus::Failed("integrated backend thread exited before startup".into())
            }
        }
    }

    fn wait_for_startup(&mut self, timeout: Duration) -> StartupStatus {
        let deadline = Instant::now() + timeout;
        loop {
            match self.poll_startup() {
                StartupStatus::Pending if Instant::now() < deadline => {
                    thread::sleep(POLL_INTERVAL);
                }
                status => return status,
            }
        }
    }

    fn is_finished(&self) -> bool {
        self.join
            .as_ref()
            .map(thread::JoinHandle::is_finished)
            .unwrap_or(true)
    }

    fn shutdown(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        while !self.is_finished() && Instant::now() < deadline {
            thread::sleep(POLL_INTERVAL);
        }

        if self.is_finished() {
            self.join_if_finished();
        } else {
            warn!(
                timeout_ms = SHUTDOWN_TIMEOUT.as_millis(),
                "integrated backend did not stop before timeout; detaching backend thread"
            );
            let _ = self.join.take();
        }
    }

    fn join_if_finished(&mut self) {
        let Some(join) = self.join.take() else {
            return;
        };

        if !join.is_finished() {
            self.join = Some(join);
            return;
        }

        if join.join().is_err() {
            warn!("integrated backend thread panicked");
        }
    }
}

enum StartupStatus {
    Started,
    Pending,
    Failed(String),
}

struct BackendStarted {
    sqlite_path: PathBuf,
}

fn run_backend_thread(
    shutdown_rx: oneshot::Receiver<()>,
    startup_tx: std_mpsc::SyncSender<StartupResult>,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("spot-lyric-backend-tokio")
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let message = format!("create integrated backend Tokio runtime: {error}");
            let _ = startup_tx.send(Err(message));
            return;
        }
    };

    runtime.block_on(async move {
        if let Err(error) = run_backend(shutdown_rx, startup_tx).await {
            error!(%error, "integrated backend stopped with error");
        }
    });
}

async fn run_backend(
    mut shutdown_rx: oneshot::Receiver<()>,
    startup_tx: std_mpsc::SyncSender<StartupResult>,
) -> Result<(), String> {
    let state = match AppState::bootstrap(data_dir_override_from_env()).await {
        Ok(state) => state,
        Err(error) => {
            let message = format!("bootstrap integrated backend: {error}");
            let _ = startup_tx.send(Err(message.clone()));
            return Err(message);
        }
    };

    let paths = state.paths();
    info!(
        app_name = backend_config::APP_NAME,
        dbus_bus_name = backend_config::DBUS_BUS_NAME,
        dbus_object_path = backend_config::DBUS_OBJECT_PATH,
        poll_interval_ms = backend_config::POLL_INTERVAL_MS,
        sqlite_path = %paths.sqlite_path.display(),
        "starting integrated spot-lyric backend"
    );

    let _dbus_runtime = match serve_dbus(state.clone()).await {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = state.shutdown().await;
            let message = format!("register integrated backend D-Bus service: {error}");
            let _ = startup_tx.send(Err(message.clone()));
            return Err(message);
        }
    };

    if startup_tx
        .send(Ok(BackendStarted {
            sqlite_path: paths.sqlite_path.clone(),
        }))
        .is_err()
    {
        state.request_shutdown();
    }

    tokio::select! {
        _ = &mut shutdown_rx => {
            info!("shutdown requested by GTK application");
            state.request_shutdown();
        }
        _ = state.wait_for_shutdown() => {
            info!("shutdown requested by integrated backend");
        }
    }

    state
        .shutdown()
        .await
        .map_err(|error| format!("shutdown integrated backend: {error}"))?;
    info!("integrated backend shutdown complete");
    Ok(())
}

fn data_dir_override_from_env() -> Option<PathBuf> {
    data_dir_override_from_env_vars(
        env::var_os(BACKEND_DATA_DIR_ENV_VAR),
        env::var_os(LEGACY_DAEMON_DATA_DIR_ENV_VAR),
    )
}

fn data_dir_override_from_env_vars(
    backend_data_dir: Option<OsString>,
    legacy_daemon_data_dir: Option<OsString>,
) -> Option<PathBuf> {
    backend_data_dir
        .or(legacy_daemon_data_dir)
        .map(PathBuf::from)
}

#[cfg(test)]
async fn backend_has_owner() -> Result<bool, String> {
    let connection = Connection::session()
        .await
        .map_err(|error| format!("connect to session bus: {error}"))?;
    let proxy = DBusProxy::new(&connection)
        .await
        .map_err(|error| format!("create D-Bus proxy: {error}"))?;
    let name = BusName::try_from(config::DAEMON_BUS_NAME)
        .expect("configured daemon D-Bus name must be valid");
    Ok(proxy.name_has_owner(name).await.unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_override_prefers_integrated_env_name() {
        let resolved = data_dir_override_from_env_vars(
            Some(OsString::from("/tmp/integrated")),
            Some(OsString::from("/tmp/legacy")),
        );

        assert_eq!(resolved, Some(PathBuf::from("/tmp/integrated")));
    }

    #[test]
    fn data_dir_override_accepts_legacy_daemon_env_name() {
        let resolved = data_dir_override_from_env_vars(None, Some(OsString::from("/tmp/legacy")));

        assert_eq!(resolved, Some(PathBuf::from("/tmp/legacy")));
    }

    #[test]
    fn backend_runtime_serves_dbus_from_current_process() {
        if env::var_os("SPOT_LYRIC_RUN_BACKEND_RUNTIME_TEST").is_none() {
            return;
        }

        let data_dir = env::temp_dir().join(format!(
            "spot-lyric-backend-runtime-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&data_dir);
        env::set_var(BACKEND_DATA_DIR_ENV_VAR, &data_dir);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test tokio runtime");
        let backend = BackendRuntime::default();

        backend.ensure_running().expect("start integrated backend");
        assert!(
            runtime
                .block_on(backend_has_owner())
                .expect("query backend owner"),
            "integrated backend should own its D-Bus name after startup"
        );

        backend.shutdown();
        runtime.block_on(async {
            let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
            while Instant::now() < deadline {
                if !backend_has_owner().await.expect("query backend owner") {
                    env::remove_var(BACKEND_DATA_DIR_ENV_VAR);
                    let _ = std::fs::remove_dir_all(&data_dir);
                    return;
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            panic!("integrated backend still owns D-Bus name after shutdown");
        });
    }
}
