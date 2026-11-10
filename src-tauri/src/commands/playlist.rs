use tauri::{AppHandle, State};

use crate::error::AppResult;
use crate::playlist::{self, LinkedPlaylist, PlaylistStore};
use crate::state::TrackRequest;

/// Brings a public Spotify playlist into rekordbox as a playlist, matching
/// any tracks not matched before. Refused while rekordbox is open.
#[tauri::command]
pub async fn import_spotify_playlist(app: AppHandle, url: String) -> AppResult<LinkedPlaylist> {
    playlist::import(&app, &url).await
}

/// The Spotify playlist a rekordbox playlist came from, if it did, with the
/// tracks the collection still lacks.
#[tauri::command]
pub async fn linked_playlist(app: AppHandle, playlist_id: String) -> AppResult<Option<LinkedPlaylist>> {
    playlist::linked(&app, playlist_id).await
}

/// Records the match the user picked for a Spotify playlist's track.
#[tauri::command]
pub fn set_playlist_match(
    app: AppHandle,
    store: State<'_, PlaylistStore>,
    id: String,
    index: usize,
    request: TrackRequest,
) -> AppResult<()> {
    store.set_match(&app, &id, index, request)
}
