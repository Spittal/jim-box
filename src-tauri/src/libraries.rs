//! The libraries jim-box manages. A library is a rekordbox `master.db`:
//! the one rekordbox uses on this machine, one on a drive, or any other.
//! Each has its own folders, settings, downloads and playlists (see
//! [`persistence`]); the Soulseek and YouTube accounts are shared.
//!
//! One library is open at a time, for the life of the process: everything
//! that works on a library is set up when it's opened ([`start`]), and
//! switching to another restarts the app into it, so nothing still running
//! for the old library can write into the new one.

use std::path::{Path, PathBuf};

use rbl_db::locate::Sources as Rekordbox;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::import::LibraryStore;
use crate::jobs::JobStore;
use crate::soulseek::Soulseek;
use crate::state::{now_ms, SettingsState};
use crate::{ingest, library, persistence, playlist, rekordbox};

/// A library that has been opened here before.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownLibrary {
    pub id: Uuid,
    pub master_db: PathBuf,
    #[serde(default)]
    pub last_opened: u64,
}

/// The library this process has open. Managed once one is.
pub struct OpenLibrary(pub KnownLibrary);

/// A library that can be opened.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryInfo {
    /// None for one found on this machine that hasn't been opened yet.
    pub id: Option<Uuid>,
    pub master_db: PathBuf,
    pub name: String,
    /// False when the file isn't there, as when its drive isn't connected.
    pub available: bool,
    pub last_opened: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryChoices {
    /// The library that is open, if one is.
    pub open: Option<Uuid>,
    /// The library that opens without asking.
    pub default_id: Option<Uuid>,
    /// Those opened before, the latest first, then others found on this machine.
    pub libraries: Vec<LibraryInfo>,
}

fn open_id(app: &AppHandle) -> Option<Uuid> {
    app.try_state::<OpenLibrary>().map(|open| open.0.id)
}

pub fn choices(app: &AppHandle) -> LibraryChoices {
    let rekordbox = Rekordbox::installed().ok();
    let default_db = rekordbox.as_ref().map(Rekordbox::default_master_db);
    let info = |id, master_db: PathBuf, last_opened| LibraryInfo {
        id,
        name: display_name(&master_db, default_db.as_deref()),
        available: master_db.is_file(),
        master_db,
        last_opened,
    };

    let mut known = persistence::load_libraries(app);
    known.sort_by_key(|library| std::cmp::Reverse(library.last_opened));
    let found: Vec<PathBuf> = found_on_this_machine(rekordbox.as_ref())
        .into_iter()
        .filter(|path| !known.iter().any(|library| library.master_db == *path))
        .collect();

    LibraryChoices {
        open: open_id(app),
        default_id: persistence::default_library(app),
        libraries: known
            .into_iter()
            .map(|library| info(Some(library.id), library.master_db, Some(library.last_opened)))
            .chain(found.into_iter().map(|path| info(None, path, None)))
            .collect(),
    }
}

/// The libraries rekordbox would offer: the one it's set to use, the one in
/// its default folder, and those on connected drives.
fn found_on_this_machine(rekordbox: Option<&Rekordbox>) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut offer = |path: PathBuf| {
        if path.is_file() && !found.contains(&path) {
            found.push(path);
        }
    };
    if let Some(rekordbox) = rekordbox {
        offer(rekordbox.configured_master_db());
        offer(rekordbox.default_master_db());
    }
    for drive in drives() {
        // rekordbox keeps a drive's library here, hidden on Mac-formatted drives.
        for folder in ["PIONEER", ".PIONEER"] {
            offer(drive.join(folder).join("Master").join("master.db"));
        }
    }
    found
}

#[cfg(target_os = "macos")]
fn drives() -> Vec<PathBuf> {
    std::fs::read_dir("/Volumes")
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default()
}

