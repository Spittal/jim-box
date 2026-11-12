//! The changes made to the library this session, to undo and redo.
//!
//! Each change keeps what rbxport's writer hands back for undoing it: the
//! exact rows and values it wrote. Undoing is a new change, written the same
//! guarded way with an update number of its own, and refused when what it
//! would write over has changed since. Nothing is kept in rekordbox's file,
//! and nothing past the session: the library is rekordbox's between runs.
//!
//! What isn't here can't be undone: files sent to the Trash, and the work
//! done in bulk (imports, analysis, consolidating). The copy of `master.db`
//! made before a session's first change covers those.

use rbl_db::write::{
    CueRevision, PlaylistCreation, PlaylistDeletion, PlaylistMove, PlaylistRename, PlaylistReorder,
    PlaylistTrackAddition, PlaylistTrackRemoval, TrackEdit, Writer,
};
use rbl_db::DbError;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Transaction, TransactionBehavior};

use super::{grid, prepare, rekordbox_err};
use crate::error::{AppError, AppResult};

/// Changes kept; the oldest go once there are more.
const KEPT: usize = 100;

/// A beat grid as `GridChange::Restore` takes it.
pub(super) type Beats = Vec<(u32, u8, u16)>;

/// One thing written, with what it takes to write it back.
pub(super) enum Step {
    Created(PlaylistCreation),
    Renamed(PlaylistRename),
    Moved(PlaylistMove),
    PlaylistDeleted(PlaylistDeletion),
    TracksAdded(PlaylistTrackAddition),
    TracksRemoved(PlaylistTrackRemoval),
    Reordered(PlaylistReorder),
    /// A rating, colour, comment or text field.
    Track(TrackEdit),
    Cues(CueRevision),
    Lock { track_id: String, before: bool, after: bool },
    /// A beat grid, in the track's analysis files, and what the app knew of
    /// it (whose grid it was, whether it had been reviewed).
    Grid {
        track_id: String,
        before: Beats,
        after: Beats,
        state_before: Option<grid::State>,
        state_after: Option<grid::State>,
    },
    /// Tracks out of the collection: first out of each playlist they were
    /// in, then gone themselves. Their files stayed where they were.
    TracksDeleted { track_ids: Vec<String>, removals: Vec<PlaylistTrackRemoval> },
}

/// What stops an undo or redo: something else has changed what it would
/// write over. Worded to follow "Couldn't undo …: ".
fn conflict(why: &str) -> AppError {
    AppError::Rekordbox(why.to_owned())
}

