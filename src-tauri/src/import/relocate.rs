//! Moving the whole music folder somewhere else (beets' `move`, for the
//! library root): files keep their paths relative to the folder, and the
//! collection's tracks follow their files.

use serde::Serialize;
use std::collections::HashSet;
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
use crate::soulseek::Soulseek;
use crate::state::{LibraryPhase, SettingsState};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelocateReport {
    pub moved: usize,
    /// "relative/path: reason" for files that couldn't be moved (left in the old folder).
    pub failed: Vec<String>,
}

/// Switches the music folder, moving the old folder's contents into the new
/// one (never overwriting), then points settings, jobs and Soulseek's shares at it.
pub async fn change_music_dir(app: &AppHandle, new_dir: PathBuf) -> AppResult<RelocateReport> {
    if !new_dir.is_absolute() {
        return Err(AppError::Validation("Choose a music folder".into()));
    }
    let settings = app.state::<SettingsState>().snapshot();
    let old_dir = settings.music_dir.clone();
    if settings.library_dirs.iter().any(|dir| new_dir.starts_with(dir) || dir.starts_with(&new_dir)) {
        return Err(AppError::Validation(
            "The new folder can't be one of the other library folders, inside one, or contain one".into(),
        ));
    }
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
    let _moving = super::Moving::begin(app)?;
    // Tracks in rekordbox's collection have to follow their files.
    let (lookup_app, dir) = (app.clone(), old_dir.clone());
    let tracks = library::blocking(move || Ok(lookup_app.state::<Collection>().tracks_under(&dir))).await?;
    if !tracks.is_empty() && crate::rekordbox::is_running() {
        return Err(AppError::Validation(format!(
            "Quit rekordbox first (and its agent in the menu bar): {} tracks in its collection are in this folder and need to be told where they went",
            tracks.len()
        )));
    }

    let move_app = app.clone();
    let (from, to) = (old_dir.clone(), new_dir.clone());
    let known: HashSet<PathBuf> = tracks.into_iter().map(|(_, path)| path).collect();
    let (report, moved) = library::blocking(move || relocate(&move_app, &from, &to, &known)).await?;
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
    // By where each file went, which isn't always where it was headed: a
    // name already taken in the new folder gives it another.
    jobs.paths_moved(app, &moved);
    library.clear(app);
    library.set_progress(app, LibraryPhase::Idle, 0, 0);
    // The collection's tracks were queued to follow as each file moved.
    ingest::flush(app).await;

    // The shared folder changed.
    let shares_app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = shares_app.state::<Soulseek>().refresh_shares(&shares_app).await {
            warn!("Failed to update what Soulseek shares after moving the library: {e}");
        }
    });
    Ok(report)
}

/// Moves every file, returning where each went. `tracks` are the files the
/// collection has: each is queued to follow its file as soon as it has
/// moved, so stopping part-way loses track of none.
fn relocate(
    app: &AppHandle,
    old: &Path,
    new: &Path,
    tracks: &HashSet<PathBuf>,
) -> AppResult<(RelocateReport, Vec<(PathBuf, PathBuf)>)> {
    std::fs::create_dir_all(new)?;
    let mut report = RelocateReport { moved: 0, failed: Vec::new() };
    let mut moved = Vec::new();
    if !old.is_dir() {
        return Ok((report, moved));
    }

    let store = app.state::<LibraryStore>();
    let files = library::walk_files(old)?;
    let total = files.len();
    for (done, file) in files.into_iter().enumerate() {
        store.set_progress(app, LibraryPhase::Moving, done, total);
        let relative = file.strip_prefix(old).unwrap_or(&file).to_path_buf();
        match library::move_file(&file, new.join(&relative)) {
            Ok(dest) => {
                report.moved += 1;
                if tracks.contains(&file) {
                    let arrival = Arrival { path: dest.clone(), replaces: Some(file.clone()), moved_only: true, ..Default::default() };
                    ingest::queue(app, vec![arrival]);
                }
                moved.push((file, dest));
            }
            Err(e) => report.failed.push(format!("{}: {e}", relative.display())),
        }
    }
    prune_empty_tree(old);
    Ok((report, moved))
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
