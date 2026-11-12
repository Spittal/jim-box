//! Changes to rekordbox's library, each made through rbxport's guarded
//! writer: refused while rekordbox is running, and preceded by a backup of
//! `master.db` the first time in a session. What it takes to undo each is
//! kept for the session (see `history`).
//!
//! The database is opened for one change and closed again, then read afresh,
//! so rekordbox can take the file back between changes.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use rbl_db::write::{
    backups_in, CueRevision, CueRow, TrackField, Writer, ATTRIBUTE_FOLDER, ATTRIBUTE_PLAYLIST, ROOT,
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use super::history::{self, History, Step};
use super::{prepare, rekordbox_err, Collection, CollectionStatus, GridChange, PlaylistNode};
use crate::error::{AppError, AppResult};

/// Backups kept; the oldest go once there are more.
const MAX_BACKUPS: usize = 10;

/// One change to the library. Ids are rekordbox's, as text.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Edit {
    /// Creates a playlist or folder, at the top level when `parent_id` is None.
    CreatePlaylist { name: String, parent_id: Option<String>, folder: bool },
    RenamePlaylist { id: String, name: String },
    /// Deletes a playlist, or a folder and everything in it. Tracks stay in
    /// the collection.
    DeletePlaylist { id: String },
    /// Moves a playlist or folder under `parent_id` (None for the top level),
    /// to `index` among its children or the end.
    MovePlaylist { id: String, parent_id: Option<String>, index: Option<usize> },
    /// Appends tracks to a playlist, skipping any already in it.
    AddTracks { playlist_id: String, track_ids: Vec<String> },
    RemoveTracks { playlist_id: String, track_ids: Vec<String> },
    /// Moves tracks within a playlist to just before `before`, or to the end.
    MoveTracks { playlist_id: String, track_ids: Vec<String>, before: Option<String> },
    /// Takes tracks out of the collection and every playlist. Their files
    /// stay, unless `trash_files` sends them to the Trash too.
    DeleteTracks {
        track_ids: Vec<String>,
        #[serde(default)]
        trash_files: bool,
    },
    SetRating { track_id: String, stars: u8 },
    /// rekordbox's colour, 1 to 8; 0 clears it.
    SetColor { track_id: String, color: u8 },
    SetComment { track_id: String, comment: String },
    /// A text field: title, artist, album, genre, label, year, key, bpm…
    SetField { track_id: String, field: String, value: String },
    /// Sets a cue, or a loop when `end_ms` is given: on a hot cue pad (0 for
    /// A), replacing what was on it, or as a memory cue when `slot` is None.
    AddCue { track_id: String, slot: Option<u8>, position_ms: u32, end_ms: Option<u32> },
    DeleteCue { cue_id: String },
    NameCue { cue_id: String, comment: String },
    /// Changes a track's beat grid, in its analysis files: all of it, or
    /// from the beat nearest `from_ms` onward.
    Grid { track_id: String, change: GridChange, from_ms: Option<u32> },
    /// Gives a track another of the grids its analysis made, to try: the
    /// one it chose, or either detector's own.
    UseGrid { track_id: String, source: super::GridSource },
    /// Sets or clears rekordbox's analysis lock, which keeps a track's beat
    /// grid from being changed or analysed over.
    LockAnalysis { track_id: String, locked: bool },
}

/// The library after a change.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditOutcome {
    pub status: CollectionStatus,
    pub playlists: Vec<PlaylistNode>,
    /// The playlist or folder a `CreatePlaylist` made.
    pub created_id: Option<String>,
    /// Something the change couldn't finish, though the library was changed.
    pub warning: Option<String>,
}

/// Moves the files of deleted tracks to the Trash; ones already gone are
/// passed over. Says why if it can't: the tracks have left the collection
/// by then.
fn trash_files(files: &[PathBuf]) -> Option<String> {
    let present: Vec<&PathBuf> = files.iter().filter(|file| file.is_file()).collect();
    if present.is_empty() {
        return None;
    }
    match trash::delete_all(&present) {
        Ok(()) => {
            tracing::info!("Moved {} deleted tracks' files to the Trash", present.len());
            None
        }
        Err(e) => {
            tracing::warn!("Couldn't move deleted tracks' files to the Trash: {e}");
            Some(format!(
                "The tracks were removed from the collection, but their files couldn't be moved to the Trash: {e}"
            ))
        }
    }
}

fn named(name: &str) -> AppResult<&str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Validation("Give it a name".into()));
    }
    Ok(name)
}

impl Edit {
    /// What the change is, to follow "Undid" or "Couldn't undo".
    fn what(&self) -> &'static str {
        match self {
            Self::CreatePlaylist { folder: true, .. } => "making the folder",
            Self::CreatePlaylist { .. } => "making the playlist",
            Self::RenamePlaylist { .. } => "the rename",
            Self::DeletePlaylist { .. } => "deleting the playlist",
            Self::MovePlaylist { .. } => "moving the playlist",
            Self::AddTracks { .. } => "adding to the playlist",
            Self::RemoveTracks { .. } => "removing from the playlist",
            Self::MoveTracks { .. } => "reordering the playlist",
            Self::DeleteTracks { .. } => "deleting from the collection",
            Self::SetRating { .. } => "the rating",
            Self::SetColor { .. } => "the colour",
            Self::SetComment { .. } => "the comment",
            Self::SetField { .. } => "the edit",
            Self::AddCue { .. } => "setting the cue",
            Self::DeleteCue { .. } => "deleting the cue",
            Self::NameCue { .. } => "naming the cue",
            Self::Grid { .. } | Self::UseGrid { .. } => "the grid change",
            Self::LockAnalysis { locked: true, .. } => "locking the analysis",
            Self::LockAnalysis { .. } => "unlocking the analysis",
        }
    }
}

