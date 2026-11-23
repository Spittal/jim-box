//! A library on a USB stick, browsed beside the collection: the stick's own
//! database read through rbxport's `rbl-export` (`device_library`), shown as
//! the collection is (the same playlist tree and track rows), with the
//! changes waiting to be written to it laid over what it holds.
//!
//! Nothing here writes to the stick. A change made to it is kept as a
//! [`Change`] until the user applies them all at once (see `others`), so
//! each names what it changes in a way that survives the stick being
//! written in between: a playlist by its place in the tree, a track by its
//! file.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use rbl_export::device_library as device;
use rbl_index::device::{DeviceRow, DeviceView};
use rbl_index::{SearchField, SortColumn, TrackSource, ViewSpec};
use serde::{Deserialize, Serialize};

use super::prepare::{self, HotCueColors, WavePreviews};
use super::{
    Audio, Collection, CollectionStatus, Cue, Edit, PlaylistKind, PlaylistNode, Preparation, Preview, Sort, Source,
    TrackPage, TrackQuery, TrackRow,
};
use crate::error::{AppError, AppResult};

/// A playlist or folder of the stick, named so it's found again after the
/// stick has been written: by its place in the tree, not its id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PlaylistRef {
    /// One the stick holds: the names from the top down, each with how
    /// many siblings before it have the same name.
    OnStick { path: Vec<(String, usize)> },
    /// One made here and not written yet, by its number.
    New { number: u32 },
}

/// A track to put in a playlist of the stick.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TrackRef {
    /// One the stick holds, by its file's path on the stick.
    OnStick { path: String },
    /// One of the collection's, which goes onto the stick with it.
    Master { id: String },
}

/// A playlist or folder of the collection going onto the stick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedList {
    /// Its id in the collection.
    pub id: String,
    pub name: String,
    /// The folder above it, when that is going too.
    pub parent: Option<String>,
    pub folder: bool,
    /// Its tracks, by their ids in the collection.
    pub tracks: Vec<String>,
}

/// A change to the stick waiting to be written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Change {
    /// A playlist or folder, at the top of `parent` (the top level for None).
    Create { number: u32, parent: Option<PlaylistRef>, name: String, folder: bool },
    Rename { playlist: PlaylistRef, name: String },
    /// A playlist, or a folder and everything in it. The tracks stay.
    Delete { playlist: PlaylistRef },
    Add { playlist: PlaylistRef, tracks: Vec<TrackRef> },
    Remove { playlist: PlaylistRef, tracks: Vec<TrackRef> },
    /// The collection's playlists, written as an export writes them: under
    /// the folders they're in there, beside what the stick holds.
    Export { lists: Vec<ExportedList> },
}

/// What the changes waiting add up to, in a few numbers.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedSummary {
    /// How many changes there are.
    pub changes: usize,
    pub playlists_made: usize,
    pub renamed: usize,
    pub deleted: usize,
    /// Playlists of the collection to write.
    pub exported: usize,
    /// Entries added to playlists, and taken out.
    pub added: usize,
    pub removed: usize,
    /// The collection's tracks to copy onto the stick, each once.
    pub tracks_to_copy: usize,
}

/// The stick as read.
struct Read {
    library: device::Library,
    /// Each track's place in `library.tracks`, by id.
    at: HashMap<u32, usize>,
    /// Track ids by their path, folded so case doesn't count.
    by_path: HashMap<String, u32>,
}

/// One track of a list as shown.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Shown {
    Stick(u32),
    Master(String),
}

/// A playlist or folder as shown: what the stick holds, with the changes.
#[derive(Debug, Clone)]
struct Node {
    id: String,
    parent: Option<String>,
    name: String,
    folder: bool,
    tracks: Vec<Shown>,
    pending: bool,
}

/// The tree as shown, in its order.
#[derive(Default)]
struct Tree {
    nodes: Vec<Node>,
    depth: Vec<usize>,
    /// For each stick track, the playlists it's in: (id, name).
    memberships: HashMap<u32, Vec<(String, String)>>,
}

impl Tree {
    fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// The tracks a list shows: a playlist's own; for a folder, those of
    /// every playlist under it, each once.
    fn tracks_of(&self, id: &str) -> AppResult<Vec<Shown>> {
        let at = self
            .nodes
            .iter()
            .position(|n| n.id == id)
            .ok_or_else(|| AppError::Validation("That playlist is no longer on the stick".into()))?;
        let node = &self.nodes[at];
        if !node.folder {
            return Ok(node.tracks.clone());
        }
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for under in self.nodes[at + 1..].iter().zip(&self.depth[at + 1..]).take_while(|(_, d)| **d > self.depth[at]) {
            for track in &under.0.tracks {
                if seen.insert(track.clone()) {
                    out.push(track.clone());
                }
            }
        }
        Ok(out)
    }
}

/// A row of a list: which track, and its place in its playlist.
#[derive(Debug, Clone)]
struct Listed {
    track: Shown,
    position: u32,
}