impl Step {
    /// Writes the step back (`back`), or again after that.
    fn write(
        &self,
        back: bool,
        writer: &mut Writer,
        index: &rbl_index::Library,
        grids: &grid::States,
    ) -> AppResult<()> {
        match self {
            Self::Created(edit) => {
                if back { writer.undo_creation(edit) } else { writer.redo_creation(edit) }.map_err(rekordbox_err)?;
            }
            Self::Renamed(edit) => {
                if back { writer.undo_rename(edit) } else { writer.redo_rename(edit) }.map_err(rekordbox_err)?;
            }
            Self::Moved(edit) => {
                if back { writer.undo_move(edit) } else { writer.redo_move(edit) }.map_err(rekordbox_err)?;
            }
            Self::PlaylistDeleted(edit) => {
                if back { writer.restore_playlist(edit) } else { writer.redo_playlist_deletion(edit) }
                    .map_err(rekordbox_err)?;
            }
            Self::TracksAdded(edit) => {
                if back { writer.undo_track_addition(edit) } else { writer.redo_track_addition(edit) }
                    .map_err(rekordbox_err)?;
            }
            Self::TracksRemoved(edit) => {
                if back { writer.undo_track_removal(edit) } else { writer.redo_track_removal(edit) }
                    .map_err(rekordbox_err)?;
            }
            Self::Reordered(edit) => {
                if back { writer.undo_reorder(edit) } else { writer.redo_reorder(edit) }.map_err(rekordbox_err)?;
            }
            Self::Track(edit) => {
                if back { writer.undo_track_edit(edit) } else { writer.redo_track_edit(edit) }
                    .map_err(rekordbox_err)?;
            }
            Self::Cues(edit) => {
                if back { writer.undo_cue_revision(edit) } else { writer.redo_cue_revision(edit) }
                    .map_err(rekordbox_err)?;
            }
            Self::Lock { track_id, before, after } => {
                let (from, to) = if back { (after, before) } else { (before, after) };
                if prepare::is_locked(writer.library(), track_id)? != *from {
                    return Err(conflict("the track's analysis lock has changed since"));
                }
                writer.set_analysis_lock(track_id, *to).map_err(rekordbox_err)?;
            }
            Self::Grid { track_id, before, after, state_before, state_after } => {
                let (from, to, state) = if back { (after, before, state_before) } else { (before, after, state_after) };
                if prepare::grid_beats(writer, index, track_id).as_ref() != Some(from) {
                    return Err(conflict("the track's beat grid has changed since"));
                }
                prepare::change_grid(writer, index, track_id, prepare::GridChange::Restore { beats: to.clone() }, None)?;
                grids.put(track_id, state.clone());
            }
            Self::TracksDeleted { track_ids, removals } => {
                if back {
                    revive_tracks(writer, track_ids)?;
                    for removal in removals.iter().rev() {
                        writer.undo_track_removal(removal).map_err(rekordbox_err)?;
                    }
                } else {
                    for removal in removals {
                        writer.redo_track_removal(removal).map_err(rekordbox_err)?;
                    }
                    writer.delete_tracks(track_ids).map_err(rekordbox_err)?;
                }
            }
        }
        Ok(())
    }
}

/// One change as the user made it: one step, or several made together.
pub(super) struct Entry {
    /// What it was, to follow "Undid" or "Couldn't undo": "renaming the playlist".
    pub what: &'static str,
    /// Changes made in a row under one name are one entry.
    group: Option<String>,
    steps: Vec<Step>,
}

impl Entry {
    /// Writes the whole entry back, last step first, or again in the order made.
    pub fn write(
        &self,
        back: bool,
        writer: &mut Writer,
        index: &rbl_index::Library,
        grids: &grid::States,
    ) -> AppResult<()> {
        let steps: Vec<&Step> = if back { self.steps.iter().rev().collect() } else { self.steps.iter().collect() };
        steps.into_iter().try_for_each(|step| step.write(back, writer, index, grids))
    }

    /// The tracks whose cues, grid or lock the entry changes: what a player
    /// showing one of them reads again.
    pub fn prepared(&self) -> Vec<String> {
        let mut tracks: Vec<String> = self
            .steps
            .iter()
            .filter_map(|step| match step {
                Step::Cues(edit) => Some(edit.content().to_owned()),
                Step::Lock { track_id, .. } | Step::Grid { track_id, .. } => Some(track_id.clone()),
                _ => None,
            })
            .collect();
        tracks.sort();
        tracks.dedup();
        tracks
    }

