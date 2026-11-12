//! Identifying collection tracks: read their files (quick, local), then
//! look them up album by album (see [`super::albums`]), falling back to one
//! by one.

use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use tracing::{info, warn};
use uuid::Uuid;

use super::{albums, LibraryStore};
use crate::autotag::{self, AlbumCandidate};
use crate::error::{AppError, AppResult};
use crate::jobs::JobStore;
use crate::library;
use crate::rekordbox::Collection;
use crate::sources::Sources;
use crate::state::{
    AlbumProposal, Confidence, ExistingTags, ItemStatus, LibraryItem, LibraryPhase, Proposal, Settings,
    SettingsState, TrackMetadata, TrackRequest,
};

/// Looks the given collection tracks up and proposes tags and a path for
/// each. Takes about a second a file: MusicBrainz allows one request a second.
pub fn identify(app: &AppHandle, track_ids: Vec<String>) -> AppResult<()> {
    let collection = app.state::<Collection>();
    let files: Vec<(String, PathBuf)> = track_ids
        .into_iter()
        .filter_map(|id| collection.path_of(&id).map(|path| (id, path)))
        .collect();
    if files.is_empty() {
        return Err(AppError::Validation("Those tracks are no longer in the collection".into()));
    }
    let task_app = app.clone();
    app.state::<LibraryStore>().spawn(app, async move {
        match read_files(&task_app, files).await {
            Ok(items) => identify_items(&task_app, items).await,
            Err(e) => warn!("Couldn't read the tracks to identify: {e}"),
        }
    })
}

/// The genres MusicBrainz has for a collection track, the likeliest first.
/// The recording is the one given (what identifying found), or is looked up
/// by the file's tags and name. Nothing is kept or changed.
pub async fn genres(app: &AppHandle, track_id: String, recording_id: Option<String>) -> AppResult<Vec<String>> {
    let sources = app.state::<Sources>();
    if let Some(id) = recording_id {
        return sources.musicbrainz.genres(&id).await;
    }
    let Some(path) = app.state::<Collection>().path_of(&track_id) else {
        return Err(AppError::Validation("That track is no longer in the collection".into()));
    };
    let settings = app.state::<SettingsState>().snapshot();
    let reading = path.clone();
    let scanned = library::blocking(move || library::read_file(&reading)).await?;
    let relative = path.strip_prefix(settings.library_root_for(&path)).unwrap_or(&path);
    autotag::genres(&sources, &scanned.tags, relative, scanned.duration_secs, None).await
}

/// Reads the tracks' files into the store, ready to be identified.
async fn read_files(app: &AppHandle, files: Vec<(String, PathBuf)>) -> AppResult<Vec<LibraryItem>> {
    let store = app.state::<LibraryStore>();
    let settings = app.state::<SettingsState>().snapshot();
    store.set_progress(app, LibraryPhase::Scanning, 0, files.len());
    let mut items = library::blocking(move || {
        Ok(files
            .into_iter()
            .map(|(track_id, path)| read_item(settings.library_root_for(&path), track_id, path))
            .collect::<Vec<_>>())
    })
    .await?;
    store.put_items(app, &mut items);
    info!("Identifying {} collection tracks", items.len());
    Ok(items.into_iter().filter(|i| i.status == ItemStatus::Pending).collect())
}

/// Identifies files album by album, falling back to one by one. Downloads
/// are identified from the search result the user picked.
async fn identify_items(app: &AppHandle, pending: Vec<LibraryItem>) {
    let store = app.state::<LibraryStore>();
    let settings = app.state::<SettingsState>().snapshot();
    let jobs = app.state::<JobStore>();
    let (downloads, others): (Vec<LibraryItem>, Vec<LibraryItem>) =
        pending.into_iter().partition(|item| jobs.find_by_output(&item.path).is_some());
    let (albums, mut singles) = albums::group(others);
    singles.extend(downloads);

    let total = albums.iter().map(Vec::len).sum::<usize>() + singles.len();
    let mut done = 0;
    let layout = Layout { settings: &settings };
    for group in albums {
        store.set_progress(app, LibraryPhase::Matching, done, total);
        match albums::identify(app, &group, layout.root_for(&group[0].path)).await {
            Ok(Some(candidate)) => {
                let album = album_proposal(&candidate);
                for (item, track) in group.into_iter().zip(candidate.found.assignment) {
                    match track {
                        Some(track) => {
                            let meta = candidate.release.tracks[track].metadata.clone();
                            let proposal =
                                propose(app, &item, layout, meta, candidate.confidence, None, Some(album.clone())).await;
                            finish_item(app, &item, Ok(Some(proposal)));
                            done += 1;
                        }
                        None => singles.push(item),
                    }
                }
            }
            Ok(None) => singles.extend(group),
            Err(e) => {
                warn!("Album matching failed: {e}");
                singles.extend(group);
            }
        }
    }
    for item in singles {
        store.set_progress(app, LibraryPhase::Matching, done, total);
        let result = identify_one(app, &item, layout).await;
        finish_item(app, &item, result);
        done += 1;
    }
}

/// A file's size and modified time (ms since the epoch).
pub(super) fn file_stamp(path: &Path) -> (u64, Option<u64>) {
    let Ok(info) = std::fs::metadata(path) else { return (0, None) };
    let modified = info
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64);
    (info.len(), modified)
}

