//! What the app remembers between launches.
//!
//! `config.json` holds what belongs to the machine: the libraries that have
//! been opened, which one opens by default, and the accounts. Each library
//! has a file of its own, `libraries/<id>.json`, for its settings,
//! downloads, playlists and tracks waiting for rekordbox to close.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use tauri::{AppHandle, Manager};
use tracing::error;
use uuid::Uuid;

use crate::error::AppResult;
use crate::libraries::{KnownLibrary, OpenLibrary};
use crate::library;
use crate::playlist::PlaylistMatch;
use crate::rekordbox::Arrival;
use crate::state::{Job, Settings};

const STORE_FILE: &str = "config.json";
const LIBRARIES_KEY: &str = "libraries";
const DEFAULT_LIBRARY_KEY: &str = "defaultLibrary";
const OPEN_NEXT_KEY: &str = "openNext";
const ACCOUNT_KEY: &str = "account";

// A library's own, in its file. Before there were libraries these were in
// `config.json`: see `adopt_legacy`.
const SETTINGS_KEY: &str = "settings";
const JOBS_KEY: &str = "jobs";
const PLAYLISTS_KEY: &str = "playlists";
const ARRIVALS_KEY: &str = "arrivals";
const LIBRARY_KEYS: [&str; 4] = [SETTINGS_KEY, JOBS_KEY, PLAYLISTS_KEY, ARRIVALS_KEY];

fn library_file(id: Uuid) -> String {
    format!("libraries/{id}.json")
}

/// Where what is known of a library's beat grids is kept: a file of its
/// own beside the library's, as it is written a track at a time. The grids
/// a track could have instead go in the folder of the same name.
pub fn grid_states_file(app: &AppHandle, id: Uuid) -> AppResult<std::path::PathBuf> {
    Ok(app.path().app_data_dir()?.join(format!("libraries/{id}.grids.json")))
}

/// The open library's file.
fn open_file(app: &AppHandle) -> String {
    library_file(app.state::<OpenLibrary>().0.id)
}

// --- Generic helpers ---

/// Each file's keys, as last read or written. A file is a JSON object.
static STORES: LazyLock<Mutex<HashMap<PathBuf, Map<String, Value>>>> = LazyLock::new(Mutex::default);

fn store_path(app: &AppHandle, file: &str) -> Option<PathBuf> {
    match app.path().app_data_dir() {
        Ok(dir) => Some(dir.join(file)),
        Err(e) => {
            error!("No folder to keep {file} in: {e}");
            None
        }
    }
}

/// Reads a file's keys. One that can't be read as JSON is moved aside, not
/// written over: what it held may yet be recovered by hand.
fn read_store(path: &Path) -> Map<String, Value> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                error!("Couldn't read {}: {e}", path.display());
            }
            return Map::new();
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(Value::Object(keys)) => keys,
        other => {
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let aside = path.with_extension(format!("corrupt-{stamp}.json"));
            let why = other.err().map_or_else(|| "not a JSON object".to_owned(), |e| e.to_string());
            match std::fs::rename(path, &aside) {
                Ok(()) => error!("{} is unreadable ({why}); kept as {}", path.display(), aside.display()),
                Err(e) => error!("{} is unreadable ({why}) and couldn't be moved aside: {e}", path.display()),
            }
            Map::new()
        }
    }
}

/// Runs `work` on a file's keys, reading them from disk the first time.
fn with_store<R>(app: &AppHandle, file: &str, work: impl FnOnce(&Path, &mut Map<String, Value>) -> R) -> Option<R> {
    let path = store_path(app, file)?;
    let mut stores = STORES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let keys = stores.entry(path.clone()).or_insert_with(|| read_store(&path));
    Some(work(&path, keys))
}

/// Writes a file's keys: whole, or not at all (see `library::write_atomic`).
fn write_store(path: &Path, keys: &Map<String, Value>) {
    let written = serde_json::to_vec(keys).map_err(std::io::Error::other).and_then(|bytes| library::write_atomic(path, &bytes));
    if let Err(e) = written {
        error!("Failed to save {}: {e}", path.display());
    }
}