/// A USB stick's library, read when opened and again when asked.
pub struct Stick {
    root: PathBuf,
    read: RwLock<Option<Arc<Read>>>,
    staged: Mutex<Vec<Change>>,
    /// The tree as shown, made again when the stick is read or a change staged.
    tree: RwLock<Arc<Tree>>,
    /// The last list asked for, so scrolling it doesn't sort it per page.
    view: Mutex<Option<(TrackQuery, Arc<Vec<Listed>>)>>,
    wave_previews: WavePreviews,
}

fn stick_err(e: impl std::fmt::Display) -> AppError {
    AppError::Validation(format!("The stick couldn't be read: {e}"))
}

/// A path on the stick folded for comparing: case and slashes aside.
fn path_key(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches('/').to_lowercase()
}

/// A stick track's id as the interface has it, and back.
fn stick_id(id: u32) -> String {
    id.to_string()
}

/// The id a collection track waiting to go onto the stick is listed under.
pub fn master_row_id(id: &str) -> String {
    format!("m{id}")
}

/// The collection track a listed id stands for, if it's one waiting to go on.
pub fn master_of(id: &str) -> Option<&str> {
    id.strip_prefix('m')
}

fn new_id(number: u32) -> String {
    format!("new{number}")
}

fn exported_id(id: &str) -> String {
    format!("x{id}")
}

impl Stick {
    /// The stick (or folder) at `root`, which holds an export. Reads it. Blocking.
    pub fn open(root: PathBuf, staged: Vec<Change>) -> AppResult<Self> {
        let stick = Self {
            root,
            read: RwLock::new(None),
            staged: Mutex::new(staged),
            tree: RwLock::default(),
            view: Mutex::new(None),
            wave_previews: Mutex::default(),
        };
        stick.reload()?;
        Ok(stick)
    }

