//! Moving the whole music folder somewhere else (beets' `move`, for the
//! library root): files keep their paths relative to the folder, and the
//! collection's tracks follow their files.

use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use tracing::{info, warn};

use super::LibraryStore;
use crate::error::{AppError, AppResult};
use crate::ingest;
use crate::jobs::JobStore;
use crate::library;
use crate::persistence;
use crate::rekordbox::{Arrival, Collection};
use crate::slskd::Daemon;
use crate::state::{LibraryPhase, SettingsState};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelocateReport {
    pub moved: usize,
    /// "relative/path: reason" for files that couldn't be moved (left in the old folder).
    pub failed: Vec<String>,
}

/// Switches the music folder, moving the old folder's contents into the new
/// one (never overwriting), then points settings, jobs and slskd at it.
pub async fn change_music_dir(app: &AppHandle, new_dir: PathBuf) -> AppResult<RelocateReport> {
    if !new_dir.is_absolute() {
        return Err(AppError::Validation("Choose a music folder".into()));
    }
    let old_dir = app.state::<SettingsState>().snapshot().music_dir;
    if new_dir == old_dir {
        return Ok(RelocateReport { moved: 0, failed: Vec::new() });
    }
    if new_dir.starts_with(&old_dir) || old_dir.starts_with(&new_dir) {
        return Err(AppError::Validation(
            "The new folder can't be inside the current one, or contain it".into(),
        ));
    }
    let jobs = app.state::<JobStore>();
    let library = app.state::<LibraryStore>();
    if jobs.has_running() || library.is_busy() {
        return Err(AppError::Validation(
            "Wait for downloads and library work to finish before moving the library".into(),
        ));
    }
    // Tracks in rekordbox's collection have to follow their files.
    let (lookup_app, dir) = (app.clone(), old_dir.clone());
    let tracks = library::blocking(move || Ok(lookup_app.state::<Collection>().tracks_under(&dir))).await?;
    if !tracks.is_empty() && rbl_db::is_rekordbox_running() {
        return Err(AppError::Validation(format!(
            "Quit rekordbox first (and its agent in the menu bar): {} tracks in its collection are in this folder and need to be told where they went",
            tracks.len()
        )));
    }

    let move_app = app.clone();
    let (from, to) = (old_dir.clone(), new_dir.clone());
    let report = library::blocking(move || relocate(&move_app, &from, &to)).await?;
    info!(
        "Moved library {} → {}: {} files, {} failed",
        old_dir.display(),
        new_dir.display(),
        report.moved,
        report.failed.len()
    );

    let updated = {
        let state = app.state::<SettingsState>();
        let mut settings = state.0.lock().expect("settings lock poisoned");
        settings.music_dir = new_dir.clone();
        settings.clone()
    };
    persistence::save_settings(app, &updated);
    jobs.rebase_output_paths(app, &old_dir, &new_dir);
    library.clear(app);
    library.set_progress(app, LibraryPhase::Idle, 0, 0);

    let followed: Vec<Arrival> = tracks
        .into_iter()
        .filter_map(|(_, old)| {
            let new = new_dir.join(old.strip_prefix(&old_dir).ok()?);
            new.is_file().then_some(Arrival { path: new, replaces: Some(old), playlist: None })
        })
        .collect();
    ingest::arrived(app, followed).await;

    // The shared folder changed.
    let daemon_app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = daemon_app.state::<Daemon>().start(&daemon_app).await {
            warn!("Failed to restart slskd after moving the library: {e}");
        }
    });
    Ok(report)
}

fn relocate(app: &AppHandle, old: &Path, new: &Path) -> AppResult<RelocateReport> {
    std::fs::create_dir_all(new)?;
    let mut report = RelocateReport { moved: 0, failed: Vec::new() };
    if !old.is_dir() {
        return Ok(report);
    }

    let store = app.state::<LibraryStore>();
    let files = library::walk_files(old)?;
    let total = files.len();
    for (done, file) in files.into_iter().enumerate() {
        store.set_progress(app, LibraryPhase::Moving, done, total);
        let relative = file.strip_prefix(old).unwrap_or(&file).to_path_buf();
        match library::move_file(&file, new.join(&relative)) {
            Ok(_) => report.moved += 1,
            Err(e) => report.failed.push(format!("{}: {e}", relative.display())),
        }
    }
    prune_empty_tree(old);
    Ok(report)
}

/// Removes every empty folder under `dir` (post-order), then `dir` itself if empty.
fn prune_empty_tree(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(Result::ok) {
            if entry.path().is_dir() {
                prune_empty_tree(&entry.path());
            }
        }
    }
    library::remove_if_empty(dir);
}
