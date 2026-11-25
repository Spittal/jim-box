//! Libraries opened beside the one Jim Box manages (the "master"): a USB
//! stick a player reads, or another rekordbox `master.db` (a backup,
//! another computer's, one kept on a drive). Each is browsed as the master
//! is, and tracks and playlists go between them.
//!
//! The interface tells them apart by their ids. The master's tracks and
//! playlists keep rekordbox's own ids; another library's carry its id in
//! front, `lib1~123`, so whatever the interface hands back (a track to
//! play, a playlist to change) says which library it's of. [`tag`] and
//! [`untag`] do that at the boundary, and nothing inside a library sees it.
//!
//! A stick isn't written to as it's changed: its changes wait (see
//! [`rekordbox::Stick`]) and are written together by [`apply`], first the
//! collection's tracks and playlists going onto it (an export that adds to
//! the stick rather than replacing it), then the changes to its playlists,
//! in each of the two databases a stick holds.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use rbl_export::device_library::{self as device, Edit as DeviceEdit};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tracing::{info, warn};

use crate::error::{AppError, AppResult};
use crate::libraries::OpenLibrary;
use crate::rekordbox::stick::{self, ExportedList, StagedSummary};
use crate::rekordbox::{
    Change, Collection, CollectionStatus, Destination, Edit, EditOutcome, Incoming, PlaylistKind, PlaylistNode,
    PlaylistRef, Preparation, Preview, Source as ListSource, Stick, TrackPage, TrackQuery, TrackRef, TrackRow,
    TransferProgress, TransferReport,
};
use crate::state::SettingsState;
use crate::{persistence, usb};

/// What joins a library's id to an id of its own.
const SEP: char = '~';

/// What an opened library is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// A `master.db`.
    Library,
    /// A USB stick's export.
    Stick,
}

/// What's remembered of a library opened here, between launches.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    /// A stick's root, or a library's `master.db`.
    pub path: PathBuf,
    pub kind: Kind,
    /// Whether it was open when last seen: one is opened again when it's there.
    #[serde(default)]
    pub open: bool,
    /// A stick's changes waiting to be written.
    #[serde(default)]
    pub staged: Vec<Change>,
}

pub enum Source {
    Library(Collection),
    Stick(Stick),
}

/// An opened library.
pub struct Other {
    pub id: String,
    pub name: String,
    /// A stick's root, or a library's `master.db`.
    pub path: PathBuf,
    /// Where tracks copied into it go, for a library.
    pub home: PathBuf,
    pub source: Source,
}

impl Other {
    fn kind(&self) -> Kind {
        match self.source {
            Source::Library(_) => Kind::Library,
            Source::Stick(_) => Kind::Stick,
        }
    }

    fn status(&self) -> AppResult<CollectionStatus> {
        match &self.source {
            Source::Library(collection) => collection.status(),
            Source::Stick(stick) => stick.status(),
        }
    }

    fn playlists(&self) -> AppResult<Vec<PlaylistNode>> {
        Ok(match &self.source {
            Source::Library(collection) => collection.playlists()?,
            Source::Stick(stick) => stick.playlists(),
        })
    }
}

/// An opened library as the sidebar shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OtherInfo {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub kind: Kind,
    pub status: CollectionStatus,
    pub playlists: Vec<PlaylistNode>,
    /// A stick's changes waiting to be written.
    pub staged: Option<StagedSummary>,
}

/// A drive plugged in that holds a library not open yet.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plugged {
    pub name: String,
    pub path: PathBuf,
    pub kind: Kind,
}

/// The libraries open beside the master.
#[derive(Default)]
pub struct Others {
    open: RwLock<Vec<Arc<Other>>>,
    next: AtomicU32,
    /// Sticks being written, so two writes don't meet.
    writing: Mutex<HashSet<String>>,
    watcher: Mutex<Option<rbl_devices::MountWatcher>>,
}

// --- Ids --------------------------------------------------------------------

/// An id of library `library`'s own, as the interface has it.
pub fn tag(library: &str, id: &str) -> String {
    format!("{library}{SEP}{id}")
}

/// The library an id is of (None for the master), and the id within it.
pub fn untag(id: &str) -> (Option<&str>, &str) {
    match id.split_once(SEP) {
        Some((library, id)) => (Some(library), id),
        None => (None, id),
    }
}

fn tag_nodes(library: &str, nodes: &mut [PlaylistNode]) {
    for node in nodes {
        node.id = tag(library, &node.id);
        node.parent_id = node.parent_id.as_deref().map(|p| tag(library, p));
    }
}

/// The library one of these ids, or else `library`, names; they must agree.
fn one_library<'a>(library: Option<&'a str>, ids: impl IntoIterator<Item = &'a str>) -> AppResult<Option<&'a str>> {
    let mut found: Option<Option<&str>> = library.map(Some);
    for id in ids {
        let (of, _) = untag(id);
        match found {
            None => found = Some(of),
            Some(was) if was != of => return Err(AppError::Validation("Those are from different libraries".into())),
            Some(_) => {}
        }
    }
    Ok(found.flatten())
}

// --- Opening and closing --------------------------------------------------------

impl Others {
    pub fn list(&self) -> Vec<Arc<Other>> {
        self.open.read().expect("others lock poisoned").clone()
    }

    pub fn get(&self, id: &str) -> AppResult<Arc<Other>> {
        self.list()
            .into_iter()
            .find(|o| o.id == id)
            .ok_or_else(|| AppError::Validation("That library has been closed".into()))
    }

    fn by_path(&self, path: &Path) -> Option<Arc<Other>> {
        self.list().into_iter().find(|o| o.path == path)
    }
}

/// The libraries in a folder: its own, the ones around it when it's inside
/// one (a stick's `PIONEER` folder), or failing those the ones a level in.
pub fn find_in(dir: &Path) -> Vec<(Kind, PathBuf)> {
    fn here(dir: &Path) -> Vec<(Kind, PathBuf)> {
        let mut found = Vec::new();
        if Stick::holds_export(dir) {
            found.push((Kind::Stick, dir.to_path_buf()));
        }
        let master = dir.join("master.db");
        if master.is_file() {
            found.push((Kind::Library, master));
        }
        found.extend(rbl_devices::libraries::on_volume(dir).into_iter().map(|db| (Kind::Library, db)));
        found
    }
    if dir.is_file() && dir.file_name().is_some_and(|n| n == "master.db") {
        return vec![(Kind::Library, dir.to_path_buf())];
    }
    let found = here(dir);
    if !found.is_empty() {
        return found;
    }
    for above in dir.ancestors().skip(1).take(3) {
        let found = here(above);
        if !found.is_empty() {
            return found;
        }
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<(Kind, PathBuf)> =
        entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).flat_map(|p| here(&p)).collect();
    found.sort();
    found.dedup();
    found
}

/// What a library is called: its drive's or folder's name.
fn name_of(kind: Kind, path: &Path) -> String {
    let dir = match kind {
        Kind::Stick => path,
        // Past `PIONEER/Master/master.db` or `rekordbox/master.db` to the folder that holds it.
        Kind::Library => path
            .ancestors()
            .skip(1)
            .find(|dir| {
                let name = dir.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                !matches!(name.as_str(), "master" | "pioneer" | ".pioneer" | "rekordbox")
            })
            .unwrap_or(path),
    };
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if name.is_empty() {
        dir.to_string_lossy().into_owned()
    } else {
        name
    }
}

