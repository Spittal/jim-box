use std::path::PathBuf;
use tauri::AppHandle;
use uuid::Uuid;

use crate::error::AppResult;
use crate::libraries::{self, LibraryChoices, Opened};

/// The libraries that can be opened, and which is open.
#[tauri::command]
pub fn library_choices(app: AppHandle) -> LibraryChoices {
    libraries::choices(&app)
}

/// Opens the library at `master_db`. When another is open the app restarts
/// into this one.
#[tauri::command]
pub async fn open_library(app: AppHandle, master_db: PathBuf, make_default: bool) -> AppResult<Opened> {
    libraries::open(&app, master_db, make_default).await
}

/// Has the open library open by default, or has the app ask at each launch.
#[tauri::command]
pub fn set_default_library(app: AppHandle, on: bool) -> AppResult<LibraryChoices> {
    libraries::set_default(&app, on)?;
    Ok(libraries::choices(&app))
}

/// Drops a library from the list, with the settings, downloads and
/// playlists saved for it.
#[tauri::command]
pub fn forget_library(app: AppHandle, id: Uuid) -> AppResult<LibraryChoices> {
    libraries::forget(&app, id)?;
    Ok(libraries::choices(&app))
}
