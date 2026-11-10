use tauri::State;

use crate::autotag;
use crate::error::{AppError, AppResult};
use crate::sources::Sources;
use crate::state::TrackRequest;

/// Deezer's results for the query, then MusicBrainz versions Deezer lacks.
#[tauri::command]
pub async fn search_tracks(sources: State<'_, Sources>, query: String) -> AppResult<Vec<TrackRequest>> {
    let query = query.trim();
    if query.is_empty() {
        return Err(AppError::Validation("Type an artist and track name".into()));
    }
    autotag::search(&sources, query).await
}

/// A fresh link to a Deezer track's 30-second clip, for when the one that
/// came with the search results has expired. `None` if it has no clip.
#[tauri::command]
pub async fn track_preview(sources: State<'_, Sources>, deezer_id: u64) -> AppResult<Option<String>> {
    Ok(sources.deezer.track(deezer_id).await?.preview_url)
}