/// Opens a library. Blocking: it's read.
fn open_one(app: &AppHandle, kind: Kind, path: &Path, staged: Vec<Change>) -> AppResult<Arc<Other>> {
    let others = app.state::<Others>();
    if let Some(open) = others.by_path(path) {
        return Ok(open);
    }
    let id = format!("lib{}", others.next.fetch_add(1, Ordering::Relaxed) + 1);
    let name = name_of(kind, path);
    let (source, home) = match kind {
        Kind::Stick => (Source::Stick(Stick::open(path.to_path_buf(), staged)?), path.to_path_buf()),
        Kind::Library => {
            if app.try_state::<OpenLibrary>().is_some_and(|open| same_file(&open.0.master_db, path)) {
                return Err(AppError::Validation("That's the library Jim Box already has open".into()));
            }
            let backups = app
                .path()
                .app_data_dir()?
                .join("rekordbox-backups")
                .join("others")
                .join(format!("{:016x}", fxhash(&path.to_string_lossy())));
            let collection = Collection::at(backups, path.to_path_buf());
            collection.reload()?;
            // Beside the library: `Imported` in the folder (or drive) that holds it.
            let holder = path.ancestors().skip(1).find(|dir| name_of(Kind::Stick, dir) == name).unwrap_or(path);
            (Source::Library(collection), holder.join("Imported"))
        }
    };
    info!("Opened {} as {id}", path.display());
    let other = Arc::new(Other { id, name, path: path.to_path_buf(), home, source });
    others.open.write().expect("others lock poisoned").push(other.clone());
    Ok(other)
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// A stable short name for a path, for a folder of its own.
fn fxhash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3))
}

/// Saves which libraries are open, and each stick's changes waiting.
fn save(app: &AppHandle) {
    let open = app.state::<Others>().list();
    let mut saved = persistence::load_others(app);
    for entry in &mut saved {
        // Shut only by closing it: one whose drive went keeps its place.
        if let Some(other) = open.iter().find(|o| o.path == entry.path) {
            entry.open = true;
            if let Source::Stick(stick) = &other.source {
                entry.staged = stick.staged();
            }
        }
    }
    for other in &open {
        if !saved.iter().any(|s| s.path == other.path) {
            let staged = match &other.source {
                Source::Stick(stick) => stick.staged(),
                Source::Library(_) => Vec::new(),
            };
            saved.push(Saved { path: other.path.clone(), kind: other.kind(), open: true, staged });
        }
    }
    // Forgotten once closed with nothing waiting.
    saved.retain(|s| s.open || !s.staged.is_empty());
    persistence::save_others(app, &saved);
}

fn changed(app: &AppHandle) {
    let _ = app.emit("libraries-changed", ());
}

/// Opens the libraries in a folder. Blocking.
pub fn open(app: &AppHandle, dir: &Path) -> AppResult<Vec<OtherInfo>> {
    let found = find_in(dir);
    if found.is_empty() {
        return Err(AppError::Validation(format!(
            "There's no rekordbox library in {}. Pick a USB stick (or the folder holding its PIONEER folder), or a folder with a master.db.",
            dir.display()
        )));
    }
    let saved = persistence::load_others(app);
    let mut opened = Vec::new();
    let mut last_error = None;
    for (kind, path) in found {
        let staged = saved.iter().find(|s| s.path == path).map(|s| s.staged.clone()).unwrap_or_default();
        match open_one(app, kind, &path, staged) {
            Ok(other) => opened.push(other),
            Err(e) => {
                warn!("Couldn't open {}: {e}", path.display());
                last_error = Some(e);
            }
        }
    }
    if opened.is_empty() {
        return Err(last_error.unwrap_or_else(|| AppError::Validation("Nothing could be opened there".into())));
    }
    save(app);
    changed(app);
    opened.iter().map(|o| info_of(o)).collect()
}

/// Closes a library. A stick's changes waiting are kept for when it's opened again.
pub fn close(app: &AppHandle, id: &str) -> AppResult<()> {
    let others = app.state::<Others>();
    let gone = others.get(id)?;
    others.open.write().expect("others lock poisoned").retain(|o| o.id != id);
    let mut saved = persistence::load_others(app);
    for entry in &mut saved {
        if entry.path == gone.path {
            entry.open = false;
        }
    }
    saved.retain(|s| s.open || !s.staged.is_empty());
    persistence::save_others(app, &saved);
    changed(app);
    Ok(())
}

/// Reads a library afresh.
pub fn reload(app: &AppHandle, id: &str) -> AppResult<OtherInfo> {
    let other = app.state::<Others>().get(id)?;
    match &other.source {
        Source::Library(collection) => {
            collection.reload()?;
        }
        Source::Stick(stick) => stick.reload()?,
    }
    info_of(&other)
}

pub fn info_of(other: &Other) -> AppResult<OtherInfo> {
    let mut playlists = other.playlists()?;
    tag_nodes(&other.id, &mut playlists);
    Ok(OtherInfo {
        id: other.id.clone(),
        name: other.name.clone(),
        path: other.path.clone(),
        kind: other.kind(),
        status: other.status()?,
        playlists,
        staged: match &other.source {
            Source::Stick(stick) => Some(stick.summary()),
            Source::Library(_) => None,
        },
    })
}

pub fn infos(app: &AppHandle) -> Vec<OtherInfo> {
    app.state::<Others>().list().iter().filter_map(|o| info_of(o).ok()).collect()
}

/// Drives plugged in with a library on them that isn't open.
pub fn plugged(app: &AppHandle) -> Vec<Plugged> {
    let open = app.state::<Others>().list();
    let master = app.try_state::<OpenLibrary>().map(|o| o.0.master_db.clone());
    rbl_devices::list()
        .into_iter()
        .flat_map(|device| {
            let mut found = Vec::new();
            if Stick::holds_export(&device.mount_point) {
                found.push((Kind::Stick, device.mount_point.clone()));
            }
            found.extend(rbl_devices::libraries::on_volume(&device.mount_point).into_iter().map(|db| (Kind::Library, db)));
            found.into_iter().map(move |(kind, path)| Plugged { name: device.name.clone(), path, kind })
        })
        .filter(|p| !open.iter().any(|o| o.path == p.path))
        .filter(|p| master.as_ref().is_none_or(|m| !same_file(m, &p.path)))
        .collect()
}

/// Opens again the libraries that were open, and watches for drives coming
/// and going: a stick's library closes when it's pulled out, and opens
/// again when it's back.
pub fn start(app: &AppHandle) {
    app.manage(Others::default());
    // Nothing to bring anything into or out of without the master.
    if app.try_state::<Collection>().is_none() {
        return;
    }
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        reopen(&handle);
        let watching = handle.clone();
        let watcher = rbl_devices::MountWatcher::start(rbl_devices::mounts::INTERVAL, move || volumes_changed(&watching));
        *handle.state::<Others>().watcher.lock().expect("watcher lock poisoned") = Some(watcher);
    });
}

