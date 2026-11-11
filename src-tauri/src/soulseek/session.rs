//! The app's connection to the Soulseek network: logs in with the saved
//! account, shares the library, reconnects when the connection drops, and
//! tracks the status the window shows.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use soulseek_rs::{CancelHandle, Client, ClientSettings, SessionLoss, SoulseekRs};
use tauri::{AppHandle, Emitter, Manager};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use super::client::{blocking, share_names, SoulseekClient};
use crate::error::{AppError, AppResult};
use crate::secrets;
use crate::state::SettingsState;

/// The port peers reach us on. It's the one slskd listens on by default, so
/// a port forwarded for an earlier build still works.
const LISTEN_PORT: u16 = 50300;
const SESSION_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Pauses before connecting again after each failure in a row; the last
/// repeats.
const RECONNECT_DELAYS: [Duration; 4] = [
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(60),
    Duration::from_secs(5 * 60),
];

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum SoulseekStatus {
    NeedsCredentials,
    /// Not logged in yet; `detail` is why the last try failed.
    Connecting { detail: Option<String> },
    Online { username: String },
    Stopped,
    Error { message: String },
}

pub struct Soulseek {
    /// The task keeping the connection up, when there is one.
    supervisor: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    status: Mutex<SoulseekStatus>,
    client: RwLock<Option<Arc<SoulseekClient>>>,
}

impl Soulseek {
    pub fn new() -> Self {
        Self {
            supervisor: tokio::sync::Mutex::new(None),
            status: Mutex::new(SoulseekStatus::Stopped),
            client: RwLock::new(None),
        }
    }

    pub fn status(&self) -> SoulseekStatus {
        self.status.lock().expect("status lock poisoned").clone()
    }

    fn set_status(&self, app: &AppHandle, status: SoulseekStatus) {
        let mut current = self.status.lock().expect("status lock poisoned");
        if *current != status {
            info!("Soulseek status: {status:?}");
            *current = status.clone();
            drop(current);
            let _ = app.emit("soulseek-status", status);
        }
    }

    /// The client, available once logged in to Soulseek.
    pub fn client(&self) -> AppResult<Arc<SoulseekClient>> {
        match self.status() {
            SoulseekStatus::Online { .. } => {}
            SoulseekStatus::Error { message } => return Err(AppError::SoulseekFailed(message)),
            _ => return Err(AppError::SoulseekNotReady),
        }
        self.client
            .read()
            .expect("client lock poisoned")
            .clone()
            .ok_or(AppError::SoulseekNotReady)
    }

    /// Waits for Soulseek to come online, e.g. for jobs queued right after launch.
    pub async fn wait_until_online(&self, timeout: Duration) -> AppResult<Arc<SoulseekClient>> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.client() {
                Ok(client) => return Ok(client),
                // It won't come online until whatever stopped it is put right.
                Err(e @ AppError::SoulseekFailed(_)) => return Err(e),
                Err(e) if Instant::now() > deadline => return Err(e),
                Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
            }
        }
    }

    /// (Re)connects with the current settings and keychain credentials.
    pub async fn start(&self, app: &AppHandle) -> AppResult<()> {
        let mut supervisor = self.supervisor.lock().await;
        if let Some(running) = supervisor.take() {
            running.abort();
        }
        self.drop_client();
        remove_slskd_data(app);

        match self.account(app) {
            Ok(Some((username, password))) => {
                self.set_status(app, SoulseekStatus::Connecting { detail: None });
                *supervisor = Some(tokio::spawn(supervise(app.clone(), username, password)));
                Ok(())
            }
            Ok(None) => {
                self.set_status(app, SoulseekStatus::NeedsCredentials);
                Ok(())
            }
            Err(e) => {
                self.set_status(app, SoulseekStatus::Error { message: e.to_string() });
                Err(e)
            }
        }
    }

    pub async fn stop(&self, app: &AppHandle) {
        if let Some(running) = self.supervisor.lock().await.take() {
            running.abort();
        }
        self.drop_client();
        self.set_status(app, SoulseekStatus::Stopped);
    }

    /// Shares the folders the settings now name. Reconnects when offline, as
    /// a folder that couldn't be reached may be what kept it so.
    pub async fn refresh_shares(&self, app: &AppHandle) -> AppResult<()> {
        let Ok(client) = self.client() else {
            return self.start(app).await;
        };
        let settings = app.state::<SettingsState>().snapshot();
        ensure_library_folder(&settings.music_dir)?;
        client.set_shares(settings.shares()).await
    }

    /// The saved account, or `None` when there isn't one. Fails when the
    /// library folder can't be reached: nothing fetched would have anywhere
    /// to go.
    fn account(&self, app: &AppHandle) -> AppResult<Option<(String, String)>> {
        let settings = app.state::<SettingsState>().snapshot();
        let Some(username) = settings.soulseek_username.clone() else {
            return Ok(None);
        };
        let Some(password) = secrets::get_password(&username)? else {
            return Ok(None);
        };
        ensure_library_folder(&settings.music_dir)?;
        Ok(Some((username, password)))
    }

    fn drop_client(&self) {
        if let Some(client) = self.client.write().expect("client lock poisoned").take() {
            client.close();
        }
    }
}

