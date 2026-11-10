use std::path::PathBuf;
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::analysis::{self, AnalysisProgress, AnalysisQueue};
use crate::error::{AppError, AppResult};
use crate::ingest::{self, ArrivalQueue, Waiting};
use crate::library;
use crate::state::SettingsState;
use crate::rekordbox::{
    Collection, CollectionStatus, Edit, EditOutcome, ImportReport, PlaylistNode, Preparation, TrackPage,
    TrackQuery,
};

/// The collection as last read, and the files waiting to join it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionState {
    pub status: CollectionStatus,
    pub playlists: Vec<PlaylistNode>,
    pub waiting: Waiting,
}

fn state(app: &AppHandle) -> AppResult<CollectionState> {
    let collection = app.state::<Collection>();
    Ok(CollectionState {
        status: collection.status()?,
        playlists: collection.playlists()?,
        waiting: app.state::<ArrivalQueue>().waiting(),
    })
}

/// Reads rekordbox's library from disk, picking up changes made in
/// rekordbox, and adds any files that were waiting for it to close.
#[tauri::command]
pub async fn open_collection(app: AppHandle) -> AppResult<CollectionState> {
    let reader = app.clone();
    library::blocking(move || reader.state::<Collection>().reload()).await?;
    ingest::flush(&app).await;
    state(&app)
}

/// The collection as last read, without reading it again.
#[tauri::command]
pub fn collection_state(app: AppHandle) -> AppResult<CollectionState> {
    state(&app)
}

/// A page of a track list, sorted and searched in the backend.
#[tauri::command]
pub fn collection_tracks(
    collection: tauri::State<'_, Collection>,
    query: TrackQuery,
    offset: usize,
    limit: usize,
) -> AppResult<TrackPage> {
    collection.tracks(&query, offset, limit.min(1000))
}

/// The id of every track in a list, for selecting them all.
#[tauri::command]
pub fn collection_track_ids(collection: tauri::State<'_, Collection>, query: TrackQuery) -> AppResult<Vec<String>> {
    collection.track_ids(&query)
}

/// A track to look for in the collection by name.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TrackName {
    pub title: String,
    pub artist: String,
}

/// For each track, the id of the collection track with that title and
/// artist (compared loosely), if there is one.
#[tauri::command]
pub async fn find_in_collection(app: AppHandle, tracks: Vec<TrackName>) -> AppResult<Vec<Option<String>>> {
    library::blocking(move || {
        let wanted: Vec<(&str, &str)> = tracks.iter().map(|t| (t.title.as_str(), t.artist.as_str())).collect();
        Ok(app.state::<Collection>().find_tracks(&wanted))
    })
    .await
}

/// Makes one change to rekordbox's library. Refused while rekordbox is open.
#[tauri::command]
pub async fn edit_collection(app: AppHandle, edit: Edit) -> AppResult<EditOutcome> {
    library::blocking(move || app.state::<Collection>().edit(edit)).await
}

/// Adds files, and the music in folders, to the collection; and to a
/// playlist, if one is given.
#[tauri::command]
pub async fn import_to_collection(
    app: AppHandle,
    paths: Vec<PathBuf>,
    playlist_id: Option<String>,
) -> AppResult<ImportReport> {
    let importer = app.clone();
    let report =
        library::blocking(move || importer.state::<Collection>().import(&paths, playlist_id.as_deref())).await?;
    if app.state::<SettingsState>().snapshot().analyse_new_tracks {
        analysis::enqueue(&app, report.track_ids.clone(), false).await;
    }
    Ok(report)
}

/// Queues tracks for analysis: BPM, beat grid, key and waveforms. Tracks
/// that have an analysis are skipped unless `again`. With no ids, queues
/// every track in the collection that has none. Returns how many were queued.
#[tauri::command]
pub async fn analyse_tracks(app: AppHandle, track_ids: Option<Vec<String>>, again: bool) -> AppResult<usize> {
    if rbl_db::is_rekordbox_running() {
        return Err(AppError::Rekordbox("rekordbox is running. Quit it before analysing.".into()));
    }
    let Some(track_ids) = track_ids else {
        let reader = app.clone();
        let all = library::blocking(move || Ok(reader.state::<Collection>().all_unanalysed())).await?;
        return Ok(analysis::enqueue(&app, all, true).await);
    };
    Ok(analysis::enqueue(&app, track_ids, again).await)
}

/// Drops the tracks still waiting to be analysed.
#[tauri::command]
pub fn cancel_analysis(app: AppHandle) {
    analysis::cancel(&app);
}

#[tauri::command]
pub fn analysis_progress(queue: tauri::State<'_, AnalysisQueue>) -> AnalysisProgress {
    queue.progress()
}

/// A track's beat grid and cues, for the player.
#[tauri::command]
pub async fn track_preparation(app: AppHandle, track_id: String) -> AppResult<Preparation> {
    library::blocking(move || app.state::<Collection>().preparation(&track_id)).await
}

/// A track's detailed waveform as bytes; see `Collection::waveform`.
#[tauri::command]
pub async fn track_waveform(app: AppHandle, track_id: String) -> AppResult<tauri::ipc::Response> {
    let bytes = library::blocking(move || app.state::<Collection>().waveform(&track_id)).await?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// Where the player's audio is kept: one decoded track at a time.
pub fn preview_dir(app: &AppHandle) -> AppResult<PathBuf> {
    let cache = app.path().app_cache_dir().map_err(|e| AppError::Io(std::io::Error::other(e)))?;
    Ok(cache.join("preview"))
}

/// Decodes a track for the player and returns the file to play. The file is
/// on rekordbox's timeline, so cues and beats land where rekordbox has them.
#[tauri::command]
pub async fn track_preview_audio(app: AppHandle, track_id: String) -> AppResult<PathBuf> {
    let dir = preview_dir(&app)?;
    library::blocking(move || {
        // The last track's audio goes: these are tens of megabytes each.
        let _ = std::fs::remove_dir_all(&dir);
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_millis());
        let out = dir.join(format!("{track_id}-{unique}.wav"));
        app.state::<Collection>().write_preview(&track_id, &out)?;
        Ok(out)
    })
    .await
}