/// Opens what was open and is there now.
fn reopen(app: &AppHandle) {
    let others = app.state::<Others>();
    let mut any = false;
    for saved in persistence::load_others(app).into_iter().filter(|s| s.open) {
        let there = match saved.kind {
            Kind::Stick => Stick::holds_export(&saved.path),
            Kind::Library => saved.path.is_file(),
        };
        if there && others.by_path(&saved.path).is_none() {
            match open_one(app, saved.kind, &saved.path, saved.staged) {
                Ok(_) => any = true,
                Err(e) => warn!("Couldn't open {} again: {e}", saved.path.display()),
            }
        }
    }
    if any {
        changed(app);
    }
}

fn volumes_changed(app: &AppHandle) {
    let others = app.state::<Others>();
    let gone: Vec<Arc<Other>> = others
        .list()
        .into_iter()
        .filter(|o| match o.kind() {
            Kind::Stick => !o.path.is_dir(),
            Kind::Library => !o.path.is_file(),
        })
        .collect();
    if !gone.is_empty() {
        // Not closed by hand: they open again when they're back.
        save(app);
        others.open.write().expect("others lock poisoned").retain(|o| !gone.iter().any(|g| g.id == o.id));
        for other in &gone {
            info!("{} went away; closed it", other.path.display());
        }
        changed(app);
    }
    reopen(app);
    let _ = app.emit("volumes-changed", ());
}

// --- Browsing ------------------------------------------------------------------

/// Which library something is of.
pub enum Lib {
    Master,
    Other(Arc<Other>),
}

pub fn lib(app: &AppHandle, library: Option<&str>) -> AppResult<Lib> {
    match library {
        None => Ok(Lib::Master),
        Some(id) => Ok(Lib::Other(app.state::<Others>().get(id)?)),
    }
}

/// The library a track list is of, and the query as that library has it.
pub fn split_query(query: &TrackQuery) -> AppResult<(Option<String>, TrackQuery)> {
    let mut query = query.clone();
    let ids: Vec<&str> = match &query.source {
        ListSource::Collection => Vec::new(),
        ListSource::Playlist { id } => vec![id.as_str()],
        ListSource::Playlists { ids } => ids.iter().map(String::as_str).collect(),
    };
    let library = one_library(query.library.as_deref(), ids)?.map(str::to_owned);
    query.source = match query.source {
        ListSource::Collection => ListSource::Collection,
        ListSource::Playlist { id } => ListSource::Playlist { id: untag(&id).1.to_owned() },
        ListSource::Playlists { ids } => ListSource::Playlists { ids: ids.iter().map(|id| untag(id).1.to_owned()).collect() },
    };
    query.library = None;
    Ok((library, query))
}

fn tag_rows(library: &str, rows: &mut [TrackRow]) {
    for row in rows {
        row.id = tag(library, &row.id);
        row.playlist_ids = row.playlist_ids.iter().map(|p| tag(library, p)).collect();
    }
}

/// A page of a track list of another library.
pub fn tracks(app: &AppHandle, library: &str, query: &TrackQuery, offset: usize, limit: usize) -> AppResult<TrackPage> {
    let other = app.state::<Others>().get(library)?;
    let mut page = match &other.source {
        Source::Library(collection) => collection.tracks(query, offset, limit)?,
        Source::Stick(stick) => stick.tracks(query, offset, limit, Some(app.state::<Collection>().inner()))?,
    };
    tag_rows(library, &mut page.rows);
    Ok(page)
}

pub fn track_ids(app: &AppHandle, library: &str, query: &TrackQuery) -> AppResult<Vec<String>> {
    let other = app.state::<Others>().get(library)?;
    let ids = match &other.source {
        Source::Library(collection) => collection.track_ids(query)?,
        Source::Stick(stick) => stick.track_ids(query, Some(app.state::<Collection>().inner()))?,
    };
    Ok(ids.iter().map(|id| tag(library, id)).collect())
}

pub fn field_values(app: &AppHandle, library: &str, field: &str) -> AppResult<Vec<String>> {
    match &app.state::<Others>().get(library)?.source {
        Source::Library(collection) => collection.field_values(field),
        Source::Stick(stick) => stick.field_values(field),
    }
}

/// Ids grouped by the library they're of, each group's ids its own.
fn by_library(ids: &[String]) -> Vec<(Option<String>, Vec<String>)> {
    let mut groups: Vec<(Option<String>, Vec<String>)> = Vec::new();
    for id in ids {
        let (library, own) = untag(id);
        let library = library.map(str::to_owned);
        match groups.iter_mut().find(|(l, _)| *l == library) {
            Some((_, group)) => group.push(own.to_owned()),
            None => groups.push((library, vec![own.to_owned()])),
        }
    }
    groups
}

/// How long these tracks, of any libraries, are together.
pub fn duration_of(app: &AppHandle, ids: &[String]) -> AppResult<u64> {
    let mut secs = 0;
    for (library, own) in by_library(ids) {
        secs += match library {
            None => app.state::<Collection>().duration_of(&own)?,
            Some(library) => match &app.state::<Others>().get(&library)?.source {
                Source::Library(collection) => collection.duration_of(&own)?,
                Source::Stick(stick) => stick.duration_of(&own)?,
            },
        };
    }
    Ok(secs)
}

/// Where one track of another library is read from.
enum Track {
    Library(Arc<Other>, String),
    Stick(Arc<Other>, String),
    /// One of the master's, waiting to go onto a stick.
    Master(String),
}

fn track(app: &AppHandle, library: &str, own: &str) -> AppResult<Track> {
    let other = app.state::<Others>().get(library)?;
    Ok(match &other.source {
        Source::Library(_) => Track::Library(other.clone(), own.to_owned()),
        Source::Stick(_) => match stick::master_of(own) {
            Some(master) => Track::Master(master.to_owned()),
            None => Track::Stick(other.clone(), own.to_owned()),
        },
    })
}

fn tag_cues(library: &str, cues: &mut [crate::rekordbox::Cue]) {
    for cue in cues {
        cue.id = tag(library, &cue.id);
    }
}

pub fn preparation(app: &AppHandle, library: &str, own: &str) -> AppResult<Preparation> {
    let mut prepared = match track(app, library, own)? {
        Track::Library(other, id) => match &other.source {
            Source::Library(collection) => collection.preparation(&id)?,
            Source::Stick(_) => unreachable!(),
        },
        Track::Stick(other, id) => match &other.source {
            Source::Stick(stick) => stick.preparation(&id)?,
            Source::Library(_) => unreachable!(),
        },
        // Changed in the library it's of, not here.
        Track::Master(id) => Preparation { locked: true, ..app.state::<Collection>().preparation(&id)? },
    };
    prepared.track_id = tag(library, own);
    tag_cues(library, &mut prepared.cues);
    Ok(prepared)
}

pub fn waveform(app: &AppHandle, library: &str, own: &str) -> AppResult<Vec<u8>> {
    match track(app, library, own)? {
        Track::Library(other, id) | Track::Stick(other, id) => match &other.source {
            Source::Library(collection) => collection.waveform(&id),
            Source::Stick(stick) => stick.waveform(&id),
        },
        Track::Master(id) => app.state::<Collection>().waveform(&id),
    }
}