    /// Whether `root` holds a library a stick can: an export.
    pub fn holds_export(root: &Path) -> bool {
        !device::formats(root).is_empty()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Reads the stick afresh. Blocking.
    pub fn reload(&self) -> AppResult<()> {
        // Device Library first, as rekordbox lists them: every player reads it.
        let format = *device::formats(&self.root)
            .first()
            .ok_or_else(|| AppError::Validation(format!("There's no rekordbox export on {}", self.root.display())))?;
        let library = device::read(&self.root, format).map_err(stick_err)?;
        let at = library.tracks.iter().enumerate().map(|(at, t)| (t.id, at)).collect();
        let by_path = library.tracks.iter().map(|t| (path_key(&t.path), t.id)).collect();
        *self.read.write().expect("stick lock poisoned") = Some(Arc::new(Read { library, at, by_path }));
        self.rebuild();
        Ok(())
    }

    fn read(&self) -> AppResult<Arc<Read>> {
        self.read.read().expect("stick lock poisoned").clone().ok_or_else(|| stick_err("not read yet"))
    }

    fn tree(&self) -> Arc<Tree> {
        self.tree.read().expect("stick lock poisoned").clone()
    }

    /// Lays the changes over what the stick holds, again.
    fn rebuild(&self) {
        let Ok(read) = self.read() else { return };
        let staged = self.staged.lock().expect("staged lock poisoned").clone();
        let tree = shown_tree(&read, &staged);
        *self.tree.write().expect("stick lock poisoned") = Arc::new(tree);
        *self.view.lock().expect("view lock poisoned") = None;
    }

    // --- What's shown --------------------------------------------------------

    pub fn status(&self) -> AppResult<CollectionStatus> {
        let read = self.read()?;
        Ok(CollectionStatus {
            master_db: self.root.clone(),
            tracks: read.library.tracks.len(),
            playlists: self.tree().nodes.iter().filter(|n| !n.folder).count(),
            rekordbox_running: rbl_db::is_rekordbox_app_running(),
        })
    }

    pub fn playlists(&self) -> Vec<PlaylistNode> {
        let tree = self.tree();
        tree.nodes
            .iter()
            .zip(&tree.depth)
            .map(|(node, &depth)| PlaylistNode {
                id: node.id.clone(),
                name: node.name.clone(),
                parent_id: node.parent.clone(),
                depth,
                kind: if node.folder { PlaylistKind::Folder } else { PlaylistKind::Playlist },
                count: (!node.folder).then_some(node.tracks.len()),
                pending: node.pending,
            })
            .collect()
    }

    /// A list's rows, ordered and searched, made once for each query.
    fn listing(&self, query: &TrackQuery, master: Option<&Collection>) -> AppResult<Arc<Vec<Listed>>> {
        if let Some((asked, listed)) = self.view.lock().expect("view lock poisoned").as_ref() {
            if asked == query {
                return Ok(listed.clone());
            }
        }
        let read = self.read()?;
        let tree = self.tree();
        let shown: Vec<Shown> = match &query.source {
            Source::Collection => read
                .library
                .tracks
                .iter()
                .map(|t| Shown::Stick(t.id))
                .chain(pending_masters(&tree))
                .collect(),
            Source::Playlist { id } => tree.tracks_of(id)?,
            Source::Playlists { ids } => {
                let mut seen = HashSet::new();
                let mut out = Vec::new();
                for id in ids {
                    out.extend(tree.tracks_of(id)?.into_iter().filter(|t| seen.insert(t.clone())));
                }
                out
            }
        };
        let numbered = !matches!(query.source, Source::Collection);
        let mut rows = Vec::new();
        let mut masters = Vec::new();
        for (at, track) in shown.into_iter().enumerate() {
            let position = if numbered { at as u32 + 1 } else { 0 };
            match track {
                Shown::Stick(id) => {
                    if let Some(row) = read.at.get(&id).map(|&i| device_row(&self.root, &read.library.tracks[i], position)) {
                        rows.push(row);
                    }
                }
                Shown::Master(id) => masters.push((id, position)),
            }
        }
        let sort = match query.sort {
            Sort::Key => SortColumn::Key,
            Sort::KeyCamelot => SortColumn::KeyCamelot,
            other => SortColumn::from(other),
        };
        let spec = ViewSpec {
            source: TrackSource::Collection,
            sort,
            descending: query.descending,
            query: query.query.trim().to_owned(),
            filter: Default::default(),
        };
        let genres: HashSet<&str> = query.genres.iter().map(|g| g.trim()).collect();
        let keep = |genre: &str, rating: u8| {
            (genres.is_empty() || genres.contains(genre.trim())) && (query.ratings.is_empty() || query.ratings.contains(&rating.min(5)))
        };
        let view = DeviceView::open(rows, &spec, SearchField::All);
        let mut listed: Vec<Listed> = view
            .rows
            .iter()
            .filter(|row| keep(&row.genre, row.rating))
            .map(|row| Listed { track: Shown::Stick(row.id), position: row.position })
            .collect();
        // What's on its way from the collection goes after, as it was added.
        if let (Some(master), false) = (master, masters.is_empty()) {
            let ids: Vec<String> = masters.iter().map(|(id, _)| id.clone()).collect();
            let words: Vec<String> = query.query.split_whitespace().map(str::to_lowercase).collect();
            for (row, (id, position)) in master.rows_of(&ids)?.into_iter().zip(masters) {
                let text = format!("{} {} {}", row.title, row.artist, row.album).to_lowercase();
                if keep(&row.genre, row.rating) && words.iter().all(|w| text.contains(w)) {
                    listed.push(Listed { track: Shown::Master(id), position });
                }
            }
        }
        let listed = Arc::new(listed);
        *self.view.lock().expect("view lock poisoned") = Some((query.clone(), listed.clone()));
        Ok(listed)
    }

    pub fn tracks(&self, query: &TrackQuery, offset: usize, limit: usize, master: Option<&Collection>) -> AppResult<TrackPage> {
        let listing = self.listing(query, master)?;
        let read = self.read()?;
        let tree = self.tree();
        let end = offset.saturating_add(limit).min(listing.len());
        let window = listing.get(offset.min(end)..end).unwrap_or(&[]);
        let wanted: Vec<String> = window
            .iter()
            .filter_map(|l| match &l.track {
                Shown::Master(id) => Some(id.clone()),
                Shown::Stick(_) => None,
            })
            .collect();
        let mut from_master: HashMap<String, TrackRow> = match master {
            Some(master) if !wanted.is_empty() => master.rows_of(&wanted)?.into_iter().map(|r| (r.id.clone(), r)).collect(),
            _ => HashMap::new(),
        };
        let rows = window
            .iter()
            .filter_map(|l| match &l.track {
                Shown::Stick(id) => {
                    let track = &read.library.tracks[*read.at.get(id)?];
                    Some(track_row(&self.root, track, l.position, tree.memberships.get(id)))
                }
                Shown::Master(id) => {
                    let mut row = from_master.remove(id)?;
                    row.id = master_row_id(id);
                    row.position = l.position;
                    row.playlists = Vec::new();
                    row.playlist_ids = Vec::new();
                    row.grid = None;
                    row.pending = true;
                    Some(row)
                }
            })
            .collect();
        let duration_secs = listing
            .iter()
            .filter_map(|l| match &l.track {
                Shown::Stick(id) => read.at.get(id).map(|&i| u64::from(read.library.tracks[i].duration_sec)),
                Shown::Master(_) => None,
            })
            .sum();
        Ok(TrackPage { total: listing.len(), duration_secs, offset, rows })
    }

    pub fn track_ids(&self, query: &TrackQuery, master: Option<&Collection>) -> AppResult<Vec<String>> {
        Ok(self
            .listing(query, master)?
            .iter()
            .map(|l| match &l.track {
                Shown::Stick(id) => stick_id(*id),
                Shown::Master(id) => master_row_id(id),
            })
            .collect())
    }

    pub fn duration_of(&self, track_ids: &[String]) -> AppResult<u64> {
        let read = self.read()?;
        Ok(track_ids
            .iter()
            .filter_map(|id| id.parse::<u32>().ok())
            .filter_map(|id| read.at.get(&id))
            .map(|&i| u64::from(read.library.tracks[i].duration_sec))
            .sum())
    }

    /// The genres (or artists, or albums) the stick's tracks have, the most used first.
    pub fn field_values(&self, field: &str) -> AppResult<Vec<String>> {
        let read = self.read()?;
        let name: fn(&device::Track) -> &str = match field {
            "artist" => |t| &t.artist,
            "album" => |t| &t.album,
            "genre" => |t| &t.genre,
            _ => return Err(AppError::Validation(format!("{field} has no names to offer"))),
        };
        let mut uses: HashMap<&str, usize> = HashMap::new();
        for track in &read.library.tracks {
            let name = name(track).trim();
            if !name.is_empty() {
                *uses.entry(name).or_default() += 1;
            }
        }
        let mut names: Vec<(&str, usize)> = uses.into_iter().collect();
        names.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        Ok(names.into_iter().map(|(n, _)| n.to_owned()).collect())
    }

    /// The tracks of one of the stick's playlists as it's shown, by their
    /// ids; ones waiting to go on from the collection are left out.
    pub fn members(&self, id: &str) -> AppResult<Vec<String>> {
        Ok(self
            .tree()
            .tracks_of(id)?
            .into_iter()
            .filter_map(|t| match t {
                Shown::Stick(id) => Some(stick_id(id)),
                Shown::Master(_) => None,
            })
            .collect())
    }

    /// Every track the stick holds, by id.
    pub fn all_track_ids(&self) -> AppResult<Vec<String>> {
        Ok(self.read()?.library.tracks.iter().map(|t| stick_id(t.id)).collect())
    }

    // --- A track ---------------------------------------------------------------

    /// One of the stick's tracks, by the id the interface has for it.
    pub fn track(&self, id: &str) -> AppResult<device::Track> {
        let read = self.read()?;
        id.parse::<u32>()
            .ok()
            .and_then(|id| read.at.get(&id))
            .map(|&i| read.library.tracks[i].clone())
            .ok_or_else(|| AppError::Validation("That track is no longer on the stick".into()))
    }

    /// Where a file the stick's library names is.
    pub fn file(&self, relative: &str) -> PathBuf {
        let spelled = within(&self.root, relative);
        if spelled.exists() {
            spelled
        } else {
            device::StickFiles::new(&self.root).locate(relative)
        }
    }

    /// A track's three analysis files, from the `.DAT` its library names.
    pub fn analysis_files(&self, track: &device::Track) -> Option<[PathBuf; 3]> {
        if track.analysis_path.trim().is_empty() {
            return None;
        }
        let dat = self.file(&track.analysis_path);
        let ext = self.file(&rbl_anlz::sibling(Path::new(&track.analysis_path), "EXT").to_string_lossy());
        let two_ex = self.file(&rbl_anlz::sibling(Path::new(&track.analysis_path), "2EX").to_string_lossy());
        Some([dat, ext, two_ex])
    }

    /// A track's cues as its analysis files hold them: the `.EXT`'s list,
    /// which has colours and names, or the `.DAT`'s.
    pub fn cue_entries(&self, track: &device::Track) -> Vec<rbl_anlz::CueEntry> {
        let Some([dat, ext, _]) = self.analysis_files(track) else { return Vec::new() };
        match rbl_anlz::Anlz::read(&ext) {
            Ok(ext) if ext.section(b"PCO2").is_some() => ext.cue_entries(),
            _ => rbl_anlz::Anlz::read(&dat).map(|dat| dat.legacy_cue_entries()).unwrap_or_default(),
        }
    }

    pub fn preparation(&self, id: &str) -> AppResult<Preparation> {
        let track = self.track(id)?;
        let colors = HotCueColors::installed();
        let beats = self
            .analysis_files(&track)
            .and_then(|[dat, ..]| rbl_anlz::Anlz::read(&dat).ok())
            .and_then(|anlz| anlz.beat_grid())
            .unwrap_or_default();
        Ok(Preparation {
            track_id: id.to_owned(),
            duration_ms: track.duration_sec.saturating_mul(1000),
            bpm: beats.first().map(|beat| f64::from(beat.tempo_x100) / 100.0),
            beats: beats.iter().map(|b| (b.time_ms, b.beat_number.clamp(1, 4) as u8, b.tempo_x100)).collect(),
            cues: cues_from(&self.cue_entries(&track), colors),
            // Nothing of a stick's track is changed from here.
            locked: true,
            pad_colors: (0..8).map(|slot| colors.color_of(0, slot, false)).collect(),
            loop_color: colors.color_of(0, 0, true),
        })
    }

    pub fn waveform(&self, id: &str) -> AppResult<Vec<u8>> {
        let track = self.track(id)?;
        Ok(self.analysis_files(&track).map(|[_, ext, two_ex]| prepare::waveform_from(&ext, &two_ex)).unwrap_or_default())
    }

    pub fn previews(&self, track_ids: &[String], prefer: u8) -> AppResult<Vec<Preview>> {
        let colors = HotCueColors::installed();
        let mut previews = Vec::new();
        for id in track_ids {
            let Ok(track) = self.track(id) else { continue };
            let mut preview = Preview {
                track_id: id.clone(),
                kind: 0,
                columns: Vec::new(),
                duration_ms: track.duration_sec.saturating_mul(1000),
                cues: cues_from(&self.cue_entries(&track), colors),
            };
            if let Some([_, ext, two_ex]) = self.analysis_files(&track) {
                let wave = prepare::wave_preview(&self.wave_previews, &track.analysis_path, &ext, &two_ex, prefer);
                preview.kind = wave.kind;
                preview.columns = wave.columns.to_vec();
                if let Some(ms) = wave.duration_ms {
                    preview.duration_ms = ms;
                }
            }
            previews.push(preview);
        }
        Ok(previews)
    }

    pub fn decode_audio(&self, id: &str, sink: impl FnMut(Audio) -> bool) -> AppResult<()> {
        let track = self.track(id)?;
        prepare::decode_file(&self.file(&track.path), u64::from(track.duration_sec), sink)
    }

    // --- Changes waiting -------------------------------------------------------

    pub fn staged(&self) -> Vec<Change> {
        self.staged.lock().expect("staged lock poisoned").clone()
    }

    /// Forgets the changes waiting, as after they're written or discarded.
    pub fn clear_staged(&self) {
        self.staged.lock().expect("staged lock poisoned").clear();
        self.rebuild();
    }

    pub fn summary(&self) -> StagedSummary {
        summarise(&self.staged())
    }

    /// Takes back the last change staged. Returns whether there was one.
    pub fn unstage_last(&self) -> bool {
        let popped = self.staged.lock().expect("staged lock poisoned").pop().is_some();
        self.rebuild();
        popped
    }

    /// Keeps a change to the stick's playlists to write later, as the tree
    /// shows them now. Returns the id of a playlist or folder it made.
    /// Tracks are the ids the list has for them: the stick's, or a
    /// collection track's as [`master_row_id`] gives it.
    pub fn stage(&self, edit: Edit) -> AppResult<Option<String>> {
        let read = self.read()?;
        let tree = self.tree();
        let mut staged = self.staged.lock().expect("staged lock poisoned");
        let playlist_ref = |id: &str| -> AppResult<PlaylistRef> {
            if let Some(number) = id.strip_prefix("new").and_then(|n| n.parse().ok()) {
                return Ok(PlaylistRef::New { number });
            }
            let id: u32 = id.parse().map_err(|_| {
                AppError::Validation("That playlist is waiting to be written to the stick. Apply the changes first.".into())
            })?;
            ref_of(&read.library, id).ok_or_else(|| AppError::Validation("That playlist is no longer on the stick".into()))
        };
        let track_ref = |id: &str| -> AppResult<TrackRef> {
            if let Some(master) = master_of(id) {
                return Ok(TrackRef::Master { id: master.to_owned() });
            }
            let track = id.parse::<u32>().ok().and_then(|id| read.at.get(&id)).map(|&i| &read.library.tracks[i]);
            let track = track.ok_or_else(|| AppError::Validation("That track is no longer on the stick".into()))?;
            Ok(TrackRef::OnStick { path: track.path.clone() })
        };
        let mut created = None;
        match edit {
            Edit::CreatePlaylist { name, parent_id, folder } => {
                let name = named(&name)?;
                let parent = parent_id.as_deref().map(playlist_ref).transpose()?;
                if let Some(id) = &parent_id {
                    if !tree.node(id).is_some_and(|n| n.folder) {
                        return Err(AppError::Validation("Playlists go in folders".into()));
                    }
                }
                let number = staged
                    .iter()
                    .filter_map(|c| match c {
                        Change::Create { number, .. } => Some(*number),
                        _ => None,
                    })
                    .max()
                    .map_or(1, |n| n + 1);
                staged.push(Change::Create { number, parent, name, folder });
                created = Some(new_id(number));
            }
            Edit::RenamePlaylist { id, name } => {
                let name = named(&name)?;
                match playlist_ref(&id)? {
                    PlaylistRef::New { number } => {
                        for change in staged.iter_mut() {
                            if let Change::Create { number: made, name: was, .. } = change {
                                if *made == number {
                                    *was = name.clone();
                                }
                            }
                        }
                    }
                    playlist => staged.push(Change::Rename { playlist, name }),
                }
            }
            Edit::DeletePlaylist { id } => {
                if let Some(master) = id.strip_prefix('x') {
                    // One of the collection's on its way: it doesn't go after all.
                    let gone = exported_subtree(&tree, &id);
                    for change in staged.iter_mut() {
                        if let Change::Export { lists } = change {
                            lists.retain(|l| l.id != master && !gone.contains(&exported_id(&l.id)));
                        }
                    }
                    staged.retain(|c| !matches!(c, Change::Export { lists } if lists.is_empty()));
                } else {
                    staged.push(Change::Delete { playlist: playlist_ref(&id)? });
                }
            }
            Edit::AddTracks { playlist_id, track_ids } => {
                if !tree.node(&playlist_id).is_some_and(|n| !n.folder) {
                    return Err(AppError::Validation("Tracks go in playlists, not folders".into()));
                }
                let playlist = playlist_ref(&playlist_id)?;
                let mut tracks = Vec::new();
                for id in &track_ids {
                    let track = track_ref(id)?;
                    if !tracks.contains(&track) {
                        tracks.push(track);
                    }
                }
                staged.push(Change::Add { playlist, tracks });
            }
            Edit::RemoveTracks { playlist_id, track_ids } => {
                let playlist = playlist_ref(&playlist_id)?;
                let tracks = track_ids.iter().map(|id| track_ref(id)).collect::<AppResult<Vec<_>>>()?;
                staged.push(Change::Remove { playlist, tracks });
            }
            Edit::MovePlaylist { .. } => {
                return Err(AppError::Validation("Playlists on a USB stick can't be moved".into()));
            }
            Edit::MoveTracks { .. } => {
                return Err(AppError::Validation("Tracks in a USB stick's playlist can't be reordered".into()));
            }
            _ => {
                return Err(AppError::Validation(
                    "Tracks on a USB stick can't be changed here. Change them in your library, then update the stick.".into(),
                ));
            }
        }
        drop(staged);
        self.rebuild();
        Ok(created)
    }

    /// Keeps the collection's playlists to write to the stick later.
    pub fn stage_export(&self, lists: Vec<ExportedList>) {
        if lists.is_empty() {
            return;
        }
        self.staged.lock().expect("staged lock poisoned").push(Change::Export { lists });
        self.rebuild();
    }
}

fn named(name: &str) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Validation("Give it a name".into()));
    }
    Ok(name.to_owned())
}

