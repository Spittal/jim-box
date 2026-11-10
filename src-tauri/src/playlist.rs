//! Spotify playlists brought into rekordbox: a public playlist becomes a
//! rekordbox playlist holding the tracks the collection already has. Each
//! other entry is matched to a search result (see [`autotag::find_track`])
//! so it can be fetched; medium-confidence matches are left for the user to
//! check.
//!
//! The link between the two playlists, and the matches (including ones the
//! user picked by hand), are saved. Importing a playlist again re-reads its
//! track list and only matches new or unmatched tracks.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tracing::{info, warn};

use crate::autotag::{self, TrackHint};
use crate::error::{AppError, AppResult};
use crate::jobs::JobStore;
use crate::persistence;
use crate::rekordbox::{Collection, Edit, PlaylistKind};
use crate::sources::{Sources, SpotifyTrack};
use crate::state::{now_ms, Confidence, Job, TrackRequest};

/// Playlists remembered, most recently imported first.
const MAX_SAVED: usize = 200;

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
    /// The rekordbox playlist this one was brought into.
    #[serde(default)]
    pub rekordbox_id: Option<String>,
}

/// Saved Spotify playlists, persisted on every change.
pub struct PlaylistStore(Mutex<Vec<PlaylistMatch>>);

impl PlaylistStore {
    pub fn load(app: &AppHandle) -> Self {
        Self(Mutex::new(persistence::load_playlists(app)))
    }

    fn get(&self, id: &str) -> Option<PlaylistMatch> {
        self.0.lock().expect("playlists lock poisoned").iter().find(|p| p.id == id).cloned()
    }

    /// The Spotify playlist brought into this rekordbox playlist, if any.
    fn linked_to(&self, rekordbox_id: &str) -> Option<PlaylistMatch> {
        self.0
            .lock()
            .expect("playlists lock poisoned")
            .iter()
            .find(|p| p.rekordbox_id.as_deref() == Some(rekordbox_id))
            .cloned()
    }

    /// Saves a playlist as the most recent.
    fn put(&self, app: &AppHandle, playlist: PlaylistMatch) {
        let mut playlists = self.0.lock().expect("playlists lock poisoned");
        playlists.retain(|p| p.id != playlist.id);
        playlists.insert(0, playlist);
        playlists.truncate(MAX_SAVED);
        persistence::save_playlists(app, &playlists);
    }

    /// Records a match the user picked for one track.
    pub fn set_match(&self, app: &AppHandle, id: &str, index: usize, request: TrackRequest) -> AppResult<()> {
        let mut playlists = self.0.lock().expect("playlists lock poisoned");
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

/// Reads a public playlist and matches its tracks, reusing saved matches for
/// tracks seen before. Emits `playlist-progress`.
async fn read(app: &AppHandle, input: &str) -> AppResult<PlaylistMatch> {
    let sources = app.state::<Sources>();
    let store = app.state::<PlaylistStore>();
    let playlist = sources.spotify.playlist(input).await?;
    let saved = store.get(&playlist.id);
    let known = saved.as_ref().map(|p| p.entries.as_slice()).unwrap_or_default();
    let total = playlist.tracks.len();

    let mut entries = Vec::with_capacity(total);
    let mut reused = 0;
    for (done, track) in playlist.tracks.into_iter().enumerate() {
        let _ = app.emit("playlist-progress", PlaylistProgress { done, total });
        // Unmatched tracks are tried again: sources and matching improve.
        if let Some(known) = known.iter().find(|e| e.track == track && e.request.is_some()) {
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

    let matched = entries.iter().filter(|e| e.request.is_some()).count();
    info!("Playlist '{}': matched {matched} of {total} tracks ({reused} from before)", playlist.name);
    let result = PlaylistMatch {
        id: playlist.id,
        name: playlist.name,
        cover_url: playlist.cover_url,
        truncated: playlist.truncated,
        entries,
        loaded_at: now_ms(),
        rekordbox_id: saved.and_then(|p| p.rekordbox_id),
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

/// A rekordbox playlist that came from Spotify, and what it still lacks.
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

/// Brings a public Spotify playlist into rekordbox: a playlist of the same
/// name, made if this one hasn't been brought in before, given the tracks
/// that are in the collection, in the playlist's order. Tracks already in it
/// stay put; nothing is removed. Refused while rekordbox is open.
pub async fn import(app: &AppHandle, input: &str) -> AppResult<LinkedPlaylist> {
    if rbl_db::is_rekordbox_running() {
        return Err(AppError::Rekordbox(
            "rekordbox is running. Quit it before bringing in a playlist, so the playlist can be made.".into(),
        ));
    }
    let mut playlist = read(app, input).await?;
    let jobs = app.state::<JobStore>().list();
    let (work_app, work) = (app.clone(), playlist.clone());
    let (rekordbox_id, present) = crate::library::blocking(move || {
        let collection = work_app.state::<Collection>();
        // Read first: a track is found by where its download landed, else by name.
        collection.reload()?;
        let track_ids: Vec<String> = locate(&collection, &jobs, &work.entries).into_iter().flatten().collect();
        let still_there = work.rekordbox_id.filter(|id| {
            collection.playlists().is_ok_and(|nodes| nodes.iter().any(|n| n.id == *id && n.kind == PlaylistKind::Playlist))
        });
        let id = match still_there {
            Some(id) => {
                collection.edit(Edit::AddTracks { playlist_id: id.clone(), track_ids: track_ids.clone() })?;
                id
            }
            None => collection.add_to_named_playlist(&work.name, &track_ids)?,
        };
        Ok((id, track_ids.len()))
    })
    .await?;

    info!("Playlist '{}' → rekordbox: {present} of {} tracks are in the collection", playlist.name, playlist.entries.len());
    playlist.rekordbox_id = Some(rekordbox_id.clone());
    app.state::<PlaylistStore>().put(app, playlist);
    let _ = app.emit("collection-changed", app.state::<crate::ingest::ArrivalQueue>().waiting());
    linked(app, rekordbox_id)
        .await?
        .ok_or_else(|| AppError::Validation("The playlist couldn't be saved".into()))
}

/// The Spotify playlist behind a rekordbox playlist, if it came from one,
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
