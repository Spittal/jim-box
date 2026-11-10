//! A job's lifecycle: identify → search → try candidates → tag → file into library.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};
use tokio::sync::OwnedSemaphorePermit;
use tracing::{info, warn};
use uuid::Uuid;

use super::ranking::{
    artist_free_queries, rank_for, search_queries, sort_and_cap, split_path, Anchor, MAX_CANDIDATES, MAX_REJECTED,
};
use super::JobStore;
use crate::error::{AppError, AppResult};
use crate::library::{self, TagMode};
use crate::rekordbox::Arrival;
use crate::autotag;
use crate::sources::Sources;
use crate::slskd::types::TransferPhase;
use crate::slskd::{Daemon, DaemonPaths, SlskdClient};
use crate::state::{now_ms, Candidate, JobSource, JobStatus, SearchAttempt, Settings, SettingsState, TrackMetadata, TransferProgress};

/// How long to wait for Soulseek to connect before failing a job.
const DAEMON_WAIT: Duration = Duration::from_secs(60);
/// Give up on a peer whose queue hasn't started our transfer in this time.
const QUEUE_TIMEOUT: Duration = Duration::from_secs(180);
/// Give up on a transfer that has stopped making progress.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);
/// Candidates tried automatically before giving up (others can be picked manually).
const MAX_AUTO_ATTEMPTS: usize = 5;
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// When no peer answers any search, Soulseek is dropping them. That can last
/// several minutes, so search again after each of these pauses (about half
/// an hour in all) before giving up. Waiting jobs give up their slot.
const QUIET_RETRY_DELAYS: [Duration; 4] = [
    Duration::from_secs(45),
    Duration::from_secs(3 * 60),
    Duration::from_secs(10 * 60),
    Duration::from_secs(20 * 60),
];

/// Runs the full pipeline for a job.
pub fn start(app: &AppHandle, id: Uuid) -> AppResult<()> {
    let task_app = app.clone();
    app.state::<JobStore>().spawn(id, async move {
        let slot = task_app.state::<JobStore>().acquire_slot().await;
        let result = run(&task_app, id, slot).await;
        finish(&task_app, id, result);
    })
}

/// Downloads one specific candidate the user picked.
pub fn start_candidate(app: &AppHandle, id: Uuid, index: usize) -> AppResult<()> {
    let store = app.state::<JobStore>();
    let job = store.get(id)?;
    if index >= job.candidates.len() {
        return Err(AppError::Validation("That source is no longer available".into()));
    }
    store.update(app, id, |j| {
        j.status = JobStatus::Queued;
        j.error = None;
        j.source = JobSource::Soulseek;
        j.youtube = None;
        // Picking another source for a finished download replaces its file.
        if let Some(previous) = j.output_path.take() {
            j.replaces = Some(previous);
        }
        j.already_in_library = false;
    })?;

    let task_app = app.clone();
    store.spawn(id, async move {
        let result = async {
            let _slot = task_app.state::<JobStore>().acquire_slot().await;
            let client = task_app.state::<Daemon>().wait_until_online(DAEMON_WAIT).await?;
            attempt(&task_app, id, &client, index).await
        }
        .await;
        finish(&task_app, id, result);
    })
}

/// Stops a job and cancels its in-flight transfer, if any.
pub async fn cancel(app: &AppHandle, id: Uuid) -> AppResult<()> {
    let store = app.state::<JobStore>();
    store.abort(id);
    let job = store.get(id)?;
    if let (Some(transfer_id), Some(candidate)) = (
        &job.transfer_id,
        job.active_candidate.and_then(|i| job.candidates.get(i)),
    ) {
        if let Ok(client) = app.state::<Daemon>().client() {
            if let Err(e) = client.cancel(&candidate.username, transfer_id).await {
                warn!("Failed to cancel transfer {transfer_id}: {e}");
            }
        }
    }
    if job.source == JobSource::Youtube {
        // yt-dlp is killed with the task; its partial download stays behind.
        if let Ok(dir) = app.path().app_data_dir() {
            let _ = std::fs::remove_dir_all(dir.join("youtube").join(id.to_string()));
        }
    }
    store.update(app, id, |j| {
        j.status = JobStatus::Cancelled;
        j.transfer_id = None;
        j.progress = None;
        j.retry_at = None;
    })
}

