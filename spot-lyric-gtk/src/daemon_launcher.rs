use std::{
    env,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use tracing::{debug, info, warn};
use zbus::{fdo::DBusProxy, names::BusName, Connection};

use crate::config;

const DAEMON_ENV_VAR: &str = "SPOT_LYRIC_DAEMON";
const DAEMON_DATA_DIR_ENV_VAR: &str = "SPOT_LYRIC_DAEMON_DATA_DIR";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(8);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Default)]
pub struct DaemonSupervisor {
    inner: Arc<Mutex<DaemonProcess>>,
}

impl DaemonSupervisor {
    pub async fn ensure_running(&self) {
        match daemon_has_owner().await {
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => {
                warn!(%error, "cannot inspect session bus; daemon autostart disabled");
                return;
            }
        }

        let start_result = {
            let mut process = self.inner.lock().expect("daemon supervisor lock poisoned");
            process.start_if_needed()
        };

        match start_result {
            StartResult::Spawned { pid, command } => {
                info!(pid, daemon = %command, "started spot-lyric daemon");
            }
            StartResult::ChildAlreadyRunning { pid } => {
                debug!(
                    pid,
                    "waiting for previously spawned daemon to own D-Bus name"
                );
            }
            StartResult::Unavailable { reason } => {
                warn!(%reason, "unable to start spot-lyric daemon");
                return;
            }
        }

        match wait_for_daemon_owner(STARTUP_TIMEOUT).await {
            Ok(true) => info!(
                bus_name = config::DAEMON_BUS_NAME,
                "spot-lyric daemon is ready"
            ),
            Ok(false) => warn!(
                bus_name = config::DAEMON_BUS_NAME,
                timeout_ms = STARTUP_TIMEOUT.as_millis(),
                "timed out waiting for spawned daemon to own D-Bus name"
            ),
            Err(error) => warn!(%error, "failed while waiting for spawned daemon"),
        }
    }

    pub fn ensure_running_blocking(&self) {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime.block_on(self.ensure_running()),
            Err(error) => warn!(%error, "failed to create daemon startup runtime"),
        }
    }

    pub fn shutdown(&self) {
        let mut process = self.inner.lock().expect("daemon supervisor lock poisoned");
        process.shutdown();
    }
}

#[derive(Default)]
struct DaemonProcess {
    child: Option<Child>,
}

impl DaemonProcess {
    fn start_if_needed(&mut self) -> StartResult {
        self.reap_exited();

        if let Some(child) = self.child.as_ref() {
            return StartResult::ChildAlreadyRunning { pid: child.id() };
        }

        let command = DaemonCommand::resolve();
        let command_display = command.display();
        match command.spawn() {
            Ok(child) => {
                let pid = child.id();
                self.child = Some(child);
                StartResult::Spawned {
                    pid,
                    command: command_display,
                }
            }
            Err(error) => StartResult::Unavailable {
                reason: format!("{command_display}: {error}"),
            },
        }
    }

    fn shutdown(&mut self) {
        self.reap_exited();

        let Some(mut child) = self.child.take() else {
            return;
        };

        let pid = child.id();
        info!(pid, "stopping spot-lyric daemon started by frontend");

        if let Err(error) = terminate_child(&mut child) {
            warn!(pid, %error, "failed to send graceful shutdown signal to daemon");
        }

        if wait_for_child(&mut child, SHUTDOWN_TIMEOUT) {
            return;
        }

        warn!(
            pid,
            timeout_ms = SHUTDOWN_TIMEOUT.as_millis(),
            "daemon did not exit after graceful shutdown signal; killing"
        );
        if let Err(error) = child.kill() {
            warn!(pid, %error, "failed to kill daemon process");
        }
        let _ = child.wait();
    }

    fn reap_exited(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };

        match child.try_wait() {
            Ok(Some(status)) => {
                warn!(pid = child.id(), %status, "spawned spot-lyric daemon exited");
                self.child = None;
            }
            Ok(None) => {}
            Err(error) => {
                warn!(pid = child.id(), %error, "failed to inspect spawned daemon status");
                self.child = None;
            }
        }
    }
}

enum StartResult {
    Spawned { pid: u32, command: String },
    ChildAlreadyRunning { pid: u32 },
    Unavailable { reason: String },
}

struct DaemonCommand {
    program: PathBuf,
    source: &'static str,
}

impl DaemonCommand {
    fn resolve() -> Self {
        if let Some(program) = env::var_os(DAEMON_ENV_VAR) {
            return Self {
                program: PathBuf::from(program),
                source: DAEMON_ENV_VAR,
            };
        }

        for program in candidate_paths() {
            if program.is_file() {
                return Self {
                    program,
                    source: "auto-detected",
                };
            }
        }

        Self {
            program: PathBuf::from("spot-lyric-daemon"),
            source: "PATH",
        }
    }

