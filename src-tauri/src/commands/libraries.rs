use tauri::AppHandle;

use crate::libraries::{self, LibraryStatus};

/// Whether the rekordbox library is open, and where it is.
#[tauri::command]
pub fn library_status(app: AppHandle) -> LibraryStatus {
    libraries::status(&app)
}
