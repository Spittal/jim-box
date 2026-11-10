//! Runs the bundled slskd as a child process bound to localhost, authenticated
//! with a per-launch API key, and tracks its Soulseek connection status.

use serde::Serialize;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::client::SlskdClient;
use crate::error::{AppError, AppResult};
use crate::secrets;
use crate::state::SettingsState;

const STATUS_POLL_INTERVAL: Duration = Duration::from_secs(2);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum DaemonStatus {
    NeedsCredentials,
    Starting,
    /// slskd is up but not logged in to the Soulseek network yet.
    Connecting { detail: Option<String> },
    Online { username: String },
    Stopped,
    Error { message: String },
}

/// Directories slskd works in, all under the app's data dir.
pub struct DaemonPaths {
    pub app_dir: PathBuf,
    /// Finished downloads land here before being tagged and moved into the library.
    pub downloads: PathBuf,
    pub incomplete: PathBuf,
}

impl DaemonPaths {
    pub fn resolve(app: &AppHandle) -> AppResult<Self> {
        let app_dir = app.path().app_data_dir()?.join("slskd");
        Ok(Self {
            downloads: app_dir.join("downloads"),
            incomplete: app_dir.join("incomplete"),
            app_dir,
        })
    }

    fn pid_file(&self) -> PathBuf {
        self.app_dir.join("slskd.pid")
    }
}

struct Process {
    kill: oneshot::Sender<()>,
    supervisor: JoinHandle<()>,
    monitor: JoinHandle<()>,
}

pub struct Daemon {
    process: tokio::sync::Mutex<Option<Process>>,
    status: Mutex<DaemonStatus>,
    client: RwLock<Option<Arc<SlskdClient>>>,
}

impl Daemon {
    pub fn new() -> Self {
        Self {
            process: tokio::sync::Mutex::new(None),
            status: Mutex::new(DaemonStatus::Stopped),
            client: RwLock::new(None),
        }
    }

    pub fn status(&self) -> DaemonStatus {
        self.status.lock().expect("status lock poisoned").clone()
    }

    fn set_status(&self, app: &AppHandle, status: DaemonStatus) {
        let mut current = self.status.lock().expect("status lock poisoned");
        if *current != status {
            info!("slskd status: {status:?}");
            *current = status.clone();
            drop(current);
            let _ = app.emit("daemon-status", status);
        }
    }

    /// The API client, available once slskd is logged in to Soulseek.
    pub fn client(&self) -> AppResult<Arc<SlskdClient>> {
        if !matches!(self.status(), DaemonStatus::Online { .. }) {
            return Err(AppError::DaemonNotReady);
        }
        self.client
            .read()
            .expect("client lock poisoned")
            .clone()
            .ok_or(AppError::DaemonNotReady)
    }

    /// Waits for slskd to come online, e.g. for jobs queued right after launch.
    pub async fn wait_until_online(&self, timeout: Duration) -> AppResult<Arc<SlskdClient>> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.client() {
                Ok(client) => return Ok(client),
                Err(e) if Instant::now() > deadline => return Err(e),
                Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
            }
        }
    }

    /// (Re)starts slskd with the current settings and keychain credentials.
    pub async fn start(&self, app: &AppHandle) -> AppResult<()> {
        let mut process = self.process.lock().await;
        if let Some(running) = process.take() {
            self.shutdown(running).await;
        }

        match self.spawn(app).await {
            Ok(Some(running)) => {
                *process = Some(running);
                Ok(())
            }
            Ok(None) => {
                self.set_status(app, DaemonStatus::NeedsCredentials);
                Ok(())
            }
            Err(e) => {
                self.set_status(app, DaemonStatus::Error { message: e.to_string() });
                Err(e)
            }
        }
    }

    pub async fn stop(&self, app: &AppHandle) {
        if let Some(running) = self.process.lock().await.take() {
            self.shutdown(running).await;
        }
        self.set_status(app, DaemonStatus::Stopped);
    }

    async fn shutdown(&self, running: Process) {
        *self.client.write().expect("client lock poisoned") = None;
        running.monitor.abort();
        let _ = running.kill.send(());
        if tokio::time::timeout(SHUTDOWN_TIMEOUT, running.supervisor).await.is_err() {
            warn!("slskd did not shut down within {SHUTDOWN_TIMEOUT:?}");
        }
    }

    /// Returns `Ok(None)` when no Soulseek credentials are configured.
    async fn spawn(&self, app: &AppHandle) -> AppResult<Option<Process>> {
        let settings = app.state::<SettingsState>().snapshot();
        let Some(username) = settings.soulseek_username.clone() else {
            return Ok(None);
        };
        let Some(password) = secrets::get_password(&username)? else {
            return Ok(None);
        };

        let binary = binary_path(app)?;
        if !binary.is_file() {
            return Err(AppError::Slskd(format!(
                "binary not found at {} (run `pnpm fetch-slskd`)",
                binary.display()
            )));
        }

        // Share the library even before the first download creates it.
        if settings.share_music_dir {
            std::fs::create_dir_all(&settings.music_dir)?;
        }

        let paths = DaemonPaths::resolve(app)?;
        for dir in [&paths.app_dir, &paths.downloads, &paths.incomplete] {
            std::fs::create_dir_all(dir)?;
        }
        kill_stale_process(&paths.pid_file());

        let port = free_port()?;
        let api_key = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());

        let mut command = Command::new(&binary);
        command
            .arg("--app-dir")
            .arg(&paths.app_dir)
            .arg("--downloads")
            .arg(&paths.downloads)
            .arg("--incomplete")
            .arg(&paths.incomplete)
            .args(["--headless", "--no-logo", "--no-version-check", "--no-https"])
            .args(["--http-ip-address", "127.0.0.1", "--http-port", &port.to_string()])
            .arg("--api-key")
            .arg(format!("cidr=127.0.0.1/32,::1/128;{api_key}"));
        for share in settings.shares() {
            command.arg("--shared").arg(share);
        }
        let mut child = command
            // Credentials go through the environment so they never hit disk or `ps` args.
            .env("SLSKD_SLSK_USERNAME", &username)
            .env("SLSKD_SLSK_PASSWORD", &password)
            .current_dir(&paths.app_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        if let Some(pid) = child.id() {
            std::fs::write(paths.pid_file(), pid.to_string())?;
        }

        let last_error = Arc::new(Mutex::new(None));
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(forward_logs(stdout, last_error.clone()));
        }
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(forward_logs(stderr, last_error.clone()));
        }

        let client = Arc::new(SlskdClient::new(port, api_key));
        *self.client.write().expect("client lock poisoned") = Some(client.clone());
        self.set_status(app, DaemonStatus::Starting);

        let (kill, killed) = oneshot::channel();
        let supervisor = tokio::spawn(supervise(app.clone(), child, killed, paths.pid_file()));
        let monitor = tokio::spawn(monitor(app.clone(), client, username, last_error));

        Ok(Some(Process { kill, supervisor, monitor }))
    }
}

