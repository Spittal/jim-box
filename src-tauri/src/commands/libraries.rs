use std::path::PathBuf;
use tauri::AppHandle;
use uuid::Uuid;

use crate::error::AppResult;
use crate::libraries::{self, LibraryChoices, Opened};
use crate::library;

/// The libraries that can be opened, and which is open.
///
/// Off the main thread, as the two below are: it looks on every drive, and
/// one that is asleep or gone takes its time to say so.
#[tauri::command]
pub async fn library_choices(app: AppHandle) -> AppResult<LibraryChoices> {
    library::blocking(move || Ok(libraries::choices(&app))).await
}

/// Opens the library at `master_db`. When another is open the app restarts
/// into this one.
#[tauri::command]
pub async fn open_library(app: AppHandle, master_db: PathBuf, make_default: bool) -> AppResult<Opened> {
    libraries::open(&app, master_db, make_default).await
}

/// Has the open library open by default, or has the app ask at each launch.
#[tauri::command]
pub async fn set_default_library(app: AppHandle, on: bool) -> AppResult<LibraryChoices> {
    library::blocking(move || {
        libraries::set_default(&app, on)?;
        Ok(libraries::choices(&app))
    })
    .await
}

/// Drops a library from the list, with the settings, downloads and
/// playlists saved for it.
#[tauri::command]
pub async fn forget_library(app: AppHandle, id: Uuid) -> AppResult<LibraryChoices> {
    library::blocking(move || {
        libraries::forget(&app, id)?;
        Ok(libraries::choices(&app))
    })
    .await
}