/// The library after an undo or redo.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoOutcome {
    pub status: CollectionStatus,
    pub playlists: Vec<PlaylistNode>,
    /// What was undone or redone, to follow "Undid"; None when there was nothing.
    pub what: Option<&'static str>,
    /// The tracks whose cues, grid or analysis lock it changed.
    pub track_ids: Vec<String>,
}

impl Collection {
    /// Makes one change and reads the library again. Blocking.
    pub fn edit(&self, edit: Edit) -> AppResult<EditOutcome> {
        self.edit_in(edit, None)
    }

    /// [`Self::edit`] as one of several changes that are undone together:
    /// those made in a row under the same `group`.
    pub fn edit_in(&self, edit: Edit, group: Option<String>) -> AppResult<EditOutcome> {
        self.edit_with(edit, group, true)
    }

    /// [`Self::edit`] for a change the app makes of its own accord, as
    /// keeping a playlist in step with Spotify is: not one for the user to
    /// find under Undo.
    pub fn edit_unasked(&self, edit: Edit) -> AppResult<EditOutcome> {
        self.edit_with(edit, None, false)
    }

    fn edit_with(&self, edit: Edit, group: Option<String>, undoable: bool) -> AppResult<EditOutcome> {
        let what = edit.what();
        // Found before the tracks go: afterwards the library no longer knows where they were.
        let doomed: Vec<PathBuf> = match &edit {
            Edit::DeleteTracks { track_ids, trash_files: true } => {
                track_ids.iter().filter_map(|id| self.path_of(id)).collect()
            }
            _ => Vec::new(),
        };
        let deleted = match &edit {
            Edit::DeleteTracks { track_ids, .. } => track_ids.clone(),
            _ => Vec::new(),
        };
        let regridded = matches!(edit, Edit::Grid { .. } | Edit::UseGrid { .. });

        let (created_id, kept) = self.write(|writer, index| {
            let (created_id, steps) = self.apply(writer, index, edit)?;
            if !undoable {
                self.history().changed();
                return Ok((created_id, false));
            }
            let kept = !steps.is_empty();
            let let_go = self.history().record(what, group, steps);
            self.grids.forget(&let_go);
            Ok((created_id, kept))
        })?;
        let warning = trash_files(&doomed);
        // A delete that can be undone keeps what was known of the tracks' grids, to come back with them.
        if !kept {
            self.grids.forget(&deleted);
        }
        if regridded {
            self.forget_view();
        }
        Ok(EditOutcome { status: self.status()?, playlists: self.playlists()?, created_id, warning })
    }

    /// Undoes the last change made this session. Blocking.
    pub fn undo(&self) -> AppResult<UndoOutcome> {
        self.step(true)
    }

    /// Makes the last change undone again. Blocking.
    pub fn redo(&self) -> AppResult<UndoOutcome> {
        self.step(false)
    }

    fn step(&self, back: bool) -> AppResult<UndoOutcome> {
        // Asked before anything is taken from the history: the writer would
        // refuse each step, and the change would be lost to undo for nothing.
        if self.loaded_or_read()?.location.is_real_install && crate::rekordbox::is_running() {
            return Err(AppError::Rekordbox("rekordbox is running. Quit it before making changes.".into()));
        }
        let done = self.write(|writer, index| {
            let Some(entry) = self.history().take(back) else { return Ok(None) };
            if let Err(e) = entry.write(back, writer, index, &self.grids) {
                // An undo that didn't happen leaves its tracks deleted, for good now.
                if back {
                    self.grids.forget(&entry.deleted());
                }
                let reason = match e {
                    AppError::Rekordbox(reason) => reason,
                    other => other.to_string(),
                };
                return Err(AppError::Rekordbox(format!(
                    "Couldn't {} {}: {reason}",
                    if back { "undo" } else { "redo" },
                    entry.what
                )));
            }
            let done = (entry.what, entry.prepared());
            self.history().put(back, entry);
            Ok(Some(done))
        })?;
        let (what, track_ids) = done.unzip();
        Ok(UndoOutcome {
            status: self.status()?,
            playlists: self.playlists()?,
            what,
            track_ids: track_ids.unwrap_or_default(),
        })
    }

    fn history(&self) -> std::sync::MutexGuard<'_, History> {
        self.history.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Opens the library for one piece of work, closes it, and reads it
    /// again (even after a failure, so the view shows what is there).
    /// `work` gets the library as it was read before. Blocking.
    pub(super) fn write<T>(
        &self,
        work: impl FnOnce(&mut Writer, &rbl_index::Library) -> AppResult<T>,
    ) -> AppResult<T> {
        self.write_then(true, work)
    }

