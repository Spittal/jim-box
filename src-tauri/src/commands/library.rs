use std::path::PathBuf;
use tauri::ipc::Response;
use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::error::AppResult;
use crate::import::{self, LibrarySnapshot, LibraryStore, RelocateReport};
use crate::state::Job;

/// The tracks being tidied and what identifying them found.
#[tauri::command]
pub fn get_library(store: State<'_, LibraryStore>) -> LibrarySnapshot {
    store.snapshot()
}

/// Looks collection tracks up and proposes tags and a path for each.
#[tauri::command]
pub fn identify_tracks(app: AppHandle, track_ids: Vec<String>) -> AppResult<()> {
    import::identify(&app, track_ids)
}

#[tauri::command]
pub fn apply_library_matches(app: AppHandle, ids: Vec<Uuid>) -> AppResult<()> {
    import::apply(&app, ids)
}

#[tauri::command]
pub fn cancel_library_task(app: AppHandle, store: State<'_, LibraryStore>) {
    store.cancel(&app);
}

/// Forgets what identifying found for these items.
#[tauri::command]
pub fn dismiss_library_items(app: AppHandle, store: State<'_, LibraryStore>, ids: Vec<Uuid>) {
    store.dismiss(&app, &ids);
}

#[tauri::command]
pub async fn change_music_dir(app: AppHandle, dir: PathBuf) -> AppResult<RelocateReport> {
    import::change_music_dir(&app, dir).await
}

/// A file's artwork as raw image bytes; empty when it has none.
#[tauri::command]
pub async fn library_cover_art(app: AppHandle, path: PathBuf) -> AppResult<Response> {
    Ok(Response::new(import::cover_art(&app, path).await?))
}

/// Downloads a collection track again; the track moves to the new file once
/// the download succeeds.
#[tauri::command]
pub async fn redownload_track(app: AppHandle, track_id: String) -> AppResult<Job> {
    import::redownload(&app, track_id).await
}
