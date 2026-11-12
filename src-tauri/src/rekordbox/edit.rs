//! Changes to rekordbox's library, each made through rbxport's guarded
//! writer: refused while rekordbox is running, and preceded by a backup of
//! `master.db` the first time in a session.
//!
//! The database is opened for one change and closed again, then read afresh,
//! so rekordbox can take the file back between changes.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use rbl_db::write::{backups_in, TrackField, Writer, ROOT};
use serde::{Deserialize, Serialize};

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

impl Collection {
    /// Makes one change and reads the library again. Blocking.
    pub fn edit(&self, edit: Edit) -> AppResult<EditOutcome> {
        let mut warning = None;
        let created_id = match edit {
            Edit::UseGrid { track_id, source } => {
                let grid = self
                    .grids
                    .choice(&track_id, source)
                    .ok_or_else(|| AppError::Validation("That grid wasn't kept. Analyse the track again to have it.".into()))?;
                let change = prepare::GridChange::Restore { beats: grid.beats };
                self.write(|writer, index| prepare::change_grid(writer, index, &track_id, change, None))?;
                self.grids.switched(&track_id, source);
                None
            }
            edit => {
                // Changing a grid by hand is looking it over.
                let regridded = match &edit {
                    Edit::Grid { track_id, .. } => Some(track_id.clone()),
                    _ => None,
                };
                // Found before the tracks go: afterwards the library no longer knows where they were.
                let doomed: Vec<PathBuf> = match &edit {
                    Edit::DeleteTracks { track_ids, trash_files: true } => {
                        track_ids.iter().filter_map(|id| self.path_of(id)).collect()
                    }
                    _ => Vec::new(),
                };
                let created_id = self.write(|writer, index| apply(writer, index, edit))?;
                warning = trash_files(&doomed);
                if let Some(track_id) = regridded {
                    self.grids.edited(&track_id);
                }
                created_id
            }
        };
        Ok(EditOutcome { status: self.status()?, playlists: self.playlists()?, created_id, warning })
    }

    /// Opens the library for one piece of work, closes it, and reads it
    /// again (even after a failure, so the view shows what is there).
    /// `work` gets the library as it was read before. Blocking.
    pub(super) fn write<T>(
        &self,
        work: impl FnOnce(&mut Writer, &rbl_index::Library) -> AppResult<T>,
    ) -> AppResult<T> {
        let _one_at_a_time = self.editing.lock().expect("edit lock poisoned");
        let loaded = self.loaded_or_read()?;
        let location = loaded.location.clone();

        let mut writer = Writer::open(location.clone(), &self.backup_dir).map_err(rekordbox_err)?;
        if self.backed_up.load(Ordering::Acquire) {
            writer.mark_backed_up();
        }
        let result = work(&mut writer, &loaded.index);
        if writer.backed_up() && !self.backed_up.swap(true, Ordering::AcqRel) {
            self.prune_backups();
        }
        drop(writer);

        self.load_from(location)?;
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
}

fn apply(writer: &mut Writer, index: &rbl_index::Library, edit: Edit) -> AppResult<Option<String>> {
    let parent = |id: &Option<String>| id.clone().unwrap_or_else(|| ROOT.to_owned());
    let mut created = None;
    match edit {
        Edit::CreatePlaylist { name, parent_id, folder } => {
            let (name, parent) = (named(&name)?, parent(&parent_id));
            created = Some(if folder {
                writer.create_folder(name, &parent)
            } else {
                writer.create_playlist(name, &parent)
            }
            .map_err(rekordbox_err)?);
        }
        Edit::RenamePlaylist { id, name } => {
            writer.rename(&id, named(&name)?).map_err(rekordbox_err)?;
        }
        Edit::DeletePlaylist { id } => {
            writer.delete_playlist(&id).map_err(rekordbox_err)?;
        }
        Edit::MovePlaylist { id, parent_id, index } => {
            writer.move_to(&id, &parent(&parent_id), index).map_err(rekordbox_err)?;
        }
        Edit::AddTracks { playlist_id, track_ids } => {
            writer.add_tracks(&playlist_id, &track_ids).map_err(rekordbox_err)?;
        }
        Edit::RemoveTracks { playlist_id, track_ids } => {
            writer.remove_tracks(&playlist_id, &track_ids).map_err(rekordbox_err)?;
        }
        Edit::MoveTracks { playlist_id, track_ids, before } => {
            let order = moved_order(index, &playlist_id, &track_ids, before.as_deref())?;
            writer.reorder(&playlist_id, &order).map_err(rekordbox_err)?;
        }
        Edit::DeleteTracks { track_ids, .. } => {
            writer.delete_tracks(&track_ids).map_err(rekordbox_err)?;
        }
        Edit::SetRating { track_id, stars } => {
            writer.set_rating(&track_id, stars).map_err(rekordbox_err)?;
        }
        Edit::SetColor { track_id, color } => {
            if color > 8 {
                return Err(AppError::Validation("That isn't one of rekordbox's colours".into()));
            }
            let id = color.to_string();
            writer
                .set_color(&track_id, (color > 0).then_some(id.as_str()))
                .map_err(rekordbox_err)?;
        }
        Edit::SetComment { track_id, comment } => {
            writer.set_comment(&track_id, &comment).map_err(rekordbox_err)?;
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
            writer.set_field(&track_id, field, &value).map_err(rekordbox_err)?;
        }
        Edit::AddCue { track_id, slot, position_ms, end_ms } => {
            if end_ms.is_some_and(|end| end <= position_ms) {
                return Err(AppError::Validation("A loop has to end after it starts".into()));
            }
            created = Some(prepare::add_cue(writer, &track_id, slot, position_ms, end_ms)?);
        }
        Edit::DeleteCue { cue_id } => {
            writer.delete_cue(&cue_id).map_err(rekordbox_err)?;
        }
        Edit::NameCue { cue_id, comment } => {
            writer.set_cue_comment(&cue_id, comment.trim()).map_err(rekordbox_err)?;
        }
        Edit::Grid { track_id, change, from_ms } => {
            prepare::change_grid(writer, index, &track_id, change, from_ms)?;
        }
        Edit::UseGrid { .. } => unreachable!("handled by `edit`"),
        Edit::LockAnalysis { track_id, locked } => {
            writer.set_analysis_lock(&track_id, locked).map_err(rekordbox_err)?;
        }
    }
    Ok(created)
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
