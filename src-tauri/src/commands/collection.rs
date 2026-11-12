use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use serde::Serialize;
use tauri::ipc::{Channel, Response};
use tauri::{AppHandle, Manager};

use crate::analysis::{self, AnalysisProgress, AnalysisQueue};
use crate::error::{AppError, AppResult};
use crate::ingest::{self, ArrivalQueue, Waiting};
use crate::library;
use crate::state::SettingsState;
use crate::rekordbox::{GridHint, 
    Audio, Collection, CollectionStatus, Edit, EditOutcome, ImportReport, PlaylistNode, Preparation, Preview, TrackPage,
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
pub async fn collection_state(app: AppHandle) -> AppResult<CollectionState> {
    library::blocking(move || state(&app)).await
}

/// A page of a track list, sorted and searched in the backend.
///
/// Off the main thread, as the rest here are: the first page of a list sorts all of it.
#[tauri::command]
pub async fn collection_tracks(app: AppHandle, query: TrackQuery, offset: usize, limit: usize) -> AppResult<TrackPage> {
    library::blocking(move || app.state::<Collection>().tracks(&query, offset, limit.min(1000))).await
}

/// The id of every track in a list, for selecting them all.
#[tauri::command]
pub async fn collection_track_ids(app: AppHandle, query: TrackQuery) -> AppResult<Vec<String>> {
    library::blocking(move || app.state::<Collection>().track_ids(&query)).await
}

/// The artists, albums or genres already in the collection, the most used
/// first, to offer as one is typed.
#[tauri::command]
pub async fn collection_field_values(app: AppHandle, field: String) -> AppResult<Vec<String>> {
    library::blocking(move || app.state::<Collection>().field_values(&field)).await
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

/// Marks tracks' beat grids as reviewed, or takes that back. Only the
/// app's own note of it changes, so rekordbox may be open.
#[tauri::command]
pub async fn review_grids(app: AppHandle, track_ids: Vec<String>, reviewed: bool) -> AppResult<()> {
    library::blocking(move || {
        app.state::<Collection>().review_grids(&track_ids, reviewed);
        Ok(())
    })
    .await
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
/// `hint` is what the user knows of the tracks: roughly their tempo, or
/// whether it moves.
#[tauri::command]
pub async fn analyse_tracks(
    app: AppHandle,
    track_ids: Option<Vec<String>>,
    again: bool,
    hint: Option<GridHint>,
) -> AppResult<usize> {
    if hint.is_some_and(|hint| hint.bpm.is_some() && hint.tempo().is_none()) {
        return Err(AppError::Validation("A tempo is between 40 and 250 BPM".into()));
    }
    if crate::rekordbox::is_running() {
        return Err(AppError::Rekordbox("rekordbox is running. Quit it before analysing.".into()));
    }
    let Some(track_ids) = track_ids else {
        let reader = app.clone();
        let all = library::blocking(move || Ok(reader.state::<Collection>().all_unanalysed())).await?;
        return Ok(analysis::enqueue(&app, all, true).await);
    };
    Ok(analysis::enqueue_with(&app, track_ids, again, hint.unwrap_or_default()).await)
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
pub async fn track_waveform(app: AppHandle, track_id: String) -> AppResult<Response> {
    let bytes = library::blocking(move || app.state::<Collection>().waveform(&track_id)).await?;
    Ok(Response::new(bytes))
}

/// What the track list shows of each of these tracks: a small waveform, in
/// the form `prefer` (7 three-band, 5 colour, 3 blue) where the track has
/// it, and its cues. At most a screenful or two at a time.
#[tauri::command]
pub async fn track_previews(app: AppHandle, mut track_ids: Vec<String>, prefer: u8) -> AppResult<Vec<Preview>> {
    track_ids.truncate(200);
    library::blocking(move || app.state::<Collection>().previews(&track_ids, prefer)).await
}

/// The player's audio being decoded: the latest stream asked for, which is
/// the only one that carries on.
#[derive(Default)]
pub struct AudioStream(AtomicU64);

/// Decodes a track for the player, sending its audio over `on_audio` as it
/// is decoded so playing starts at once: first eight bytes (the sample rate
/// and about how many frames are coming, each a little-endian u32), then
/// 16-bit stereo samples in pieces, then an empty message at the end. The
/// audio is on rekordbox's timeline, so cues and beats land where rekordbox
/// has them. `stream` is the caller's name for it; a later one replaces it.
#[tauri::command]
pub async fn track_audio(
    app: AppHandle,
    track_id: String,
    stream: u64,
    on_audio: Channel<Response>,
) -> AppResult<()> {
    app.state::<AudioStream>().0.store(stream, Ordering::SeqCst);
    library::blocking(move || {
        let current = || app.state::<AudioStream>().0.load(Ordering::SeqCst) == stream;
        app.state::<Collection>().decode_audio(&track_id, |audio| {
            let bytes = match audio {
                Audio::Start { rate, frames } => [rate.to_le_bytes(), frames.to_le_bytes()].concat(),
                Audio::Pcm(pcm) => pcm,
            };
            current() && on_audio.send(Response::new(bytes)).is_ok()
        })?;
        if current() {
            let _ = on_audio.send(Response::new(Vec::new()));
        }
        Ok(())
    })
    .await
}

/// Stops decoding a stream the player no longer wants.
#[tauri::command]
pub fn stop_track_audio(current: tauri::State<'_, AudioStream>, stream: u64) {
    let _ = current.0.compare_exchange(stream, 0, Ordering::SeqCst, Ordering::SeqCst);
}