/// Small waveforms and cues for tracks of any libraries.
pub fn previews(app: &AppHandle, ids: &[String], prefer: u8) -> AppResult<Vec<Preview>> {
    let mut out = Vec::new();
    for (library, own) in by_library(ids) {
        let Some(library) = library else {
            out.extend(app.state::<Collection>().previews(&own, prefer)?);
            continue;
        };
        let other = app.state::<Others>().get(&library)?;
        let (masters, own): (Vec<String>, Vec<String>) = own.into_iter().partition(|id| stick::master_of(id).is_some());
        let mut previews = match &other.source {
            Source::Library(collection) => collection.previews(&own, prefer)?,
            Source::Stick(stick) => stick.previews(&own, prefer)?,
        };
        let plain: Vec<String> = masters.iter().filter_map(|m| stick::master_of(m).map(str::to_owned)).collect();
        for mut preview in app.state::<Collection>().previews(&plain, prefer)? {
            preview.track_id = stick::master_row_id(&preview.track_id);
            previews.push(preview);
        }
        for preview in &mut previews {
            preview.track_id = tag(&library, &preview.track_id);
            tag_cues(&library, &mut preview.cues);
        }
        out.extend(previews);
    }
    Ok(out)
}

pub fn decode_audio(app: &AppHandle, library: &str, own: &str, sink: impl FnMut(crate::rekordbox::Audio) -> bool) -> AppResult<()> {
    match track(app, library, own)? {
        Track::Library(other, id) | Track::Stick(other, id) => match &other.source {
            Source::Library(collection) => collection.decode_audio(&id, sink),
            Source::Stick(stick) => stick.decode_audio(&id, sink),
        },
        Track::Master(id) => app.state::<Collection>().decode_audio(&id, sink),
    }
}

/// Takes the library's tag off every id in an edit, for `library`.
fn untag_edit(edit: Edit, library: &str) -> AppResult<Edit> {
    let mut wrong = false;
    let edit = edit.map_ids(&mut |id: String| match untag(&id) {
        (Some(of), own) if of == library => own.to_owned(),
        // A collection track going onto a stick.
        (None, own) => stick::master_row_id(own),
        _ => {
            wrong = true;
            id
        }
    });
    if wrong {
        return Err(AppError::Validation("That's from another library. Drag it across to bring it in.".into()));
    }
    Ok(edit)
}

/// One change to another library: made, for a `master.db`; kept to write
/// later, for a stick.
pub fn edit(app: &AppHandle, library: &str, edit: Edit, group: Option<String>) -> AppResult<EditOutcome> {
    let other = app.state::<Others>().get(library)?;
    let edit = untag_edit(edit, library)?;
    let mut outcome = match &other.source {
        Source::Library(collection) => {
            if let Edit::AddTracks { track_ids, .. } = &edit {
                if track_ids.iter().any(|id| stick::master_of(id).is_some()) {
                    return Err(AppError::Validation("That's from another library. Drag it across to bring it in.".into()));
                }
            }
            collection.edit_in(edit, group)?
        }
        Source::Stick(stick) => {
            let created_id = stick.stage(edit)?;
            save(app);
            EditOutcome { status: stick.status()?, playlists: stick.playlists(), created_id, warning: None, skipped: 0 }
        }
    };
    tag_nodes(library, &mut outcome.playlists);
    outcome.created_id = outcome.created_id.map(|id| tag(library, &id));
    changed(app);
    Ok(outcome)
}

// --- Between libraries -----------------------------------------------------------

/// What going from one library to another did.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Moved {
    #[serde(flatten)]
    pub report: TransferReport,
    /// The library they went to.
    pub library: Option<String>,
    /// Kept to write to a stick later, rather than done.
    pub staged: bool,
}

/// A stick's tracks as they'd go into a library.
fn stick_incoming(stick: &Stick, ids: &[String]) -> Vec<Incoming> {
    ids.iter()
        .filter_map(|id| stick.track(id).ok().map(|track| (id, track)))
        .map(|(id, track)| Incoming {
            source_id: id.clone(),
            file: stick.file(&track.path),
            analysis: stick.analysis_files(&track).filter(|[dat, ..]| dat.is_file()),
            cues: stick.cue_entries(&track),
            identity: (track.master_db_id != 0 && track.master_content_id != 0)
                .then_some((track.master_db_id, track.master_content_id)),
            title: track.title,
            artist: track.artist,
            album: track.album,
            duration_sec: track.duration_sec,
            bpm_x100: track.bpm_x100,
            key: track.key,
            rating: track.rating,
            color: track.color,
            comment: track.comment,
        })
        .collect()
}

/// Tracks of a library, by their own ids, as they'd go into another.
fn incoming(app: &AppHandle, from: &Lib, ids: &[String]) -> AppResult<Vec<Incoming>> {
    match from {
        Lib::Master => app.state::<Collection>().incoming(ids),
        Lib::Other(other) => match &other.source {
            Source::Library(collection) => collection.incoming(ids),
            Source::Stick(stick) => Ok(stick_incoming(stick, ids)),
        },
    }
}

/// Says how a transfer is going to the window, as `transfer-progress`:
/// every change of stage, and the rest a few times a second.
fn progress_to(app: &AppHandle) -> impl Fn(TransferProgress) + Sync + '_ {
    let last: Mutex<Option<(Instant, &'static str)>> = Mutex::new(None);
    move |progress: TransferProgress| {
        let mut last = last.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((at, stage)) = *last {
            if stage == progress.stage && at.elapsed() < Duration::from_millis(100) {
                return;
            }
        }
        *last = Some((Instant::now(), progress.stage));
        drop(last);
        let _ = app.emit("transfer-progress", progress);
    }
}

/// Where tracks copied into the master go.
fn master_home(app: &AppHandle) -> PathBuf {
    app.state::<SettingsState>().snapshot().music_dir.join("Imported")
}

/// The library tracks go into, with where their copies go.
fn into_collection<'a>(app: &'a AppHandle, to: &'a Lib) -> Option<(&'a Collection, PathBuf)> {
    match to {
        Lib::Master => Some((app.state::<Collection>().inner(), master_home(app))),
        Lib::Other(other) => match &other.source {
            Source::Library(collection) => Some((collection, other.home.clone())),
            Source::Stick(_) => None,
        },
    }
}

fn library_id(lib: &Lib) -> Option<&str> {
    match lib {
        Lib::Master => None,
        Lib::Other(other) => Some(&other.id),
    }
}

