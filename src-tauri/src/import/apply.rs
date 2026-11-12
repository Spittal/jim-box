//! Applying reviewed proposals: write tags, then move files into place.

use tauri::{AppHandle, Manager};
use uuid::Uuid;

use super::LibraryStore;
use crate::error::{AppError, AppResult};
use crate::ingest;
use crate::jobs::JobStore;
use crate::library::{self, TagMode};
use crate::rekordbox::Arrival;
use crate::sources::Sources;
use crate::state::{ExistingTags, ItemStatus, LibraryPhase, SettingsState};

/// Applies the proposals for the given items in the background. Refused
/// while rekordbox is open: a moved file would show as missing there until
/// the collection could be told where it went.
pub fn apply(app: &AppHandle, ids: Vec<Uuid>) -> AppResult<()> {
    if crate::rekordbox::is_running() {
        return Err(AppError::Validation(
            "Quit rekordbox first (and its agent in the menu bar): its library can't be updated while it's open".into(),
        ));
    }
    let task_app = app.clone();
    app.state::<LibraryStore>().spawn(app, async move { run_apply(&task_app, ids).await })
}

/// Tracks applied between each time the collection takes them in.
const FLUSH_EVERY: usize = 25;

async fn run_apply(app: &AppHandle, ids: Vec<Uuid>) {
    let store = app.state::<LibraryStore>();
    let settings = app.state::<SettingsState>().snapshot();
    // Tracks of an album come together and share artwork: the last one's is kept.
    let mut last_cover: Option<(String, Option<Vec<u8>>)> = None;

    let total = ids.len();
    store.set_applying(true);
    for (done, id) in ids.into_iter().enumerate() {
        if store.stop_requested() {
            break;
        }
        store.set_progress(app, LibraryPhase::Applying, done, total);
        // rekordbox's library is read again after each write: once for
        // several tracks, not once a track.
        if done > 0 && done % FLUSH_EVERY == 0 {
            flush_detached(app).await;
        }
        let Some(item) = store.get(id) else { continue };
        let Some(proposal) = item.proposal.clone() else { continue };
        store.update_item(app, id, |i| i.status = ItemStatus::Applying);

        let cover_key = proposal.metadata.cover_art_urls.join("|");
        if last_cover.as_ref().is_none_or(|(key, _)| *key != cover_key) {
            let cover = app.state::<Sources>().fetch_cover(&proposal.metadata.cover_art_urls).await;
            last_cover = Some((cover_key, cover));
        }
        let cover = last_cover.as_ref().and_then(|(_, cover)| cover.clone());

        let root_dir = settings.library_root_for(&item.path).to_path_buf();
        let (src, meta, target) = (item.path.clone(), proposal.metadata.clone(), proposal.target_path.clone());
        // On a task of its own, so stopping the batch can't come between a
        // file moving and the collection being told where it went.
        let (work_app, from) = (app.clone(), item.path.clone());
        let result = tauri::async_runtime::spawn(async move {
            let (tagged, moved) = (meta.clone(), src.clone());
            let dest = library::blocking(move || {
                library::write_tags(&src, &meta, cover.as_deref(), TagMode::Merge)?;
                if target == src {
                    return Ok(src);
                }
                let dest = library::move_file(&src, target)?;
                if let Some(parent) = src.parent() {
                    library::remove_empty_dirs(parent, &root_dir);
                }
                Ok(dest)
            })
            .await?;
            work_app.state::<JobStore>().file_moved(&work_app, &moved, &dest, &tagged);
            // The track follows its file and picks up the new tags: queued
            // (and saved) now, taken in with the rest of the batch.
            ingest::queue(&work_app, vec![Arrival { path: dest.clone(), replaces: Some(from), ..Default::default() }]);
            Ok(dest)
        })
        .await
        .unwrap_or_else(|e| Err(AppError::Io(std::io::Error::other(e))));

        store.update_item(app, id, |i| match result {
            Ok(dest) => {
                i.relative_path =
                    dest.strip_prefix(settings.library_root_for(&dest)).unwrap_or(&dest).to_string_lossy().into_owned();
                (i.size, i.modified) = super::identify::file_stamp(&dest);
                i.path = dest;
                i.tags = ExistingTags::from(&proposal.metadata);
                i.status = ItemStatus::Applied;
                i.error = None;
            }
            Err(e) => {
                i.status = ItemStatus::Failed;
                i.error = Some(e.to_string());
            }
        });
    }
    flush_detached(app).await;
    store.set_applying(false);
}

/// Takes the queued tracks into the collection, to the end even if the batch is stopped.
async fn flush_detached(app: &AppHandle) {
    let app = app.clone();
    let _ = tauri::async_runtime::spawn(async move { ingest::flush(&app).await }).await;
}
