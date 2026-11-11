use tauri::{AppHandle, State};

use crate::error::AppResult;
use std::collections::HashMap;

use crate::playlist::{self, Followed, LinkedPlaylist, PlaylistStore, SyncSettings};
use crate::state::TrackRequest;

/// Makes a rekordbox playlist follow a public Spotify playlist, and brings
/// it in step. For one that follows it already, reads Spotify again.
#[tauri::command]
pub async fn link_spotify_playlist(app: AppHandle, url: String, playlist_id: String) -> AppResult<LinkedPlaylist> {
    playlist::link(&app, &url, playlist_id).await
}

/// The rekordbox playlists that follow a Spotify playlist, by id.
#[tauri::command]
pub fn followed_playlists(store: State<'_, PlaylistStore>) -> HashMap<String, Followed> {
    store.followed()
}

/// Changes how a rekordbox playlist follows its Spotify playlist, and brings
/// it in step under the new settings.
#[tauri::command]
pub fn set_playlist_settings(
    app: AppHandle,
    store: State<'_, PlaylistStore>,
    playlist_id: String,
    settings: SyncSettings,
) -> AppResult<()> {
    store.set_settings(&app, &playlist_id, settings)?;
    tauri::async_runtime::spawn(playlist::sync_linked(app.clone(), playlist_id));
    Ok(())
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