/// Tracks from one library to another, into a playlist there or just its
/// collection. Into a stick they wait to be written; into a library they go now.
pub fn move_tracks(app: &AppHandle, track_ids: &[String], to: Option<&str>, playlist: Option<&str>) -> AppResult<Moved> {
    let from_id = one_library(None, track_ids.iter().map(String::as_str))?;
    if from_id == to {
        return Err(AppError::Validation("They're in that library already".into()));
    }
    let own: Vec<String> = track_ids.iter().map(|id| untag(id).1.to_owned()).filter(|id| stick::master_of(id).is_none()).collect();
    let from = lib(app, from_id)?;
    let target = lib(app, to)?;
    let playlist = playlist.map(|p| untag(p).1.to_owned());
    if let Lib::Other(other) = &target {
        if let Source::Stick(stick) = &other.source {
            if !matches!(from, Lib::Master) {
                return Err(AppError::Validation(
                    "Only your library's tracks go onto a stick. Bring them into your library first.".into(),
                ));
            }
            let Some(playlist) = playlist else {
                return Err(AppError::Validation("Drop them on one of the stick's playlists".into()));
            };
            let ids: Vec<String> = own.iter().map(|id| stick::master_row_id(id)).collect();
            stick.stage(Edit::AddTracks { playlist_id: playlist, track_ids: ids })?;
            save(app);
            changed(app);
            return Ok(Moved { library: to.map(str::to_owned), staged: true, ..Moved::default() });
        }
    }
    let (collection, home) = into_collection(app, &target).expect("not a stick");
    let items = incoming(app, &from, &own)?;
    let into = playlist.map_or(Destination::Collection, Destination::Playlist);
    let all = [(into, (0..items.len()).collect())];
    let mut report = collection.bring_in_with(&items, &all, &home, &progress_to(app))?;
    retag(&target, &mut report);
    after_bringing_in(app, &target, &report);
    Ok(Moved { report, library: to.map(str::to_owned), staged: false })
}

fn retag(target: &Lib, report: &mut TransferReport) {
    if let Some(id) = library_id(target) {
        report.track_ids = report.track_ids.iter().map(|t| tag(id, t)).collect();
        report.playlist_id = report.playlist_id.as_deref().map(|p| tag(id, p));
    }
}

/// Lets the rest of the app know a library took tracks in.
fn after_bringing_in(app: &AppHandle, target: &Lib, report: &TransferReport) {
    match target {
        Lib::Master => {
            let _ = app.emit("collection-changed", app.state::<crate::ingest::ArrivalQueue>().waiting());
        }
        Lib::Other(_) => changed(app),
    }
    info!("Brought in {} tracks ({} new); {} failed", report.track_ids.len(), report.added, report.failed.len());
}

/// A library's playlist tree, with each list's tracks: for going elsewhere.
struct Lists {
    nodes: Vec<PlaylistNode>,
    members: HashMap<String, Vec<String>>,
}

fn lists_of(app: &AppHandle, from: &Lib) -> AppResult<Lists> {
    let (nodes, members): (Vec<PlaylistNode>, Box<dyn Fn(&str) -> AppResult<Vec<String>>>) = match from {
        Lib::Master => {
            let master = app.state::<Collection>();
            (master.playlists()?, Box::new(move |id| app.state::<Collection>().playlist_members(id)))
        }
        Lib::Other(other) => match &other.source {
            Source::Library(collection) => (collection.playlists()?, Box::new(|id| collection.playlist_members(id))),
            Source::Stick(stick) => (stick.playlists(), Box::new(|id| stick.members(id))),
        },
    };
    let mut lists = HashMap::new();
    for node in nodes.iter().filter(|n| n.kind == PlaylistKind::Playlist) {
        lists.insert(node.id.clone(), members(&node.id)?);
    }
    Ok(Lists { nodes, members: lists })
}

impl Lists {
    fn node(&self, id: &str) -> Option<&PlaylistNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// `id` and everything under it, in the tree's order.
    fn subtree(&self, id: &str) -> Vec<&PlaylistNode> {
        let Some(at) = self.nodes.iter().position(|n| n.id == id) else { return Vec::new() };
        let depth = self.nodes[at].depth;
        std::iter::once(&self.nodes[at]).chain(self.nodes[at + 1..].iter().take_while(|n| n.depth > depth)).collect()
    }

    /// The folders from `top`'s parent down to `node`'s.
    fn folders_between(&self, top: &PlaylistNode, node: &PlaylistNode) -> Vec<String> {
        let mut names = Vec::new();
        let mut at = node.parent_id.as_deref();
        while let Some(id) = at {
            if Some(id) == top.parent_id.as_deref() {
                break;
            }
            let Some(folder) = self.node(id) else { break };
            names.push(folder.name.clone());
            at = folder.parent_id.as_deref();
        }
        names.reverse();
        names
    }
}

/// Playlists (and folders, with what's in them) from one library to
/// another: a playlist found there by the same name in the same folders is
/// made to hold what this one does, otherwise one is made. `under` is a
/// folder of the other library to put them in. Onto a stick, the master's
/// go as an export puts them, waiting to be written.
pub fn move_playlists(app: &AppHandle, playlist_ids: &[String], to: Option<&str>, under: Option<&str>) -> AppResult<Moved> {
    let from_id = one_library(None, playlist_ids.iter().map(String::as_str))?;
    if from_id == to {
        return Err(AppError::Validation("They're in that library already".into()));
    }
    let from = lib(app, from_id)?;
    let target = lib(app, to)?;
    let own: Vec<String> = playlist_ids.iter().map(|id| untag(id).1.to_owned()).collect();
    let lists = lists_of(app, &from)?;

    if let Lib::Other(other) = &target {
        if let Source::Stick(stick) = &other.source {
            if !matches!(from, Lib::Master) {
                return Err(AppError::Validation(
                    "Only your library's playlists go onto a stick. Bring them into your library first.".into(),
                ));
            }
            let mut going: Vec<ExportedList> = Vec::new();
            for id in &own {
                for node in lists.subtree(id) {
                    if node.kind == PlaylistKind::Smart || going.iter().any(|l| l.id == node.id) {
                        continue;
                    }
                    going.push(ExportedList {
                        id: node.id.clone(),
                        name: node.name.clone(),
                        parent: node.parent_id.clone().filter(|p| going.iter().any(|l| &l.id == p)),
                        folder: node.kind == PlaylistKind::Folder,
                        tracks: lists.members.get(&node.id).cloned().unwrap_or_default(),
                    });
                }
            }
            stick.stage_export(going);
            save(app);
            changed(app);
            return Ok(Moved { library: to.map(str::to_owned), staged: true, ..Moved::default() });
        }
    }

    let (collection, home) = into_collection(app, &target).expect("not a stick");
    let under = under.map(|u| untag(u).1.to_owned());
    let mut tracks: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    let mut wanted = Vec::new();
    for id in &own {
        let Some(top) = lists.node(id) else { continue };
        for node in lists.subtree(id).into_iter().filter(|n| n.kind == PlaylistKind::Playlist) {
            let members = lists.members.get(&node.id).cloned().unwrap_or_default();
            tracks.extend(members.iter().filter(|m| seen.insert((*m).clone())).cloned());
            let mut folders = lists.folders_between(top, node);
            if node.id == top.id {
                folders.clear();
            }
            wanted.push((Destination::Named { under: under.clone(), folders, name: node.name.clone() }, members));
        }
    }
    let items = incoming(app, &from, &tracks)?;
    let placements = crate::rekordbox::placements(&items, wanted);
    // A track whose file couldn't be found leaves the rest where they are.
    let mut report = collection.bring_in_with(&items, &placements, &home, &progress_to(app))?;
    retag(&target, &mut report);
    after_bringing_in(app, &target, &report);
    Ok(Moved { report, library: to.map(str::to_owned), staged: false })
}