fn store_get<T: DeserializeOwned>(app: &AppHandle, file: &str, key: &str) -> Option<T> {
    let value = with_store(app, file, |_, keys| keys.get(key).cloned())??;
    match serde_json::from_value(value) {
        Ok(v) => Some(v),
        Err(e) => {
            error!("Ignoring unreadable '{key}' in {file}: {e}");
            None
        }
    }
}

fn store_set<T: Serialize>(app: &AppHandle, file: &str, key: &str, value: &T) {
    let value = match serde_json::to_value(value) {
        Ok(v) => v,
        Err(e) => {
            error!("Failed to serialize {key}: {e}");
            return;
        }
    };
    // Written under the lock, so two saves land in the order they were made.
    with_store(app, file, |path, keys| {
        keys.insert(key.to_owned(), value);
        write_store(path, keys);
    });
}

// --- The libraries ---

pub fn load_libraries(app: &AppHandle) -> Vec<KnownLibrary> {
    store_get(app, STORE_FILE, LIBRARIES_KEY).unwrap_or_default()
}

pub fn save_libraries(app: &AppHandle, libraries: &[KnownLibrary]) {
    store_set(app, STORE_FILE, LIBRARIES_KEY, &libraries);
}

/// The library that opens without asking, if one is chosen.
pub fn default_library(app: &AppHandle) -> Option<Uuid> {
    store_get::<Option<Uuid>>(app, STORE_FILE, DEFAULT_LIBRARY_KEY).flatten()
}

pub fn set_default_library(app: &AppHandle, id: Option<Uuid>) {
    store_set(app, STORE_FILE, DEFAULT_LIBRARY_KEY, &id);
}

/// The library to open at the next launch only: how the app switches
/// library, by restarting into the other one.
pub fn set_open_next(app: &AppHandle, id: Option<Uuid>) {
    store_set(app, STORE_FILE, OPEN_NEXT_KEY, &id);
}

pub fn take_open_next(app: &AppHandle) -> Option<Uuid> {
    let id = store_get::<Option<Uuid>>(app, STORE_FILE, OPEN_NEXT_KEY).flatten()?;
    set_open_next(app, None);
    Some(id)
}

/// Removes what was saved for a library. Its rekordbox library isn't touched.
pub fn delete_library_data(app: &AppHandle, id: Uuid) {
    let Ok(dir) = app.path().app_data_dir() else { return };
    STORES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&dir.join(library_file(id)));
    for file in [
        dir.join(library_file(id)),
        dir.join(format!("libraries/{id}.jobs.json")),
        dir.join(format!("libraries/{id}.grids.json")),
    ] {
        if let Err(e) = std::fs::remove_file(&file) {
            if e.kind() != std::io::ErrorKind::NotFound {
                error!("Couldn't remove {}: {e}", file.display());
            }
        }
    }
    // The grids to choose from, and the backups made of its master.db.
    for folder in [dir.join(format!("libraries/{id}.grids")), dir.join(format!("rekordbox-backups/{id}"))] {
        if let Err(e) = std::fs::remove_dir_all(&folder) {
            if e.kind() != std::io::ErrorKind::NotFound {
                error!("Couldn't remove {}: {e}", folder.display());
            }
        }
    }
}

/// The settings saved for a library that may not be the open one, read
/// from its file.
pub fn peek_settings(app: &AppHandle, id: Uuid) -> Option<Settings> {
    let file = app.path().app_data_dir().ok()?.join(library_file(id));
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    serde_json::from_value(saved.get(SETTINGS_KEY)?.clone()).ok()
}

/// True when `config.json` still holds what the app saved before it had
/// libraries.
pub fn has_legacy_data(app: &AppHandle) -> bool {
    with_store(app, STORE_FILE, |_, keys| keys.contains_key(SETTINGS_KEY)).unwrap_or(false)
}