pub(super) fn finish(app: &AppHandle, id: Uuid, result: AppResult<()>) {
    if let Err(e) = result {
        warn!("Job {id} failed: {e}");
        let _ = app.state::<JobStore>().update(app, id, |j| {
            j.status = JobStatus::Failed;
            j.error = Some(e.to_string());
            j.transfer_id = None;
            j.progress = None;
            j.retry_at = None;
        });
    }
}

async fn run(app: &AppHandle, id: Uuid, slot: OwnedSemaphorePermit) -> AppResult<()> {
    let store = app.state::<JobStore>();
    let job = store.get(id)?;

    let meta = match job.metadata {
        Some(meta) => meta,
        None => {
            set_status(app, id, JobStatus::Identifying)?;
            let meta = autotag::resolve(&app.state::<Sources>(), &job.request).await;
            store.update(app, id, |j| j.metadata = Some(meta.clone()))?;
            meta
        }
    };

    let settings = app.state::<SettingsState>().snapshot();
    // Upgrades and re-downloads (`replaces`) are deliberate; anything else
    // already in the library is skipped. A lossy copy is only kept if no
    // lossless one turns up.
    let mut lossy_copy = None;
    if job.replaces.is_none() {
        match existing_copy(app, id, &meta, &settings).await {
            Some((existing, true)) => {
                info!("Job {id}: already in the library at {}", existing.display());
                return complete_with_existing(app, id, existing).await;
            }
            Some((existing, false)) => lossy_copy = Some(existing),
            None => {}
        }
    }
    // Replacing a lossy file with another is pointless.
    let replaces_lossy = match &job.replaces {
        Some(path) => !is_lossless_file(path.clone()).await,
        None => false,
    };
    let allow_lossy = settings.lossy_fallback && lossy_copy.is_none() && !replaces_lossy;

    set_status(app, id, JobStatus::Searching)?;
    let (_slot, found) = search(app, id, slot, &meta, &settings, allow_lossy).await?;

    if found.accepted.is_empty() {
        if let Some(existing) = lossy_copy {
            info!("Job {id}: no lossless copy; keeping the lossy one at {}", existing.display());
            store.update(app, id, |j| j.candidates = found.rejected)?;
            return complete_with_existing(app, id, existing).await;
        }
    }

    // Files that failed a check come last and are only tried by hand.
    let count = found.accepted.len();
    store.update(app, id, |j| {
        j.candidates = found.accepted.into_iter().chain(found.rejected).collect();
        if count == 0 {
            j.status = JobStatus::NotFound;
        }
        // A lossless download upgrades the lossy copy, which goes to the Trash.
        if lossy_copy.is_some() {
            j.replaces = lossy_copy;
        }
    })?;

    // Soulseek may have reconnected while the job was waiting.
    let client = app.state::<Daemon>().wait_until_online(DAEMON_WAIT).await?;
    let mut last_error = None;
    for index in 0..count.min(MAX_AUTO_ATTEMPTS) {
        match attempt(app, id, &client, index).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                warn!("Job {id}: candidate {index} failed: {e}");
                last_error = Some(e);
            }
        }
    }
    match last_error {
        Some(e) => Err(AppError::Slskd(format!(
            "Tried {} sources without success. Last error: {e}",
            count.min(MAX_AUTO_ATTEMPTS)
        ))),
        None => Ok(()), // No candidates: already marked NotFound.
    }
}

/// What a job's searches found.
struct Found {
    /// Lossless files that passed every check or, failing those and if
    /// allowed, lossy ones.
    accepted: Vec<Candidate>,
    /// Files with the right title that failed a check, from every search run.
    rejected: Vec<Candidate>,
}