/// A file the stick's library names, under the stick.
fn within(root: &Path, relative: &str) -> PathBuf {
    relative.split(['/', '\\']).filter(|p| !p.is_empty() && *p != "." && *p != "..").fold(root.to_path_buf(), |path, part| path.join(part))
}

/// The ids of an exported node and everything under it.
fn exported_subtree(tree: &Tree, id: &str) -> HashSet<String> {
    let mut gone = HashSet::from([id.to_owned()]);
    for node in &tree.nodes {
        if node.parent.as_ref().is_some_and(|p| gone.contains(p)) {
            gone.insert(node.id.clone());
        }
    }
    gone
}

/// The collection tracks waiting to go on, each once, in the order added.
fn pending_masters(tree: &Tree) -> Vec<Shown> {
    let mut seen = HashSet::new();
    tree.nodes
        .iter()
        .flat_map(|n| &n.tracks)
        .filter(|t| matches!(t, Shown::Master(_)) && seen.insert((*t).clone()))
        .cloned()
        .collect()
}

/// A node of the stick's library, by its place in the tree.
pub fn ref_of(library: &device::Library, id: u32) -> Option<PlaylistRef> {
    let mut path = Vec::new();
    let mut at = library.node(id)?;
    for _ in 0..=library.nodes.len() {
        let place = library.nodes.iter().filter(|n| n.parent == at.parent && n.name == at.name && (n.sequence, n.id) < (at.sequence, at.id)).count();
        path.push((at.name.clone(), place));
        if at.parent == 0 {
            path.reverse();
            return Some(PlaylistRef::OnStick { path });
        }
        at = library.node(at.parent)?;
    }
    None
}

