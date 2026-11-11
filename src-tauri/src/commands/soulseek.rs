use tauri::{AppHandle, State};

use crate::error::AppResult;
use crate::soulseek::{Soulseek, SoulseekStatus};

#[tauri::command]
pub fn get_soulseek_status(soulseek: State<'_, Soulseek>) -> SoulseekStatus {
    soulseek.status()
}

#[tauri::command]
pub async fn reconnect_soulseek(app: AppHandle, soulseek: State<'_, Soulseek>) -> AppResult<()> {
    soulseek.start(&app).await
}