#[cfg(target_os = "windows")]
fn drives() -> Vec<PathBuf> {
    // Not A: and B:, which can stall on machines that still have them.
    ('C'..='Z').map(|letter| PathBuf::from(format!("{letter}:\\"))).filter(|drive| drive.is_dir()).collect()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn drives() -> Vec<PathBuf> {
    Vec::new()
}

/// What to call a library: its drive when it's kept the way rekordbox keeps
/// one on a drive, else the folder it's in.
fn display_name(master_db: &Path, default_db: Option<&Path>) -> String {
    if Some(master_db) == default_db {
        return "This computer".into();
    }
    let folders: Vec<&Path> = master_db.ancestors().skip(1).collect();
    let name_of = |path: &Path| match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        // A drive's root, as in `E:\`.
        None => path.to_string_lossy().trim_end_matches(['\\', '/']).to_owned(),
    };
    let named = |index: usize, name: &str| {
        folders.get(index).and_then(|folder| folder.file_name()).is_some_and(|n| n.eq_ignore_ascii_case(name))
    };
    let on_a_drive = named(0, "Master") && (named(1, "PIONEER") || named(1, ".PIONEER"));
    let folder = if on_a_drive { folders.get(2) } else { folders.first() };
    folder.map(|folder| name_of(folder)).filter(|name| !name.is_empty()).unwrap_or_else(|| "rekordbox library".into())
}

/// The known library at `master_db`, added to the list if it's new and
/// rekordbox's key opens it.
async fn register(app: &AppHandle, master_db: PathBuf) -> AppResult<KnownLibrary> {
    let mut known = persistence::load_libraries(app);
    if let Some(library) = known.iter().find(|library| library.master_db == master_db) {
        return Ok(library.clone());
    }
    if !master_db.is_absolute() || !master_db.is_file() {
        return Err(AppError::Validation("Choose a rekordbox library's master.db file".into()));
    }
    let rekordbox = Rekordbox::installed().map_err(|e| AppError::Rekordbox(e.to_string()))?;
    let location = rekordbox.location_of(&master_db).map_err(|e| AppError::Rekordbox(e.to_string()))?;
    library::blocking(move || {
        rbl_db::Library::open(location, rbl_db::OpenMode::ReadOnly)
            .map(drop)
            .map_err(|e| AppError::Validation(format!("That isn't a rekordbox library jim-box can open ({e})")))
    })
    .await?;

    let library = KnownLibrary { id: Uuid::new_v4(), master_db, last_opened: 0 };
    // What was saved before there were libraries was for rekordbox's own.
    if persistence::has_legacy_data(app) && library.master_db == rekordbox.configured_master_db() {
        info!("Giving {} the settings and downloads saved before libraries", library.master_db.display());
        persistence::adopt_legacy(app, library.id);
    }
    known.push(library.clone());
    persistence::save_libraries(app, &known);
    Ok(library)
}

/// What [`open`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Opened {
    /// The library is open in this process.
    Open,
    /// Another library was open: the app is restarting into this one.
    Restarting,
}

/// Opens the library at `master_db`, and has it open by default from now
/// on, or has none do.
pub async fn open(app: &AppHandle, master_db: PathBuf, make_default: bool) -> AppResult<Opened> {
    let library = register(app, master_db).await?;
    if !library.master_db.is_file() {
        return Err(AppError::Validation(format!(
            "{} isn't there. Is its drive connected?",
            library.master_db.display()
        )));
    }
    persistence::set_default_library(app, make_default.then_some(library.id));
    match open_id(app) {
        Some(open) if open == library.id => Ok(Opened::Open),
        Some(_) => {
            persistence::set_open_next(app, Some(library.id));
            // Stops Soulseek on the way out (see `RunEvent::Exit`).
            app.request_restart();
            Ok(Opened::Restarting)
        }
        None => {
            start(app, library)?;
            Ok(Opened::Open)
        }
    }
}

/// Opens the library chosen before a restart, else the default one. Leaves
/// none open, for the welcome screen to ask, when there is neither or its
/// file is missing.
pub fn open_at_launch(app: &AppHandle) {
    let Some(id) = persistence::take_open_next(app).or_else(|| persistence::default_library(app)) else { return };
    let Some(library) = persistence::load_libraries(app).into_iter().find(|library| library.id == id) else { return };
    if !library.master_db.is_file() {
        warn!("{} isn't there, so asking which library to open", library.master_db.display());
        return;
    }
    if let Err(e) = start(app, library) {
        error!("Couldn't open the library: {e}");
    }
}