    /// [`Self::write`] without reading the library again afterwards: for a
    /// run of changes that don't build on each other, as analysing tracks one
    /// after another is. What was read stays as it was until `refresh`, or
    /// the next `write`, which reads it first.
    pub(super) fn write_deferred<T>(
        &self,
        work: impl FnOnce(&mut Writer, &rbl_index::Library) -> AppResult<T>,
    ) -> AppResult<T> {
        self.write_then(false, work)
    }

    fn write_then<T>(
        &self,
        read_again: bool,
        work: impl FnOnce(&mut Writer, &rbl_index::Library) -> AppResult<T>,
    ) -> AppResult<T> {
        let _one_at_a_time = self.editing.lock().expect("edit lock poisoned");
        if read_again {
            // A change that may build on the ones not yet read.
            self.refresh()?;
        }
        let loaded = self.loaded_or_read()?;
        let location = loaded.location.clone();

        let mut writer = Writer::open(location.clone(), &self.backup_dir).map_err(rekordbox_err)?;
        if self.backed_up.load(Ordering::Acquire) {
            writer.mark_backed_up();
        }
        // Changed by rekordbox since the last change here: nothing kept for undo fits it now.
        let let_go = self.history().seen(history::update_count(writer.library().connection()));
        self.grids.forget(&let_go);
        let result = work(&mut writer, &loaded.index).map_err(|e| damage_report(&writer, e));
        self.history().wrote(history::update_count(writer.library().connection()));
        if writer.backed_up() && !self.backed_up.swap(true, Ordering::AcqRel) {
            self.prune_backups();
        }
        drop(writer);

        if read_again {
            self.load_from(location)?;
        } else {
            self.stale.store(true, Ordering::Release);
        }
        result
    }

    fn prune_backups(&self) {
        let backups = backups_in(&self.backup_dir);
        let extra = backups.len().saturating_sub(MAX_BACKUPS);
        for old in &backups[..extra] {
            if let Err(e) = std::fs::remove_file(old) {
                tracing::warn!("Couldn't remove old backup {}: {e}", old.display());
            }
        }
    }

