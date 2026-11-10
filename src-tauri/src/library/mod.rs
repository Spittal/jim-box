//! The music folder on disk: finding and moving files, rendering the path
//! template, and reading and writing tags. Synchronous; see [`blocking`].

mod files;
mod organizer;
mod tagger;

pub use files::{extension, AUDIO_EXTENSIONS, move_file, remove_empty_dirs, remove_if_empty, walk_files};
pub use organizer::{convert_legacy_template, library_path, render, validate_template};
pub use tagger::{read_cover, read_file, write_tags, TagMode};

use crate::error::{AppError, AppResult};

/// Runs filesystem or tagging work off the async runtime.
pub async fn blocking<T: Send + 'static>(work: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| AppError::Io(std::io::Error::other(e)))?
}
