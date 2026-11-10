//! Tidying collection tracks, after beets' importer: identify their files
//! (album by album where possible), and apply MusicBrainz tags and the path
//! template once the user has reviewed them. The tracks keep their place in
//! rekordbox's collection throughout. Also moves the whole music folder.

mod albums;
mod apply;
mod explore;
mod identify;
mod relocate;

pub use apply::apply;
pub use explore::{cover_art, redownload};
pub use identify::identify;
pub use relocate::{change_music_dir, RelocateReport};

use serde::Serialize;
use std::future::Future;
use std::sync::Mutex;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::{ItemStatus, LibraryItem, LibraryPhase, LibraryProgress};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySnapshot {
    pub items: Vec<LibraryItem>,
    pub progress: LibraryProgress,
}

/// The tracks being tidied: what identifying them found, until it's applied
/// or dismissed. Kept for the session only.
/// Emits `library-reset`, `library-item` and `library-progress` events.
pub struct LibraryStore {
    items: Mutex<Vec<LibraryItem>>,
    progress: Mutex<LibraryProgress>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl LibraryStore {
    pub fn new() -> Self {
        Self {
            items: Mutex::new(Vec::new()),
            progress: Mutex::new(LibraryProgress::IDLE),
            task: Mutex::new(None),
        }
    }

    pub fn snapshot(&self) -> LibrarySnapshot {
        LibrarySnapshot {
            items: self.items.lock().expect("library lock poisoned").clone(),
            progress: self.progress.lock().expect("progress lock poisoned").clone(),
        }
    }

    fn get(&self, id: Uuid) -> Option<LibraryItem> {
        self.items
            .lock()
            .expect("library lock poisoned")
            .iter()
            .find(|i| i.id == id)
            .cloned()
    }

    /// What's known about a collection track, if it has been looked at.
    fn for_track(&self, track_id: &str) -> Option<LibraryItem> {
        self.items
            .lock()
            .expect("library lock poisoned")
            .iter()
            .find(|i| i.track_id == track_id)
            .cloned()
    }

    /// Adds items, each taking the place (and id) of an earlier one for the
    /// same track.
    fn put_items(&self, app: &AppHandle, new: &mut [LibraryItem]) {
        {
            let mut items = self.items.lock().expect("library lock poisoned");
            for item in new.iter_mut() {
                match items.iter_mut().find(|i| i.track_id == item.track_id) {
                    Some(old) => {
                        item.id = old.id;
                        *old = item.clone();
                    }
                    None => items.push(item.clone()),
                }
            }
        }
        let _ = app.emit("library-reset", self.snapshot());
    }

    /// Forgets what identifying found for these items, unless it's being applied.
    pub fn dismiss(&self, app: &AppHandle, ids: &[Uuid]) {
        self.items
            .lock()
            .expect("library lock poisoned")
            .retain(|i| i.status == ItemStatus::Applying || !ids.contains(&i.id));
        let _ = app.emit("library-reset", self.snapshot());
    }

    /// Forgets everything, as after the files have all moved.
    fn clear(&self, app: &AppHandle) {
        self.items.lock().expect("library lock poisoned").clear();
        let _ = app.emit("library-reset", self.snapshot());
    }

    fn update_item(&self, app: &AppHandle, id: Uuid, f: impl FnOnce(&mut LibraryItem)) {
        let updated = {
            let mut items = self.items.lock().expect("library lock poisoned");
            items.iter_mut().find(|i| i.id == id).map(|item| {
                f(item);
                item.clone()
            })
        };
        if let Some(item) = updated {
            let _ = app.emit("library-item", item);
        }
    }

    fn set_progress(&self, app: &AppHandle, phase: LibraryPhase, done: usize, total: usize) {
        let progress = LibraryProgress { phase, done, total };
        *self.progress.lock().expect("progress lock poisoned") = progress.clone();
        let _ = app.emit("library-progress", progress);
    }

    pub fn is_busy(&self) -> bool {
        self.task
            .lock()
            .expect("task lock poisoned")
            .as_ref()
            .is_some_and(|t| !t.inner().is_finished())
    }

    /// Runs identifying or applying in the background; only one at a time.
    fn spawn<F>(&self, app: &AppHandle, work: F) -> AppResult<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut task = self.task.lock().expect("task lock poisoned");
        if task.as_ref().is_some_and(|t| !t.inner().is_finished()) {
            return Err(AppError::Validation("Still working on the last batch; wait for it to finish".into()));
        }
        let app = app.clone();
        *task = Some(tauri::async_runtime::spawn(async move {
            work.await;
            let store = tauri::Manager::state::<LibraryStore>(&app);
            store.set_progress(&app, LibraryPhase::Idle, 0, 0);
        }));
        Ok(())
    }

    /// Stops identifying or applying. Files already changed stay changed.
    pub fn cancel(&self, app: &AppHandle) {
        if let Some(task) = self.task.lock().expect("task lock poisoned").take() {
            task.abort();
        }
        // Whatever was mid-flight goes back to what it was waiting as.
        let stuck: Vec<Uuid> = self
            .items
            .lock()
            .expect("library lock poisoned")
            .iter()
            .filter(|i| i.status == ItemStatus::Applying)
            .map(|i| i.id)
            .collect();
        for id in stuck {
            self.update_item(app, id, |i| i.status = ItemStatus::Matched);
        }
        self.set_progress(app, LibraryPhase::Idle, 0, 0);
    }
}
