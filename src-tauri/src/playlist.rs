//! rekordbox playlists that follow a public Spotify playlist. Any playlist
//! can be linked to one: it is given the tracks of the Spotify playlist
//! that the collection has, and each other track is matched to a search
//! result (see [`autotag::find_track`]) and, when the match is sure,
//! fetched; medium-confidence matches are left for the user to check.
//!
//! While the window is open, every linked playlist is read again every
//! [`SYNC_EVERY`], and tracks added on Spotify are matched and fetched.
//! Nothing is taken out of the rekordbox playlist unless it is set to
//! mirror the Spotify one: then whatever isn't on Spotify leaves it (and
//! stays in the collection). rekordbox's library can only be changed while
//! rekordbox is closed, so those changes wait until it is.
//!
//! The link between the two playlists, and the matches (including ones the
//! user picked by hand), are saved.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tracing::{info, warn};

use crate::autotag::{self, TrackHint};
use crate::error::{AppError, AppResult};
use crate::jobs::{self, Fetch, JobStore};
use crate::persistence;
use crate::rekordbox::{Collection, Edit, PlaylistKind};
use crate::sources::{Sources, SpotifyTrack};
use crate::state::{now_ms, Confidence, Job, TrackRequest};

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
    /// The search for it failed (no network, a service down), which isn't
    /// finding no match: it's searched for again at the next sync.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unsearched: bool,
}

/// How one rekordbox playlist follows its Spotify playlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncSettings {
    /// Fetch tracks added on Spotify whose match is sure.
    pub auto_fetch: bool,
    /// Keep the rekordbox playlist to exactly the Spotify playlist's
    /// tracks: whatever isn't on Spotify is taken out of it.
    pub mirror: bool,
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self { auto_fetch: true, mirror: false }
    }
}

/// A rekordbox playlist's link to Spotify, for the playlist tree.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Followed {
    pub spotify_id: String,
    /// The Spotify playlist's name.
    pub name: String,
    pub settings: SyncSettings,
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
    #[serde(default)]
    pub settings: SyncSettings,
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

    /// The rekordbox playlists that follow a Spotify playlist, by id.
    pub fn followed(&self) -> HashMap<String, Followed> {
        self.lock()
            .iter()
            .filter_map(|p| {
                let followed = Followed { spotify_id: p.id.clone(), name: p.name.clone(), settings: p.settings };
                Some((p.rekordbox_id.clone()?, followed))
            })
            .collect()
    }

    /// Changes how a rekordbox playlist follows its Spotify playlist.
    pub fn set_settings(&self, app: &AppHandle, rekordbox_id: &str, settings: SyncSettings) -> AppResult<()> {
        let mut playlists = self.lock();
        let playlist = playlists
            .iter_mut()
            .find(|p| p.rekordbox_id.as_deref() == Some(rekordbox_id))
            .ok_or_else(|| AppError::Validation("That playlist doesn't follow a Spotify playlist".into()))?;
        playlist.settings = settings;
        persistence::save_playlists(app, &playlists);
        Ok(())
    }

    /// Makes a rekordbox playlist follow a saved Spotify playlist, in place
    /// of any it followed before. A new link starts with the default
    /// settings, so nothing is removed until that is asked for.
    fn link(&self, app: &AppHandle, id: &str, rekordbox_id: &str) {
        let mut playlists = self.lock();
        for playlist in playlists.iter_mut() {
            let linked_here = playlist.rekordbox_id.as_deref() == Some(rekordbox_id);
            if playlist.id == id {
                if !linked_here {
                    playlist.rekordbox_id = Some(rekordbox_id.to_owned());
                    playlist.settings = SyncSettings::default();
                }
            } else if linked_here {
                playlist.rekordbox_id = None;
            }
        }
        persistence::save_playlists(app, &playlists);
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
        }
        persistence::save_playlists(app, &playlists);
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
/// searches every minute. Emits `playlist-progress` while matching.
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
        if let Some(known) = known.iter().find(|e| e.track == track && !e.unsearched && (e.request.is_some() || !rematch)) {
            entries.push(known.clone());
            reused += 1;
            continue;
        }
        let (request, confidence, unsearched) = match autotag::find_track(&sources, &hint(&track)).await {
            Ok(Some((request, confidence))) => (Some(request), Some(confidence), false),
            Ok(None) => (None, None, false),
            Err(e) => {
                warn!("Couldn't search for {}: {e}", track.title);
                (None, None, true)
            }
        };
        entries.push(PlaylistEntry { track, request, confidence, unsearched });
    }
    let _ = app.emit("playlist-progress", PlaylistProgress { done: total, total });

    // Matching takes a while: the link and settings may have changed since.
    let current = store.get(&playlist.id);
    let matched = entries.iter().filter(|e| e.request.is_some()).count();
    info!("Playlist '{}': matched {matched} of {total} tracks ({reused} from before)", playlist.name);
    let result = PlaylistMatch {
        id: playlist.id,
        name: playlist.name,
        cover_url: playlist.cover_url,
        truncated: playlist.truncated,
        entries,
        loaded_at: now_ms(),
        settings: current.as_ref().map(|p| p.settings).unwrap_or_default(),
        rekordbox_id: current.and_then(|p| p.rekordbox_id),
    };
    store.put(app, result.clone());
    Ok(result)
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

