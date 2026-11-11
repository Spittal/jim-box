//! Spotify playlists kept in step with rekordbox playlists: a public
//! playlist becomes a rekordbox playlist holding the tracks the collection
//! already has. Each other entry is matched to a search result (see
//! [`autotag::find_track`]) and, when the match is sure, fetched;
//! medium-confidence matches are left for the user to check.
//!
//! While the window is open, every linked playlist is read again every
//! [`SYNC_EVERY`]: tracks added on Spotify are matched and fetched, and
//! tracks taken off it leave the rekordbox playlist (they stay in the
//! collection). rekordbox's library can only be changed while rekordbox is
//! closed, so those changes wait until it is.
//!
//! The link between the two playlists, and the matches (including ones the
//! user picked by hand), are saved.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tracing::{info, warn};

use crate::autotag::{self, TrackHint};
use crate::error::{AppError, AppResult};
use crate::jobs::{self, JobStore};
use crate::persistence;
use crate::rekordbox::{Collection, Edit, PlaylistKind};
use crate::sources::{Sources, SpotifyTrack};
use crate::state::{now_ms, Confidence, Job, JobStatus, TrackRequest};

/// Unlinked playlists remembered, most recently read first. Linked ones are
/// always kept.
const MAX_SAVED: usize = 200;
/// How often linked playlists are read from Spotify again.
const SYNC_EVERY: Duration = Duration::from_secs(60);
/// The first read after launch, once the collection has had time to open.
const FIRST_SYNC_AFTER: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistEntry {
    pub track: SpotifyTrack,
    /// The matching search result; `None` when no confident match was found.
    pub request: Option<TrackRequest>,
    pub confidence: Option<Confidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistMatch {
    pub id: String,
    pub name: String,
    pub cover_url: Option<String>,
    pub truncated: bool,
    pub entries: Vec<PlaylistEntry>,
    /// When the track list was last read from Spotify.
    #[serde(default)]
    pub loaded_at: u64,
    /// The rekordbox playlist this one is kept in step with.
    #[serde(default)]
    pub rekordbox_id: Option<String>,
    /// Tracks taken off the Spotify playlist that have yet to leave the
    /// rekordbox one: rekordbox was open, or their download hasn't landed.
    #[serde(default)]
    pub removed: Vec<PlaylistEntry>,
}

/// Saved Spotify playlists, persisted on every change.
pub struct PlaylistStore {
    playlists: Mutex<Vec<PlaylistMatch>>,
    /// Held while a playlist is read and brought into rekordbox, so the
    /// timer and the user don't do it at once.
    syncing: tokio::sync::Mutex<()>,
}

impl PlaylistStore {
    pub fn load(app: &AppHandle) -> Self {
        Self { playlists: Mutex::new(persistence::load_playlists(app)), syncing: tokio::sync::Mutex::new(()) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<PlaylistMatch>> {
        self.playlists.lock().expect("playlists lock poisoned")
    }

    fn get(&self, id: &str) -> Option<PlaylistMatch> {
        self.lock().iter().find(|p| p.id == id).cloned()
    }

    /// The Spotify playlist kept in step with this rekordbox playlist, if any.
    fn linked_to(&self, rekordbox_id: &str) -> Option<PlaylistMatch> {
        self.lock().iter().find(|p| p.rekordbox_id.as_deref() == Some(rekordbox_id)).cloned()
    }

    /// The Spotify playlists that are kept in step with a rekordbox playlist.
    fn linked(&self) -> Vec<PlaylistMatch> {
        self.lock().iter().filter(|p| p.rekordbox_id.is_some()).cloned().collect()
    }

    /// The rekordbox playlists that follow a Spotify playlist.
    pub fn linked_ids(&self) -> Vec<String> {
        self.lock().iter().filter_map(|p| p.rekordbox_id.clone()).collect()
    }

    /// Saves a playlist as the most recent.
    fn put(&self, app: &AppHandle, playlist: PlaylistMatch) {
        let mut playlists = self.lock();
        playlists.retain(|p| p.id != playlist.id);
        playlists.insert(0, playlist);
        let mut unlinked = 0;
        playlists.retain(|p| {
            p.rekordbox_id.is_some() || {
                unlinked += 1;
                unlinked <= MAX_SAVED
            }
        });
        persistence::save_playlists(app, &playlists);
    }

    /// Notes that a playlist was read just now and hadn't changed. Not
    /// saved: it's only for showing, and happens every minute.
    fn touch(&self, id: &str) {
        if let Some(playlist) = self.lock().iter_mut().find(|p| p.id == id) {
            playlist.loaded_at = now_ms();
        }
    }

    /// Stops keeping a rekordbox playlist in step with Spotify. The
    /// playlist and the matches stay.
    pub fn unlink(&self, app: &AppHandle, rekordbox_id: &str) {
        let mut playlists = self.lock();
        for playlist in playlists.iter_mut().filter(|p| p.rekordbox_id.as_deref() == Some(rekordbox_id)) {
            playlist.rekordbox_id = None;
            playlist.removed.clear();
        }
        persistence::save_playlists(app, &playlists);
    }

    /// Replaces the tracks still to be taken out of the rekordbox playlist.
    fn set_removed(&self, app: &AppHandle, id: &str, removed: Vec<PlaylistEntry>) {
        let mut playlists = self.lock();
        if let Some(playlist) = playlists.iter_mut().find(|p| p.id == id) {
            playlist.removed = removed;
            persistence::save_playlists(app, &playlists);
        }
    }

    /// Records a match the user picked for one track.
    pub fn set_match(&self, app: &AppHandle, id: &str, index: usize, request: TrackRequest) -> AppResult<()> {
        let mut playlists = self.lock();
        let entry = playlists
            .iter_mut()
            .find(|p| p.id == id)
            .and_then(|p| p.entries.get_mut(index))
            .ok_or_else(|| AppError::Validation("That playlist track no longer exists".into()))?;
        entry.request = Some(request);
        entry.confidence = Some(Confidence::High);
        persistence::save_playlists(app, &playlists);
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlaylistProgress {
    done: usize,
    total: usize,
}

/// Reads a public playlist and matches the tracks that are new to it,
/// keeping saved matches. Tracks that found no match before are searched
/// for again only with `rematch`, so the timer doesn't repeat failed
/// searches every minute. Tracks that have gone are noted in `removed`.
/// Emits `playlist-progress` while matching.
async fn read(app: &AppHandle, input: &str, rematch: bool) -> AppResult<PlaylistMatch> {
    let sources = app.state::<Sources>();
    let store = app.state::<PlaylistStore>();
    let playlist = sources.spotify.playlist(input).await?;
    let saved = store.get(&playlist.id);
    let known = saved.as_ref().map(|p| p.entries.as_slice()).unwrap_or_default();

    let unchanged = known.len() == playlist.tracks.len() && known.iter().zip(&playlist.tracks).all(|(e, t)| e.track == *t);
    if let Some(saved) = saved.as_ref().filter(|p| unchanged && !(rematch && p.entries.iter().any(|e| e.request.is_none()))) {
        store.touch(&saved.id);
        return Ok(PlaylistMatch { loaded_at: now_ms(), ..saved.clone() });
    }

    let total = playlist.tracks.len();
    let mut entries = Vec::with_capacity(total);
    let mut reused = 0;
    for (done, track) in playlist.tracks.into_iter().enumerate() {
        let _ = app.emit("playlist-progress", PlaylistProgress { done, total });
        if let Some(known) = known.iter().find(|e| e.track == track && (e.request.is_some() || !rematch)) {
            entries.push(known.clone());
            reused += 1;
            continue;
        }
        let (request, confidence) = match autotag::find_track(&sources, &hint(&track)).await {
            Ok(Some((request, confidence))) => (Some(request), Some(confidence)),
            Ok(None) => (None, None),
            Err(e) => {
                warn!("Couldn't search for {}: {e}", track.title);
                (None, None)
            }
        };
        entries.push(PlaylistEntry { track, request, confidence });
    }
    let _ = app.emit("playlist-progress", PlaylistProgress { done: total, total });

    let removed = gone(saved.as_ref().map(|p| p.removed.as_slice()).unwrap_or_default(), known, &entries, playlist.truncated);

    let matched = entries.iter().filter(|e| e.request.is_some()).count();
    info!(
        "Playlist '{}': matched {matched} of {total} tracks ({reused} from before), {} gone",
        playlist.name,
        removed.len()
    );
    let result = PlaylistMatch {
        id: playlist.id,
        name: playlist.name,
        cover_url: playlist.cover_url,
        truncated: playlist.truncated,
        entries,
        loaded_at: now_ms(),
        rekordbox_id: saved.and_then(|p| p.rekordbox_id),
        removed,
    };
    store.put(app, result.clone());
    Ok(result)
}

/// The tracks still to be taken out of the rekordbox playlist: what has
/// gone since the last read (`known` is what was there then), with what
/// was waiting already, less anything that is back. A list cut off at
/// Spotify's limit may have pushed tracks out of sight, and an empty one is
/// more likely a bad read than an emptied playlist: neither removes anything.
fn gone(waiting: &[PlaylistEntry], known: &[PlaylistEntry], entries: &[PlaylistEntry], truncated: bool) -> Vec<PlaylistEntry> {
    let lost = if truncated || entries.is_empty() { &[] } else { known };
    let mut removed: Vec<PlaylistEntry> = Vec::new();
    for entry in waiting.iter().chain(lost) {
        let back = entries.iter().any(|e| e.track == entry.track);
        if !back && !removed.iter().any(|e| e.track == entry.track) {
            removed.push(entry.clone());
        }
    }
    removed
}

/// A Spotify track that isn't in the collection yet.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MissingEntry {
    /// Its place in the Spotify playlist.
    pub index: usize,
    #[serde(flatten)]
    pub entry: PlaylistEntry,
}

/// A rekordbox playlist that follows a Spotify one, and what it still lacks.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedPlaylist {
    pub spotify_id: String,
    /// The rekordbox playlist, as text.
    pub playlist_id: String,
    pub name: String,
    /// Spotify only lists a playlist's first tracks without signing in.
    pub truncated: bool,
    pub loaded_at: u64,
    /// Tracks on the Spotify playlist.
    pub total: usize,
    /// Those that aren't in the collection yet, in the playlist's order.
    pub missing: Vec<MissingEntry>,
}

/// Links a public Spotify playlist to a rekordbox playlist and brings the
/// two in step (see [`reconcile`]). One that isn't linked yet gets a new
/// rekordbox playlist of the same name under `parent_id` (the top level
/// when None), which needs rekordbox closed. One that is linked is read
/// again, and tracks with no match are searched for again.
pub async fn import(app: &AppHandle, input: &str, parent_id: Option<String>) -> AppResult<LinkedPlaylist> {
    let store = app.state::<PlaylistStore>();
    let _one_at_a_time = store.syncing.lock().await;
    let mut playlist = read(app, input, true).await?;

    let (work_app, work) = (app.clone(), playlist.clone());
    let rekordbox_id = crate::library::blocking(move || {
        let collection = work_app.state::<Collection>();
        // Read first, so tracks added in rekordbox are found.
        collection.reload()?;
        if let Some(id) = work.rekordbox_id.filter(|id| is_playlist(&collection, id) == Some(true)) {
            return Ok(id);
        }
        if rbl_db::is_rekordbox_running() {
            return Err(AppError::Rekordbox(
                "rekordbox is running. Quit it before bringing in a playlist, so the playlist can be made.".into(),
            ));
        }
        collection
            .edit(Edit::CreatePlaylist { name: work.name, parent_id, folder: false })?
            .created_id
            .ok_or_else(|| AppError::Rekordbox("The playlist couldn't be made".into()))
    })
    .await?;

    playlist.rekordbox_id = Some(rekordbox_id.clone());
    store.put(app, playlist.clone());
    reconcile(app, playlist).await?;
    let _ = app.emit("collection-changed", app.state::<crate::ingest::ArrivalQueue>().waiting());
    linked(app, rekordbox_id)
        .await?
        .ok_or_else(|| AppError::Validation("The playlist couldn't be saved".into()))
}

/// Reads every linked playlist again and brings rekordbox in step, every
/// [`SYNC_EVERY`] while the window is open, for the app's lifetime.
pub async fn watch(app: AppHandle) {
    tokio::time::sleep(FIRST_SYNC_AFTER).await;
    loop {
        let open = crate::menu::main_window(&app).is_some_and(|w| w.is_visible().unwrap_or(true));
        if open {
            for playlist in app.state::<PlaylistStore>().linked() {
                if let Err(e) = sync(&app, &playlist).await {
                    warn!("Couldn't sync playlist '{}' with Spotify: {e}", playlist.name);
                }
            }
        }
        tokio::time::sleep(SYNC_EVERY).await;
    }
}

/// One timed read of a linked playlist. Emits `playlist-synced` with the
/// rekordbox playlist's id.
async fn sync(app: &AppHandle, playlist: &PlaylistMatch) -> AppResult<()> {
    let store = app.state::<PlaylistStore>();
    let _one_at_a_time = store.syncing.lock().await;
    // It may have been unlinked while this waited.
    let Some(rekordbox_id) = store.get(&playlist.id).and_then(|p| p.rekordbox_id) else { return Ok(()) };
    let playlist = read(app, &playlist.id, false).await?;
    if reconcile(app, playlist).await? {
        let _ = app.emit("collection-changed", app.state::<crate::ingest::ArrivalQueue>().waiting());
    }
    let _ = app.emit("playlist-synced", rekordbox_id);
    Ok(())
}

/// Whether `id` is a plain playlist in the collection; None when the
/// collection can't be read.
fn is_playlist(collection: &Collection, id: &str) -> Option<bool> {
    let nodes = collection.playlists().ok()?;
    Some(nodes.iter().any(|n| n.id == id && n.kind == PlaylistKind::Playlist))
}

/// What bringing a rekordbox playlist in step with Spotify takes.
struct Plan {
    /// Collection tracks on the Spotify playlist that the rekordbox one lacks.
    add: Vec<String>,
    /// Tracks in the rekordbox playlist that were taken off the Spotify one.
    remove: Vec<String>,
    /// Removed entries whose download may yet land in the playlist.
    awaiting: Vec<PlaylistEntry>,
    /// Sure matches that aren't in the collection and were never fetched.
    fetch: Vec<TrackRequest>,
}

/// Brings a linked rekordbox playlist in step with what was read from
/// Spotify: adds the tracks the collection has, takes out the ones that
/// left the Spotify playlist, and starts fetching sure matches the
/// collection lacks. Changes to rekordbox wait while it is running. A
/// playlist deleted in rekordbox is unlinked. Returns whether the
/// collection changed.
async fn reconcile(app: &AppHandle, playlist: PlaylistMatch) -> AppResult<bool> {
    let Some(rekordbox_id) = playlist.rekordbox_id.clone() else { return Ok(false) };
    let store = app.state::<PlaylistStore>();
    let job_store = app.state::<JobStore>();

    // A download under way for a track that has left Spotify still lands
    // in the collection, but no longer in the playlist.
    for job in job_store.list() {
        let left = job.playlist_id.as_deref() == Some(rekordbox_id.as_str())
            && fetches(&job, &playlist.removed)
            && !fetches(&job, &playlist.entries);
        if left {
            let _ = job_store.update(app, job.id, |j| j.playlist_id = None);
        }
    }

    let jobs = job_store.list();
    let (work_app, work, list_id) = (app.clone(), playlist.clone(), rekordbox_id.clone());
    let outcome = crate::library::blocking(move || {
        let collection = work_app.state::<Collection>();
        match is_playlist(&collection, &list_id) {
            Some(true) => {}
            Some(false) => return Ok(None),
            None => return Err(AppError::Rekordbox("The rekordbox library isn't open".into())),
        }
        let mut plan = plan(&collection, &jobs, &work, &list_id)?;
        let mut changed = false;
        if !(plan.add.is_empty() && plan.remove.is_empty()) && !rbl_db::is_rekordbox_running() {
            if !plan.remove.is_empty() {
                collection.edit(Edit::RemoveTracks { playlist_id: list_id.clone(), track_ids: std::mem::take(&mut plan.remove) })?;
            }
            if !plan.add.is_empty() {
                collection.edit(Edit::AddTracks { playlist_id: list_id.clone(), track_ids: std::mem::take(&mut plan.add) })?;
            }
            changed = true;
        }
        Ok(Some((plan, changed)))
    })
    .await?;

    let Some((plan, changed)) = outcome else {
        info!("Playlist '{}' was deleted in rekordbox; no longer syncing it", playlist.name);
        store.unlink(app, &rekordbox_id);
        return Ok(false);
    };

    // Keep only what is still to leave the playlist.
    let pending: Vec<PlaylistEntry> = if plan.remove.is_empty() {
        plan.awaiting
    } else {
        playlist.removed.clone()
    };
    if pending.len() != playlist.removed.len() {
        store.set_removed(app, &playlist.id, pending);
    }

    for request in plan.fetch {
        info!("Playlist '{}': fetching {} - {}", playlist.name, request.artist, request.title);
        let mut job = Job::new(request, None);
        job.playlist_id = Some(rekordbox_id.clone());
        let id = job.id;
        job_store.insert(app, job);
        if let Err(e) = jobs::start(app, id) {
            warn!("Couldn't start a fetch for playlist '{}': {e}", playlist.name);
        }
    }
    if changed {
        info!("Playlist '{}': rekordbox playlist brought in step with Spotify", playlist.name);
    }
    Ok(changed)
}

/// Works out what a rekordbox playlist needs to match its Spotify one.
fn plan(collection: &Collection, jobs: &[Job], playlist: &PlaylistMatch, rekordbox_id: &str) -> AppResult<Plan> {
    let members: HashSet<String> = collection.playlist_members(rekordbox_id)?.into_iter().collect();
    let found = locate(collection, jobs, &playlist.entries);
    let wanted: HashSet<&String> = found.iter().flatten().collect();

    let mut add: Vec<String> = Vec::new();
    for id in found.iter().flatten() {
        if !members.contains(id) && !add.contains(id) {
            add.push(id.clone());
        }
    }

    let (mut remove, mut awaiting) = (Vec::new(), Vec::new());
    for (entry, at) in playlist.removed.iter().zip(locate(collection, jobs, &playlist.removed)) {
        match at {
            // Unless the same track is on the playlist under another entry.
            Some(id) if members.contains(&id) && !wanted.contains(&id) => remove.push(id),
            Some(_) => {}
            // Not in the collection: only a download can still bring it in.
            None => {
                let may_land = jobs
                    .iter()
                    .any(|job| fetches(job, std::slice::from_ref(entry)) && (!job.status.is_finished() || job.status == JobStatus::Completed));
                if may_land {
                    awaiting.push(entry.clone());
                }
            }
        }
    }

    let fetch = playlist
        .entries
        .iter()
        .zip(&found)
        .filter(|(entry, found)| found.is_none() && entry.confidence == Some(Confidence::High))
        .filter_map(|(entry, _)| entry.request.clone())
        // One that was fetched before, even if it failed or was cancelled,
        // is the user's to retry.
        .filter(|request| !jobs.iter().any(|job| same_track(&job.request, request)))
        .fold(Vec::<TrackRequest>::new(), |mut all, request| {
            if !all.iter().any(|other| same_track(other, &request)) {
                all.push(request);
            }
            all
        });

    Ok(Plan { add, remove, awaiting, fetch })
}

/// Whether a job downloads one of these entries' matches.
fn fetches(job: &Job, entries: &[PlaylistEntry]) -> bool {
    entries.iter().filter_map(|e| e.request.as_ref()).any(|request| same_track(&job.request, request))
}

/// The Spotify playlist behind a rekordbox playlist, if it follows one,
/// with the tracks the collection still lacks.
pub async fn linked(app: &AppHandle, rekordbox_id: String) -> AppResult<Option<LinkedPlaylist>> {
    let Some(playlist) = app.state::<PlaylistStore>().linked_to(&rekordbox_id) else {
        return Ok(None);
    };
    let jobs = app.state::<JobStore>().list();
    let work_app = app.clone();
    crate::library::blocking(move || {
        let found = locate(&work_app.state::<Collection>(), &jobs, &playlist.entries);
        let total = playlist.entries.len();
        let missing = playlist
            .entries
            .into_iter()
            .zip(found)
            .enumerate()
            .filter(|(_, (_, found))| found.is_none())
            .map(|(index, (entry, _))| MissingEntry { index, entry })
            .collect();
        Ok(Some(LinkedPlaylist {
            spotify_id: playlist.id,
            playlist_id: rekordbox_id,
            name: playlist.name,
            truncated: playlist.truncated,
            loaded_at: playlist.loaded_at,
            total,
            missing,
        }))
    })
    .await
}

/// The collection track each entry is, where it's there: found by where its
/// download landed, else by title and artist.
fn locate(collection: &Collection, jobs: &[Job], entries: &[PlaylistEntry]) -> Vec<Option<String>> {
    let names: Vec<(&str, &str)> = entries
        .iter()
        .map(|e| (e.track.title.as_str(), e.track.artists.first().map(String::as_str).unwrap_or_default()))
        .collect();
    let by_name = collection.find_tracks(&names);
    entries
        .iter()
        .zip(by_name)
        .map(|(entry, by_name)| {
            let downloaded = entry.request.as_ref().and_then(|request| {
                jobs.iter()
                    .filter(|job| same_track(&job.request, request))
                    .filter_map(|job| job.output_path.as_deref())
                    .find_map(|path| collection.track_at(path))
            });
            downloaded.or(by_name)
        })
        .collect()
}

/// Whether two search results are the same track (as the frontend's `trackKey`).
fn same_track(a: &TrackRequest, b: &TrackRequest) -> bool {
    match (a.deezer_id, b.deezer_id) {
        (Some(a), Some(b)) => a == b,
        (None, None) => a.mb_recording_id.is_some() && a.mb_recording_id == b.mb_recording_id,
        _ => false,
    }
}

fn hint(track: &SpotifyTrack) -> TrackHint {
    TrackHint {
        title: track.title.clone(),
        artist: track.artists.first().cloned().unwrap_or_default(),
        duration_secs: Some(track.duration_secs),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str) -> PlaylistEntry {
        let track = SpotifyTrack { title: title.into(), artists: vec!["Artist".into()], duration_secs: 200 };
        PlaylistEntry { track, request: None, confidence: None }
    }

    fn titles(entries: &[PlaylistEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.track.title.as_str()).collect()
    }

    #[test]
    fn tracks_taken_off_spotify_are_noted() {
        let known = [entry("a"), entry("b"), entry("c")];
        // "b" went; "x" was waiting to be taken out already.
        let now = [entry("a"), entry("c"), entry("d")];
        assert_eq!(titles(&gone(&[entry("x")], &known, &now, false)), ["x", "b"]);
        // One that is back is no longer to be taken out.
        assert_eq!(titles(&gone(&[entry("c"), entry("x")], &known, &now, false)), ["x", "b"]);
        // Nothing new is taken out on a cut-off or empty read.
        assert_eq!(titles(&gone(&[entry("x")], &known, &now, true)), ["x"]);
        assert_eq!(titles(&gone(&[entry("x")], &known, &[], false)), ["x"]);
        // A first read removes nothing.
        assert!(gone(&[], &[], &now, false).is_empty());
    }

    /// Hits Spotify and Deezer. Run with `cargo test -- --ignored live_playlist --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_playlist_match_rate() {
        let sources = Sources::new();
        let playlist = sources
            .spotify
            .playlist("https://open.spotify.com/playlist/37i9dQZF1DX4UtSsGT1Sbe")
            .await
            .unwrap();
        let mut matched = 0;
        for track in &playlist.tracks {
            match autotag::find_track(&sources, &hint(track)).await.unwrap() {
                Some((request, confidence)) => {
                    matched += 1;
                    println!("{confidence:?}\t{} - {}  =>  {} - {}", track.artists[0], track.title, request.artist, request.title);
                }
                None => println!("NONE\t{} - {} ({}s)", track.artists[0], track.title, track.duration_secs),
            }
        }
        println!("matched {matched}/{}", playlist.tracks.len());
        assert!(matched * 10 >= playlist.tracks.len() * 8, "match rate below 80%");
    }

    /// Tracks missing from Deezer fall back to MusicBrainz. Hits both.
    #[tokio::test]
    #[ignore]
    async fn live_musicbrainz_fallback() {
        let sources = Sources::new();
        for (title, artist, secs) in [("Wave to Anchor", "Hundred Waters", 279), ("Immaterial", "SOPHIE", 233)] {
            let track = SpotifyTrack { title: title.into(), artists: vec![artist.into()], duration_secs: secs };
            let (request, confidence) = autotag::find_track(&sources, &hint(&track)).await.unwrap().expect("no match");
            println!("{confidence:?} {} - {} ({}, {}s)", request.artist, request.title, request.album, request.duration_secs);
            assert!(request.mb_recording_id.is_some());
            let meta = autotag::resolve(&sources, &request).await;
            println!("  tagged as {} / {} / {:?} {}", meta.album_artist, meta.album, meta.track_number, meta.title);
        }
    }
}