    /// Makes a change, and returns what it takes to undo it: nothing for a
    /// change that left everything as it was, or that can't be undone.
    fn apply(
        &self,
        writer: &mut Writer,
        index: &rbl_index::Library,
        edit: Edit,
    ) -> AppResult<(Option<String>, Vec<Step>)> {
        let parent = |id: &Option<String>| id.clone().unwrap_or_else(|| ROOT.to_owned());
        let mut created = None;
        let mut steps = Vec::new();
        match edit {
            Edit::CreatePlaylist { name, parent_id, folder } => {
                let (name, parent) = (named(&name)?, parent(&parent_id));
                let attribute = if folder { ATTRIBUTE_FOLDER } else { ATTRIBUTE_PLAYLIST };
                let (id, creation) = writer.create_with_undo(name, &parent, attribute, None).map_err(rekordbox_err)?;
                created = Some(id);
                steps.push(Step::Created(creation));
            }
            Edit::RenamePlaylist { id, name } => {
                let (_, rename) = writer.rename_with_undo(&id, named(&name)?).map_err(rekordbox_err)?;
                steps.push(Step::Renamed(rename));
            }
            Edit::DeletePlaylist { id } => {
                let (_, deletion) = writer.delete_playlist_with_undo(&id).map_err(rekordbox_err)?;
                if !deletion.is_empty() {
                    steps.push(Step::PlaylistDeleted(deletion));
                }
            }
            Edit::MovePlaylist { id, parent_id, index } => {
                let (_, moved) = writer.move_with_undo(&id, &parent(&parent_id), index).map_err(rekordbox_err)?;
                steps.push(Step::Moved(moved));
            }
            Edit::AddTracks { playlist_id, track_ids } => {
                let (_, addition) =
                    writer.add_tracks_with_undo(&playlist_id, &track_ids, false).map_err(rekordbox_err)?;
                if !addition.is_empty() {
                    steps.push(Step::TracksAdded(addition));
                }
            }
            Edit::RemoveTracks { playlist_id, track_ids } => {
                let (_, removal) = writer.remove_tracks_with_undo(&playlist_id, &track_ids).map_err(rekordbox_err)?;
                if !removal.is_empty() {
                    steps.push(Step::TracksRemoved(removal));
                }
            }
            Edit::MoveTracks { playlist_id, track_ids, before } => {
                let order = moved_order(index, &playlist_id, &track_ids, before.as_deref())?;
                let (_, reorder) = writer.reorder_with_undo(&playlist_id, &order).map_err(rekordbox_err)?;
                if !reorder.is_empty() {
                    steps.push(Step::Reordered(reorder));
                }
            }
            // With their files in the Trash there is no bringing them back from here.
            Edit::DeleteTracks { track_ids, trash_files: true } => {
                writer.delete_tracks(&track_ids).map_err(rekordbox_err)?;
                self.history().changed();
            }
            Edit::DeleteTracks { track_ids, trash_files: false } => {
                steps.push(history::delete_tracks(writer, &track_ids)?);
            }
            Edit::SetRating { track_id, stars } => {
                let (_, change) = writer.set_rating_with_undo(&track_id, stars).map_err(rekordbox_err)?;
                steps.push(Step::Track(change));
            }
            Edit::SetColor { track_id, color } => {
                if color > 8 {
                    return Err(AppError::Validation("That isn't one of rekordbox's colours".into()));
                }
                let id = color.to_string();
                let (_, change) = writer
                    .set_color_with_undo(&track_id, (color > 0).then_some(id.as_str()))
                    .map_err(rekordbox_err)?;
                steps.push(Step::Track(change));
            }
            Edit::SetComment { track_id, comment } => {
                let (_, change) = writer.set_comment_with_undo(&track_id, &comment).map_err(rekordbox_err)?;
                steps.push(Step::Track(change));
            }
            Edit::SetField { track_id, field, value } => {
                let field = TrackField::parse(&field)
                    .ok_or_else(|| AppError::Validation(format!("{field} can't be edited")))?;
                let mut value = value.trim().to_owned();
                if field == TrackField::Key && !value.is_empty() {
                    // Stored as rekordbox spells it, however it was given; a key
                    // the library hasn't met yet is added to the ones it knows.
                    let key = rbl_core::musickey::parse(&value)
                        .ok_or_else(|| AppError::Validation(format!("{value} isn't a musical key")))?;
                    value = key.name();
                    writer.ensure_detected_key(&value).map_err(rekordbox_err)?;
                }
                if field == TrackField::Bpm {
                    // A tempo typed over the grid's has rbxport write the track new
                    // analysis files, which it keeps no way back from: not undone.
                    writer.set_field(&track_id, field, &value).map_err(rekordbox_err)?;
                    self.history().changed();
                } else {
                    let (_, change) = writer.set_field_with_undo(&track_id, field, &value).map_err(rekordbox_err)?;
                    steps.push(Step::Track(change));
                }
            }
            Edit::AddCue { track_id, slot, position_ms, end_ms } => {
                if end_ms.is_some_and(|end| end <= position_ms) {
                    return Err(AppError::Validation("A loop has to end after it starts".into()));
                }
                let before = writer.cue_rows(&track_id).map_err(rekordbox_err)?;
                created = Some(prepare::add_cue(writer, &track_id, slot, position_ms, end_ms)?);
                steps.push(cue_step(writer, &track_id, &before)?);
            }
            Edit::DeleteCue { cue_id } => {
                let before = cue_track(writer, &cue_id)?;
                writer.delete_cue(&cue_id).map_err(rekordbox_err)?;
                if let Some((track_id, before)) = before {
                    steps.push(cue_step(writer, &track_id, &before)?);
                }
            }
            Edit::NameCue { cue_id, comment } => {
                let before = cue_track(writer, &cue_id)?;
                writer.set_cue_comment(&cue_id, comment.trim()).map_err(rekordbox_err)?;
                if let Some((track_id, before)) = before {
                    steps.push(cue_step(writer, &track_id, &before)?);
                }
            }
            Edit::Grid { track_id, change, from_ms } => {
                steps.extend(self.regrid(writer, index, &track_id, |writer| {
                    prepare::change_grid(writer, index, &track_id, change, from_ms)?;
                    // Changing a grid by hand is looking it over.
                    self.grids.edited(&track_id);
                    Ok(())
                })?);
            }
            Edit::UseGrid { track_id, source } => {
                let grid = self
                    .grids
                    .choice(&track_id, source)
                    .ok_or_else(|| AppError::Validation("That grid wasn't kept. Analyse the track again to have it.".into()))?;
                steps.extend(self.regrid(writer, index, &track_id, |writer| {
                    prepare::change_grid(writer, index, &track_id, prepare::GridChange::Restore { beats: grid.beats }, None)?;
                    self.grids.switched(&track_id, source);
                    Ok(())
                })?);
            }
            Edit::LockAnalysis { track_id, locked } => {
                let before = prepare::is_locked(writer.library(), &track_id)?;
                writer.set_analysis_lock(&track_id, locked).map_err(rekordbox_err)?;
                if before != locked {
                    steps.push(Step::Lock { track_id, before, after: locked });
                }
            }
        }
        steps.retain(|step| !matches!(step, Step::Track(change) if change.is_empty()));
        Ok((created, steps))
    }

    /// Runs a change to a track's beat grid, and returns what undoes it:
    /// the beats, and what was known of the grid, before and after.
    fn regrid(
        &self,
        writer: &mut Writer,
        index: &rbl_index::Library,
        track_id: &str,
        change: impl FnOnce(&mut Writer) -> AppResult<()>,
    ) -> AppResult<Option<Step>> {
        let before = prepare::grid_beats(writer, index, track_id);
        let state_before = self.grids.get(track_id);
        change(writer)?;
        let after = prepare::grid_beats(writer, index, track_id);
        let state_after = self.grids.get(track_id);
        Ok(match (before, after) {
            (Some(before), Some(after)) if before != after || state_before != state_after => {
                Some(Step::Grid { track_id: track_id.to_owned(), before, after, state_before, state_after })
            }
            _ => None,
        })
    }
}