/// Owns the child process: kills it on request, or reports an unexpected exit.
async fn supervise(
    app: AppHandle,
    mut child: tokio::process::Child,
    killed: oneshot::Receiver<()>,
    pid_file: PathBuf,
) {
    tokio::select! {
        exit = child.wait() => {
            let message = match exit {
                Ok(status) => format!("slskd exited unexpectedly ({status})"),
                Err(e) => format!("slskd exited unexpectedly: {e}"),
            };
            let daemon = app.state::<Daemon>();
            *daemon.client.write().expect("client lock poisoned") = None;
            daemon.set_status(&app, DaemonStatus::Error { message });
        }
        _ = killed => {
            if let Err(e) = child.kill().await {
                warn!("Failed to kill slskd: {e}");
            }
        }
    }
    let _ = std::fs::remove_file(pid_file);
}

/// Polls slskd for its Soulseek login state.
async fn monitor(
    app: AppHandle,
    client: Arc<SlskdClient>,
    username: String,
    last_error: Arc<Mutex<Option<String>>>,
) {
    loop {
        tokio::time::sleep(STATUS_POLL_INTERVAL).await;
        // Until the API answers, slskd is still booting; keep the `Starting` status.
        let Ok(application) = client.application().await else {
            continue;
        };
        let status = if application.server.is_logged_in {
            DaemonStatus::Online { username: username.clone() }
        } else {
            let detail = last_error.lock().expect("log lock poisoned").clone();
            DaemonStatus::Connecting { detail }
        };
        app.state::<Daemon>().set_status(&app, status);
    }
}

/// Forwards slskd's console output to our logs, remembering the latest
/// connection-related problem so it can be surfaced while connecting.
async fn forward_logs(stream: impl AsyncRead + Unpin, last_error: Arc<Mutex<Option<String>>>) {
    let mut lines = BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        // slskd lines look like `[14:21:47 WRN] message`.
        let Some((_, message)) = line.split_once(" WRN] ").or_else(|| line.split_once(" ERR] ")) else {
            // Info lines include each search's response count, which the log
            // file needs to diagnose searches nobody answered.
            if line.contains(" INF] ") {
                info!(target: "slskd", "{line}");
            } else {
                debug!(target: "slskd", "{line}");
            }
            continue;
        };
        warn!(target: "slskd", "{line}");
        let lower = message.to_lowercase();
        if line.contains(" ERR] ") || lower.contains("log in") || lower.contains("login") || lower.contains("connect") {
            *last_error.lock().expect("log lock poisoned") = Some(message.to_string());
        }
    }
}

/// The bundled slskd. Dev builds run it from `resources/` instead of the copy
/// Tauri refreshes in `target/` on every build: macOS SIGKILLs a signed binary
/// that was overwritten in place after it ran, as it caches the old signature.
fn binary_path(app: &AppHandle) -> AppResult<PathBuf> {
    if cfg!(debug_assertions) {
        return Ok(Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/slskd/slskd"));
    }
    Ok(app.path().resource_dir()?.join("slskd").join("slskd"))
}

fn free_port() -> AppResult<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// Kills an slskd left behind by a previous crash, which would otherwise hold
/// the Soulseek listen port and fight us over the login session.
fn kill_stale_process(pid_file: &Path) {
    let Some(pid) = std::fs::read_to_string(pid_file)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
    else {
        return;
    };
    let is_slskd = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().ends_with("slskd"))
        .unwrap_or(false);
    if is_slskd {
        warn!("Killing stale slskd process {pid}");
        let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
        std::thread::sleep(Duration::from_millis(500));
    }
    let _ = std::fs::remove_file(pid_file);
}