/// The node of the stick's library a reference names, if it still has it.
pub fn find(library: &device::Library, path: &[(String, usize)]) -> Option<u32> {
    let mut parent = 0;
    for (name, place) in path {
        let mut same: Vec<&device::Node> = library.nodes.iter().filter(|n| n.parent == parent && &n.name == name).collect();
        same.sort_by_key(|n| (n.sequence, n.id));
        parent = same.get(*place)?.id;
    }
    (parent != 0).then_some(parent)
}

/// The stick's tree with the changes laid over it.
fn shown_tree(read: &Read, staged: &[Change]) -> Tree {
    let library = &read.library;
    let mut nodes: HashMap<String, Node> = HashMap::new();
    let mut children: HashMap<Option<String>, Vec<String>> = HashMap::new();
    for node in &library.nodes {
        let id = node.id.to_string();
        let parent = (node.parent != 0).then(|| node.parent.to_string());
        children.entry(parent.clone()).or_default().push(id.clone());
        nodes.insert(
            id.clone(),
            Node {
                id,
                parent,
                name: node.name.clone(),
                folder: node.folder,
                tracks: node.tracks.iter().map(|&t| Shown::Stick(t)).collect(),
                pending: false,
            },
        );
    }
    let resolve = |playlist: &PlaylistRef| -> Option<String> {
        match playlist {
            PlaylistRef::OnStick { path } => find(library, path).map(|id| id.to_string()),
            PlaylistRef::New { number } => Some(new_id(*number)),
        }
    };
    let track_of = |track: &TrackRef| -> Option<Shown> {
        match track {
            TrackRef::OnStick { path } => read.by_path.get(&path_key(path)).map(|&id| Shown::Stick(id)),
            TrackRef::Master { id } => Some(Shown::Master(id.clone())),
        }
    };
    for change in staged {
        match change {
            Change::Create { number, parent, name, folder } => {
                let parent = match parent {
                    Some(p) => match resolve(p).filter(|id| nodes.contains_key(id)) {
                        Some(id) => Some(id),
                        None => continue,
                    },
                    None => None,
                };
                let id = new_id(*number);
                children.entry(parent.clone()).or_default().insert(0, id.clone());
                nodes.insert(id.clone(), Node { id, parent, name: name.clone(), folder: *folder, tracks: Vec::new(), pending: true });
            }
            Change::Rename { playlist, name } => {
                if let Some(node) = resolve(playlist).and_then(|id| nodes.get_mut(&id)) {
                    node.name.clone_from(name);
                    node.pending = true;
                }
            }
            Change::Delete { playlist } => {
                if let Some(id) = resolve(playlist) {
                    let mut stack = vec![id];
                    while let Some(id) = stack.pop() {
                        if let Some(node) = nodes.remove(&id) {
                            if let Some(siblings) = children.get_mut(&node.parent) {
                                siblings.retain(|s| *s != id);
                            }
                            stack.extend(children.remove(&Some(id)).unwrap_or_default());
                        }
                    }
                }
            }
            Change::Add { playlist, tracks } => {
                if let Some(node) = resolve(playlist).and_then(|id| nodes.get_mut(&id)) {
                    for track in tracks.iter().filter_map(track_of) {
                        if !node.tracks.contains(&track) {
                            node.tracks.push(track);
                            node.pending = true;
                        }
                    }
                }
            }
            Change::Remove { playlist, tracks } => {
                if let Some(node) = resolve(playlist).and_then(|id| nodes.get_mut(&id)) {
                    let going: HashSet<Shown> = tracks.iter().filter_map(track_of).collect();
                    let before = node.tracks.len();
                    node.tracks.retain(|t| !going.contains(t));
                    node.pending |= node.tracks.len() != before;
                }
            }
            Change::Export { lists } => {
                for list in lists {
                    let id = exported_id(&list.id);
                    if nodes.contains_key(&id) {
                        continue;
                    }
                    let parent = list.parent.as_deref().map(exported_id).filter(|p| nodes.contains_key(p));
                    children.entry(parent.clone()).or_default().push(id.clone());
                    nodes.insert(
                        id.clone(),
                        Node {
                            id,
                            parent,
                            name: list.name.clone(),
                            folder: list.folder,
                            tracks: list.tracks.iter().map(|t| Shown::Master(t.clone())).collect(),
                            pending: true,
                        },
                    );
                }
            }
        }
    }

    // Depth first from the top, each level in its order.
    let mut tree = Tree::default();
    let mut stack: Vec<(String, usize)> =
        children.get(&None).map(|top| top.iter().rev().map(|id| (id.clone(), 0)).collect()).unwrap_or_default();
    while let Some((id, depth)) = stack.pop() {
        let Some(node) = nodes.remove(&id) else { continue };
        if let Some(under) = children.get(&Some(id.clone())) {
            stack.extend(under.iter().rev().map(|child| (child.clone(), depth + 1)));
        }
        if !node.folder {
            for track in &node.tracks {
                if let Shown::Stick(t) = track {
                    let lists = tree.memberships.entry(*t).or_default();
                    if lists.last().is_none_or(|(last, _)| *last != node.id) {
                        lists.push((node.id.clone(), node.name.clone()));
                    }
                }
            }
        }
        tree.nodes.push(node);
        tree.depth.push(depth);
    }
    tree
}