/// What a change to a track's cues came to, from how they were before it.
fn cue_step(writer: &Writer, track_id: &str, before: &[CueRow]) -> AppResult<Step> {
    let after = writer.cue_rows(track_id).map_err(rekordbox_err)?;
    Ok(Step::Cues(CueRevision::between(track_id, before, &after)))
}

/// The track a cue is on, and all that track's cues as they are.
fn cue_track(writer: &Writer, cue_id: &str) -> AppResult<Option<(String, Vec<CueRow>)>> {
    let track_id: Option<String> = writer
        .library()
        .connection()
        .query_row("SELECT ContentID FROM djmdCue WHERE ID = ?1", [cue_id], |row| row.get(0))
        .optional()
        .map_err(rekordbox_err)?
        .flatten();
    let Some(track_id) = track_id else { return Ok(None) };
    let rows = writer.cue_rows(&track_id).map_err(rekordbox_err)?;
    Ok(Some((track_id, rows)))
}

/// A playlist's tracks with `moved` taken out and put back before `before`
/// (or at the end), as the order to write.
fn moved_order(
    index: &rbl_index::Library,
    playlist_id: &str,
    moved: &[String],
    before: Option<&str>,
) -> AppResult<Vec<String>> {
    let playlists = index.playlists();
    let at = playlist_id
        .parse()
        .ok()
        .and_then(|id| playlists.index_of(id))
        .ok_or_else(|| AppError::Rekordbox("That playlist no longer exists".into()))?;
    let current: Vec<String> = playlists.members[at]
        .iter()
        .map(|&row| index.ids[row as usize].to_string())
        .collect();
    let moved: Vec<&String> = moved.iter().filter(|id| current.contains(id)).collect();
    let mut order: Vec<String> = current.iter().filter(|id| !moved.contains(id)).cloned().collect();
    // A drop onto one of the moved tracks itself lands where that track was.
    let target = before.and_then(|b| {
        order.iter().position(|id| id == b).or_else(|| {
            let from = current.iter().position(|id| id == b)?;
            current[from..].iter().find_map(|id| order.iter().position(|o| o == id))
        })
    });
    let at = target.unwrap_or(order.len());
    order.splice(at..at, moved.into_iter().cloned());
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, query};
    use super::super::{PlaylistKind, Sort, Source};
    use super::*;

    fn ids(collection: &Collection, source: Source, sort: Sort) -> Vec<String> {
        let page = collection.tracks(&query(source, sort), 0, 1000).unwrap();
        page.rows.into_iter().map(|r| r.id).collect()
    }

    fn playlist(id: &str) -> Source {
        Source::Playlist { id: id.to_owned() }
    }

    fn create(collection: &Collection, name: &str, parent: Option<&str>, folder: bool) -> String {
        collection
            .edit(Edit::CreatePlaylist { name: name.into(), parent_id: parent.map(str::to_owned), folder })
            .unwrap()
            .created_id
            .unwrap()
    }

    #[test]
    fn playlists_are_created_renamed_moved_and_deleted() {
        let (_dir, collection) = fixture();
        let before = collection.playlists().unwrap().len();

        let folder = create(&collection, "Sets", None, true);
        let list = create(&collection, " Warm up ", None, false);
        let nodes = collection.playlists().unwrap();
        assert_eq!(nodes.len(), before + 2);
        let node = nodes.iter().find(|n| n.id == list).unwrap();
        assert_eq!((node.name.as_str(), node.kind, node.count), ("Warm up", PlaylistKind::Playlist, Some(0)));

        collection.edit(Edit::RenamePlaylist { id: list.clone(), name: "Opening".into() }).unwrap();
        collection
            .edit(Edit::MovePlaylist { id: list.clone(), parent_id: Some(folder.clone()), index: None })
            .unwrap();
        let nodes = collection.playlists().unwrap();
        let node = nodes.iter().find(|n| n.id == list).unwrap();
        assert_eq!((node.name.as_str(), node.parent_id.as_deref(), node.depth), ("Opening", Some(folder.as_str()), 1));

        // Deleting the folder takes what's in it too.
        collection.edit(Edit::DeletePlaylist { id: folder }).unwrap();
        assert_eq!(collection.playlists().unwrap().len(), before);
        assert!(collection.edit(Edit::RenamePlaylist { id: list, name: " ".into() }).is_err());
    }

    #[test]
    fn tracks_are_added_moved_and_removed() {
        let (_dir, collection) = fixture();
        let tracks = ids(&collection, Source::Collection, Sort::Title);
        let list = create(&collection, "Test", None, false);
        let picked: Vec<String> = tracks[..4].to_vec();

        collection.edit(Edit::AddTracks { playlist_id: list.clone(), track_ids: picked.clone() }).unwrap();
        // Adding again doesn't duplicate.
        collection.edit(Edit::AddTracks { playlist_id: list.clone(), track_ids: picked[..2].to_vec() }).unwrap();
        assert_eq!(ids(&collection, playlist(&list), Sort::TrackNo), picked);

        // Last to the front.
        collection
            .edit(Edit::MoveTracks {
                playlist_id: list.clone(),
                track_ids: vec![picked[3].clone()],
                before: Some(picked[0].clone()),
            })
            .unwrap();
        let expected = vec![picked[3].clone(), picked[0].clone(), picked[1].clone(), picked[2].clone()];
        assert_eq!(ids(&collection, playlist(&list), Sort::TrackNo), expected);

        // First two to the end.
        collection
            .edit(Edit::MoveTracks { playlist_id: list.clone(), track_ids: expected[..2].to_vec(), before: None })
            .unwrap();
        let expected = vec![picked[1].clone(), picked[2].clone(), picked[3].clone(), picked[0].clone()];
        assert_eq!(ids(&collection, playlist(&list), Sort::TrackNo), expected);

        collection
            .edit(Edit::RemoveTracks { playlist_id: list.clone(), track_ids: vec![picked[2].clone()] })
            .unwrap();
        let expected = vec![picked[1].clone(), picked[3].clone(), picked[0].clone()];
        assert_eq!(ids(&collection, playlist(&list), Sort::TrackNo), expected);
        // The track is still in the collection.
        assert_eq!(collection.status().unwrap().tracks, tracks.len());
    }

    #[test]
    fn track_details_are_edited() {
        let (_dir, collection) = fixture();
        let id = ids(&collection, Source::Collection, Sort::Title)[0].clone();
        let edits = [
            Edit::SetRating { track_id: id.clone(), stars: 4 },
            Edit::SetColor { track_id: id.clone(), color: 3 },
            Edit::SetComment { track_id: id.clone(), comment: "peak time".into() },
            Edit::SetField { track_id: id.clone(), field: "title".into(), value: "Zzz Renamed".into() },
            Edit::SetField { track_id: id.clone(), field: "artist".into(), value: "New Artist".into() },
            Edit::SetField { track_id: id.clone(), field: "genre".into(), value: "Techno".into() },
        ];
        for edit in edits {
            collection.edit(edit).unwrap();
        }
        let mut q = query(Source::Collection, Sort::Title);
        q.query = "Zzz Renamed".into();
        let row = collection.tracks(&q, 0, 10).unwrap().rows.into_iter().find(|r| r.id == id).unwrap();
        assert_eq!((row.rating, row.color, row.comment.as_str()), (4, 3, "peak time"));
        assert_eq!((row.artist.as_str(), row.genre.as_str()), ("New Artist", "Techno"));

        // A key is kept as rekordbox spells it, and can be cleared.
        let key = |value: &str| Edit::SetField { track_id: id.clone(), field: "key".into(), value: value.into() };
        let stored = |collection: &Collection| {
            collection.tracks(&q, 0, 10).unwrap().rows.into_iter().find(|r| r.id == id).unwrap().key
        };
        collection.edit(key("F#m")).unwrap();
        assert_eq!(stored(&collection), "F#m");
        collection.edit(key("8A")).unwrap();
        assert_eq!(stored(&collection), "Am");
        assert!(collection.edit(key("H sharp")).is_err());
        collection.edit(key("")).unwrap();
        assert_eq!(stored(&collection), "");

        collection.edit(Edit::SetColor { track_id: id.clone(), color: 0 }).unwrap();
        let row = collection.tracks(&q, 0, 10).unwrap().rows.into_iter().find(|r| r.id == id).unwrap();
        assert_eq!(row.color, 0);

        assert!(collection.edit(Edit::SetRating { track_id: id.clone(), stars: 6 }).is_err());
        assert!(collection.edit(Edit::SetColor { track_id: id.clone(), color: 9 }).is_err());
        assert!(collection
            .edit(Edit::SetField { track_id: id, field: "path".into(), value: "x".into() })
            .is_err());
    }

    fn undone(collection: &Collection) -> Option<&'static str> {
        collection.undo().unwrap().what
    }

    fn redone(collection: &Collection) -> Option<&'static str> {
        collection.redo().unwrap().what
    }

    /// (name, parent, depth) of each playlist and folder, in the tree's order.
    fn tree(collection: &Collection) -> Vec<(String, Option<String>, usize)> {
        collection.playlists().unwrap().into_iter().map(|n| (n.name, n.parent_id, n.depth as usize)).collect()
    }

    #[test]
    fn playlist_changes_are_undone_and_redone() {
        let (_dir, collection) = fixture();
        assert_eq!(undone(&collection), None);
        let start = tree(&collection);

        let folder = create(&collection, "Sets", None, true);
        let list = create(&collection, "Warm up", None, false);
        let made = tree(&collection);
        collection.edit(Edit::RenamePlaylist { id: list.clone(), name: "Opening".into() }).unwrap();
        let renamed = tree(&collection);
        collection
            .edit(Edit::MovePlaylist { id: list.clone(), parent_id: Some(folder.clone()), index: None })
            .unwrap();
        let moved = tree(&collection);
        collection.edit(Edit::DeletePlaylist { id: folder.clone() }).unwrap();
        let deleted = tree(&collection);
        assert_eq!(deleted, start);

        // Back through each, newest first.
        assert_eq!(undone(&collection), Some("deleting the playlist"));
        assert_eq!(tree(&collection), moved);
        assert_eq!(undone(&collection), Some("moving the playlist"));
        assert_eq!(tree(&collection), renamed);
        assert_eq!(undone(&collection), Some("the rename"));
        assert_eq!(tree(&collection), made);
        assert_eq!(undone(&collection), Some("making the playlist"));
        assert_eq!(undone(&collection), Some("making the folder"));
        assert_eq!(tree(&collection), start);
        assert_eq!(undone(&collection), None);

        // And forward again, to where it was.
        for _ in 0..5 {
            assert!(redone(&collection).is_some());
        }
        assert_eq!(redone(&collection), None);
        assert_eq!(tree(&collection), deleted);

        // A new change leaves nothing to redo.
        assert!(undone(&collection).is_some());
        create(&collection, "Another", None, false);
        assert_eq!(redone(&collection), None);
    }

    #[test]
    fn a_playlists_tracks_are_put_back_as_they_were() {
        let (_dir, collection) = fixture();
        let tracks = ids(&collection, Source::Collection, Sort::Title);
        let list = create(&collection, "Test", None, false);
        let other = create(&collection, "Other", None, false);
        let picked: Vec<String> = tracks[..4].to_vec();
        let members = |id: &str| ids(&collection, playlist(id), Sort::TrackNo);

        collection.edit(Edit::AddTracks { playlist_id: list.clone(), track_ids: picked.clone() }).unwrap();
        // Adding what is there already changes nothing, and leaves nothing to undo.
        collection.edit(Edit::AddTracks { playlist_id: list.clone(), track_ids: picked[..2].to_vec() }).unwrap();
        collection
            .edit(Edit::MoveTracks { playlist_id: list.clone(), track_ids: vec![picked[3].clone()], before: Some(picked[0].clone()) })
            .unwrap();
        let reordered = members(&list);
        collection
            .edit(Edit::RemoveTracks { playlist_id: list.clone(), track_ids: vec![picked[1].clone(), picked[2].clone()] })
            .unwrap();
        let removed = members(&list);

        assert_eq!(undone(&collection), Some("removing from the playlist"));
        assert_eq!(members(&list), reordered);
        assert_eq!(undone(&collection), Some("reordering the playlist"));
        assert_eq!(members(&list), picked);
        assert_eq!(undone(&collection), Some("adding to the playlist"));
        assert!(members(&list).is_empty());
        for _ in 0..3 {
            redone(&collection);
        }
        assert_eq!(members(&list), removed);

        // Changes made together are undone together.
        collection.edit(Edit::AddTracks { playlist_id: other.clone(), track_ids: picked.clone() }).unwrap();
        for id in [&list, &other] {
            collection
                .edit_in(Edit::RemoveTracks { playlist_id: id.clone(), track_ids: vec![picked[0].clone()] }, Some("one".into()))
                .unwrap();
        }
        assert!(!members(&list).contains(&picked[0]) && !members(&other).contains(&picked[0]));
        assert_eq!(undone(&collection), Some("removing from the playlist"));
        assert_eq!(members(&list), removed);
        assert_eq!(members(&other), picked);
    }

    #[test]
    fn track_details_are_undone() {
        let (_dir, collection) = fixture();
        let id = ids(&collection, Source::Collection, Sort::Title)[0].clone();
        let row = |collection: &Collection| {
            let rows = collection.tracks(&query(Source::Collection, Sort::Title), 0, 1000).unwrap().rows;
            let r = rows.into_iter().find(|r| r.id == id).unwrap();
            (r.title, r.artist, r.genre, r.key, r.rating, r.color, r.comment)
        };
        let start = row(&collection);
        let field = |field: &str, value: &str| Edit::SetField { track_id: id.clone(), field: field.into(), value: value.into() };
        let edits = [
            Edit::SetRating { track_id: id.clone(), stars: 4 },
            Edit::SetColor { track_id: id.clone(), color: 3 },
            Edit::SetComment { track_id: id.clone(), comment: "peak time".into() },
            field("title", "Zzz Renamed"),
            field("artist", "New Artist"),
            field("genre", "Techno"),
            field("key", "8A"),
        ];
        let count = edits.len();
        for edit in edits {
            collection.edit(edit).unwrap();
        }
        let edited = row(&collection);
        assert_ne!(edited, start);
        // The same value again is no change, and nothing more to undo.
        collection.edit(Edit::SetRating { track_id: id.clone(), stars: 4 }).unwrap();
        collection.edit(field("genre", "Techno")).unwrap();

        assert_eq!(undone(&collection), Some("the edit"));
        for _ in 1..count {
            assert!(undone(&collection).is_some());
        }
        assert_eq!(row(&collection), start);
        assert_eq!(undone(&collection), None);
        for _ in 0..count {
            assert!(redone(&collection).is_some());
        }
        assert_eq!(row(&collection), edited);
    }

    #[test]
    fn deleted_tracks_come_back_where_they_were() {
        let (_dir, collection) = fixture();
        let tracks = ids(&collection, Source::Collection, Sort::Title);
        let (one, two) = (create(&collection, "One", None, false), create(&collection, "Two", None, false));
        collection.edit(Edit::AddTracks { playlist_id: one.clone(), track_ids: tracks[..5].to_vec() }).unwrap();
        collection.edit(Edit::AddTracks { playlist_id: two.clone(), track_ids: tracks[2..8].to_vec() }).unwrap();
        let members = |id: &str| ids(&collection, playlist(id), Sort::TrackNo);
        let gone = vec![tracks[0].clone(), tracks[3].clone(), tracks[20].clone()];

        collection.edit(Edit::DeleteTracks { track_ids: gone.clone(), trash_files: false }).unwrap();
        assert_eq!(collection.status().unwrap().tracks, tracks.len() - 3);
        assert_eq!(members(&one), [&tracks[1], &tracks[2], &tracks[4]].map(Clone::clone));
        assert_eq!(members(&two).len(), 5);

        assert_eq!(undone(&collection), Some("deleting from the collection"));
        assert_eq!(ids(&collection, Source::Collection, Sort::Title), tracks);
        assert_eq!(members(&one), tracks[..5]);
        assert_eq!(members(&two), tracks[2..8]);

        assert_eq!(redone(&collection), Some("deleting from the collection"));
        assert_eq!(collection.status().unwrap().tracks, tracks.len() - 3);
        assert_eq!(members(&two).len(), 5);
        undone(&collection);
        assert_eq!(members(&one), tracks[..5]);

        // With the files in the Trash there is no undoing it.
        collection.edit(Edit::DeleteTracks { track_ids: gone, trash_files: true }).unwrap();
        assert_eq!(redone(&collection), None);
        // What is there to undo is adding them to a playlist, which they're no longer in.
        assert!(collection.undo().is_err());
        assert_eq!(collection.status().unwrap().tracks, tracks.len() - 3);
    }

    #[test]
    fn an_undo_that_would_write_over_a_later_change_is_refused() {
        let (_dir, collection) = fixture();
        let id = ids(&collection, Source::Collection, Sort::Title)[0].clone();
        collection.edit(Edit::SetRating { track_id: id.clone(), stars: 2 }).unwrap();
        collection.edit(Edit::SetRating { track_id: id.clone(), stars: 4 }).unwrap();
        // Something the history doesn't know of rates it too.
        collection.write(|writer, _| writer.set_rating(&id, 5).map(|_| ()).map_err(rekordbox_err)).unwrap();

        let refused = collection.undo().unwrap_err().to_string();
        assert!(refused.contains("Couldn't undo the rating") && refused.contains("changed since"), "{refused}");
        let rating = |collection: &Collection| {
            let rows = collection.tracks(&query(Source::Collection, Sort::Title), 0, 1000).unwrap().rows;
            rows.into_iter().find(|r| r.id == id).unwrap().rating
        };
        assert_eq!(rating(&collection), 5);
    }

    #[test]
    fn changes_made_elsewhere_leave_nothing_to_undo() {
        let (_dir, collection) = fixture();
        let before = tree(&collection);
        create(&collection, "Mine", None, false);
        let made = tree(&collection);
        assert_ne!(made, before);
        // rekordbox ran in between, and moved its count of changes on.
        {
            let location = collection.loaded().unwrap().location.clone();
            let writer = Writer::open(location, &collection.backup_dir).unwrap();
            writer
                .library()
                .connection()
                .execute("UPDATE agentRegistry SET int_1 = int_1 + 7 WHERE registry_id = 'localUpdateCount'", [])
                .unwrap();
        }
        assert_eq!(undone(&collection), None);
        assert_eq!(tree(&collection), made);
    }

    #[test]
    fn the_library_is_backed_up_once_per_session() {
        let (dir, collection) = fixture();
        let backups = dir.path().join("backups");
        assert!(backups_in(&backups).is_empty());
        create(&collection, "One", None, false);
        create(&collection, "Two", None, false);
        assert_eq!(backups_in(&backups).len(), 1);
    }
}

