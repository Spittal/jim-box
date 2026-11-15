//! The library Jim Box manages: the rekordbox `master.db` rekordbox itself
//! is set to use on this computer, which is the one in its default folder
//! unless it was moved in rekordbox. There is one, so nothing is chosen; it
//! is opened at launch, and everything that works on it is set up then
//! ([`start`]), once for the life of the process.
//!
//! What is saved for it (folders, settings, downloads, playlists; see
//! [`persistence`]) is kept under an id, as it was when other libraries
//! could be opened too.

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
use crate::{ingest, persistence, playlist, rekordbox};

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

/// Whether the library is open, and where it is.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStatus {
    pub open: bool,
    /// The open library's `master.db`, else where rekordbox would keep one.
    /// None when that can't be told, as when rekordbox isn't installed.
    pub master_db: Option<PathBuf>,
}

pub fn status(app: &AppHandle) -> LibraryStatus {
    let open = app.try_state::<OpenLibrary>().map(|open| open.0.master_db.clone());
    LibraryStatus {
        open: open.is_some(),
        master_db: open.or_else(|| Rekordbox::installed().ok().map(|rekordbox| rekordbox.configured_master_db())),
    }
}

/// The known library at `master_db`, added to the list if it's new and
/// rekordbox's key opens it. Blocking.
fn register(app: &AppHandle, rekordbox: &Rekordbox, master_db: PathBuf) -> AppResult<KnownLibrary> {
    let mut known = persistence::load_libraries(app);
    if let Some(library) = known.iter().find(|library| library.master_db == master_db) {
        return Ok(library.clone());
    }
    let location = rekordbox.location_of(&master_db).map_err(|e| AppError::Rekordbox(e.to_string()))?;
    rbl_db::Library::open(location, rbl_db::OpenMode::ReadOnly)
        .map(drop)
        .map_err(|e| AppError::Validation(format!("Jim Box can't open the rekordbox library ({e})")))?;

    let library = KnownLibrary { id: Uuid::new_v4(), master_db, last_opened: 0 };
    // What was saved before libraries had ids was for this one.
    if persistence::has_legacy_data(app) {
        info!("Giving {} the settings and downloads saved before libraries", library.master_db.display());
        persistence::adopt_legacy(app, library.id);
    }
    known.push(library.clone());
    persistence::save_libraries(app, &known);
    Ok(library)
}

/// Opens the library rekordbox is set to use.
fn open(app: &AppHandle) -> AppResult<()> {
    let rekordbox = Rekordbox::installed().map_err(|e| AppError::Rekordbox(e.to_string()))?;
    let master_db = rekordbox.configured_master_db();
    if !master_db.is_file() {
        return Err(AppError::Validation(format!(
            "There's no rekordbox library at {}",
            master_db.display()
        )));
    }
    let library = register(app, &rekordbox, master_db)?;
    start(app, library)
}

/// Opens the library as the app starts. When it can't be (most likely
/// rekordbox isn't installed) none is open, and the window says so.
pub fn open_at_launch(app: &AppHandle) {
    if let Err(e) = open(app) {
        warn!("Couldn't open the rekordbox library: {e}");
    }
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