/// Runs the track's searches in order until one has usable results. Every
/// search is recorded on the job. When no peer answers any of them, the job
/// waits without its slot and searches again, per `QUIET_RETRY_DELAYS`.
/// Lossy files are used, if `allow_lossy`, only when no search finds a
/// lossless one. Returns the slot it holds at the end.
async fn search(
    app: &AppHandle,
    id: Uuid,
    mut slot: OwnedSemaphorePermit,
    meta: &TrackMetadata,
    settings: &Settings,
    allow_lossy: bool,
) -> AppResult<(OwnedSemaphorePermit, Found)> {
    let store = app.state::<JobStore>();
    let mut rejected: Vec<Candidate> = Vec::new();
    let mut lossy: Vec<Candidate> = Vec::new();
    for round in 0..=QUIET_RETRY_DELAYS.len() {
        if round > 0 {
            let delay = QUIET_RETRY_DELAYS[round - 1];
            warn!("Job {id}: no peers answered; searching again in {delay:?}");
            drop(slot);
            store.update(app, id, |j| {
                j.status = JobStatus::Waiting;
                j.retry_at = Some(now_ms() + delay.as_millis() as u64);
            })?;
            tokio::time::sleep(delay).await;
            slot = store.acquire_slot().await;
            store.update(app, id, |j| {
                j.status = JobStatus::Searching;
                j.retry_at = None;
            })?;
        }
        // Fetched each round: slskd may have restarted while the job waited.
        let client = app.state::<Daemon>().wait_until_online(DAEMON_WAIT).await?;
        let mut anyone_answered = false;
        // Peers ignore searches naming a banned artist, so when nobody answers
        // those, search without the name before deciding Soulseek is quiet.
        let mut artist_answered = false;
        let mut queries = search_queries(meta);
        let mut next = 0;
        while let Some(query) = queries.get(next).cloned() {
            next += 1;
            let responses = client.search(&query.text).await?;
            let ranked = rank_for(&query, &responses, meta, settings);
            info!(
                "Search '{}': {} peers responded, {} usable candidates, {} rejected, {} lossy",
                query.text,
                responses.len(),
                ranked.accepted.len(),
                ranked.rejected.len(),
                ranked.lossy.len()
            );
            anyone_answered |= !responses.is_empty();
            artist_answered |= query.anchor == Anchor::Artist && !responses.is_empty();
            store.update(app, id, |j| {
                j.searches.push(SearchAttempt {
                    query: query.text.clone(),
                    peers: responses.len(),
                    usable: ranked.accepted.len(),
                    rejected: ranked.rejected.len(),
                })
            })?;
            add_new(&mut rejected, ranked.rejected);
            if allow_lossy {
                add_new(&mut lossy, ranked.lossy);
            }
            if !ranked.accepted.is_empty() {
                let accepted = ranked.accepted;
                rejected.retain(|r| !accepted.iter().any(|a| a.username == r.username && a.filename == r.filename));
                sort_and_cap(&mut rejected, MAX_REJECTED);
                return Ok((slot, Found { accepted, rejected }));
            }
            if next == queries.len() && !artist_answered && queries.iter().all(|q| q.anchor != Anchor::ArtistInPath) {
                info!("Job {id}: nobody answered searches naming the artist; it may be a banned phrase");
                queries.extend(artist_free_queries(meta));
            }
        }
        if anyone_answered {
            break;
        }
    }
    sort_and_cap(&mut rejected, MAX_REJECTED);
    sort_and_cap(&mut lossy, MAX_CANDIDATES);
    if !lossy.is_empty() {
        info!("Job {id}: no lossless copy found; falling back to {} lossy files", lossy.len());
    }
    Ok((slot, Found { accepted: lossy, rejected }))
}

/// Adds the candidates `into` doesn't already have.
fn add_new(into: &mut Vec<Candidate>, candidates: Vec<Candidate>) {
    for c in candidates {
        if !into.iter().any(|r| r.username == c.username && r.filename == c.filename) {
            into.push(c);
        }
    }
}

