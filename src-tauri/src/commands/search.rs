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
