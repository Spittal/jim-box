use tauri::{AppHandle, State};

use crate::error::AppResult;
use crate::playlist::{self, PlaylistMatch, PlaylistStore, PlaylistSummary, PlaylistSync};
use crate::state::TrackRequest;

/// Reads a playlist from Spotify and matches any tracks not matched before.
#[tauri::command]
pub async fn load_spotify_playlist(app: AppHandle, url: String) -> AppResult<PlaylistMatch> {
    playlist::load(&app, &url).await
}

#[tauri::command]
pub fn list_playlists(store: State<'_, PlaylistStore>) -> Vec<PlaylistSummary> {
    store.list()
}

/// A cached playlist as last loaded, without contacting Spotify.
#[tauri::command]
pub fn get_playlist(store: State<'_, PlaylistStore>, id: String) -> AppResult<PlaylistMatch> {
    store.get(&id)
}

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

#[tauri::command]
pub fn remove_playlist(app: AppHandle, store: State<'_, PlaylistStore>, id: String) {
    store.remove(&app, &id);
}

/// Copies a playlist into rekordbox, as far as its tracks are in the
/// collection. Refused while rekordbox is open.
#[tauri::command]
pub async fn sync_playlist_to_rekordbox(app: AppHandle, id: String) -> AppResult<PlaylistSync> {
    playlist::sync_to_rekordbox(&app, &id).await
}