/// Gives a library the settings, downloads and playlists saved before the
/// app had libraries, when there was only rekordbox's own.
pub fn adopt_legacy(app: &AppHandle, id: Uuid) {
    // The accounts stay with the machine.
    save_account(app, &load_account(app));
    let moved: Vec<(&str, Value)> = with_store(app, STORE_FILE, |_, keys| {
        LIBRARY_KEYS.iter().filter_map(|&key| keys.get(key).map(|value| (key, value.clone()))).collect()
    })
    .unwrap_or_default();
    // Into the library's file first: a crash between the two leaves both with them.
    with_store(app, &library_file(id), |path, keys| {
        for (key, value) in moved {
            keys.insert(key.to_owned(), value);
        }
        write_store(path, keys);
    });
    with_store(app, STORE_FILE, |path, keys| {
        for key in LIBRARY_KEYS {
            keys.remove(key);
        }
        write_store(path, keys);
    });
}

// --- The accounts, shared by every library ---

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Account {
    soulseek_username: Option<String>,
    youtube_cookies_browser: Option<String>,
}

impl Account {
    fn of(settings: &Settings) -> Self {
        Self {
            soulseek_username: settings.soulseek_username.clone(),
            youtube_cookies_browser: settings.youtube_cookies_browser.clone(),
        }
    }
}

fn load_account(app: &AppHandle) -> Account {
    store_get(app, STORE_FILE, ACCOUNT_KEY)
        // From before there were libraries.
        .or_else(|| store_get::<Settings>(app, STORE_FILE, SETTINGS_KEY).map(|settings| Account::of(&settings)))
        .unwrap_or_default()
}

fn save_account(app: &AppHandle, account: &Account) {
    store_set(app, STORE_FILE, ACCOUNT_KEY, account);
}

// --- The open library ---

/// The open library's settings, with the accounts every library shares.
/// A library opened for the first time gets `default_music_dir`.
pub fn load_settings(app: &AppHandle, default_music_dir: impl FnOnce() -> AppResult<std::path::PathBuf>) -> AppResult<Settings> {
    let mut settings = match store_get::<Settings>(app, &open_file(app), SETTINGS_KEY) {
        Some(settings) => settings,
        None => Settings::new(default_music_dir()?),
    };
    let account = load_account(app);
    settings.soulseek_username = account.soulseek_username;
    settings.youtube_cookies_browser = account.youtube_cookies_browser;
    if let Some(old) = settings.legacy_path_template.take() {
        settings.paths.default = library::convert_legacy_template(&old);
        save_settings(app, &settings);
    }
    Ok(settings)
}

pub fn save_settings(app: &AppHandle, settings: &Settings) {
    store_set(app, &open_file(app), SETTINGS_KEY, settings);
    save_account(app, &Account::of(settings));
}

/// The open library's downloads have a file of their own: they are most of
/// what a library saves, and change the most often.
fn jobs_file(app: &AppHandle) -> String {
    format!("libraries/{}.jobs.json", app.state::<OpenLibrary>().0.id)
}

pub fn load_jobs(app: &AppHandle) -> Vec<Job> {
    store_get(app, &jobs_file(app), JOBS_KEY)
        // From when they were kept with the library's settings.
        .or_else(|| store_get(app, &open_file(app), JOBS_KEY))
        .unwrap_or_default()
}

pub fn save_jobs(app: &AppHandle, jobs: &[Job]) {
    store_set(app, &jobs_file(app), JOBS_KEY, &jobs);
    // Now that they are safely in their own file.
    with_store(app, &open_file(app), |path, keys| {
        if keys.remove(JOBS_KEY).is_some() {
            write_store(path, keys);
        }
    });
}

pub fn load_playlists(app: &AppHandle) -> Vec<PlaylistMatch> {
    store_get(app, &open_file(app), PLAYLISTS_KEY).unwrap_or_default()
}

pub fn save_playlists(app: &AppHandle, playlists: &[PlaylistMatch]) {
    store_set(app, &open_file(app), PLAYLISTS_KEY, &playlists);
}

pub fn load_arrivals(app: &AppHandle) -> Vec<Arrival> {
    store_get(app, &open_file(app), ARRIVALS_KEY).unwrap_or_default()
}

pub fn save_arrivals(app: &AppHandle, arrivals: &[Arrival]) {
    store_set(app, &open_file(app), ARRIVALS_KEY, &arrivals);
}
