//! The rekordbox collection: rekordbox's own `master.db`, read through
//! rbxport's `rbl-db` (the encrypted database) and `rbl-index` (the library
//! in memory as columns, where sorting and searching happen).
//!
//! The database is opened, indexed and closed again, and opened for each
//! change in turn (see [`edit`]), so nothing here holds it open while
//! rekordbox runs.

mod analyse;
mod edit;
mod ingest;
mod prepare;

pub use edit::{Edit, EditOutcome};
pub use ingest::{Arrival, ImportReport};
pub use prepare::{Audio, GridChange, Preparation};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};

use rbl_db::{LibraryLocation, OpenMode};
use rbl_index::{SortColumn, TrackSource, View, ViewSpec, NO_ID};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// rekordbox's `FileType` codes.
fn file_type_name(code: u8) -> &'static str {
    match code {
        1 => "mp3",
        4 => "m4a",
        5 => "flac",
        11 => "wav",
        12 => "aiff",
        _ => "",
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionStatus {
    pub master_db: PathBuf,
    pub tracks: usize,
    pub playlists: usize,
    /// Changes are refused while rekordbox (or its agent) is running.
    pub rekordbox_running: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PlaylistKind {
    Playlist,
    Folder,
    /// An intelligent playlist: its tracks are whatever its rule matches.
    Smart,
}

/// A row of the playlist tree, in display order.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistNode {
    /// `djmdPlaylist.ID`, as text: ids don't fit a JavaScript number safely.
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub depth: usize,
    pub kind: PlaylistKind,
    /// Tracks in a playlist; None for folders and intelligent playlists.
    pub count: Option<usize>,
}

/// Which tracks to list.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Source {
    Collection,
    Playlist { id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Sort {
    /// A playlist's own order; the collection's stored order.
    TrackNo,
    Title,
    Artist,
    Album,
    Genre,
    Bpm,
    Key,
    Duration,
    Rating,
    DateAdded,
    /// Where the file is.
    Path,
    /// The names of the playlists a track is in; tracks in none come last.
    Playlists,
}

impl From<Sort> for SortColumn {
    fn from(sort: Sort) -> Self {
        match sort {
            Sort::TrackNo => Self::TrackNo,
            Sort::Title => Self::Title,
            Sort::Artist => Self::Artist,
            Sort::Album => Self::Album,
            Sort::Genre => Self::Genre,
            Sort::Bpm => Self::Bpm,
            Sort::Key => Self::Key,
            Sort::Duration => Self::Duration,
            Sort::Rating => Self::Rating,
            Sort::DateAdded => Self::DateAdded,
            Sort::Path => Self::Location,
            // Sorted here, from the list's own order: see `open_view`.
            Sort::Playlists => Self::TrackNo,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackQuery {
    pub source: Source,
    pub sort: Sort,
    pub descending: bool,
    #[serde(default)]
    pub query: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackRow {
    /// `djmdContent.ID`, as text.
    pub id: String,
    /// Position in the playlist (its `TrackNo`), or in the list.
    pub position: u32,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub key: String,
    pub bpm: Option<f64>,
    pub duration_secs: u32,
    /// Stars, 0 to 5.
    pub rating: u8,
    /// rekordbox's colour, 1 to 8; 0 for none.
    pub color: u8,
    pub date_added: String,
    pub format: &'static str,
    /// kbps
    pub bitrate: u32,
    pub path: String,
    pub comment: String,
    /// The playlists the track is in, by name. Intelligent playlists aren't counted.
    pub playlists: Vec<String>,
    /// Whether the track has a beat grid and waveforms.
    pub analysed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackPage {
    /// Tracks in the whole list, not just this page.
    pub total: usize,
    pub offset: usize,
    pub rows: Vec<TrackRow>,
}

/// A file's path as this system writes it, from rekordbox's record of it.
/// rekordbox on Windows stores `C:/Music/a.flac`, where everything else here
/// (settings, jobs, the file dialogs) has `C:\Music\a.flac`; paths are
/// compared as text in places, so they have to agree.
pub(crate) fn local_path(stored: &str) -> String {
    if cfg!(windows) {
        stored.replace('/', "\\")
    } else {
        stored.to_owned()
    }
}

struct Loaded {
    index: rbl_index::Library,
    location: LibraryLocation,
    /// Track ids by the path of their file.
    by_path: HashMap<String, String>,
    /// For each track's row, the playlists it's in, as indices into the index's playlists.
    memberships: Vec<Vec<u32>>,
}

impl Loaded {
    /// The names of the playlists the track at `row` is in, in the tree's stored order.
    fn playlist_names(&self, row: usize) -> Vec<String> {
        let playlists = self.index.playlists();
        self.memberships
            .get(row)
            .map(|lists| lists.iter().map(|&i| playlists.name(i as usize).to_owned()).collect())
            .unwrap_or_default()
    }
}

/// The collection as last read from `master.db`, and the list being browsed.
pub struct Collection {
    loaded: RwLock<Option<Arc<Loaded>>>,
    /// The last list opened, so scrolling it doesn't sort it again per page.
    view: Mutex<Option<(TrackQuery, Arc<View>)>>,
    /// Where `master.db` is copied before the first change of a session.
    backup_dir: PathBuf,
    backed_up: AtomicBool,
    /// Held for the length of a change: one at a time.
    editing: Mutex<()>,
}

fn rekordbox_err(e: impl std::fmt::Display) -> AppError {
    AppError::Rekordbox(e.to_string())
}

impl Collection {
    pub fn new(backup_dir: PathBuf) -> Self {
        Self {
            loaded: RwLock::new(None),
            view: Mutex::new(None),
            backup_dir,
            backed_up: AtomicBool::new(false),
            editing: Mutex::new(()),
        }
    }

    /// Reads the installed library afresh. Blocking.
    pub fn reload(&self) -> AppResult<CollectionStatus> {
        self.load_from(rbl_db::detect().map_err(rekordbox_err)?)
    }

    /// Reads the library at `location`. Blocking.
    pub fn load_from(&self, location: LibraryLocation) -> AppResult<CollectionStatus> {
        let db = rbl_db::Library::open(location.clone(), OpenMode::ReadOnly).map_err(rekordbox_err)?;
        let (index, _) = rbl_index::load(&db).map_err(rekordbox_err)?;
        drop(db);
        let by_path = (0..index.len())
            .map(|row| (local_path(index.folder_path.get(row)), index.ids[row].to_string()))
            .collect();
        let mut memberships = vec![Vec::new(); index.len()];
        {
            let playlists = index.playlists();
            for (list, members) in playlists.members.iter().enumerate() {
                if playlists.is_folder(list) || playlists.is_smart(list) {
                    continue;
                }
                for &row in members {
                    if let Some(lists) = memberships.get_mut(row as usize) {
                        // A track twice in a playlist is named once.
                        if lists.last() != Some(&(list as u32)) {
                            lists.push(list as u32);
                        }
                    }
                }
            }
        }
        *self.view.lock().expect("view lock poisoned") = None;
        *self.loaded.write().expect("collection lock poisoned") =
            Some(Arc::new(Loaded { index, location, by_path, memberships }));
        self.status()
    }

    /// The library as last read, reading it now if it hasn't been. Blocking.
    fn loaded_or_read(&self) -> AppResult<Arc<Loaded>> {
        if self.loaded.read().expect("collection lock poisoned").is_none() {
            self.reload()?;
        }
        self.loaded()
    }

    /// The track whose file is at `path`, if it's in the collection.
    pub fn track_at(&self, path: &Path) -> Option<String> {
        self.loaded().ok()?.by_path.get(path.to_string_lossy().as_ref()).cloned()
    }

    /// Where a track's file is.
    pub fn path_of(&self, track_id: &str) -> Option<PathBuf> {
        let loaded = self.loaded().ok()?;
        let row = loaded.index.row_of(track_id)?;
        Some(PathBuf::from(local_path(loaded.index.folder_path.get(row as usize))))
    }

    /// Every track whose file is under `dir`, with its path. Reads the
    /// library if it hasn't been; empty when there is none. Blocking.
    pub fn tracks_under(&self, dir: &Path) -> Vec<(String, PathBuf)> {
        let Ok(loaded) = self.loaded_or_read() else { return Vec::new() };
        loaded
            .by_path
            .iter()
            .filter(|(path, _)| Path::new(path).starts_with(dir))
            .map(|(path, id)| (id.clone(), PathBuf::from(path)))
            .collect()
    }

    /// The file of every track that has one of its own (streamed tracks
    /// don't). Reads the library if it hasn't been; empty when there is
    /// none. Blocking.
    pub fn track_files(&self) -> Vec<PathBuf> {
        let Ok(loaded) = self.loaded_or_read() else { return Vec::new() };
        loaded.by_path.keys().map(PathBuf::from).filter(|path| path.is_absolute()).collect()
    }

    fn loaded(&self) -> AppResult<Arc<Loaded>> {
        self.loaded
            .read()
            .expect("collection lock poisoned")
            .clone()
            .ok_or_else(|| AppError::Rekordbox("The rekordbox library isn't open".into()))
    }

    pub fn status(&self) -> AppResult<CollectionStatus> {
        let loaded = self.loaded()?;
        let playlists = loaded.index.playlists();
        Ok(CollectionStatus {
            master_db: loaded.location.master_db.clone(),
            tracks: loaded.index.len(),
            playlists: (0..playlists.len()).filter(|&i| !playlists.is_folder(i)).count(),
            rekordbox_running: rbl_db::is_rekordbox_running(),
        })
    }

    /// The playlist tree, flattened depth first in rekordbox's order.
    pub fn playlists(&self) -> AppResult<Vec<PlaylistNode>> {
        let loaded = self.loaded()?;
        let playlists = loaded.index.playlists();
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); playlists.len()];
        let mut roots = Vec::new();
        for (i, &parent) in playlists.parent.iter().enumerate() {
            match children.get_mut(parent as usize) {
                Some(siblings) if parent != NO_ID => siblings.push(i),
                _ => roots.push(i),
            }
        }
        let by_seq = |list: &mut Vec<usize>| list.sort_by_key(|&i| playlists.seq[i]);
        by_seq(&mut roots);
        children.iter_mut().for_each(by_seq);

        let mut nodes = Vec::with_capacity(playlists.len());
        // Depth first, without recursion: a stack of (index, depth).
        let mut stack: Vec<(usize, usize)> = roots.into_iter().rev().map(|i| (i, 0)).collect();
        while let Some((i, depth)) = stack.pop() {
            let kind = if playlists.is_folder(i) {
                PlaylistKind::Folder
            } else if playlists.is_smart(i) {
                PlaylistKind::Smart
            } else {
                PlaylistKind::Playlist
            };
            let parent = playlists.parent[i];
            nodes.push(PlaylistNode {
                id: playlists.ids[i].to_string(),
                name: playlists.name(i).to_owned(),
                parent_id: (parent != NO_ID).then(|| playlists.ids[parent as usize].to_string()),
                depth,
                kind,
                count: (kind == PlaylistKind::Playlist).then(|| playlists.members[i].len()),
            });
            stack.extend(children[i].iter().rev().map(|&child| (child, depth + 1)));
        }
        Ok(nodes)
    }

    fn open_view(&self, loaded: &Loaded, query: &TrackQuery) -> AppResult<Arc<View>> {
        let mut cached = self.view.lock().expect("view lock poisoned");
        if let Some((key, view)) = cached.as_ref() {
            if key == query {
                return Ok(view.clone());
            }
        }
        let source = match &query.source {
            Source::Collection => TrackSource::Collection,
            Source::Playlist { id } => {
                let playlists = loaded.index.playlists();
                let index = id
                    .parse()
                    .ok()
                    .and_then(|id| playlists.index_of(id))
                    .ok_or_else(|| AppError::Rekordbox("That playlist no longer exists".into()))?;
                if playlists.is_folder(index) {
                    TrackSource::PlaylistFolder(index)
                } else if playlists.is_smart(index) {
                    TrackSource::SmartPlaylist(index)
                } else {
                    TrackSource::Playlist(index)
                }
            }
        };
        let by_playlists = query.sort == Sort::Playlists;
        let mut view = loaded.index.open_view(&ViewSpec {
            source,
            sort: query.sort.into(),
            // The playlists sort below starts from the list's own order.
            descending: query.descending && !by_playlists,
            query: query.query.trim().to_owned(),
            filter: Default::default(),
        });
        if by_playlists {
            let keys: HashMap<u32, String> = view
                .rows
                .iter()
                .map(|&row| (row, loaded.playlist_names(row as usize).join(", ").to_lowercase()))
                .collect();
            // Stable, so tracks in the same playlists keep the list's order.
            view.rows.sort_by(|a, b| {
                let (a, b) = (&keys[a], &keys[b]);
                let named = if query.descending { b.cmp(a) } else { a.cmp(b) };
                a.is_empty().cmp(&b.is_empty()).then(named)
            });
        }
        let view = Arc::new(view);
        *cached = Some((query.clone(), view.clone()));
        Ok(view)
    }

    /// The id of every track in a list, in its order: for selecting them all.
    pub fn track_ids(&self, query: &TrackQuery) -> AppResult<Vec<String>> {
        let loaded = self.loaded()?;
        let view = self.open_view(&loaded, query)?;
        Ok(view.window(0, view.len()).iter().map(|&row| loaded.index.ids[row as usize].to_string()).collect())
    }

    /// `limit` rows of a list from `offset`.
    pub fn tracks(&self, query: &TrackQuery, offset: usize, limit: usize) -> AppResult<TrackPage> {
        let loaded = self.loaded()?;
        let view = self.open_view(&loaded, query)?;
        let index = &loaded.index;
        let rows = view
            .window(offset, limit)
            .iter()
            .enumerate()
            .map(|(i, &row)| {
                let r = row as usize;
                let bpm = index.bpm_x100[r];
                TrackRow {
                    id: index.ids[r].to_string(),
                    position: view.track_no_at(offset + i),
                    title: index.title.get(r).to_owned(),
                    artist: index.artist_name(row).to_owned(),
                    album: index.album_name(row).to_owned(),
                    genre: index.genre_name(row).to_owned(),
                    key: index.key_name(row).to_owned(),
                    bpm: (bpm > 0).then(|| f64::from(bpm) / 100.0),
                    duration_secs: index.length_sec[r],
                    rating: index.rating[r],
                    color: index.color[r],
                    date_added: index.date_added.get(r).to_owned(),
                    format: file_type_name(index.file_type[r]),
                    bitrate: index.bitrate[r],
                    path: local_path(index.folder_path.get(r)),
                    comment: index.comment.get(r).to_owned(),
                    playlists: loaded.playlist_names(r),
                    analysed: analyse::is_analysed(index.analysed[r]),
                }
            })
            .collect();
        Ok(TrackPage { total: view.len(), offset, rows })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rbl_db::fixture::Shape;

    pub(super) fn query(source: Source, sort: Sort) -> TrackQuery {
        TrackQuery { source, sort, descending: false, query: String::new() }
    }

    /// A fixture library with rekordbox's real schema, in a temp folder.
    pub(super) fn fixture() -> (tempfile::TempDir, Collection) {
        let dir = tempfile::tempdir().unwrap();
        let location = rbl_db::fixture::build(dir.path(), Shape { tracks: 50, ..Shape::default() }).unwrap();
        let collection = Collection::new(dir.path().join("backups"));
        collection.load_from(location).unwrap();
        (dir, collection)
    }

    #[test]
    fn lists_every_id_of_a_list_in_its_order() {
        let (_dir, collection) = fixture();
        let q = query(Source::Collection, Sort::Title);
        let ids = collection.track_ids(&q).unwrap();
        let rows: Vec<String> = collection.tracks(&q, 0, 1000).unwrap().rows.into_iter().map(|r| r.id).collect();
        assert_eq!(ids.len(), collection.status().unwrap().tracks);
        assert_eq!(ids, rows);
    }

    #[test]
    fn lists_the_collection_in_pages() {
        let (_dir, collection) = fixture();
        let status = collection.status().unwrap();
        assert_eq!(status.tracks, 50);

        let q = query(Source::Collection, Sort::Title);
        let first = collection.tracks(&q, 0, 20).unwrap();
        assert_eq!(first.total, 50);
        assert_eq!(first.rows.len(), 20);
        let rest = collection.tracks(&q, 40, 20).unwrap();
        assert_eq!(rest.rows.len(), 10);

        let titles: Vec<_> = first.rows.iter().map(|r| r.title.to_lowercase()).collect();
        let mut sorted = titles.clone();
        sorted.sort();
        assert_eq!(titles, sorted);
    }

    #[test]
    fn search_narrows_the_list() {
        let (_dir, collection) = fixture();
        let all = collection.tracks(&query(Source::Collection, Sort::Title), 0, 1).unwrap();
        let title = all.rows[0].title.clone();
        let mut q = query(Source::Collection, Sort::Title);
        q.query = title.clone();
        let found = collection.tracks(&q, 0, 100).unwrap();
        assert!(found.total >= 1 && found.total <= all.total);
        assert!(found.rows.iter().any(|r| r.title == title));
    }

    #[test]
    fn tracks_name_their_playlists_and_sort_by_them() {
        let (_dir, collection) = fixture();
        let nodes = collection.playlists().unwrap();
        let node = nodes.iter().find(|n| n.kind == PlaylistKind::Playlist && n.count.unwrap_or(0) > 0).expect("a playlist with tracks");
        let q = query(Source::Playlist { id: node.id.clone() }, Sort::TrackNo);
        for row in collection.tracks(&q, 0, 1000).unwrap().rows {
            assert!(row.playlists.contains(&node.name), "{} names {}", row.title, node.name);
        }

        let all = collection.tracks(&query(Source::Collection, Sort::Playlists), 0, 1000).unwrap();
        assert_eq!(all.total, 50);
        let keys: Vec<String> = all.rows.iter().map(|r| r.playlists.join(", ").to_lowercase()).collect();
        let named: Vec<&String> = keys.iter().take_while(|k| !k.is_empty()).collect();
        assert!(!named.is_empty());
        assert!(named.windows(2).all(|pair| pair[0] <= pair[1]));
        // Tracks in no playlist come last.
        assert!(keys[named.len()..].iter().all(String::is_empty));

        let by_path = collection.tracks(&query(Source::Collection, Sort::Path), 0, 1000).unwrap();
        assert_eq!(by_path.total, 50);
    }

    #[test]
    fn playlists_list_their_tracks() {
        let (_dir, collection) = fixture();
        let nodes = collection.playlists().unwrap();
        for node in nodes.iter().filter(|n| n.kind == PlaylistKind::Playlist) {
            let q = query(Source::Playlist { id: node.id.clone() }, Sort::TrackNo);
            let page = collection.tracks(&q, 0, 1000).unwrap();
            assert_eq!(Some(page.total), node.count, "playlist {}", node.name);
        }
        // Children follow their parent, one level deeper.
        for (i, node) in nodes.iter().enumerate() {
            if let Some(parent) = &node.parent_id {
                let at = nodes.iter().position(|n| &n.id == parent).expect("parent listed");
                assert!(at < i);
                assert_eq!(nodes[at].depth + 1, node.depth);
            }
        }
    }

    #[test]
    fn unknown_playlist_is_an_error() {
        let (_dir, collection) = fixture();
        let q = query(Source::Playlist { id: "999999999".into() }, Sort::TrackNo);
        assert!(collection.tracks(&q, 0, 10).is_err());
    }

    /// Reads a copy of the installed rekordbox library; the original is only
    /// copied, never opened. Run with `cargo test live_collection -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_collection() {
        let installed = rbl_db::detect().expect("rekordbox library");
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("master.db");
        std::fs::copy(&installed.master_db, &copy).unwrap();
        let collection = Collection::new(dir.path().join("backups"));
        let status = collection
            .load_from(LibraryLocation { master_db: copy, is_real_install: false, ..installed })
            .unwrap();
        println!("{} tracks, {} playlists", status.tracks, status.playlists);

        let page = collection.tracks(&query(Source::Collection, Sort::Title), 0, 5).unwrap();
        assert_eq!(page.total, status.tracks);
        let analysed = collection
            .tracks(&query(Source::Collection, Sort::Bpm), 0, 100_000)
            .unwrap()
            .rows
            .iter()
            .filter(|r| r.bpm.is_some() && !r.key.is_empty())
            .count();
        println!("{analysed} with BPM and key");
        for node in collection.playlists().unwrap() {
            let q = query(Source::Playlist { id: node.id.clone() }, Sort::TrackNo);
            let total = collection.tracks(&q, 0, 1).unwrap().total;
            println!("{}{:?} ({total} tracks)", "  ".repeat(node.depth), node.kind);
        }
    }
}
