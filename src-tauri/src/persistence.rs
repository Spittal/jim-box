//! What the app remembers between launches.
//!
//! `config.json` holds what belongs to the machine: the libraries that have
//! been opened, which one opens by default, and the accounts. Each library
//! has a file of its own, `libraries/<id>.json`, for its settings,
//! downloads, playlists and tracks waiting for rekordbox to close.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager};
use tauri_plugin_store::StoreExt;
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

fn store_get<T: DeserializeOwned>(app: &AppHandle, file: &str, key: &str) -> Option<T> {
    let store = app.store(file).ok()?;
    let value = store.get(key)?;
    match serde_json::from_value(value) {
        Ok(v) => Some(v),
        Err(e) => {
            error!("Ignoring unreadable '{key}' in {file}: {e}");
            None
        }
    }
}

fn store_set<T: Serialize>(app: &AppHandle, file: &str, key: &str, value: &T) {
    let store = match app.store(file) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open {file} for {key}: {e}");
            return;
        }
    };
    match serde_json::to_value(value) {
        Ok(v) => store.set(key, v),
        Err(e) => {
            error!("Failed to serialize {key}: {e}");
            return;
        }
    }
    if let Err(e) = store.save() {
        error!("Failed to persist {key}: {e}");
    }
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
    for file in [dir.join(library_file(id)), dir.join(format!("libraries/{id}.grids.json"))] {
        if let Err(e) = std::fs::remove_file(&file) {
            if e.kind() != std::io::ErrorKind::NotFound {
                error!("Couldn't remove {}: {e}", file.display());
            }
        }
    }
    let grids = dir.join(format!("libraries/{id}.grids"));
    if let Err(e) = std::fs::remove_dir_all(&grids) {
        if e.kind() != std::io::ErrorKind::NotFound {
            error!("Couldn't remove {}: {e}", grids.display());
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
    app.store(STORE_FILE).is_ok_and(|store| store.has(SETTINGS_KEY))
}

/// Gives a library the settings, downloads and playlists saved before the
/// app had libraries, when there was only rekordbox's own.
pub fn adopt_legacy(app: &AppHandle, id: Uuid) {
    // The accounts stay with the machine.
    save_account(app, &load_account(app));
    let (Ok(config), Ok(library)) = (app.store(STORE_FILE), app.store(library_file(id))) else { return };
    for key in LIBRARY_KEYS {
        if let Some(value) = config.get(key) {
            library.set(key, value);
        }
    }
    if let Err(e) = library.save() {
        error!("Failed to move the saved library into its own file: {e}");
        return;
    }
    for key in LIBRARY_KEYS {
        config.delete(key);
    }
    if let Err(e) = config.save() {
        error!("Failed to persist {STORE_FILE}: {e}");
    }
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

pub fn load_jobs(app: &AppHandle) -> Vec<Job> {
    store_get(app, &open_file(app), JOBS_KEY).unwrap_or_default()
}

pub fn save_jobs(app: &AppHandle, jobs: &[Job]) {
    store_set(app, &open_file(app), JOBS_KEY, &jobs);
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
