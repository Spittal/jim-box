//! The libraries opened beside the master: see `others`.

use std::path::PathBuf;

use tauri::AppHandle;

use crate::error::AppResult;
use crate::library;
use crate::others::{self, Applied, Moved, OtherInfo, Plugged};

/// The libraries open beside the master.
#[tauri::command]
pub async fn other_libraries(app: AppHandle) -> AppResult<Vec<OtherInfo>> {
    library::blocking(move || Ok(others::infos(&app))).await
}

/// Opens the rekordbox libraries in a folder: a USB stick, or a `master.db`.
#[tauri::command]
pub async fn open_other_library(app: AppHandle, path: PathBuf) -> AppResult<Vec<OtherInfo>> {
    library::blocking(move || others::open(&app, &path)).await
}

#[tauri::command]
pub async fn close_other_library(app: AppHandle, id: String) -> AppResult<()> {
    library::blocking(move || others::close(&app, &id)).await
}

/// Reads a library again.
#[tauri::command]
pub async fn reload_other_library(app: AppHandle, id: String) -> AppResult<OtherInfo> {
    library::blocking(move || others::reload(&app, &id)).await
}

/// Drives plugged in with a library on them that isn't open.
#[tauri::command]
pub async fn plugged_libraries(app: AppHandle) -> AppResult<Vec<Plugged>> {
    library::blocking(move || Ok(others::plugged(&app))).await
}

/// Tracks from one library into another (`to`, None for the master), into
/// one of its playlists or just its collection.
#[tauri::command]
pub async fn move_tracks(app: AppHandle, track_ids: Vec<String>, to: Option<String>, playlist_id: Option<String>) -> AppResult<Moved> {
    library::blocking(move || others::move_tracks(&app, &track_ids, to.as_deref(), playlist_id.as_deref())).await
}

/// Playlists and folders from one library into another, under one of its
/// folders or at its top level.
#[tauri::command]
pub async fn move_playlists(app: AppHandle, playlist_ids: Vec<String>, to: Option<String>, under: Option<String>) -> AppResult<Moved> {
    library::blocking(move || others::move_playlists(&app, &playlist_ids, to.as_deref(), under.as_deref())).await
}

/// Everything of another library into the master.
#[tauri::command]
pub async fn import_other_library(app: AppHandle, id: String) -> AppResult<Moved> {
    library::blocking(move || others::import_all(&app, &id)).await
}

/// Lines up a stick's playlists from the master to be written again.
#[tauri::command]
pub async fn update_stick(app: AppHandle, id: String) -> AppResult<OtherInfo> {
    library::blocking(move || others::stage_update(&app, &id)).await
}

/// Writes a stick's changes waiting, and ejects it if asked.
#[tauri::command]
pub async fn apply_stick_changes(
    app: AppHandle,
    id: String,
    eject: bool,
    convert: Option<rbl_export::CompatibilityFormat>,
) -> AppResult<Applied> {
    library::blocking(move || others::apply(&app, &id, eject, convert)).await
}

/// Forgets a stick's changes waiting.
#[tauri::command]
pub async fn discard_stick_changes(app: AppHandle, id: String) -> AppResult<OtherInfo> {
    library::blocking(move || others::discard(&app, &id)).await
}