fn summarise(staged: &[Change]) -> StagedSummary {
    let mut summary = StagedSummary { changes: staged.len(), ..StagedSummary::default() };
    let mut masters = HashSet::new();
    for change in staged {
        match change {
            Change::Create { folder, .. } => summary.playlists_made += usize::from(!folder),
            Change::Rename { .. } => summary.renamed += 1,
            Change::Delete { .. } => summary.deleted += 1,
            Change::Add { tracks, .. } => {
                summary.added += tracks.len();
                masters.extend(tracks.iter().filter_map(|t| match t {
                    TrackRef::Master { id } => Some(id.clone()),
                    TrackRef::OnStick { .. } => None,
                }));
            }
            Change::Remove { tracks, .. } => summary.removed += tracks.len(),
            Change::Export { lists } => {
                summary.exported += lists.iter().filter(|l| !l.folder).count();
                masters.extend(lists.iter().flat_map(|l| l.tracks.iter().cloned()));
            }
        }
    }
    summary.tracks_to_copy = masters.len();
    summary
}

/// A stick track as `rbl-index` orders and searches them.
fn device_row(root: &Path, track: &device::Track, position: u32) -> DeviceRow {
    DeviceRow {
        id: track.id,
        position,
        path: within(root, &track.path),
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: track.album.clone(),
        genre: track.genre.clone(),
        label: track.label.clone(),
        key: track.key.clone(),
        comment: track.comment.clone(),
        date_added: track.date_added.clone(),
        bpm_x100: track.bpm_x100,
        duration_sec: track.duration_sec,
        rating: track.rating,
        color: track.color,
        analysed: !track.analysis_path.is_empty(),
        has_artwork: !track.artwork_path.is_empty(),
        cues: Vec::new(),
    }
}