/// Everything of another library into the master: every playlist, by the
/// same names in the same folders, and every track besides.
pub fn import_all(app: &AppHandle, id: &str) -> AppResult<Moved> {
    let from = lib(app, Some(id))?;
    let lists = lists_of(app, &from)?;
    let all: Vec<String> = match &from {
        Lib::Other(other) => match &other.source {
            Source::Library(collection) => collection.all_track_ids(),
            Source::Stick(stick) => stick.all_track_ids()?,
        },
        Lib::Master => return Err(AppError::Validation("That's your library".into())),
    };
    let mut wanted = Vec::new();
    let tops = lists.nodes.iter().filter(|n| n.parent_id.is_none());
    for top in tops {
        for node in lists.subtree(&top.id).into_iter().filter(|n| n.kind == PlaylistKind::Playlist) {
            let members = lists.members.get(&node.id).cloned().unwrap_or_default();
            let mut folders = lists.folders_between(top, node);
            if node.id != top.id {
                folders.insert(0, top.name.clone());
            }
            wanted.push((Destination::Named { under: None, folders, name: node.name.clone() }, members));
        }
    }
    let items = incoming(app, &from, &all)?;
    let placements = crate::rekordbox::placements(&items, wanted);
    let mut report = app.state::<Collection>().bring_in_with(&items, &placements, &master_home(app), &progress_to(app))?;
    retag(&Lib::Master, &mut report);
    after_bringing_in(app, &Lib::Master, &report);
    Ok(Moved { report, library: None, staged: false })
}

// --- Writing a stick ---------------------------------------------------------------

/// What writing a stick's changes did.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    /// The export of the collection's tracks and playlists, when there was one.
    pub export: Option<usb::Report>,
    /// Changes to the stick's playlists made, in each of its databases.
    pub edits: usize,
    /// What couldn't be done.
    pub problems: Vec<String>,
    pub ejected: bool,
    pub eject_error: Option<String>,
}

/// Lets go of a stick when writing it is over, however that ends.
struct Writing<'a>(&'a Others, String);

impl Drop for Writing<'_> {
    fn drop(&mut self) {
        self.0.writing.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&self.1);
    }
}

/// Puts the master's playlists a stick was last written with from it back
/// on its list of changes, so they're brought up to date with the rest.
pub fn stage_update(app: &AppHandle, id: &str) -> AppResult<OtherInfo> {
    let other = app.state::<Others>().get(id)?;
    let Source::Stick(stick) = &other.source else {
        return Err(AppError::Validation("Only a USB stick is updated from your library".into()));
    };
    let master = app.state::<Collection>();
    let db_id = master.library_db_id().unwrap_or(0);
    let ids: Vec<String> = rbl_export::Manifest::load(stick.root())
        .filter(|m| db_id != 0 && m.db_id == db_id)
        .map(|m| m.playlists.iter().filter(|p| !p.folder && !p.device_only && p.library_id != 0).map(|p| p.library_id.to_string()).collect())
        .unwrap_or_default();
    let known: HashSet<String> = master.playlists()?.into_iter().map(|p| p.id).collect();
    let ids: Vec<String> = ids.into_iter().filter(|id| known.contains(id)).collect();
    if ids.is_empty() {
        return Err(AppError::Validation(
            "This stick hasn't been written from your library, so there's nothing on it to update. Drag playlists onto it instead.".into(),
        ));
    }
    move_playlists(app, &ids, Some(id), None)?;
    info_of(&other)
}

/// Writes a stick's changes waiting, then ejects it if asked. Blocking, for
/// as long as copying the collection's tracks onto it takes.
pub fn apply(app: &AppHandle, id: &str, eject: bool, convert: Option<rbl_export::CompatibilityFormat>) -> AppResult<Applied> {
    let others = app.state::<Others>();
    let other = others.get(id)?;
    let Source::Stick(stick) = &other.source else {
        return Err(AppError::Validation("Only a USB stick has changes waiting".into()));
    };
    let root = stick.root().to_path_buf();
    if rbl_db::is_rekordbox_app_running() {
        return Err(AppError::Usb("Quit rekordbox before writing to the stick, so only one app writes to it".into()));
    }
    if !others.writing.lock().expect("writing lock poisoned").insert(id.to_owned()) {
        return Err(AppError::Usb("That stick is being written to already".into()));
    }
    let _writing = Writing(&others, id.to_owned());
    let staged = stick.staged();
    let mut applied = Applied::default();

    // The collection's first: what the changes to the stick's playlists put in them has to be on it.
    let mut lists: Vec<String> = Vec::new();
    let mut loose: Vec<String> = Vec::new();
    for change in &staged {
        match change {
            Change::Export { lists: going } => {
                // A folder stands for what's in it; its lists are given too, so only the top ones.
                let ids: HashSet<&str> = going.iter().map(|l| l.id.as_str()).collect();
                lists.extend(going.iter().filter(|l| l.parent.as_deref().is_none_or(|p| !ids.contains(p))).map(|l| l.id.clone()));
            }
            Change::Add { tracks, .. } => loose.extend(tracks.iter().filter_map(|t| match t {
                TrackRef::Master { id } => Some(id.clone()),
                TrackRef::OnStick { .. } => None,
            })),
            _ => {}
        }
    }
    lists.dedup();
    loose.sort();
    loose.dedup();
    let master = app.state::<Collection>();
    let known: HashSet<String> = master.playlists()?.into_iter().map(|p| p.id).collect();
    let gone = lists.iter().filter(|l| !known.contains(*l)).count();
    if gone > 0 {
        applied.problems.push(format!("{gone} of the playlists to write are no longer in your library"));
    }
    lists.retain(|l| known.contains(l));
    loose.retain(|t| master.rows_of(std::slice::from_ref(t)).is_ok_and(|r| !r.is_empty()));
    if !lists.is_empty() || !loose.is_empty() {
        let report = usb::write(app, &root, |c| c.export_selection_with(&lists, &loose), convert, true, false)?;
        applied.problems.extend(report.left_out.iter().cloned());
        applied.export = Some(report);
    }

    // Then the stick's playlists, the same in each of its databases.
    let edits: Vec<&Change> = staged.iter().filter(|c| !matches!(c, Change::Export { .. })).collect();
    if !edits.is_empty() {
        let master_db = master.library_db_id().unwrap_or(0);
        let formats = device::formats(&root);
        for (at, format) in formats.iter().enumerate() {
            usb::announce(app, &root, "database", at, formats.len());
            let library = device::read(&root, *format).map_err(|e| AppError::Usb(format!("The stick couldn't be read: {e}")))?;
            let (done, problems) = apply_edits(&root, *format, &library, &edits, master_db);
            applied.edits += done;
            // Each database meets the same problems: said once.
            if at == 0 {
                applied.problems.extend(problems);
            }
        }
    }

    stick.clear_staged();
    save(app);
    if let Err(e) = stick.reload() {
        warn!("Couldn't read {} again after writing it: {e}", root.display());
    }
    if eject {
        usb::announce(app, &root, "ejecting", 0, 0);
        match rbl_devices::eject::eject(&root) {
            Ok(()) => applied.ejected = true,
            Err(e) => applied.eject_error = Some(e.to_string()),
        }
    }
    changed(app);
    info!("Wrote {} changes to {}", staged.len(), root.display());
    Ok(applied)
}

