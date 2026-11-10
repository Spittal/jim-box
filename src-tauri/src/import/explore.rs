//! Actions on single collection tracks: showing their artwork and
//! downloading a fresh copy.

use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

use super::LibraryStore;
use crate::autotag;
use crate::error::{AppError, AppResult};
use crate::jobs::{self, JobStore};
use crate::library;
use crate::rekordbox::Collection;
use crate::sources::Sources;
use crate::state::{Job, LibraryItem, SettingsState, TrackRequest};

/// Image files next to the audio that players use as album art.
const FOLDER_ART: [&str; 6] = ["cover.jpg", "cover.png", "folder.jpg", "folder.png", "front.jpg", "front.png"];

/// A track's artwork: embedded in its file, else a cover image in its
/// folder (or the album folder above a disc folder). Empty when there's
/// none. Only for files in the collection or the music folder.
pub async fn cover_art(app: &AppHandle, path: PathBuf) -> AppResult<Vec<u8>> {
    let root = app.state::<SettingsState>().snapshot().music_dir;
    let known = app.state::<Collection>().track_at(&path).is_some()
        || (path.starts_with(&root) && !path.components().any(|c| c == std::path::Component::ParentDir));
    if !known {
        return Err(AppError::Validation("Not a file in the collection".into()));
    }
    library::blocking(move || {
        if let Ok(Some(bytes)) = library::read_cover(&path) {
            return Ok(bytes);
        }
        Ok(folder_art(&path).unwrap_or_default())
    })
    .await
}

fn folder_art(path: &Path) -> Option<Vec<u8>> {
    let dir = path.parent()?;
    let in_disc_folder = dir.file_name().is_some_and(|name| super::albums::is_disc_folder(&name.to_string_lossy()));
    let dirs = [Some(dir), dir.parent().filter(|_| in_disc_folder)];
    dirs.into_iter().flatten().find_map(|dir| {
        let entries: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.filter_map(Result::ok).map(|e| e.path()).collect();
        FOLDER_ART.iter().find_map(|name| {
            let found = entries.iter().find(|p| {
                p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.eq_ignore_ascii_case(name))
            })?;
            std::fs::read(found).ok()
        })
    })
}

/// Downloads a collection track again (the best copy Soulseek has). The
/// track then points at the new file, keeping its cues and playlists; the
/// old file goes to the Trash.
pub async fn redownload(app: &AppHandle, track_id: String) -> AppResult<Job> {
    let path = app
        .state::<Collection>()
        .path_of(&track_id)
        .ok_or_else(|| AppError::Validation("That track is no longer in the collection".into()))?;
    let jobs_store = app.state::<JobStore>();
    if let Some(job) = jobs_store.list().iter().find(|j| j.replaces.as_ref() == Some(&path) && !j.status.is_finished()) {
        return Ok(job.clone());
    }

    // What identifying found, if it has been; else what the file says.
    let item = match app.state::<LibraryStore>().for_track(&track_id).filter(|i| i.path == path) {
        Some(item) => item,
        None => {
            let root = app.state::<SettingsState>().snapshot().music_dir;
            let (id, file) = (track_id.clone(), path.clone());
            library::blocking(move || Ok(super::identify::read_item(&root, id, file))).await?
        }
    };
    let Some(request) = request_for(app, &item).await else {
        return Err(AppError::Validation(
            "Couldn't tell which track this is. Identify it first, or fix its tags.".into(),
        ));
    };
    let job = Job::new(request, Some(path));
    jobs_store.insert(app, job.clone());
    jobs::start(app, job.id)?;
    Ok(job)
}

/// What to search for: the download that made the file, the match a scan
/// found, its MusicBrainz tags, or a search with its tags.
async fn request_for(app: &AppHandle, item: &LibraryItem) -> Option<TrackRequest> {
    if let Some(job) = app.state::<JobStore>().find_by_output(&item.path) {
        return Some(job.request);
    }
    if let Some(request) = item.proposal.as_ref().and_then(|p| p.request.clone()) {
        return Some(request);
    }
    let meta = item.proposal.as_ref().map(|p| p.metadata.clone()).unwrap_or_else(|| item.tags.as_metadata());
    if meta.title.is_empty() {
        return None;
    }
    if let Some(request) = autotag::lossless_source(&app.state::<Sources>(), &meta).await {
        return Some(request);
    }
    let mb_recording_id = meta.mb_recording_id.clone()?;
    Some(TrackRequest {
        deezer_id: None,
        mb_recording_id: Some(mb_recording_id),
        title: meta.title,
        artist: meta.artist,
        album: meta.album,
        isrc: meta.isrc,
        duration_secs: item.duration_secs.unwrap_or_default(),
        track_number: meta.track_number,
        disc_number: meta.disc_number,
        release_date: meta.date,
        cover_url: None,
        cover_xl_url: None,
        preview_url: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn finds_folder_art_next_to_the_file_or_above_a_disc_folder() {
        let root = std::env::temp_dir().join(format!("jim-box-test-art-{}", Uuid::new_v4()));
        let album = root.join("Artist/Album");
        std::fs::create_dir_all(album.join("CD1")).unwrap();
        std::fs::write(album.join("Cover.JPG"), "art").unwrap();

        assert_eq!(folder_art(&album.join("01 a.flac")), Some(b"art".to_vec()));
        assert_eq!(folder_art(&album.join("CD1/01 a.flac")), Some(b"art".to_vec()));
        assert_eq!(folder_art(&root.join("Artist/x.flac")), None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