/// Makes a rekordbox playlist follow a public Spotify playlist, and brings
/// it in step (see [`reconcile`]). A playlist that follows it already is
/// read again, and tracks with no match are searched for again.
pub async fn link(app: &AppHandle, input: &str, rekordbox_id: String) -> AppResult<LinkedPlaylist> {
    let store = app.state::<PlaylistStore>();
    let _one_at_a_time = store.syncing.lock().await;
    let mut playlist = read(app, input, true).await?;

    let (work_app, target, other) = (app.clone(), rekordbox_id.clone(), playlist.rekordbox_id.clone());
    crate::library::blocking(move || {
        let collection = work_app.state::<Collection>();
        // Read first, so tracks added in rekordbox are found.
        collection.reload()?;
        if is_playlist(&collection, &target) != Some(true) {
            return Err(AppError::Validation("Only a plain playlist can follow a Spotify playlist".into()));
        }
        // One rekordbox playlist per Spotify playlist: fetched tracks have one place to land.
        let taken = other.filter(|id| *id != target).and_then(|id| {
            let nodes = collection.playlists().ok()?;
            nodes.into_iter().find(|n| n.id == id && n.kind == PlaylistKind::Playlist)
        });
        match taken {
            Some(node) => Err(AppError::Validation(format!(
                "That Spotify playlist is already linked to \"{}\". Unlink it there first.",
                node.name
            ))),
            None => Ok(()),
        }
    })
    .await?;

    store.link(app, &playlist.id, &rekordbox_id);
    let saved = store.get(&playlist.id);
    playlist.rekordbox_id = Some(rekordbox_id.clone());
    playlist.settings = saved.map(|p| p.settings).unwrap_or_default();
    reconcile(app, playlist).await?;
    let _ = app.emit("collection-changed", app.state::<crate::ingest::ArrivalQueue>().waiting());
    linked(app, rekordbox_id)
        .await?
        .ok_or_else(|| AppError::Validation("The playlist couldn't be linked".into()))
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

/// Reads one linked playlist again now, after its settings changed.
pub async fn sync_linked(app: AppHandle, rekordbox_id: String) {
    let Some(playlist) = app.state::<PlaylistStore>().linked_to(&rekordbox_id) else { return };
    if let Err(e) = sync(&app, &playlist).await {
        warn!("Couldn't sync playlist '{}' with Spotify: {e}", playlist.name);
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
    /// Tracks in a mirrored rekordbox playlist that aren't on the Spotify one.
    remove: Vec<String>,
    /// Sure matches that aren't in the collection and were never fetched.
    fetch: Vec<TrackRequest>,
}

/// Brings a linked rekordbox playlist in step with what was read from
/// Spotify: adds the tracks the collection has, starts fetching sure
/// matches it lacks, and, for a mirrored playlist, takes out what isn't on
/// Spotify. Changes to rekordbox wait while it is running. A playlist
/// deleted in rekordbox is unlinked. Returns whether the collection changed.
async fn reconcile(app: &AppHandle, playlist: PlaylistMatch) -> AppResult<bool> {
    let Some(rekordbox_id) = playlist.rekordbox_id.clone() else { return Ok(false) };
    let store = app.state::<PlaylistStore>();

    let jobs = app.state::<JobStore>().fetches();
    let (work_app, work, list_id) = (app.clone(), playlist.clone(), rekordbox_id.clone());
    let outcome = crate::library::blocking(move || {
        let collection = work_app.state::<Collection>();
        match is_playlist(&collection, &list_id) {
            Some(true) => {}
            Some(false) => return Ok(None),
            None => return Err(AppError::Rekordbox("The rekordbox library isn't open".into())),
        }
        let members = collection.playlist_members(&list_id)?;
        let found = locate(&collection, &jobs, &work.entries);
        let plan = plan(&work, &members, &found, &jobs);
        let mut changed = false;
        if !(plan.add.is_empty() && plan.remove.is_empty()) && !crate::rekordbox::is_running() {
            if !plan.remove.is_empty() {
                info!("Playlist '{}': taking out {} tracks that aren't on Spotify", work.name, plan.remove.len());
                collection.edit_unasked(Edit::RemoveTracks { playlist_id: list_id.clone(), track_ids: plan.remove })?;
            }
            if !plan.add.is_empty() {
                collection.edit_unasked(Edit::AddTracks { playlist_id: list_id.clone(), track_ids: plan.add })?;
            }
            changed = true;
        }
        Ok(Some((plan.fetch, changed)))
    })
    .await?;

    let Some((fetch, changed)) = outcome else {
        info!("Playlist '{}' was deleted in rekordbox; no longer syncing it", playlist.name);
        store.unlink(app, &rekordbox_id);
        return Ok(false);
    };

    start_fetches(app, &playlist.name, &rekordbox_id, fetch);
    Ok(changed)
}

/// Starts fetching tracks that land in a rekordbox playlist.
fn start_fetches(app: &AppHandle, name: &str, rekordbox_id: &str, fetch: Vec<TrackRequest>) {
    let job_store = app.state::<JobStore>();
    for request in fetch {
        info!("Playlist '{name}': fetching {} - {}", request.artist, request.title);
        let mut job = Job::new(request, None);
        job.playlist_id = Some(rekordbox_id.to_owned());
        let id = job.id;
        job_store.insert(app, job);
        if let Err(e) = jobs::start(app, id) {
            warn!("Couldn't start a fetch for playlist '{name}': {e}");
        }
    }
}

/// What importing a Spotify playlist once did.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    /// The Spotify playlist's name.
    pub name: String,
    /// Spotify only lists a playlist's first tracks without signing in.
    pub truncated: bool,
    /// Tracks on the Spotify playlist.
    pub total: usize,
    /// Collection tracks put in the rekordbox playlist.
    pub added: usize,
    /// Those the rekordbox playlist had already.
    pub present: usize,
    /// Sure matches now being fetched; they land in the playlist.
    pub fetching: usize,
    /// The rest: no sure match, or fetched before.
    pub skipped: usize,
}

/// Brings a public Spotify playlist's tracks into a rekordbox playlist
/// once, without following it: the tracks the collection has are added,
/// and sure matches it lacks are fetched. Nothing is taken out, and the
/// Spotify playlist isn't read again. It may be one that another playlist
/// follows.
pub async fn import(app: &AppHandle, input: &str, rekordbox_id: String) -> AppResult<Imported> {
    let store = app.state::<PlaylistStore>();
    let _one_at_a_time = store.syncing.lock().await;
    // Nothing reads Spotify again later, so the tracks can't wait for rekordbox to close.
    let running = || AppError::Rekordbox("rekordbox is running. Quit it before importing.".into());
    if crate::rekordbox::is_running() {
        return Err(running());
    }
    if store.linked_to(&rekordbox_id).is_some() {
        return Err(AppError::Validation("This playlist follows a Spotify playlist. Unlink it first.".into()));
    }
    let playlist = read(app, input, true).await?;
    // Add only, whatever the settings of a playlist that follows it.
    let work = PlaylistMatch { settings: SyncSettings::default(), ..playlist };

    let jobs = app.state::<JobStore>().fetches();
    let (work_app, list_id) = (app.clone(), rekordbox_id.clone());
    let (imported, fetch) = crate::library::blocking(move || {
        let collection = work_app.state::<Collection>();
        // Read first, so tracks added in rekordbox are found.
        collection.reload()?;
        if is_playlist(&collection, &list_id) != Some(true) {
            return Err(AppError::Validation("Tracks can only be imported into a plain playlist".into()));
        }
        let members = collection.playlist_members(&list_id)?;
        let found = locate(&collection, &jobs, &work.entries);
        let plan = plan(&work, &members, &found, &jobs);
        let in_collection: HashSet<&String> = found.iter().flatten().collect();
        let (added, fetching) = (plan.add.len(), plan.fetch.len());
        let missing = found.iter().filter(|f| f.is_none()).count();
        let imported = Imported {
            name: work.name.clone(),
            truncated: work.truncated,
            total: work.entries.len(),
            added,
            present: in_collection.len() - added,
            fetching,
            skipped: missing - fetching,
        };
        if !plan.add.is_empty() {
            if crate::rekordbox::is_running() {
                return Err(running());
            }
            collection.edit_unasked(Edit::AddTracks { playlist_id: list_id, track_ids: plan.add })?;
        }
        Ok((imported, plan.fetch))
    })
    .await?;

    info!(
        "Playlist '{}' imported once: {} added, {} fetching, {} skipped",
        imported.name, imported.added, imported.fetching, imported.skipped
    );
    start_fetches(app, &imported.name, &rekordbox_id, fetch);
    if imported.added > 0 {
        let _ = app.emit("collection-changed", app.state::<crate::ingest::ArrivalQueue>().waiting());
    }
    Ok(imported)
}

/// Works out what a rekordbox playlist needs to match its Spotify one.
/// `members` are the tracks in it, and `found` the collection track each
/// Spotify entry is, where the collection has it.
fn plan(playlist: &PlaylistMatch, members: &[String], found: &[Option<String>], jobs: &[Fetch]) -> Plan {
    let wanted: HashSet<&String> = found.iter().flatten().collect();

    let mut add: Vec<String> = Vec::new();
    for id in found.iter().flatten() {
        if !members.contains(id) && !add.contains(id) {
            add.push(id.clone());
        }
    }

    // A list cut off at Spotify's limit doesn't say what is past it, and an
    // empty one is more likely a bad read than an emptied playlist: neither
    // takes anything out.
    let whole = !playlist.truncated && !playlist.entries.is_empty();
    let remove = if playlist.settings.mirror && whole {
        members.iter().filter(|id| !wanted.contains(id)).cloned().collect()
    } else {
        Vec::new()
    };

    let mut fetch: Vec<TrackRequest> = Vec::new();
    if playlist.settings.auto_fetch {
        for (entry, found) in playlist.entries.iter().zip(found) {
            let Some(request) = entry.request.as_ref() else { continue };
            // One that was fetched before, even if it failed or was
            // cancelled, is the user's to retry.
            let fetched = jobs.iter().any(|job| same_track(&job.request, request));
            let queued = fetch.iter().any(|other| same_track(other, request));
            if found.is_none() && entry.confidence == Some(Confidence::High) && !fetched && !queued {
                fetch.push(request.clone());
            }
        }
    }

    Plan { add, remove, fetch }
}

/// The Spotify playlist behind a rekordbox playlist, if it follows one,
/// with the tracks the collection still lacks.
pub async fn linked(app: &AppHandle, rekordbox_id: String) -> AppResult<Option<LinkedPlaylist>> {
    let Some(playlist) = app.state::<PlaylistStore>().linked_to(&rekordbox_id) else {
        return Ok(None);
    };
    let jobs = app.state::<JobStore>().fetches();
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
fn locate(collection: &Collection, jobs: &[Fetch], entries: &[PlaylistEntry]) -> Vec<Option<String>> {
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

    fn request(deezer_id: u64) -> TrackRequest {
        serde_json::from_value(serde_json::json!({
            "deezerId": deezer_id, "title": "t", "artist": "a", "album": "", "durationSecs": 200
        }))
        .unwrap()
    }

    fn playlist(sure: &[u64], settings: SyncSettings) -> PlaylistMatch {
        let entries = sure
            .iter()
            .map(|&id| PlaylistEntry {
                track: SpotifyTrack { title: id.to_string(), artists: vec!["a".into()], duration_secs: 200 },
                request: Some(request(id)),
                confidence: Some(Confidence::High),
                unsearched: false,
            })
            .collect();
        PlaylistMatch {
            id: "spotify".into(),
            name: "Mix".into(),
            cover_url: None,
            truncated: false,
            entries,
            loaded_at: 0,
            rekordbox_id: Some("1".into()),
            settings,
        }
    }

    fn ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn a_followed_playlist_gains_tracks_and_keeps_its_own() {
        // Entries 1 and 2 are in the collection as tracks A and B; 3 isn't.
        let list = playlist(&[1, 2, 3], SyncSettings::default());
        let found = [Some("A".to_string()), Some("B".to_string()), None];
        let plan = plan(&list, &ids(&["A", "mine"]), &found, &[]);
        assert_eq!(plan.add, ids(&["B"]));
        assert!(plan.remove.is_empty(), "nothing leaves unless mirrored");
        assert_eq!(plan.fetch.iter().map(|r| r.deezer_id).collect::<Vec<_>>(), [Some(3)]);

        // Fetched before: not started again. And not at all with fetching off.
        let job = Fetch { request: request(3), output_path: None };
        assert!(super::plan(&list, &[], &found, &[job]).fetch.is_empty());
        let manual = playlist(&[1, 2, 3], SyncSettings { auto_fetch: false, mirror: false });
        assert!(super::plan(&manual, &[], &found, &[]).fetch.is_empty());
    }

    #[test]
    fn a_mirrored_playlist_loses_what_is_not_on_spotify() {
        let mirror = SyncSettings { auto_fetch: true, mirror: true };
        let found = [Some("A".to_string()), None];
        let members = ids(&["A", "mine", "gone"]);
        assert_eq!(plan(&playlist(&[1, 2], mirror), &members, &found, &[]).remove, ids(&["mine", "gone"]));

        // Not when the read was cut off or came back empty.
        let mut cut_off = playlist(&[1, 2], mirror);
        cut_off.truncated = true;
        assert!(plan(&cut_off, &members, &found, &[]).remove.is_empty());
        assert!(plan(&playlist(&[], mirror), &members, &[], &[]).remove.is_empty());
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
