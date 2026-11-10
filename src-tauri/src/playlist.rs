//! Bulk finding from a public Spotify playlist: match each entry to a search
//! result (see [`autotag::find_track`]), which the user can then queue as
//! download jobs. Medium-confidence matches are left for the user to check.
//!
//! Matches are cached per playlist, including ones the user picked by hand.
//! Reloading a playlist re-reads its track list and only matches new or
//! unmatched tracks.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tracing::{info, warn};

use crate::autotag::{self, TrackHint};
use crate::error::{AppError, AppResult};
use crate::jobs::JobStore;
use crate::persistence;
use crate::rekordbox::Collection;
use crate::sources::{Sources, SpotifyTrack};
use crate::state::{now_ms, Confidence, TrackRequest};

/// Playlists kept in the cache, most recently loaded first.
const MAX_CACHED: usize = 20;

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
}

/// A cached playlist, for the recent playlists list.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistSummary {
    pub id: String,
    pub name: String,
    pub cover_url: Option<String>,
    pub tracks: usize,
    pub matched: usize,
    pub loaded_at: u64,
}

impl From<&PlaylistMatch> for PlaylistSummary {
    fn from(p: &PlaylistMatch) -> Self {
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            cover_url: p.cover_url.clone(),
            tracks: p.entries.len(),
            matched: p.entries.iter().filter(|e| e.request.is_some()).count(),
            loaded_at: p.loaded_at,
        }
    }
}

/// Cached playlist matches, persisted on every change.
pub struct PlaylistStore(Mutex<Vec<PlaylistMatch>>);

impl PlaylistStore {
    pub fn load(app: &AppHandle) -> Self {
        Self(Mutex::new(persistence::load_playlists(app)))
    }

    pub fn list(&self) -> Vec<PlaylistSummary> {
        self.0.lock().expect("playlists lock poisoned").iter().map(PlaylistSummary::from).collect()
    }

    pub fn get(&self, id: &str) -> AppResult<PlaylistMatch> {
        self.0
            .lock()
            .expect("playlists lock poisoned")
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or_else(|| AppError::Validation("That playlist is no longer saved".into()))
    }

    /// Saves a playlist as the most recent.
    fn put(&self, app: &AppHandle, playlist: PlaylistMatch) {
        let mut playlists = self.0.lock().expect("playlists lock poisoned");
        playlists.retain(|p| p.id != playlist.id);
        playlists.insert(0, playlist);
        playlists.truncate(MAX_CACHED);
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

    pub fn remove(&self, app: &AppHandle, id: &str) {
        let mut playlists = self.0.lock().expect("playlists lock poisoned");
        playlists.retain(|p| p.id != id);
        persistence::save_playlists(app, &playlists);
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlaylistProgress {
    done: usize,
    total: usize,
}

/// Reads a public playlist and matches its tracks, reusing cached matches for
/// tracks seen before. Emits `playlist-progress`.
pub async fn load(app: &AppHandle, input: &str) -> AppResult<PlaylistMatch> {
    let sources = app.state::<Sources>();
    let store = app.state::<PlaylistStore>();
    let playlist = sources.spotify.playlist(input).await?;
    let cached = store.get(&playlist.id).map(|p| p.entries).unwrap_or_default();
    let total = playlist.tracks.len();

    let mut entries = Vec::with_capacity(total);
    let mut reused = 0;
    for (done, track) in playlist.tracks.into_iter().enumerate() {
        let _ = app.emit("playlist-progress", PlaylistProgress { done, total });
        // Unmatched tracks are tried again: sources and matching improve.
        if let Some(known) = cached.iter().find(|e| e.track == track && e.request.is_some()) {
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
    info!("Playlist '{}': matched {matched} of {total} tracks ({reused} from cache)", playlist.name);
    let result = PlaylistMatch {
        id: playlist.id,
        name: playlist.name,
        cover_url: playlist.cover_url,
        truncated: playlist.truncated,
        entries,
        loaded_at: now_ms(),
    };
    store.put(app, result.clone());
    Ok(result)
}

/// What copying a playlist into rekordbox came to.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistSync {
    /// The rekordbox playlist, as text.
    pub playlist_id: String,
    pub name: String,
    /// Tracks of the playlist that are in the collection, and so in it.
    pub present: usize,
    /// Tracks that aren't in the collection yet.
    pub missing: usize,
}

/// Puts the playlist's tracks that are in the collection into a rekordbox
/// playlist of the same name, made if there isn't one, in the playlist's
/// order. Tracks already in it stay put; nothing is removed.
pub async fn sync_to_rekordbox(app: &AppHandle, id: &str) -> AppResult<PlaylistSync> {
    let playlist = app.state::<PlaylistStore>().get(id)?;
    let jobs = app.state::<JobStore>().list();
    let name = playlist.name.clone();
    let work_app = app.clone();
    crate::library::blocking(move || {
        let collection = work_app.state::<Collection>();
        // Read first: a track is found by where its download landed, else by name.
        collection.reload()?;
        let total = playlist.entries.len();
        let track_ids: Vec<String> = playlist
            .entries
            .iter()
            .filter_map(|entry| {
                let downloaded = entry.request.as_ref().and_then(|request| {
                    jobs.iter()
                        .filter(|job| same_track(&job.request, request))
                        .filter_map(|job| job.output_path.as_deref())
                        .find_map(|path| collection.track_at(path))
                });
                downloaded.or_else(|| {
                    let artist = entry.track.artists.first().map(String::as_str).unwrap_or_default();
                    collection.find_track(&entry.track.title, artist)
                })
            })
            .collect();
        let playlist_id = collection.add_to_named_playlist(&playlist.name, &track_ids)?;
        Ok(PlaylistSync { playlist_id, name: playlist.name, present: track_ids.len(), missing: total - track_ids.len() })
    })
    .await
    .inspect(|sync| {
        info!("Playlist '{name}' → rekordbox: {} tracks, {} not in the collection", sync.present, sync.missing);
        let _ = app.emit("collection-changed", app.state::<crate::ingest::ArrivalQueue>().waiting());
    })
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