/// Where downloads are kept until they're tagged and moved into the library.
pub fn staging_dir(app: &AppHandle) -> AppResult<PathBuf> {
    Ok(app.path().app_data_dir()?.join("soulseek").join("downloads"))
}

/// Deletes what slskd, which earlier builds ran for Soulseek, left in the
/// app's data folder.
fn remove_slskd_data(app: &AppHandle) {
    if let Ok(dir) = app.path().app_data_dir().map(|dir| dir.join("slskd")) {
        if dir.is_dir() {
            tokio::task::spawn_blocking(move || std::fs::remove_dir_all(dir));
        }
    }
}

/// Makes sure the folder new tracks are filed in is there, creating it so it
/// can be shared before the first download. It can't be when it's on a drive
/// that isn't connected.
fn ensure_library_folder(dir: &Path) -> AppResult<()> {
    std::fs::create_dir_all(dir).map_err(|e| {
        warn!("Library folder {} is unavailable: {e}", dir.display());
        AppError::LibraryFolderUnavailable(dir.to_path_buf())
    })
}

/// Keeps the connection up: logs in, watches the session, and connects again
/// when it drops. Ends when the account is refused or used elsewhere.
async fn supervise(app: AppHandle, username: String, password: String) {
    let soulseek = app.state::<Soulseek>();
    let mut failures = 0;
    loop {
        // Read each time: the shared folders may have changed since.
        let shares = app.state::<SettingsState>().snapshot().shares();
        match connect(&username, &password, shares).await {
            Ok(client) => {
                failures = 0;
                let client = Arc::new(client);
                *soulseek.client.write().expect("client lock poisoned") = Some(client.clone());
                soulseek.set_status(&app, SoulseekStatus::Online { username: username.clone() });

                let loss = loop {
                    tokio::time::sleep(SESSION_POLL_INTERVAL).await;
                    if let Some(loss) = client.session_loss() {
                        break loss;
                    }
                };
                warn!("Soulseek session ended: {loss}");
                soulseek.drop_client();
                match loss {
                    // Logging in again would only throw the other session
                    // out in turn.
                    SessionLoss::Displaced => {
                        let message = "This account signed in to Soulseek somewhere else. Close the other \
                                       client, then press Reconnect."
                            .to_string();
                        soulseek.set_status(&app, SoulseekStatus::Error { message });
                        return;
                    }
                    SessionLoss::Disconnected => {
                        let detail = Some("the connection dropped".to_string());
                        soulseek.set_status(&app, SoulseekStatus::Connecting { detail });
                    }
                }
            }
            Err(SoulseekRs::AuthenticationFailed) => {
                let message = "Soulseek didn't accept that username and password.".to_string();
                soulseek.set_status(&app, SoulseekStatus::Error { message });
                return;
            }
            Err(e) => {
                warn!("Couldn't connect to Soulseek: {e}");
                failures += 1;
                soulseek.set_status(&app, SoulseekStatus::Connecting { detail: Some(e.to_string()) });
            }
        }
        let delay = RECONNECT_DELAYS[failures.min(RECONNECT_DELAYS.len() - 1)];
        tokio::time::sleep(delay).await;
    }
}

/// Connects and logs in, scanning the shared folders on the way.
async fn connect(username: &str, password: &str, shares: Vec<PathBuf>) -> Result<SoulseekClient, SoulseekRs> {
    let mut client = Client::with_settings(ClientSettings {
        listen_port: LISTEN_PORT,
        shared_directories: share_names(&shares),
        ..ClientSettings::new(username, password)
    });
    // Connecting can't be interrupted from here, so if this task is stopped
    // midway, tell the client to give up rather than log in behind the back
    // of whatever replaced it.
    let mut abandon = Abandon(Some(client.cancel_handle()));
    let client = blocking(move || {
        client.connect()?;
        match client.login()? {
            true => Ok(client),
            false => Err(SoulseekRs::AuthenticationFailed),
        }
    })
    .await
    .map_err(|_| SoulseekRs::NotConnected)??;
    abandon.0 = None;
    Ok(SoulseekClient::new(client))
}

/// Cancels a connection attempt when dropped before it's disarmed.
struct Abandon(Option<CancelHandle>);

impl Drop for Abandon {
    fn drop(&mut self) {
        if let Some(cancel) = &self.0 {
            cancel.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreachable_library_folder_says_what_to_do() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("new/library");
        ensure_library_folder(&fresh).unwrap();
        assert!(fresh.is_dir());

        // Nothing can be created under a file, as under an unmounted drive.
        let file = dir.path().join("file");
        std::fs::write(&file, "").unwrap();
        let unreachable = file.join("Music");
        let error = ensure_library_folder(&unreachable).unwrap_err().to_string();
        assert!(error.contains(&unreachable.display().to_string()), "{error}");
        assert!(error.contains("connect it"), "{error}");
    }
}