/// Makes the changes to a stick's playlists in one of its databases.
/// `library` is that database as read before them: what they name is found
/// in it. Returns how many were made, and what couldn't be.
fn apply_edits(root: &Path, format: device::Format, library: &device::Library, edits: &[&Change], master_db: u64) -> (usize, Vec<String>) {
    let by_path: HashMap<String, u32> =
        library.tracks.iter().map(|t| (t.path.replace('\\', "/").trim_start_matches('/').to_lowercase(), t.id)).collect();
    let by_master: HashMap<String, u32> = library
        .tracks
        .iter()
        .filter(|t| t.master_content_id != 0 && (master_db == 0 || t.master_db_id == 0 || t.master_db_id == master_db))
        .map(|t| (t.master_content_id.to_string(), t.id))
        .collect();
    let mut made: HashMap<u32, u32> = HashMap::new();
    let resolve = |playlist: &PlaylistRef, made: &HashMap<u32, u32>| -> Option<u32> {
        match playlist {
            PlaylistRef::OnStick { path } => stick::find(library, path),
            PlaylistRef::New { number } => made.get(number).copied(),
        }
    };
    let track = |t: &TrackRef| -> Option<u32> {
        match t {
            TrackRef::OnStick { path } => by_path.get(&path.replace('\\', "/").trim_start_matches('/').to_lowercase()).copied(),
            TrackRef::Master { id } => by_master.get(id).copied(),
        }
    };
    let mut done = 0;
    let mut problems = Vec::new();
    for change in edits {
        let edit = match change {
            Change::Create { parent, name, folder, .. } => {
                let parent = match parent {
                    Some(p) => match resolve(p, &made) {
                        Some(id) => id,
                        None => {
                            problems.push(format!("“{name}” wasn't made: its folder is gone"));
                            continue;
                        }
                    },
                    None => 0,
                };
                DeviceEdit::Create { parent, name: name.clone(), folder: *folder }
            }
            Change::Rename { playlist, name } => match resolve(playlist, &made) {
                Some(id) => DeviceEdit::Rename { id, name: name.clone() },
                None => {
                    problems.push(format!("A playlist to call “{name}” is gone"));
                    continue;
                }
            },
            Change::Delete { playlist } => match resolve(playlist, &made) {
                Some(id) => DeviceEdit::Delete { id },
                None => continue,
            },
            Change::Add { playlist, tracks } | Change::Remove { playlist, tracks } => {
                let Some(id) = resolve(playlist, &made) else {
                    problems.push("A playlist to change is gone".to_owned());
                    continue;
                };
                let ids: Vec<u32> = tracks.iter().filter_map(track).collect();
                if ids.len() < tracks.len() {
                    problems.push(format!("{} tracks couldn't be found on the stick", tracks.len() - ids.len()));
                }
                if matches!(change, Change::Add { .. }) {
                    DeviceEdit::Add { playlist: id, tracks: ids, duplicates: false }
                } else {
                    DeviceEdit::Remove { playlist: id, tracks: ids }
                }
            }
            Change::Export { .. } => continue,
        };
        match device::apply(root, format, &edit) {
            Ok(result) => {
                done += 1;
                if let Change::Create { number, .. } = change {
                    made.insert(*number, result.id);
                }
            }
            Err(e) => problems.push(e.to_string()),
        }
    }
    (done, problems)
}

/// Reads a stick open here again after it was written another way (an export).
pub fn written(app: &AppHandle, root: &Path) {
    if let Some(other) = app.state::<Others>().by_path(root) {
        if let Source::Stick(stick) = &other.source {
            if let Err(e) = stick.reload() {
                warn!("Couldn't read {} again: {e}", root.display());
            }
            changed(app);
        }
    }
}

/// Forgets a stick's changes waiting.
pub fn discard(app: &AppHandle, id: &str) -> AppResult<OtherInfo> {
    let other = app.state::<Others>().get(id)?;
    if let Source::Stick(stick) = &other.source {
        stick.clear_staged();
        save(app);
    }
    changed(app);
    info_of(&other)
}