/// What SQLite says of a database with a page or index it can't make sense of.
const MALFORMED: &str = "database disk image is malformed";

/// A change refused because the library file is damaged says so, and says
/// where: SQLite's own message names neither the step that met the damage
/// nor the table it is in. The copy made before a session's first change
/// reads every page of the file, so damage anywhere, however old and however
/// far from what is being changed, stops that first; rekordbox, reading only
/// what it needs, may never have met it. What a check of the file finds goes
/// in the log, as it is what says how to mend it.
fn damage_report(writer: &Writer, error: AppError) -> AppError {
    let said = error.to_string();
    if !said.contains(MALFORMED) {
        return error;
    }
    let step = if writer.backed_up() { "making the change" } else { "copying the library before changing it" };
    let found: Vec<String> = writer
        .library()
        .connection()
        .prepare("PRAGMA quick_check(20)")
        .and_then(|mut check| check.query_map([], |row| row.get::<_, String>(0))?.collect())
        .unwrap_or_else(|e| vec![format!("the check itself failed: {e}")]);
    tracing::error!(
        "The rekordbox library at {} is damaged; met while {step}. quick_check: {}",
        writer.library().location().master_db.display(),
        found.join(" | ")
    );
    let first = found.first().filter(|line| line.as_str() != "ok").map(|line| format!(" ({line})")).unwrap_or_default();
    AppError::Rekordbox(format!(
        "rekordbox's library file is damaged{first}, which stopped {step}. Nothing was changed. The details are in the log."
    ))
}