/// A stick track as the list shows it.
fn track_row(root: &Path, track: &device::Track, position: u32, lists: Option<&Vec<(String, String)>>) -> TrackRow {
    let path = within(root, &track.path);
    let format = match path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
        Some("mp3") => "mp3",
        Some("m4a" | "aac" | "mp4") => "m4a",
        Some("flac") => "flac",
        Some("wav") => "wav",
        Some("aif" | "aiff") => "aiff",
        _ => "",
    };
    let lists = lists.cloned().unwrap_or_default();
    TrackRow {
        id: stick_id(track.id),
        position,
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: track.album.clone(),
        genre: track.genre.clone(),
        key: track.key.clone(),
        bpm: (track.bpm_x100 > 0).then(|| f64::from(track.bpm_x100) / 100.0),
        duration_secs: track.duration_sec,
        rating: track.rating.min(5),
        color: track.color,
        date_added: track.date_added.clone(),
        format,
        bitrate: 0,
        path: path.to_string_lossy().into_owned(),
        comment: track.comment.clone(),
        playlist_ids: lists.iter().map(|(id, _)| id.clone()).collect(),
        playlists: lists.into_iter().map(|(_, name)| name).collect(),
        analysed: !track.analysis_path.is_empty(),
        grid: None,
        pending: false,
    }
}