/// Takes back the last change to a stick, or undoes one in a library.
pub fn undo(app: &AppHandle, id: &str, redo: bool) -> AppResult<crate::rekordbox::UndoOutcome> {
    let other = app.state::<Others>().get(id)?;
    let mut outcome = match &other.source {
        Source::Library(collection) => {
            if redo {
                collection.redo()?
            } else {
                collection.undo()?
            }
        }
        Source::Stick(stick) => {
            let what = (!redo && stick.unstage_last()).then_some("the last change waiting for the stick");
            save(app);
            crate::rekordbox::UndoOutcome { status: stick.status()?, playlists: stick.playlists(), what, track_ids: Vec::new() }
        }
    };
    tag_nodes(id, &mut outcome.playlists);
    outcome.track_ids = outcome.track_ids.iter().map(|t| tag(id, t)).collect();
    changed(app);
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rekordbox::tests::fixture;
    use crate::rekordbox::{Sort, Source as ListSource};

    fn query(source: ListSource) -> TrackQuery {
        crate::rekordbox::tests::query(source, Sort::TrackNo)
    }

    /// Exports what's picked out of `collection` to the stick at `root`.
    fn export(collection: &Collection, root: &Path, lists: &[String], loose: &[String], keep_unlisted: bool) {
        let selection = collection.export_selection_with(lists, loose).unwrap();
        rbl_export::export_cancellable(
            root,
            &selection.tracks,
            &selection.playlists,
            &selection.my_tags,
            &rbl_export::ExportOptions { sync: Some(&selection.sync), keep_unlisted, ..Default::default() },
            &mut |_| {},
            &|| false,
        )
        .unwrap();
    }

    /// A library's track, analysed and cued, in a playlist of its own.
    fn cued_track(collection: &Collection, music: &Path, name: &str) -> (String, String) {
        let file = music.join(format!("{name}.wav"));
        crate::rekordbox::analyse::tests::click_track(&file, 124.0, 20);
        let id = collection.import(&[file], None).unwrap().track_ids[0].clone();
        collection.analyse(&id, false).unwrap();
        collection.edit(Edit::AddCue { track_id: id.clone(), slot: Some(1), position_ms: 2000, end_ms: None }).unwrap();
        let made = collection.edit(Edit::CreatePlaylist { name: name.into(), parent_id: None, folder: false }).unwrap();
        let list = made.created_id.unwrap();
        collection.edit(Edit::AddTracks { playlist_id: list.clone(), track_ids: vec![id.clone()] }).unwrap();
        (id, list)
    }

    #[test]
    fn a_stick_is_browsed_changed_in_both_its_databases_and_brought_into_a_library() {
        let (dir, master) = fixture();
        let music = dir.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        let (peak_track, peak) = cued_track(&master, &music, "Peak");
        let (loose_track, _) = cued_track(&master, &music, "Loose");
        let usb = tempfile::tempdir().unwrap();
        export(&master, usb.path(), std::slice::from_ref(&peak), &[], false);

        // Browsed as the collection is.
        let stick = Stick::open(usb.path().to_path_buf(), Vec::new()).unwrap();
        let lists = stick.playlists();
        let on_stick = lists.iter().find(|n| n.name == "Peak").expect("the playlist is on the stick");
        let page = stick.tracks(&query(ListSource::Playlist { id: on_stick.id.clone() }), 0, 10, None).unwrap();
        assert_eq!(page.total, 1);
        let row = &page.rows[0];
        assert_eq!(row.playlists, ["Peak"]);
        let prepared = stick.preparation(&row.id).unwrap();
        assert!(!prepared.beats.is_empty(), "the grid comes with it");
        assert_eq!(prepared.cues.iter().filter(|c| c.slot == Some(1)).count(), 1, "and the hot cue on B");
        assert!(!stick.waveform(&row.id).unwrap().is_empty());

        // Changed, and shown changed before it's written.
        let made = stick.stage(Edit::CreatePlaylist { name: "Saturday".into(), parent_id: None, folder: false }).unwrap().unwrap();
        stick.stage(Edit::AddTracks { playlist_id: made.clone(), track_ids: vec![row.id.clone(), stick::master_row_id(&loose_track)] }).unwrap();
        stick.stage(Edit::RenamePlaylist { id: on_stick.id.clone(), name: "Peak Time".into() }).unwrap();
        let shown: Vec<(String, bool)> = stick.playlists().into_iter().map(|n| (n.name, n.pending)).collect();
        assert_eq!(shown, [("Saturday".to_owned(), true), ("Peak Time".to_owned(), true)]);
        let saturday = stick.tracks(&query(ListSource::Playlist { id: made.clone() }), 0, 10, Some(&master)).unwrap();
        assert_eq!(saturday.rows.iter().map(|r| r.pending).collect::<Vec<_>>(), [false, true]);
        assert!(stick.stage(Edit::SetRating { track_id: row.id.clone(), stars: 3 }).is_err(), "its tracks aren't changed here");

        // Written: the collection's track first, then the changes, to both databases alike.
        export(&master, usb.path(), &[], std::slice::from_ref(&loose_track), true);
        let staged = stick.staged();
        let edits: Vec<&Change> = staged.iter().collect();
        let db_id = master.library_db_id().unwrap();
        let formats = device::formats(usb.path());
        assert_eq!(formats.len(), 2);
        for format in formats {
            let library = device::read(usb.path(), format).unwrap();
            let (done, problems) = apply_edits(usb.path(), format, &library, &edits, db_id);
            assert!(problems.is_empty(), "{problems:?}");
            assert_eq!(done, 3);
            let after = device::read(usb.path(), format).unwrap();
            let names: Vec<&str> = after.nodes.iter().map(|n| n.name.as_str()).collect();
            assert!(names.contains(&"Saturday") && names.contains(&"Peak Time"), "{format:?}: {names:?}");
            let saturday = after.nodes.iter().find(|n| n.name == "Saturday").unwrap();
            assert_eq!(saturday.tracks.len(), 2, "{format:?}");
        }
        stick.clear_staged();
        stick.reload().unwrap();
        assert!(stick.playlists().iter().all(|n| !n.pending));
        assert!(rbl_export::verify(usb.path()).unwrap().is_ok(), "the stick still reads back right");

        // Into another library, with what the stick knows of them.
        let (_other_dir, other) = fixture();
        let saturday = stick.playlists().into_iter().find(|n| n.name == "Saturday").unwrap();
        let ids = stick.members(&saturday.id).unwrap();
        let items = stick_incoming(&stick, &ids);
        assert_eq!(items.len(), 2);
        let copy_to = usb.path().join("..").join("copies");
        let into = Destination::Named { under: None, folders: Vec::new(), name: "Saturday".into() };
        let report = other.bring_in(&items, &into, &copy_to).unwrap();
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(report.added, 2);
        for id in &report.track_ids {
            let prepared = other.preparation(id).unwrap();
            assert!(!prepared.beats.is_empty(), "the grid came in");
            assert_eq!(prepared.cues.iter().filter(|c| c.slot == Some(1)).count(), 1, "the cue came in");
        }
        assert_eq!(other.playlist_members(report.playlist_id.as_ref().unwrap()).unwrap(), report.track_ids);

        // And back into the library it came from: the same tracks, found by where they came from.
        let back = master.bring_in(&items, &Destination::Collection, &copy_to).unwrap();
        assert_eq!((back.added, back.updated), (0, 2), "{:?}", back.failed);
        let mut expected = vec![peak_track, loose_track];
        let mut got = back.track_ids.clone();
        expected.sort();
        got.sort();
        assert_eq!(got, expected);
    }

    #[test]
    fn a_stick_sorts_and_filters_by_playlists() {
        let (dir, master) = fixture();
        let music = dir.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        let (_, peak) = cued_track(&master, &music, "Peak");
        let (loose_track, _) = cued_track(&master, &music, "Loose");
        let usb = tempfile::tempdir().unwrap();
        // Peak's track in its playlist; Loose's on the stick in none.
        export(&master, usb.path(), std::slice::from_ref(&peak), std::slice::from_ref(&loose_track), false);
        let stick = Stick::open(usb.path().to_path_buf(), Vec::new()).unwrap();
        let listed = |q: &TrackQuery| -> Vec<Vec<String>> {
            stick.tracks(q, 0, 10, None).unwrap().rows.into_iter().map(|r| r.playlists).collect()
        };

        // In none sorts lowest: first going up, last coming down.
        let mut q = crate::rekordbox::tests::query(ListSource::Collection, Sort::Playlists);
        assert_eq!(listed(&q), [vec![], vec!["Peak".to_owned()]]);
        q.descending = true;
        assert_eq!(listed(&q), [vec!["Peak".to_owned()], vec![]]);

        let mut q = query(ListSource::Collection);
        q.playlisted = Some(false);
        assert_eq!(listed(&q), [Vec::<String>::new()]);
        q.playlisted = Some(true);
        assert_eq!(listed(&q), [vec!["Peak".to_owned()]]);
    }

    #[test]
    fn ids_say_which_library_they_are_of() {
        assert_eq!(untag("123"), (None, "123"));
        assert_eq!(untag(&tag("lib2", "123")), (Some("lib2"), "123"));
        assert_eq!(one_library(None, ["lib1~1", "lib1~2"]).unwrap(), Some("lib1"));
        assert_eq!(one_library(None, ["1", "2"]).unwrap(), None);
        assert!(one_library(None, ["1", "lib1~2"]).is_err());
    }

    #[test]
    fn a_library_is_found_inside_a_folder_around_it_or_one_level_in() {
        let dir = tempfile::tempdir().unwrap();
        let drive = dir.path().join("DRIVE");
        std::fs::create_dir_all(drive.join("PIONEER/Master")).unwrap();
        std::fs::write(drive.join("PIONEER/Master/master.db"), b"").unwrap();
        let found = |at: &Path| find_in(at).into_iter().map(|(_, p)| p).collect::<Vec<_>>();
        let db = drive.join("PIONEER/Master/master.db");
        assert_eq!(found(&drive), [db.clone()]);
        assert_eq!(found(&drive.join("PIONEER")), [db.clone()]);
        assert_eq!(found(dir.path()), [db.clone()]);
        assert_eq!(name_of(Kind::Library, &db), "DRIVE");
        assert!(find_in(&dir.path().join("nowhere")).is_empty());
    }
}