    /// The tracks the entry took out of the collection.
    pub fn deleted(&self) -> Vec<String> {
        self.steps
            .iter()
            .filter_map(|step| match step {
                Step::TracksDeleted { track_ids, .. } => Some(track_ids.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }
}

/// The session's changes: those made, newest last, and those undone.
#[derive(Default)]
pub(super) struct History {
    done: Vec<Entry>,
    undone: Vec<Entry>,
    /// rekordbox's count of local changes after the last one made here.
    counter: Option<i64>,
}

/// The tracks deleted by entries that can no longer be undone.
fn let_go(entries: impl IntoIterator<Item = Entry>) -> Vec<String> {
    entries.into_iter().flat_map(|entry| entry.deleted()).collect()
}

impl History {
    /// Keeps a change just made. Returns the tracks whose deletion can no
    /// longer be undone, for what the app knew of them to be forgotten.
    pub fn record(&mut self, what: &'static str, group: Option<String>, steps: Vec<Step>) -> Vec<String> {
        if steps.is_empty() {
            return Vec::new();
        }
        // What was undone would be redone onto a library it no longer fits.
        self.undone.clear();
        match self.done.last_mut() {
            Some(last) if group.is_some() && last.group == group => last.steps.extend(steps),
            _ => self.done.push(Entry { what, group, steps }),
        }
        let extra = self.done.len().saturating_sub(KEPT);
        let_go(self.done.drain(..extra))
    }

    /// A change was made that isn't kept here: what was undone before it
    /// can't be redone onto it.
    pub fn changed(&mut self) {
        self.undone.clear();
    }

    /// Called with rekordbox's count of changes before anything is written:
    /// one that isn't as it was left means the library was changed elsewhere,
    /// and what is kept here no longer describes it. Returns as `record` does.
    pub fn seen(&mut self, counter: Option<i64>) -> Vec<String> {
        if counter.is_some() && counter == self.counter {
            return Vec::new();
        }
        self.undone.clear();
        let_go(std::mem::take(&mut self.done))
    }

    /// Called with the count once a change has been written.
    pub fn wrote(&mut self, counter: Option<i64>) {
        self.counter = counter;
    }

    /// The change to undo (`back`) or redo, taken out.
    pub fn take(&mut self, back: bool) -> Option<Entry> {
        if back { self.done.pop() } else { self.undone.pop() }
    }

    /// Keeps an entry that was undone (`back`) or redone, for the other.
    pub fn put(&mut self, back: bool, entry: Entry) {
        if back { self.undone.push(entry) } else { self.done.push(entry) }
    }
}

/// rekordbox's count of the changes made to the library on this machine,
/// which every writer of it moves on.
pub(super) fn update_count(conn: &Connection) -> Option<i64> {
    conn.query_row(
        "SELECT COALESCE(int_1, 0) FROM agentRegistry WHERE registry_id = 'localUpdateCount'",
        [],
        |row| row.get(0),
    )
    .ok()
}

/// The playlists holding any of `track_ids`, each with those of them it holds.
pub(super) fn playlists_holding(conn: &Connection, track_ids: &[String]) -> AppResult<Vec<(String, Vec<String>)>> {
    let mut held: Vec<(String, Vec<String>)> = Vec::new();
    for chunk in track_ids.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        let mut statement = conn
            .prepare(&format!(
                "SELECT DISTINCT PlaylistID, ContentID FROM djmdSongPlaylist
                 WHERE rb_local_deleted = 0 AND ContentID IN ({marks}) ORDER BY PlaylistID"
            ))
            .map_err(rekordbox_err)?;
        let found = statement
            .query_map(params_from_iter(chunk), |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .and_then(Iterator::collect::<rusqlite::Result<Vec<_>>>)
            .map_err(rekordbox_err)?;
        for (playlist, track) in found {
            match held.iter_mut().find(|(id, _)| *id == playlist) {
                Some((_, tracks)) => tracks.push(track),
                None => held.push((playlist, vec![track])),
            }
        }
    }
    Ok(held)
}

/// Deletes tracks so that it can be undone: out of each playlist first, the
/// writer keeping the rows it took, then out of the collection, which is
/// then all `delete_tracks` has left to do. The rows end as it alone leaves
/// them. If it can't finish, the playlists get their tracks back.
pub(super) fn delete_tracks(writer: &mut Writer, track_ids: &[String]) -> AppResult<Step> {
    let held = playlists_holding(writer.library().connection(), track_ids)?;
    let mut removals: Vec<PlaylistTrackRemoval> = Vec::new();
    let mut work = || -> Result<(), DbError> {
        for (playlist, tracks) in &held {
            let (_, removal) = writer.remove_tracks_with_undo(playlist, tracks)?;
            if !removal.is_empty() {
                removals.push(removal);
            }
        }
        writer.delete_tracks(track_ids).map(|_| ())
    };
    if let Err(e) = work() {
        for removal in removals.iter().rev() {
            if let Err(e) = writer.undo_track_removal(removal) {
                tracing::warn!("Couldn't put a playlist's tracks back after a delete that failed: {e}");
            }
        }
        return Err(rekordbox_err(e));
    }
    Ok(Step::TracksDeleted { track_ids: track_ids.to_vec(), removals })
}

/// Brings deleted tracks back into the collection: the one thing undone
/// here that rbxport's writer has no call for. A deleted track is its row
/// with `rb_local_deleted` set and nothing else changed, so clearing that
/// is all of it; stamped as the writer stamps a row it changes, with the
/// next update numbers and the counter moved on in the same transaction.
fn revive_tracks(writer: &Writer, track_ids: &[String]) -> AppResult<()> {
    // The writer asks this before each change of its own.
    if writer.library().location().is_real_install && crate::rekordbox::is_running() {
        return Err(AppError::Rekordbox("rekordbox is running. Quit it before making changes.".into()));
    }
    let tx = Transaction::new_unchecked(writer.library().connection(), TransactionBehavior::Immediate)
        .map_err(rekordbox_err)?;

    let mut paths: Vec<String> = Vec::new();
    for id in track_ids {
        let row: Option<(i64, Option<String>)> = tx
            .query_row("SELECT COALESCE(rb_local_deleted, 0), FolderPath FROM djmdContent WHERE ID = ?1", [id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
            .map_err(rekordbox_err)?;
        match row {
            Some((1, path)) => paths.extend(path.filter(|path| !path.is_empty())),
            Some(_) => return Err(conflict("the tracks are back in the collection already")),
            None => return Err(conflict("the tracks have gone from rekordbox's library for good")),
        }
    }
    // A file added again since is in the collection as another track.
    for chunk in paths.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        let again: bool = tx
            .query_row(
                &format!("SELECT EXISTS (SELECT 1 FROM djmdContent WHERE rb_local_deleted = 0 AND FolderPath IN ({marks}))"),
                params_from_iter(chunk),
                |row| row.get(0),
            )
            .map_err(rekordbox_err)?;
        if again {
            return Err(conflict("a file of theirs has been added to the collection again since"));
        }
    }

    let stamp = rbl_core::time::now();
    let mut usn = next_usn(&tx)?;
    for (at, id) in track_ids.iter().enumerate() {
        if at > 0 {
            usn = usn.checked_add(1).ok_or_else(|| AppError::Rekordbox("local update counter overflow".into()))?;
        }
        tx.execute(
            "UPDATE djmdContent SET rb_local_deleted = 0, rb_local_usn = ?1, updated_at = ?2
             WHERE ID = ?3 AND rb_local_deleted = 1",
            params![usn, stamp, id],
        )
        .map_err(rekordbox_err)?;
    }
    if !track_ids.is_empty() {
        let moved = tx
            .execute(
                "UPDATE agentRegistry SET int_1 = ?1, updated_at = ?2 WHERE registry_id = 'localUpdateCount'",
                params![usn, stamp],
            )
            .map_err(rekordbox_err)?;
        if moved != 1 {
            return Err(AppError::Rekordbox("missing or duplicate local update counter".into()));
        }
    }
    tx.commit().map_err(rekordbox_err)
}

/// The next update number, as rbxport's writer finds it (its own is not
/// public): one past the counter and every `rb_local_usn` that shares it.
fn next_usn(conn: &Connection) -> AppResult<i64> {
    let mut highest = update_count(conn).ok_or_else(|| AppError::Rekordbox("missing local update counter".into()))?;
    for table in ["djmdContent", "djmdPlaylist", "djmdSongPlaylist", "djmdSongHotCueBanklist"] {
        let exists: bool = conn
            .query_row("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)", [table], |row| {
                row.get(0)
            })
            .map_err(rekordbox_err)?;
        if exists {
            let max: i64 = conn
                .query_row(&format!("SELECT COALESCE(MAX(rb_local_usn), 0) FROM {table}"), [], |row| row.get(0))
                .map_err(rekordbox_err)?;
            highest = highest.max(max);
        }
    }
    highest.checked_add(1).ok_or_else(|| AppError::Rekordbox("local update counter overflow".into()))
}