/// Cues from an analysis file, as the player shows them.
fn cues_from(entries: &[rbl_anlz::CueEntry], colors: HotCueColors) -> Vec<Cue> {
    let mut cues: Vec<Cue> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.kind, 1 | 2))
        .map(|(at, e)| {
            let slot = (e.hot_cue > 0).then(|| (e.hot_cue - 1) as u8);
            let end_ms = (e.kind == 2 && e.loop_time_ms > e.time_ms).then_some(e.loop_time_ms);
            let color = slot.map(|slot| match e.rgb {
                Some([r, g, b]) if [r, g, b] != [0, 0, 0] => format!("#{r:02x}{g:02x}{b:02x}"),
                _ => colors.color_of(i64::from(e.color_code.unwrap_or(0)), slot, end_ms.is_some()),
            });
            Cue {
                id: format!("cue{at}"),
                slot,
                position_ms: e.time_ms,
                end_ms,
                comment: e.comment.clone().unwrap_or_default(),
                color,
            }
        })
        .collect();
    cues.sort_by_key(|c| (c.position_ms, c.slot));
    cues
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u32, parent: u32, name: &str, folder: bool, sequence: u32, tracks: &[u32]) -> device::Node {
        device::Node { id, parent, name: name.into(), folder, sequence, tracks: tracks.to_vec() }
    }

    fn library() -> device::Library {
        device::Library {
            format: device::Format::DeviceLibrary,
            nodes: vec![
                node(1, 0, "Sets", true, 0, &[]),
                node(2, 1, "Friday", false, 0, &[10, 11]),
                node(3, 1, "Friday", false, 1, &[11]),
                node(4, 0, "Warmup", false, 1, &[12]),
            ],
            tracks: [10, 11, 12]
                .iter()
                .map(|&id| device::Track { id, title: format!("T{id}"), path: format!("/Contents/a/{id}.mp3"), ..Default::default() })
                .collect(),
        }
    }

    fn read() -> Read {
        let library = library();
        let at = library.tracks.iter().enumerate().map(|(at, t)| (t.id, at)).collect();
        let by_path = library.tracks.iter().map(|t| (path_key(&t.path), t.id)).collect();
        Read { library, at, by_path }
    }

    #[test]
    fn a_playlist_is_found_again_by_its_place_even_beside_one_of_the_same_name() {
        let library = library();
        for id in [1, 2, 3, 4] {
            let Some(PlaylistRef::OnStick { path }) = ref_of(&library, id) else { panic!("{id} has a place") };
            assert_eq!(find(&library, &path), Some(id));
        }
        assert_eq!(ref_of(&library, 3), Some(PlaylistRef::OnStick { path: vec![("Sets".into(), 0), ("Friday".into(), 1)] }));
    }

    #[test]
    fn changes_are_shown_over_what_the_stick_holds() {
        let read = read();
        let sets = ref_of(&read.library, 1).unwrap();
        let warmup = ref_of(&read.library, 4).unwrap();
        let staged = vec![
            Change::Create { number: 1, parent: Some(sets), name: "Saturday".into(), folder: false },
            Change::Add {
                playlist: PlaylistRef::New { number: 1 },
                tracks: vec![TrackRef::OnStick { path: "/CONTENTS/a/10.mp3".into() }, TrackRef::Master { id: "77".into() }],
            },
            Change::Rename { playlist: warmup.clone(), name: "Opening".into() },
            Change::Remove { playlist: warmup, tracks: vec![TrackRef::OnStick { path: "/Contents/a/12.mp3".into() }] },
            Change::Delete { playlist: ref_of(&read.library, 3).unwrap() },
            Change::Export {
                lists: vec![ExportedList { id: "5".into(), name: "Peak".into(), parent: None, folder: false, tracks: vec!["77".into(), "78".into()] }],
            },
        ];
        let tree = shown_tree(&read, &staged);
        let names: Vec<(&str, usize, bool)> = tree.nodes.iter().zip(&tree.depth).map(|(n, d)| (n.name.as_str(), *d, n.pending)).collect();
        assert_eq!(
            names,
            [("Sets", 0, false), ("Saturday", 1, true), ("Friday", 1, false), ("Opening", 0, true), ("Peak", 0, true)]
        );
        assert_eq!(tree.node("new1").unwrap().tracks, [Shown::Stick(10), Shown::Master("77".into())]);
        assert!(tree.node("4").unwrap().tracks.is_empty());
        // A folder lists what's under it, each once.
        assert_eq!(tree.tracks_of("1").unwrap(), [Shown::Stick(10), Shown::Master("77".into()), Shown::Stick(11)]);
        let summary = summarise(&staged);
        assert_eq!((summary.playlists_made, summary.exported, summary.tracks_to_copy), (1, 1, 2));
    }
}