/// Finishes a job with the copy already on disk, which joins the collection
/// (and the job's playlist) if it isn't there yet.
async fn complete_with_existing(app: &AppHandle, id: Uuid, existing: PathBuf) -> AppResult<()> {
    let playlist = app.state::<JobStore>().update(app, id, |j| {
        j.status = JobStatus::Completed;
        j.output_path = Some(existing.clone());
        j.already_in_library = true;
        j.playlist.clone()
    })?;
    crate::ingest::arrived(app, vec![Arrival { path: existing, replaces: None, playlist }]).await;
    Ok(())
}

/// Whether the file on disk is lossless; false if it can't be read.
async fn is_lossless_file(path: PathBuf) -> bool {
    library::blocking(move || library::read_file(&path))
        .await
        .is_ok_and(|f| f.lossless)
}

/// A copy of the track already in the library, and whether it's lossless:
/// at its templated path, or the output of another finished download of the
/// same recording. Lossless copies are preferred.
async fn existing_copy(app: &AppHandle, id: Uuid, meta: &TrackMetadata, settings: &Settings) -> Option<(PathBuf, bool)> {
    let relative = library::render(&settings.paths, meta);
    let mut paths: Vec<PathBuf> = library::AUDIO_EXTENSIONS
        .iter()
        .map(|ext| library::library_path(&settings.music_dir, &relative, ext))
        .collect();
    if let Some(recording) = meta.mb_recording_id.as_ref() {
        paths.extend(app.state::<JobStore>().list().into_iter().filter_map(|j| {
            let same = j.id != id
                && j.status == JobStatus::Completed
                && j.metadata.as_ref().and_then(|m| m.mb_recording_id.as_ref()) == Some(recording);
            j.output_path.filter(|_| same)
        }));
    }
    let mut lossy = None;
    for path in paths.into_iter().filter(|p| p.is_file()) {
        if is_lossless_file(path.clone()).await {
            return Some((path, true));
        }
        lossy.get_or_insert(path);
    }
    lossy.map(|path| (path, false))
}

/// Downloads one candidate, then tags and files it. Marks the candidate as
/// failed on error so the UI can show which sources were tried.
async fn attempt(app: &AppHandle, id: Uuid, client: &SlskdClient, index: usize) -> AppResult<()> {
    let result = try_candidate(app, id, client, index).await;
    if result.is_err() {
        let _ = app.state::<JobStore>().update(app, id, |j| {
            if let Some(c) = j.candidates.get_mut(index) {
                c.failed = true;
            }
            j.transfer_id = None;
            j.progress = None;
        });
    }
    result
}

async fn try_candidate(app: &AppHandle, id: Uuid, client: &SlskdClient, index: usize) -> AppResult<()> {
    let store = app.state::<JobStore>();
    let job = store.get(id)?;
    let candidate = job
        .candidates
        .get(index)
        .cloned()
        .ok_or_else(|| AppError::Validation("Unknown source".into()))?;
    let meta = job
        .metadata
        .unwrap_or_else(|| TrackMetadata::from_request(&job.request));

    store.update(app, id, |j| {
        j.status = JobStatus::Downloading;
        j.active_candidate = Some(index);
        j.progress = None;
        j.error = None;
    })?;

    let downloaded = download(app, id, client, &candidate).await?;

    store.update(app, id, |j| {
        j.status = JobStatus::Tagging;
        j.transfer_id = None;
        j.progress = None;
    })?;
    let dest = file_into_library(app, id, downloaded, meta, job.replaces.clone()).await?;
    info!("Job {id} completed: {}", dest.display());

    store.update(app, id, |j| {
        j.status = JobStatus::Completed;
        j.output_path = Some(dest.clone());
    })?;
    let arrival = Arrival { path: dest, replaces: job.replaces, playlist: job.playlist };
    crate::ingest::arrived(app, vec![arrival]).await;
    Ok(())
}

