use tauri::{AppHandle, State};

use crate::error::AppResult;
use crate::slskd::{Daemon, DaemonStatus};

#[tauri::command]
pub fn get_daemon_status(daemon: State<'_, Daemon>) -> DaemonStatus {
    daemon.status()
}

#[tauri::command]
pub async fn restart_daemon(app: AppHandle, daemon: State<'_, Daemon>) -> AppResult<()> {
    daemon.start(&app).await
}
