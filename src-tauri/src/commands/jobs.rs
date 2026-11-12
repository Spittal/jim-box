use std::path::PathBuf;
use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::jobs::{self, JobStore};
use crate::sources::youtube::{YtDlp, YtDlpInfo};
use crate::state::{Job, JobStatus, TrackRequest};

#[tauri::command]
pub fn list_jobs(store: State<'_, JobStore>) -> Vec<Job> {
    store.list()
}

/// `playlist_id` is a rekordbox playlist the track joins once it's downloaded.
#[tauri::command]
pub fn start_job(
    app: AppHandle,
    store: State<'_, JobStore>,
    track: TrackRequest,
    replaces: Option<PathBuf>,
    playlist_id: Option<String>,
) -> AppResult<Job> {
    let mut job = Job::new(track, replaces);
    job.playlist_id = playlist_id;
    store.insert(&app, job.clone());
    jobs::start(&app, job.id)?;
    Ok(job)
}

/// Re-runs a finished job from the start.
#[tauri::command]
pub fn retry_job(app: AppHandle, store: State<'_, JobStore>, id: Uuid) -> AppResult<()> {
    ensure_idle(&store, id)?;
    store.update(&app, id, |j| j.reset())?;
    jobs::start(&app, id)
}

/// Retries every job that found no source.
#[tauri::command]
pub fn retry_not_found_jobs(app: AppHandle, store: State<'_, JobStore>) -> AppResult<()> {
    let ids: Vec<Uuid> = store.read(|jobs| jobs.iter().filter(|j| j.status == JobStatus::NotFound).map(|j| j.id).collect());
    let ids: Vec<Uuid> = ids.into_iter().filter(|&id| !store.is_running(id)).collect();
    for id in ids {
        store.update(&app, id, |j| j.reset())?;
        jobs::start(&app, id)?;
    }
    Ok(())
}

/// Downloads a finished job's track again, replacing the file it points to.
/// For tracks skipped as already in the library, or a wrong file.
#[tauri::command]
pub fn redownload_job(app: AppHandle, store: State<'_, JobStore>, id: Uuid) -> AppResult<()> {
    ensure_idle(&store, id)?;
    store.update(&app, id, |j| {
        j.replaces = j.output_path.clone().or(j.replaces.take());
        j.reset();
    })?;
    jobs::start(&app, id)
}

#[tauri::command]
pub async fn cancel_job(app: AppHandle, id: Uuid) -> AppResult<()> {
    jobs::cancel(&app, id).await
}

/// Downloads a specific source the user picked from the job's candidates.
#[tauri::command]
pub fn download_candidate(app: AppHandle, store: State<'_, JobStore>, id: Uuid, index: usize) -> AppResult<()> {
    ensure_idle(&store, id)?;
    jobs::start_candidate(&app, id, index)
}

/// Downloads the track from YouTube Music, for tracks Soulseek doesn't have,
/// including ones still waiting to search Soulseek again.
#[tauri::command]
pub fn download_from_youtube(app: AppHandle, id: Uuid) -> AppResult<()> {
    jobs::start_youtube(&app, id)
}

/// The installed yt-dlp, or why it can't be used.
#[tauri::command]
pub async fn youtube_tool() -> AppResult<YtDlpInfo> {
    YtDlp::locate(None)?.info().await
}

#[tauri::command]
pub fn remove_job(app: AppHandle, store: State<'_, JobStore>, id: Uuid) -> AppResult<()> {
    store.remove(&app, id)
}

fn ensure_idle(store: &JobStore, id: Uuid) -> AppResult<()> {
    if store.is_running(id) {
        return Err(AppError::Validation("This download is already running".into()));
    }
    Ok(())
}
