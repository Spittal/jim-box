//! A job's lifecycle: identify → search → try candidates → tag → file into library.

use std::path::{Path, PathBuf};
use std::sync::Arc;
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
use crate::soulseek::types::TransferPhase;
use crate::soulseek::{can_save, staging_dir, Soulseek, SoulseekClient, UNSAVEABLE_NAME};
use crate::state::{now_ms, Candidate, JobSource, JobStatus, SearchAttempt, Settings, SettingsState, TrackMetadata, TransferProgress};

/// How long to wait for Soulseek to connect before failing a job.
const ONLINE_WAIT: Duration = Duration::from_secs(60);
/// Give up on a peer whose queue hasn't started our transfer in this time.
const QUEUE_TIMEOUT: Duration = Duration::from_secs(180);
/// Give up on a transfer that has stopped making progress.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);
/// Candidates tried automatically before giving up (others can be picked manually).
const MAX_AUTO_ATTEMPTS: usize = 5;
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How often a queued transfer asks the peer where it stands.
const QUEUE_PLACE_INTERVAL: Duration = Duration::from_secs(30);
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
            attempt(&task_app, id, index).await
        }
        .await;
        finish(&task_app, id, result);
    })
}

/// Stops a job and cancels its in-flight transfer, if any. One that has
/// finished is left as it is, and one being filed into the library can't be
/// stopped: its files are half moved.
pub async fn cancel(app: &AppHandle, id: Uuid) -> AppResult<()> {
    let store = app.state::<JobStore>();
    let job = store.get(id)?;
    let mark = |j: &mut crate::state::Job| {
        if j.status.is_finished() {
            return Ok(false);
        }
        if j.status == JobStatus::Tagging {
            return Err(AppError::Validation("It's being filed into the library, which can't be stopped".into()));
        }
        j.status = JobStatus::Cancelled;
        j.transfer_id = None;
        j.progress = None;
        j.retry_at = None;
        Ok(true)
    };
    // Marked before the task is stopped: `complete` won't start filing a cancelled job.
    if !store.update(app, id, mark)?? {
        return Ok(());
    }
    store.abort(id);
    // By the source being tried, not the transfer's id: a transfer stopped
    // while it was being asked for has none yet.
    if let Some(candidate) = job.active_candidate.and_then(|i| job.candidates.get(i)) {
        if job.status == JobStatus::Downloading {
            if let Ok(client) = app.state::<Soulseek>().client() {
                client.cancel(&candidate.username, &candidate.filename);
            }
        }
    }
    clear_staging(app, id);
    if job.source == JobSource::Youtube {
        // yt-dlp is killed with the task; its partial download stays behind.
        if let Ok(dir) = app.path().app_data_dir() {
            let _ = std::fs::remove_dir_all(dir.join("youtube").join(id.to_string()));
        }
    }
    // The task may have set a status of its own on its way out.
    store.update(app, id, mark)??;
    Ok(())
}