    fn spawn(&self) -> std::io::Result<Child> {
        let mut command = Command::new(&self.program);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        if let Some(data_dir) = env::var_os(DAEMON_DATA_DIR_ENV_VAR) {
            command.arg("--data-dir").arg(data_dir);
        }

        command.spawn()
    }

    fn display(&self) -> String {
        format!("{} ({})", self.program.display(), self.source)
    }
}

fn candidate_paths() -> Vec<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let current_exe_dir = env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));

    candidate_paths_for(&manifest_dir, current_exe_dir.as_deref())
}

fn candidate_paths_for(manifest_dir: &Path, current_exe_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(dir) = current_exe_dir {
        paths.push(dir.join("spot-lyric-daemon"));
    }

    if let Some(repo_root) = manifest_dir.parent() {
        let daemon_dir = repo_root.join("spot-lyric-daemon");
        paths.push(daemon_dir.join("target/debug/spot-lyric-daemon"));
        paths.push(daemon_dir.join("target/release/spot-lyric-daemon"));
    }

    paths
}

async fn daemon_has_owner() -> Result<bool, String> {
    let connection = Connection::session()
        .await
        .map_err(|error| format!("connect to session bus: {error}"))?;
    let proxy = DBusProxy::new(&connection)
        .await
        .map_err(|error| format!("create D-Bus proxy: {error}"))?;

    Ok(proxy_has_daemon_owner(&proxy).await)
}

async fn wait_for_daemon_owner(timeout: Duration) -> Result<bool, String> {
    let connection = Connection::session()
        .await
        .map_err(|error| format!("connect to session bus: {error}"))?;
    let proxy = DBusProxy::new(&connection)
        .await
        .map_err(|error| format!("create D-Bus proxy: {error}"))?;
    let deadline = Instant::now() + timeout;

    loop {
        if proxy_has_daemon_owner(&proxy).await {
            return Ok(true);
        }

        if Instant::now() >= deadline {
            return Ok(false);
        }

        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn proxy_has_daemon_owner(proxy: &DBusProxy<'_>) -> bool {
    let name = BusName::try_from(config::DAEMON_BUS_NAME)
        .expect("configured daemon D-Bus name must be valid");
    proxy.name_has_owner(name).await.unwrap_or(false)
}

fn wait_for_child(child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                debug!(pid = child.id(), %status, "daemon process exited");
                return true;
            }
            Ok(None) => {}
            Err(error) => {
                warn!(pid = child.id(), %error, "failed to wait for daemon process");
                return true;
            }
        }

        if Instant::now() >= deadline {
            return false;
        }

        thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(unix)]
fn terminate_child(child: &mut Child) -> std::io::Result<()> {
    let result = unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn terminate_child(child: &mut Child) -> std::io::Result<()> {
    child.kill()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_paths_include_installed_and_sibling_daemon_builds() {
        let manifest_dir = Path::new("/repo/spot-lyric-gtk");
        let exe_dir = Path::new("/install/bin");

        let paths = candidate_paths_for(manifest_dir, Some(exe_dir));

        assert_eq!(paths[0], PathBuf::from("/install/bin/spot-lyric-daemon"));
        assert!(paths.contains(&PathBuf::from(
            "/repo/spot-lyric-daemon/target/debug/spot-lyric-daemon"
        )));
        assert!(paths.contains(&PathBuf::from(
            "/repo/spot-lyric-daemon/target/release/spot-lyric-daemon"
        )));
    }

    #[test]
    fn supervisor_starts_daemon_under_session_bus() {
        if env::var_os("SPOT_LYRIC_RUN_DAEMON_LAUNCH_TEST").is_none() {
            return;
        }

        let data_dir =
            env::temp_dir().join(format!("spot-lyric-gtk-launch-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data_dir);
        env::set_var(DAEMON_DATA_DIR_ENV_VAR, &data_dir);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test tokio runtime");
        let supervisor = DaemonSupervisor::default();

        runtime.block_on(supervisor.ensure_running());
        assert!(
            runtime
                .block_on(daemon_has_owner())
                .expect("query daemon owner"),
            "daemon should own its D-Bus name after supervisor startup"
        );

        supervisor.shutdown();
        runtime.block_on(async {
            let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
            while Instant::now() < deadline {
                if !daemon_has_owner().await.expect("query daemon owner") {
                    let _ = std::fs::remove_dir_all(&data_dir);
                    return;
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            panic!("daemon still owns D-Bus name after supervisor shutdown");
        });
    }
}