/// Has the open library open by default, or none.
pub fn set_default(app: &AppHandle, on: bool) -> AppResult<()> {
    let open = open_id(app).ok_or_else(|| AppError::Validation("No library is open".into()))?;
    persistence::set_default_library(app, on.then_some(open));
    Ok(())
}

/// Drops a library from the list, with what jim-box saved for it. Its
/// `master.db` and music aren't touched.
pub fn forget(app: &AppHandle, id: Uuid) -> AppResult<()> {
    if open_id(app) == Some(id) {
        return Err(AppError::Validation("That library is open. Switch to another first".into()));
    }
    let mut known = persistence::load_libraries(app);
    known.retain(|library| library.id != id);
    persistence::save_libraries(app, &known);
    if persistence::default_library(app) == Some(id) {
        persistence::set_default_library(app, None);
    }
    persistence::delete_library_data(app, id);
    Ok(())
}

/// Sets up everything that works on a library, and starts Soulseek sharing
/// its folders. Once per process.
fn start(app: &AppHandle, mut library: KnownLibrary) -> AppResult<()> {
    library.last_opened = now_ms();
    let mut known = persistence::load_libraries(app);
    if let Some(saved) = known.iter_mut().find(|saved| saved.id == library.id) {
        saved.last_opened = library.last_opened;
    }
    persistence::save_libraries(app, &known);

    info!("Opening the library at {}", library.master_db.display());
    let backups = app.path().app_data_dir()?.join("rekordbox-backups").join(library.id.to_string());
    let collection = rekordbox::Collection::at(backups, library.master_db.clone())
        .with_grid_states(persistence::grid_states_file(app, library.id)?);
    let others: Vec<Uuid> = known.iter().map(|other| other.id).filter(|id| *id != library.id).collect();
    if !app.manage(OpenLibrary(library)) {
        return Err(AppError::Validation("A library is open already".into()));
    }

    let settings = persistence::load_settings(app, || unused_music_dir(app, &others))?;
    app.manage(SettingsState(std::sync::Mutex::new(settings)));
    app.manage(JobStore::load(app));
    app.manage(LibraryStore::new());
    app.manage(ingest::ArrivalQueue::load(app));
    app.manage(playlist::PlaylistStore::load(app));
    app.manage(collection);

    tauri::async_runtime::spawn(ingest::retry_when_rekordbox_closes(app.clone()));
    tauri::async_runtime::spawn(playlist::watch(app.clone()));
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = handle.state::<Soulseek>().start(&handle).await {
            error!("Failed to connect to Soulseek: {e}");
        }
    });
    Ok(())
}

/// Where a library opened for the first time puts new tracks until a folder
/// is chosen for it: `jim-box` in the Music folder, or `jim-box 2` and so
/// on when another library has that.
fn unused_music_dir(app: &AppHandle, others: &[Uuid]) -> AppResult<PathBuf> {
    let taken: Vec<PathBuf> = others
        .iter()
        .filter_map(|id| persistence::peek_settings(app, *id))
        .flat_map(|settings| settings.library_roots().map(Path::to_path_buf).collect::<Vec<_>>())
        .collect();
    let music = app.path().audio_dir()?;
    Ok((1..)
        .map(|n| music.join(if n == 1 { "jim-box".to_owned() } else { format!("jim-box {n}") }))
        .find(|dir| !taken.contains(dir))
        .expect("an unused folder name"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_library_is_named_after_its_drive_or_folder() {
        let default = Path::new("/Users/a/Library/Pioneer/rekordbox/master.db");
        let name = |path: &str| display_name(Path::new(path), Some(default));
        assert_eq!(name("/Users/a/Library/Pioneer/rekordbox/master.db"), "This computer");
        assert_eq!(name("/Volumes/Gigs/PIONEER/Master/master.db"), "Gigs");
        assert_eq!(name("/Volumes/Gigs/.PIONEER/Master/master.db"), "Gigs");
        assert_eq!(name("/Users/a/Desktop/Wedding set/master.db"), "Wedding set");
        assert_eq!(name("/master.db"), "rekordbox library");
    }
}
