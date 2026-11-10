//! Applying reviewed proposals: write tags, then move files into place.

use std::collections::HashMap;
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
    if rbl_db::is_rekordbox_running() {
        return Err(AppError::Validation(
            "Quit rekordbox first (and its agent in the menu bar): its library can't be updated while it's open".into(),
        ));
    }
    let task_app = app.clone();
    app.state::<LibraryStore>().spawn(app, async move { run_apply(&task_app, ids).await })
}

async fn run_apply(app: &AppHandle, ids: Vec<Uuid>) {
    let store = app.state::<LibraryStore>();
    let root = app.state::<SettingsState>().snapshot().music_dir;
    let mut covers: HashMap<String, Option<Vec<u8>>> = HashMap::new();

    let total = ids.len();
    for (done, id) in ids.into_iter().enumerate() {
        store.set_progress(app, LibraryPhase::Applying, done, total);
        let Some(item) = store.get(id) else { continue };
        let Some(proposal) = item.proposal.clone() else { continue };
        store.update_item(app, id, |i| i.status = ItemStatus::Applying);

        // Tracks from the same album share artwork; fetch it once.
        let cover_key = proposal.metadata.cover_art_urls.join("|");
        if !covers.contains_key(&cover_key) {
            let cover = app.state::<Sources>().fetch_cover(&proposal.metadata.cover_art_urls).await;
            covers.insert(cover_key.clone(), cover);
        }
        let cover = covers[&cover_key].clone();

        let (src, root_dir, meta, target) = (item.path.clone(), root.clone(), proposal.metadata.clone(), proposal.target_path.clone());
        let result = library::blocking(move || {
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
        .await;

        if let Ok(dest) = &result {
            app.state::<JobStore>().file_moved(app, &item.path, dest, &proposal.metadata);
            // The track follows its file and picks up the new tags. Should
            // rekordbox have been opened meanwhile, this waits its turn.
            let arrival = Arrival { path: dest.clone(), replaces: Some(item.path.clone()), playlist: None };
            ingest::arrived(app, vec![arrival]).await;
        }
        store.update_item(app, id, |i| match result {
            Ok(dest) => {
                i.relative_path = dest.strip_prefix(&root).unwrap_or(&dest).to_string_lossy().into_owned();
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
}