/// Queues the file with slskd and follows it until it's on disk.
async fn download(app: &AppHandle, id: Uuid, client: &SlskdClient, candidate: &Candidate) -> AppResult<PathBuf> {
    let store = app.state::<JobStore>();
    let destination = id.to_string();
    let transfer = client
        .enqueue(&candidate.username, &candidate.filename, candidate.size, &destination)
        .await?;
    store.update(app, id, |j| j.transfer_id = Some(transfer.id.clone()))?;

    let queued_at = Instant::now();
    let mut last_bytes = 0;
    let mut last_progress_at = Instant::now();
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        let t = client.transfer(&candidate.username, &transfer.id).await?;
        store.update(app, id, |j| {
            j.progress = Some(TransferProgress {
                bytes_transferred: t.bytes_transferred,
                size: t.size,
                speed: t.average_speed,
                place_in_queue: t.place_in_queue,
            })
        })?;

        let give_up = |reason: &str| AppError::Slskd(format!("{reason} ({})", candidate.username));
        match t.phase() {
            TransferPhase::Succeeded => break,
            TransferPhase::Failed => return Err(give_up(&format!("Transfer failed: {}", t.failure_reason()))),
            TransferPhase::Queued if queued_at.elapsed() > QUEUE_TIMEOUT => {
                let _ = client.cancel(&candidate.username, &transfer.id).await;
                return Err(give_up("Peer's upload queue is too long"));
            }
            TransferPhase::InProgress if t.bytes_transferred > last_bytes => {
                last_bytes = t.bytes_transferred;
                last_progress_at = Instant::now();
            }
            TransferPhase::InProgress if last_progress_at.elapsed() > STALL_TIMEOUT => {
                let _ = client.cancel(&candidate.username, &transfer.id).await;
                return Err(give_up("Transfer stalled"));
            }
            _ => {}
        }
    }

    // Clear the finished transfer from slskd's list; it's ours to track now.
    let _ = client.cancel(&candidate.username, &transfer.id).await;

    let staging = DaemonPaths::resolve(app)?.downloads.join(&destination);
    let (_, basename) = split_path(&candidate.filename);
    let expected = staging.join(basename);
    if expected.is_file() {
        return Ok(expected);
    }
    // slskd may adjust the name (e.g. for characters invalid on this OS).
    first_file_in(&staging)?.ok_or_else(|| {
        AppError::Slskd(format!("Download finished but no file found in {}", staging.display()))
    })
}

/// Tags the downloaded file and moves it to its templated library path. The
/// file it `replaces` goes to the Trash first, so the new one can take its name.
pub(super) async fn file_into_library(
    app: &AppHandle,
    id: Uuid,
    src: PathBuf,
    meta: TrackMetadata,
    replaces: Option<PathBuf>,
) -> AppResult<PathBuf> {
    let cover = app.state::<Sources>().fetch_cover(&meta.cover_art_urls).await;
    let settings = app.state::<SettingsState>().snapshot();
    let relative = library::render(&settings.paths, &meta);
    let ext = library::extension(&src);

    let dest = library::blocking(move || {
        library::write_tags(&src, &meta, cover.as_deref(), TagMode::Replace)?;
        if let Some(old) = replaces.filter(|old| old.is_file()) {
            match trash::delete(&old) {
                Ok(()) => info!("Moved replaced file to the Trash: {}", old.display()),
                Err(e) => warn!("Couldn't move {} to the Trash: {e}", old.display()),
            }
        }
        library::move_file(&src, library::library_path(&settings.music_dir, &relative, &ext))
    })
    .await?;

    // The per-job staging folder should now be empty.
    if let Ok(paths) = DaemonPaths::resolve(app) {
        let _ = std::fs::remove_dir(paths.downloads.join(id.to_string()));
    }
    Ok(dest)
}

fn first_file_in(dir: &Path) -> AppResult<Option<PathBuf>> {
    if !dir.is_dir() {
        return Ok(None);
    }
    Ok(std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.is_file()))
}

fn set_status(app: &AppHandle, id: Uuid, status: JobStatus) -> AppResult<()> {
    app.state::<JobStore>().update(app, id, |j| j.status = status)
}
