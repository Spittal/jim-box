use tauri::{AppHandle, State};

use crate::error::AppResult;
use crate::playlist::{self, LinkedPlaylist, PlaylistStore};
use crate::state::TrackRequest;

/// Links a public Spotify playlist to a rekordbox playlist, made under
/// `parent_id` if this one isn't linked yet (rekordbox must be closed for
/// that), and brings the two in step.
#[tauri::command]
pub async fn import_spotify_playlist(app: AppHandle, url: String, parent_id: Option<String>) -> AppResult<LinkedPlaylist> {
    playlist::import(&app, &url, parent_id).await
}

/// The rekordbox playlists that follow a Spotify playlist.
#[tauri::command]
pub fn linked_playlist_ids(store: State<'_, PlaylistStore>) -> Vec<String> {
    store.linked_ids()
}

/// Stops keeping a rekordbox playlist in step with Spotify. The playlist stays.
#[tauri::command]
pub fn unlink_playlist(app: AppHandle, store: State<'_, PlaylistStore>, playlist_id: String) {
    store.unlink(&app, &playlist_id);
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