/// Reads a track's file as an item waiting to be identified.
pub(super) fn read_item(root: &Path, track_id: String, path: PathBuf) -> LibraryItem {
    let relative_path = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();
    let (size, modified) = file_stamp(&path);
    let mut item = LibraryItem {
        id: Uuid::new_v4(),
        track_id,
        relative_path,
        format: library::extension(&path).to_uppercase(),
        lossless: false,
        duration_secs: None,
        bit_depth: None,
        sample_rate: None,
        bitrate: None,
        size,
        modified,
        tags: ExistingTags::default(),
        proposal: None,
        status: ItemStatus::Pending,
        error: None,
        path,
    };
    match library::read_file(&item.path) {
        Ok(scanned) => {
            item.format = scanned.format;
            item.lossless = scanned.lossless;
            item.duration_secs = scanned.duration_secs;
            item.bit_depth = scanned.bit_depth;
            item.sample_rate = scanned.sample_rate;
            item.bitrate = scanned.bitrate;
            item.tags = scanned.tags;
        }
        Err(e) => {
            item.status = ItemStatus::Failed;
            item.error = Some(e.to_string());
        }
    }
    item
}

/// Identifies a file on its own: from the download that produced it, or by
/// its tags and name.
async fn identify_one(app: &AppHandle, item: &LibraryItem, layout: Layout<'_>) -> AppResult<Option<Proposal>> {
    let sources = app.state::<Sources>();

    // A download's original search result says which album the user picked,
    // which the file's tags may have lost.
    if let Some(job) = app.state::<JobStore>().find_by_output(&item.path) {
        let meta = autotag::resolve(&sources, &job.request).await;
        return Ok(Some(propose(app, item, layout, meta, Confidence::High, Some(job.request), None).await));
    }

    let relative = item.path.strip_prefix(layout.root_for(&item.path)).unwrap_or(&item.path);
    let Some(found) = autotag::identify_file(&sources, &item.tags, relative, item.duration_secs).await? else {
        return Ok(None);
    };
    Ok(Some(propose(app, item, layout, found.metadata, found.confidence, found.request, None).await))
}

/// A proposal to tag `item` with `meta` and file it by the template. Lossy
/// files get a search result attached so they can be upgraded.
async fn propose(
    app: &AppHandle,
    item: &LibraryItem,
    layout: Layout<'_>,
    meta: TrackMetadata,
    confidence: Confidence,
    request: Option<TrackRequest>,
    album: Option<AlbumProposal>,
) -> Proposal {
    let request = match request {
        None if !item.lossless => autotag::lossless_source(&app.state::<Sources>(), &meta).await,
        request => request,
    };
    Proposal { target_path: layout.target(&item.path, &meta), metadata: meta, confidence, request, album }
}

fn album_proposal(candidate: &AlbumCandidate) -> AlbumProposal {
    let summary = &candidate.release.summary;
    AlbumProposal {
        release_id: summary.id.clone(),
        title: summary.title.clone(),
        artist: summary.artist.clone(),
        date: summary.date.clone(),
        matched: candidate.found.matched(),
        tracks: candidate.release.tracks.len(),
        distance: candidate.found.distance,
    }
}

/// The library folders and their path templates.
#[derive(Clone, Copy)]
struct Layout<'a> {
    settings: &'a Settings,
}

impl<'a> Layout<'a> {
    /// The folder a file's path is read relative to.
    fn root_for(&self, path: &Path) -> &'a Path {
        self.settings.library_root_for(path)
    }

    /// Where the template puts a file with `meta`, keeping its extension
    /// and the library folder it's in. Files outside every library folder
    /// stay where they are: only their tags are ours to fix.
    fn target(&self, path: &Path, meta: &TrackMetadata) -> PathBuf {
        let Some(root) = self.settings.library_root_of(path) else {
            return path.to_path_buf();
        };
        library::library_path(root, &library::render(&self.settings.paths, meta), &library::extension(path))
    }
}

/// Whether writing `meta` would leave the file's main tags as they are.
fn same_tags(tags: &ExistingTags, meta: &TrackMetadata) -> bool {
    let proposed = ExistingTags::from(meta);
    tags.title == proposed.title
        && tags.artist == proposed.artist
        && tags.album == proposed.album
        && tags.album_artist == proposed.album_artist
        && tags.track_number == proposed.track_number
        && tags.disc_number == proposed.disc_number
        && tags.mb_recording_id == proposed.mb_recording_id
}

/// Records an identification result on a library item.
fn finish_item(app: &AppHandle, item: &LibraryItem, result: AppResult<Option<Proposal>>) {
    let (proposal, status, error) = match result {
        // Files already where they belong, with MusicBrainz tags or with
        // exactly the tags the match would write (matches MusicBrainz
        // doesn't know have no MusicBrainz ID).
        Ok(Some(p))
            if p.target_path == item.path
                && (item.tags.mb_recording_id.is_some() || same_tags(&item.tags, &p.metadata)) =>
        {
            (None, ItemStatus::Organized, None)
        }
        Ok(Some(p)) => (Some(p), ItemStatus::Matched, None),
        Ok(None) => (None, ItemStatus::Unmatched, None),
        Err(e) => (None, ItemStatus::Failed, Some(e.to_string())),
    };
    app.state::<LibraryStore>().update_item(app, item.id, |i| {
        i.proposal = proposal;
        i.status = status;
        i.error = error;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_already_tagged_with_the_match_count_as_unchanged() {
        let meta = TrackMetadata {
            title: "Pop 4".into(),
            artist: "GAS".into(),
            album: "Pop".into(),
            album_artist: "GAS".into(),
            track_number: Some(4),
            disc_number: Some(1),
            ..Default::default()
        };
        let mut tags = ExistingTags::from(&meta);
        tags.date = Some("2016-10-28".into());
        assert!(same_tags(&tags, &meta));
        tags.artist = Some("Gas".into());
        assert!(!same_tags(&tags, &meta));
    }
}