pub(super) fn finish(app: &AppHandle, id: Uuid, result: AppResult<()>) {
    if let Err(e) = result {
        let _ = app.state::<JobStore>().update(app, id, |j| {
            // Stopped by the user: that's not a failure.
            if j.status == JobStatus::Cancelled {
                return;
            }
            warn!("Job {id} failed: {e}");
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
    // Files this computer can't save under their own name are left for the
    // others, and don't use up an attempt.
    let auto: Vec<usize> = (0..count).filter(|&i| can_save(&found.accepted[i].filename)).take(MAX_AUTO_ATTEMPTS).collect();
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

    let mut last_error = None;
    if count > 0 && auto.is_empty() {
        return Err(AppError::Soulseek(UNSAVEABLE_NAME.into()));
    }
    for &index in &auto {
        match attempt(app, id, index).await {
            Ok(()) => return Ok(()),
            // Soulseek is gone, which says nothing of this source or the
            // rest: they're left untried for when it's back.
            Err(e @ (AppError::SoulseekNotReady | AppError::SoulseekFailed(_))) => return Err(e),
            Err(e) => {
                warn!("Job {id}: candidate {index} failed: {e}");
                last_error = Some(e);
            }
        }
    }
    match last_error {
        Some(e) => Err(AppError::Soulseek(format!(
            "Tried {} sources without success. Last error: {e}",
            auto.len()
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
    // A title with no words in it ("?", "…") matches every file by the
    // artist: there's nothing to search for or to tell the right file by.
    if crate::text::tokens(&crate::text::split_version(&meta.title).0).is_empty() {
        return Ok((slot, Found { accepted: Vec::new(), rejected }));
    }
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
        // Fetched each round: Soulseek may have reconnected while the job waited.
        let mut client = app.state::<Soulseek>().wait_until_online(ONLINE_WAIT).await?;
        let mut anyone_answered = false;
        // Peers ignore searches naming a banned artist, so when nobody answers
        // those, search without the name before deciding Soulseek is quiet.
        let mut artist_answered = false;
        let mut queries = search_queries(meta);
        let mut next = 0;
        while let Some(query) = queries.get(next).cloned() {
            next += 1;
            let responses = match client.search(&query.text).await {
                // The connection dropped: the search is run again on the next one.
                Err(AppError::SoulseekNotReady) => {
                    client = app.state::<Soulseek>().wait_until_online(ONLINE_WAIT).await?;
                    client.search(&query.text).await?
                }
                responses => responses?,
            };
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
    let playlist_id = app.state::<JobStore>().update(app, id, |j| {
        j.status = JobStatus::Completed;
        j.output_path = Some(existing.clone());
        j.already_in_library = true;
        j.playlist_id.clone()
    })?;
    crate::ingest::arrived(app, vec![Arrival { path: existing, playlist_id, ..Default::default() }]).await;
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
        paths.extend(app.state::<JobStore>().read(|jobs| {
            let same = |j: &&crate::state::Job| {
                j.id != id
                    && j.status == JobStatus::Completed
                    && j.metadata.as_ref().and_then(|m| m.mb_recording_id.as_ref()) == Some(recording)
            };
            jobs.iter().filter(same).filter_map(|j| j.output_path.clone()).collect::<Vec<_>>()
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

/// Times a source is tried again after Soulseek's connection dropped under it.
const MAX_RECONNECTS: usize = 2;

/// Downloads one candidate, then tags and files it. Marks the candidate as
/// failed on error so the UI can show which sources were tried; not when
/// it was Soulseek's connection that failed, which is waited for and the
/// source tried again.
async fn attempt(app: &AppHandle, id: Uuid, index: usize) -> AppResult<()> {
    let mut reconnects = 0;
    let result = loop {
        // Asked for each time: a client whose connection dropped stays closed.
        let result = match app.state::<Soulseek>().wait_until_online(ONLINE_WAIT).await {
            Ok(client) => try_candidate(app, id, &client, index).await,
            Err(e) => Err(e),
        };
        match result {
            Err(AppError::SoulseekNotReady) if reconnects < MAX_RECONNECTS => {
                reconnects += 1;
                warn!("Job {id}: Soulseek dropped while fetching; waiting for it to reconnect");
                clear_staging(app, id);
            }
            result => break result,
        }
    };
    if let Err(e) = &result {
        let offline = matches!(e, AppError::SoulseekNotReady | AppError::SoulseekFailed(_));
        clear_staging(app, id);
        let _ = app.state::<JobStore>().update(app, id, |j| {
            if let Some(c) = j.candidates.get_mut(index).filter(|_| !offline) {
                c.failed = true;
            }
            j.transfer_id = None;
            j.progress = None;
        });
    }
    result
}

async fn try_candidate(app: &AppHandle, id: Uuid, client: &Arc<SoulseekClient>, index: usize) -> AppResult<()> {
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

    complete(app, id, downloaded, meta, job.replaces, |_| {}).await
}

/// Tags a download, files it into the library and hands it to the
/// collection. Once this starts the job can't be stopped (see `cancel`), and
/// it runs to its end even if the job's task is: a file is never left tagged
/// but not moved, or moved but unknown to the collection.
pub(super) async fn complete(
    app: &AppHandle,
    id: Uuid,
    src: PathBuf,
    meta: TrackMetadata,
    replaces: Option<PathBuf>,
    note: impl FnOnce(&mut crate::state::Job),
) -> AppResult<()> {
    let filing = app.state::<JobStore>().update(app, id, |j| {
        if j.status == JobStatus::Cancelled {
            return false;
        }
        j.status = JobStatus::Tagging;
        j.transfer_id = None;
        j.progress = None;
        note(j);
        true
    })?;
    if !filing {
        return Err(AppError::Validation("The download was cancelled".into()));
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let dest = file_into_library(&app, id, src, meta, replaces.clone()).await?;
        info!("Job {id} completed: {}", dest.display());
        let playlist_id = app.state::<JobStore>().update(&app, id, |j| {
            j.status = JobStatus::Completed;
            j.output_path = Some(dest.clone());
            j.playlist_id.clone()
        })?;
        let arrival = Arrival { path: dest, replaces, playlist_id, ..Default::default() };
        crate::ingest::arrived(&app, vec![arrival]).await;
        Ok(())
    })
    .await
    .map_err(|e| AppError::Io(std::io::Error::other(e)))?
}

/// Asks the peer for the file and follows the transfer until it's on disk.
async fn download(app: &AppHandle, id: Uuid, client: &Arc<SoulseekClient>, candidate: &Candidate) -> AppResult<PathBuf> {
    let store = app.state::<JobStore>();
    let staging = staging_dir(app)?.join(id.to_string());
    // A partial file left by another peer's copy of the same name would be
    // taken for the start of this one.
    clear_staging(app, id);
    let (username, filename) = (&candidate.username, &candidate.filename);
    let transfer = client.enqueue(username, filename, candidate.size, &staging).await?;
    store.update(app, id, |j| j.transfer_id = Some(transfer.token.to_string()))?;

    let queued_at = Instant::now();
    let mut asked_place_at = Instant::now();
    let mut last_bytes = 0;
    let mut last_progress_at = Instant::now();
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        let t = client.transfer(username, filename)?;
        let progress = Some(TransferProgress {
            bytes_transferred: t.bytes_transferred,
            size: t.size,
            speed: t.speed,
            place_in_queue: t.place_in_queue,
        });
        // Told to the frontend only when it has moved: a transfer waiting in
        // a peer's queue reads the same for minutes.
        store.set_progress(app, id, progress)?;

        let give_up = |reason: &str| AppError::Soulseek(format!("{reason} ({username})"));
        match t.phase {
            TransferPhase::Succeeded => break,
            TransferPhase::Failed(reason) => {
                client.forget(username, filename);
                return Err(give_up(&format!("Transfer failed: {reason}")));
            }
            TransferPhase::Queued if queued_at.elapsed() > QUEUE_TIMEOUT => {
                client.cancel(username, filename);
                return Err(give_up("Peer's upload queue is too long"));
            }
            TransferPhase::Queued if asked_place_at.elapsed() > QUEUE_PLACE_INTERVAL => {
                client.request_place_in_queue(username, filename);
                asked_place_at = Instant::now();
            }
            TransferPhase::InProgress if t.bytes_transferred > last_bytes => {
                last_bytes = t.bytes_transferred;
                last_progress_at = Instant::now();
            }
            TransferPhase::InProgress if last_progress_at.elapsed() > STALL_TIMEOUT => {
                client.cancel(username, filename);
                return Err(give_up("Transfer stalled"));
            }
            _ => {}
        }
    }

    // Clear the finished transfer from the client's list; it's ours to track now.
    client.forget(username, filename);

    let (_, basename) = split_path(filename);
    let expected = staging.join(basename);
    if expected.is_file() {
        return Ok(expected);
    }
    // The name may not have survived as it was (e.g. characters invalid on this OS).
    first_file_in(&staging)?.ok_or_else(|| {
        AppError::Soulseek(format!("Download finished but no file found in {}", staging.display()))
    })
}

/// Empties a job's staging folder of whatever a download left there.
fn clear_staging(app: &AppHandle, id: Uuid) {
    if let Ok(dir) = staging_dir(app) {
        let _ = std::fs::remove_dir_all(dir.join(id.to_string()));
    }
}

/// Tags the downloaded file and moves it to its templated library path. The
/// file it `replaces` goes to the Trash only once the new one is safely in
/// the library, under a name of its own, and then the new one takes its name.
async fn file_into_library(
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
        let dest = library::library_path(&settings.music_dir, &relative, &ext);
        let Some(old) = replaces.filter(|old| old.is_file()) else {
            return library::move_file(&src, dest);
        };
        let stem = dest.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let arrived = library::move_file(&src, dest.with_file_name(format!("{stem} (new).{ext}")))?;
        match trash::delete(&old) {
            Ok(()) => info!("Moved replaced file to the Trash: {}", old.display()),
            Err(e) => warn!("Couldn't move {} to the Trash: {e}", old.display()),
        }
        // It is in the library either way; under its own name if this fails.
        Ok(library::move_file(&arrived, dest).unwrap_or(arrived))
    })
    .await?;

    // The per-job staging folder should now be empty.
    if let Ok(dir) = staging_dir(app) {
        let _ = std::fs::remove_dir(dir.join(id.to_string()));
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
