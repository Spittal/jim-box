//! Preparing a track: what the player shows of it (waveform, beat grid,
//! cues), and the changes made there — cues, loops and the beat grid.
//!
//! Cues live in `master.db` (`djmdCue`); the beat grid lives in the track's
//! analysis files, with its tempo mirrored on the track's row.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rbl_anlz::grid::{apply_with_duration, tempo_x100, validate, Edit as AnlzEdit};
use rbl_db::write::Writer;
use rbl_db::OpenMode;
use serde::{Deserialize, Serialize};

use super::analyse::replace_file;
use super::{rekordbox_err, Collection};
use crate::error::{AppError, AppResult};

/// Hot cue slots in pad order, as `djmdCue.Kind`: A to C are 1 to 3, and 4
/// is taken (a memory loop set as the Active Loop), so D onward are 5 up.
const HOT_KINDS: [u8; 16] = [1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17];
const MEMORY: u8 = 0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cue {
    /// `djmdCue.ID`.
    pub id: String,
    /// The hot cue pad, 0 for A; None for a memory cue.
    pub slot: Option<u8>,
    pub position_ms: u32,
    /// Where a loop ends; None for a plain cue.
    pub end_ms: Option<u32>,
    pub comment: String,
    /// The colour rekordbox draws a hot cue in, as `#RRGGBB`: its own, or
    /// its pad's default. None for a memory cue.
    pub color: Option<String>,
}

/// rekordbox's View › Color › HOT CUE color: the colours hot cues without one
/// of their own are drawn in, by pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HotCueColors {
    Cdj,
    Cold1,
    Cold2,
    Colorful,
}

impl HotCueColors {
    /// What rekordbox is set to (`HotCueColorType` in `rekordbox3.settings`);
    /// COLORFUL, its default, when that can't be read.
    fn installed() -> Self {
        rbl_db::locate::Sources::installed()
            .ok()
            .and_then(|sources| std::fs::read_to_string(sources.rekordbox_settings?).ok())
            .map_or(Self::Colorful, |settings| Self::from_settings(&settings))
    }

    fn from_settings(settings: &str) -> Self {
        let value = settings
            .split_once("name=\"HotCueColorType\"")
            .and_then(|(_, rest)| rest.split_once("val=\"")?.1.split_once('"'))
            .map(|(value, _)| value);
        match value {
            Some("1") => Self::Cdj,
            Some("2") => Self::Cold1,
            Some("3") => Self::Cold2,
            _ => Self::Colorful,
        }
    }

    /// The `ColorTableIndex` of each of the first eight pads; the next eight
    /// repeat them. rbxport read these tables from rekordbox 7.
    fn pads(self) -> [u8; 8] {
        match self {
            Self::Cdj => [22; 8],
            Self::Cold1 => [4, 18, 58, 8, 12, 60, 14, 1],
            Self::Cold2 => [14, 11, 8, 5, 3, 1, 61, 59],
            Self::Colorful => [43, 8, 23, 60, 19, 38, 1, 32],
        }
    }

    /// The colour of the hot cue on pad `slot` (0 for A) whose
    /// `djmdCue.ColorTableIndex` is `index`. One given a colour keeps it; one
    /// without (0, NULL or off the palette) takes its pad's, or if it's a
    /// loop, orange in every mode.
    fn color_of(self, index: i64, slot: u8, is_loop: bool) -> String {
        /// Every table's entry for a loop.
        const LOOP: u8 = 36;
        let own = u8::try_from(index).ok().filter(|index| (1..=64).contains(index));
        let index = own.unwrap_or_else(|| match (is_loop, self) {
            (true, _) => LOOP,
            // The CDJ table's second eight pads are a lighter green.
            (false, Self::Cdj) if slot >= 8 => 26,
            (false, _) => self.pads()[usize::from(slot % 8)],
        });
        let [r, g, b] = rbl_anlz::cue_colour_drawn(index).unwrap_or([0x3c, 0xeb, 0x50]);
        format!("#{r:02x}{g:02x}{b:02x}")
    }
}

/// A track as the player needs it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preparation {
    pub track_id: String,
    pub duration_ms: u32,
    /// Beats as (milliseconds, place in the bar 1 to 4, tempo there in
    /// hundredths of a BPM); empty before analysis.
    pub beats: Vec<(u32, u8, u16)>,
    /// The grid's tempo at its start.
    pub bpm: Option<f64>,
    /// Ordered by position.
    pub cues: Vec<Cue>,
    /// rekordbox's analysis lock: the grid can't be changed.
    pub locked: bool,
    /// The colour a hot cue set now on each of pads A to H is drawn in, so
    /// one can be shown before it's written.
    pub pad_colors: Vec<String>,
    /// And the colour of a loop set on any of them.
    pub loop_color: String,
}

/// How many columns a track list preview has: enough for the column's width
/// on a dense screen.
const PREVIEW_COLUMNS: usize = 400;
/// Previews held at once, about a kilobyte each; past it they're made afresh.
const MAX_HELD_PREVIEWS: usize = 8000;

/// A track's small waveform as made from its analysis files, kept while
/// those files stay as they were.
#[derive(Clone)]
pub(super) struct WavePreview {
    /// When the two files it could come from were last written.
    stamp: [Option<std::time::SystemTime>; 2],
    /// 7, 5 or 3; 0 when the files hold no waveform.
    kind: u8,
    columns: std::sync::Arc<Vec<u8>>,
    duration_ms: Option<u32>,
}

/// What the track list shows of a track: its whole waveform, small, and its cues.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub track_id: String,
    /// The form `columns` is in, as [`Collection::waveform`] names them: 7,
    /// 5 or 3; 0 before analysis.
    pub kind: u8,
    /// The whole track in a few hundred columns of that form, each the
    /// loudest of the detailed columns it covers.
    pub columns: Vec<u8>,
    /// The length the waveform covers, which cues are placed along.
    pub duration_ms: u32,
    /// Ordered by position.
    pub cues: Vec<Cue>,
}

/// Bytes in a column of a detailed waveform of this kind.
fn stride_of(kind: u8) -> usize {
    match kind {
        7 => 3,
        5 => 2,
        _ => 1,
    }
}

/// A detailed waveform in `width` columns (or as many as it has, if fewer),
/// each the loudest of those it covers: as the player draws its overview.
fn shrink(kind: u8, data: &[u8], width: usize) -> Vec<u8> {
    let stride = stride_of(kind);
    let columns = data.len() / stride;
    let width = width.min(columns);
    let mut out = Vec::with_capacity(width * stride);
    for px in 0..width {
        let from = px * columns / width;
        let to = ((px + 1) * columns / width).clamp(from + 1, columns);
        let covered = data[from * stride..to * stride].chunks_exact(stride);
        match kind {
            // Three bands, each its own loudest.
            7 => {
                let mut bands = [0u8; 3];
                for column in covered {
                    for (band, &value) in bands.iter_mut().zip(column) {
                        *band = (*band).max(value);
                    }
                }
                out.extend_from_slice(&bands);
            }
            // Colour and height in one: the tallest column, whole.
            5 => {
                let tallest = covered.max_by_key(|column| (column[1] >> 2) & 31).unwrap_or(&[0, 0]);
                out.extend_from_slice(tallest);
            }
            _ => out.push(covered.map(|column| column[0]).max_by_key(|byte| byte & 31).unwrap_or(0)),
        }
    }
    out
}

/// A change to a track's beat grid.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum GridChange {
    /// Slides the grid by milliseconds.
    Nudge { ms: i32 },
    /// Slides the grid so its nearest beat is at `time_ms`.
    Align { time_ms: u32 },
    /// Slides the grid so its nearest beat is at `time_ms`, and makes that
    /// beat the first of a bar.
    Downbeat { time_ms: u32 },
    Double,
    Halve,
    /// Respaces the grid at a tempo, holding its first beat.
    Tempo { bpm: f64 },
    /// Widens or narrows the spacing of the beats, by moving the beat
    /// nearest `time_ms` this many milliseconds while the first one stays.
    Stretch { by_ms: i32, time_ms: u32 },
    /// A tapped tempo: respaces the grid at it, with a bar starting at
    /// `anchor_ms`, where the first tap was.
    Tap { bpm: f64, anchor_ms: u32 },
    /// Puts back a grid as it was read, for undo: (milliseconds, place in
    /// the bar, tempo in hundredths of a BPM) for each beat.
    Restore { beats: Vec<(u32, u8, u16)> },
}

fn slot_of(kind: i64) -> Option<u8> {
    HOT_KINDS.iter().position(|&k| i64::from(k) == kind).map(|slot| slot as u8)
}

/// The three analysis files of a track, from its `AnalysisDataPath`.
fn analysis_files(share_root: &Path, relative: &str) -> [PathBuf; 3] {
    let dat = rbl_anlz::resolve(share_root, relative);
    [dat.clone(), rbl_anlz::sibling(&dat, "EXT"), rbl_anlz::sibling(&dat, "2EX")]
}

pub(super) fn is_locked(db: &rbl_db::Library, track_id: &str) -> AppResult<bool> {
    db.connection()
        .query_row(
            "SELECT (COALESCE(Analysed, 0) & 128) != 0 FROM djmdContent WHERE ID = ?1 AND rb_local_deleted = 0",
            [track_id],
            |r| r.get(0),
        )
        .map_err(rekordbox_err)
}

fn cues_of(db: &rbl_db::Library, track_id: &str) -> AppResult<Vec<Cue>> {
    Ok(cues_of_each(db, &[track_id])?.remove(track_id).unwrap_or_default())
}

/// The cues of each of these tracks that has any, by track id, in one query.
fn cues_of_each(db: &rbl_db::Library, track_ids: &[&str]) -> AppResult<HashMap<String, Vec<Cue>>> {
    let mut found: HashMap<String, Vec<Cue>> = HashMap::new();
    if track_ids.is_empty() {
        return Ok(found);
    }
    let places = vec!["?"; track_ids.len()].join(",");
    let mut statement = db
        .connection()
        .prepare(&format!(
            "SELECT ID, COALESCE(Kind, 0), COALESCE(InMsec, 0), COALESCE(OutMsec, -1), COALESCE(Comment, ''),
                    COALESCE(ColorTableIndex, 0), ContentID
             FROM djmdCue WHERE ContentID IN ({places}) AND rb_local_deleted = 0"
        ))
        .map_err(rekordbox_err)?;
    for (at, track_id) in track_ids.iter().enumerate() {
        statement.raw_bind_parameter(at + 1, track_id).map_err(rekordbox_err)?;
    }
    let colors = HotCueColors::installed();
    let mut rows = statement.raw_query();
    while let Some(r) = rows.next().map_err(rekordbox_err)? {
        let (id, kind): (String, i64) = (r.get(0).map_err(rekordbox_err)?, r.get(1).map_err(rekordbox_err)?);
        let (start, end): (i64, i64) = (r.get(2).map_err(rekordbox_err)?, r.get(3).map_err(rekordbox_err)?);
        let (comment, color): (String, i64) = (r.get(4).map_err(rekordbox_err)?, r.get(5).map_err(rekordbox_err)?);
        let track: String = r.get(6).map_err(rekordbox_err)?;
        let position_ms = u32::try_from(start).unwrap_or(0);
        let slot = slot_of(kind);
        let end_ms = u32::try_from(end).ok().filter(|&end| end > position_ms);
        found.entry(track).or_default().push(Cue {
            id,
            slot,
            position_ms,
            end_ms,
            comment,
            color: slot.map(|slot| colors.color_of(color, slot, end_ms.is_some())),
        });
    }
    for cues in found.values_mut() {
        cues.sort_by_key(|cue| (cue.position_ms, cue.slot));
    }
    Ok(found)
}

impl Collection {
    /// A track's beat grid and cues, read from disk now. Blocking.
    pub fn preparation(&self, track_id: &str) -> AppResult<Preparation> {
        let loaded = self.loaded_or_read()?;
        let index = &loaded.index;
        let row = index
            .row_of(track_id)
            .ok_or_else(|| AppError::Rekordbox("That track is no longer in the collection".into()))?
            as usize;
        let db = rbl_db::Library::open(loaded.location.clone(), OpenMode::ReadOnly).map_err(rekordbox_err)?;
        let colors = HotCueColors::installed();

        let relative = index.analysis_path.get(row);
        let beats = if relative.is_empty() {
            Vec::new()
        } else {
            let [dat, ..] = analysis_files(&loaded.location.share_root, relative);
            rbl_anlz::Anlz::read(&dat).ok().and_then(|anlz| anlz.beat_grid()).unwrap_or_default()
        };
        Ok(Preparation {
            track_id: track_id.to_owned(),
            duration_ms: index.length_sec[row].saturating_mul(1000),
            bpm: beats.first().map(|beat| f64::from(beat.tempo_x100) / 100.0),
            beats: beats
                .iter()
                .map(|beat| (beat.time_ms, beat.beat_number.clamp(1, 4) as u8, beat.tempo_x100))
                .collect(),
            cues: cues_of(&db, track_id)?,
            locked: is_locked(&db, track_id)?,
            pad_colors: (0..8).map(|slot| colors.color_of(0, slot, false)).collect(),
            loop_color: colors.color_of(0, 0, true),
        })
    }

    /// A track's detailed waveforms, for drawing: rekordbox's own columns,
    /// 150 a second, in each form its analysis files hold. Each is a byte
    /// for the kind, four for the length (little-endian), then the columns:
    /// kind 7 is the three-band waveform (`PWV7`, a byte each for low, mid
    /// and high), 5 the colour one (`PWV5`, two bytes a column) and 3 the
    /// blue one (`PWV3`, one byte). Empty before analysis. Blocking.
    pub fn waveform(&self, track_id: &str) -> AppResult<Vec<u8>> {
        let loaded = self.loaded_or_read()?;
        let Some(row) = loaded.index.row_of(track_id) else { return Ok(Vec::new()) };
        let relative = loaded.index.analysis_path.get(row as usize);
        if relative.is_empty() {
            return Ok(Vec::new());
        }
        let [_, ext, two_ex] = analysis_files(&loaded.location.share_root, relative);
        let mut bytes = Vec::new();
        for (file, kind, tag) in [(&two_ex, 7u8, b"PWV7"), (&ext, 5, b"PWV5"), (&ext, 3, b"PWV3")] {
            let Ok(anlz) = rbl_anlz::Anlz::read(file) else { continue };
            if let Some((_, columns)) = anlz.waveform(tag).filter(|(_, columns)| !columns.is_empty()) {
                bytes.push(kind);
                bytes.extend_from_slice(&(columns.len() as u32).to_le_bytes());
                bytes.extend_from_slice(columns);
            }
        }
        Ok(bytes)
    }

    /// What the track list shows of each of these tracks: a small waveform
    /// in the form `prefer` (7, 5 or 3) when the track has it, or else the
    /// first it has of those; and its cues. Tracks no longer in the
    /// collection are left out. Blocking.
    pub fn previews(&self, track_ids: &[String], prefer: u8) -> AppResult<Vec<Preview>> {
        const FORMS: [(u8, &[u8; 4]); 3] = [(7, b"PWV7"), (5, b"PWV5"), (3, b"PWV3")];
        let loaded = self.loaded_or_read()?;
        let index = &loaded.index;
        let db = rbl_db::Library::open(loaded.location.clone(), OpenMode::ReadOnly).map_err(rekordbox_err)?;
        let ids: Vec<&str> = track_ids.iter().map(String::as_str).collect();
        let mut cues = cues_of_each(&db, &ids)?;
        drop(db);

        let mut previews = Vec::with_capacity(track_ids.len());
        for track_id in track_ids {
            let Some(row) = index.row_of(track_id) else { continue };
            let row = row as usize;
            let mut preview = Preview {
                track_id: track_id.clone(),
                kind: 0,
                columns: Vec::new(),
                duration_ms: index.length_sec[row].saturating_mul(1000),
                cues: cues.remove(track_id.as_str()).unwrap_or_default(),
            };
            let relative = index.analysis_path.get(row);
            if !relative.is_empty() {
                let [_, ext, two_ex] = analysis_files(&loaded.location.share_root, relative);
                // Shrinking a waveform means reading the whole of its file: done
                // once for each state of the files, not each time a row is shown.
                let stamp = [&ext, &two_ex].map(|file| std::fs::metadata(file).and_then(|m| m.modified()).ok());
                let key = (relative.to_owned(), prefer);
                let held = self.wave_previews.lock().expect("previews lock poisoned").get(&key).filter(|held| held.stamp == stamp).cloned();
                let wave = held.unwrap_or_else(|| {
                    let mut wave = WavePreview { stamp, kind: 0, columns: std::sync::Arc::default(), duration_ms: None };
                    let preferred = FORMS.iter().filter(|(kind, _)| *kind == prefer);
                    let mut read: [Option<Option<rbl_anlz::Anlz>>; 2] = [None, None];
                    for &(kind, tag) in preferred.chain(FORMS.iter().filter(|(kind, _)| *kind != prefer)) {
                        // Each file is read once, whichever of its forms are looked for.
                        let (at, file) = if kind == 7 { (1, &two_ex) } else { (0, &ext) };
                        let Some(anlz) = read[at].get_or_insert_with(|| rbl_anlz::Anlz::read(file).ok()) else { continue };
                        let Some((_, data)) = anlz.waveform(tag).filter(|(_, data)| !data.is_empty()) else { continue };
                        wave.kind = kind;
                        wave.columns = std::sync::Arc::new(shrink(kind, data, PREVIEW_COLUMNS));
                        // 150 columns a second: the waveform's own length, to the millisecond.
                        let ms = (data.len() / stride_of(kind)) as u64 * 1000 / 150;
                        wave.duration_ms = Some(u32::try_from(ms).unwrap_or(u32::MAX));
                        break;
                    }
                    let mut previews = self.wave_previews.lock().expect("previews lock poisoned");
                    if previews.len() >= MAX_HELD_PREVIEWS {
                        previews.clear();
                    }
                    previews.insert(key, wave.clone());
                    wave
                });
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

    /// Decodes a track's audio for the player, handing it to `sink` as it
    /// goes so playing can start on the first of it: once what it is, then
    /// the samples in pieces, a short one first. `sink` returns false to
    /// stop. The audio is on the timeline rekordbox keeps for the file: the
    /// one its cues and beat grid are measured on, which for some MP3s is
    /// not where other players start. Blocking, for about a second.
    pub fn decode_audio(&self, track_id: &str, mut sink: impl FnMut(Audio) -> bool) -> AppResult<()> {
        let loaded = self.loaded_or_read()?;
        let row = loaded
            .index
            .row_of(track_id)
            .ok_or_else(|| AppError::Rekordbox("That track is no longer in the collection".into()))?
            as usize;
        let path = PathBuf::from(super::local_path(loaded.index.folder_path.get(row)));
        let length_secs = u64::from(loaded.index.length_sec[row]);
        drop(loaded);
        if !path.is_file() {
            return Err(AppError::Rekordbox(format!("Its file is missing: {}", path.display())));
        }
        let mut stream = rbl_audio::MonoStream::open(&path)
            .map_err(|e| AppError::Rekordbox(format!("The file couldn't be decoded: {e}")))?;

        let mut pcm: Vec<u8> = Vec::new();
        let mut started = false;
        // Bytes to gather before handing them on: a fifth of a second to
        // start with, then eight seconds at a time.
        let mut piece = 0;
        while let Some((_, frames)) = stream.next_frames() {
            pcm.reserve(frames.len() * 4);
            for [left, right] in frames {
                pcm.extend_from_slice(&left.to_le_bytes());
                pcm.extend_from_slice(&right.to_le_bytes());
            }
            if !started {
                // The rate is sure once a packet has been decoded.
                started = true;
                let rate = stream.sample_rate();
                let frames = stream.expected_frames().unwrap_or((length_secs + 1) * u64::from(rate));
                if !sink(Audio::Start { rate, frames: u32::try_from(frames).unwrap_or(u32::MAX) }) {
                    return Ok(());
                }
                piece = rate as usize * 4 / 5;
            }
            if pcm.len() >= piece {
                piece = stream.sample_rate() as usize * 4 * 8;
                if !sink(Audio::Pcm(std::mem::take(&mut pcm))) {
                    return Ok(());
                }
            }
        }
        if !started {
            return Err(AppError::Rekordbox("The file has no audio that could be decoded".into()));
        }
        if !pcm.is_empty() {
            sink(Audio::Pcm(pcm));
        }
        Ok(())
    }
}

/// A track's audio as [`Collection::decode_audio`] hands it out.
pub enum Audio {
    /// First: the sample rate, and about how many frames are coming.
    Start { rate: u32, frames: u32 },
    /// 16-bit stereo samples, little-endian, left then right.
    Pcm(Vec<u8>),
}

/// Sets a cue or, with `end_ms`, a loop: on a hot cue pad (0 for A), taking
/// the place of what was on it, or as a memory cue.
pub(super) fn add_cue(
    writer: &mut Writer,
    track_id: &str,
    slot: Option<u8>,
    position_ms: u32,
    end_ms: Option<u32>,
) -> AppResult<String> {
    let kind = match slot {
        Some(slot) => *HOT_KINDS
            .get(usize::from(slot))
            .ok_or_else(|| AppError::Validation("rekordbox has hot cues A to P".into()))?,
        None => MEMORY,
    };
    let existing = cues_of(writer.library(), track_id)?;
    if let Some(slot) = slot {
        for cue in existing.iter().filter(|cue| cue.slot == Some(slot)) {
            writer.delete_cue(&cue.id).map_err(rekordbox_err)?;
        }
    } else if existing.iter().any(|cue| cue.slot.is_none() && cue.position_ms == position_ms && cue.end_ms == end_ms) {
        return Err(AppError::Validation("There's a memory cue there already".into()));
    }
    match end_ms {
        Some(end_ms) => writer.add_loop(track_id, kind, position_ms, end_ms, 0),
        None => writer.add_cue(track_id, kind, position_ms),
    }
    .map_err(rekordbox_err)
}

/// Changes a track's beat grid in its analysis files, and its tempo on its
/// row. The extended grid some `.EXT` files carry can't be re-derived, so
/// it's emptied when it no longer describes the beats, as rbxport does.
pub(super) fn change_grid(
    writer: &mut Writer,
    index: &rbl_index::Library,
    track_id: &str,
    change: GridChange,
    from_ms: Option<u32>,
) -> AppResult<()> {
    let row = index
        .row_of(track_id)
        .ok_or_else(|| AppError::Rekordbox("That track is no longer in the collection".into()))?
        as usize;
    if is_locked(writer.library(), track_id)? {
        return Err(AppError::Rekordbox("Its analysis is locked in rekordbox".into()));
    }
    let relative = index.analysis_path.get(row);
    if relative.is_empty() {
        return Err(AppError::Validation("Analyse the track first: it has no beat grid".into()));
    }
    let [dat, ext, _] = analysis_files(&writer.library().location().share_root, relative);
    let parsed = rbl_anlz::Anlz::read(&dat).map_err(rekordbox_err)?;
    let beats = parsed.beat_grid().filter(|beats| !beats.is_empty()).ok_or_else(|| {
        AppError::Validation("Analyse the track first: it has no beat grid".into())
    })?;

    let next = match change {
        GridChange::Restore { beats: restored } => {
            let ordered = restored.windows(2).all(|pair| pair[1].0 > pair[0].0);
            if restored.is_empty() || !ordered || restored.iter().any(|&(_, bar, _)| !(1..=4).contains(&bar)) {
                return Err(AppError::Validation("That isn't a beat grid".into()));
            }
            restored
                .into_iter()
                .map(|(time_ms, bar, tempo_x100)| rbl_anlz::Beat { beat_number: u16::from(bar), tempo_x100, time_ms })
                .collect()
        }
        change => {
            let tempo = |bpm: f64| {
                if (40.0..=499.0).contains(&bpm) {
                    Ok(bpm)
                } else {
                    Err(AppError::Validation("A tempo is between 40 and 499 BPM".into()))
                }
            };
            // Where the change starts: the whole grid, or the beat nearest `from_ms`.
            let first = from_ms
                .and_then(|ms| rbl_anlz::grid::nearest_index(&beats, ms))
                .map_or(beats[0].time_ms, |at| beats[at].time_ms);
            let edit = match change {
                GridChange::Nudge { ms } => AnlzEdit::Nudge(ms),
                GridChange::Align { time_ms } => AnlzEdit::Align { time_ms },
                GridChange::Downbeat { time_ms } => AnlzEdit::Downbeat { time_ms },
                GridChange::Double => AnlzEdit::Double,
                GridChange::Halve => AnlzEdit::Halve,
                GridChange::Tempo { bpm } => {
                    AnlzEdit::Tempo { bpm_x100: (tempo(bpm)? * 100.0).round() as u16, anchor_ms: first }
                }
                GridChange::Stretch { by_ms, time_ms } => AnlzEdit::Stretch { by_ms, time_ms },
                GridChange::Tap { bpm, anchor_ms } => AnlzEdit::Tap { bpm: tempo(bpm)?, anchor_ms },
                GridChange::Restore { .. } => unreachable!("handled above"),
            };
            validate(&beats, from_ms, edit).map_err(|reason| AppError::Validation(reason.into()))?;
            let duration_ms = match index.length_sec[row].saturating_mul(1000) {
                0 => beats.last().map_or(0, |beat| beat.time_ms),
                ms => ms,
            };
            apply_with_duration(&beats, from_ms, edit, duration_ms)
        }
    };
    if next.is_empty() {
        return Err(AppError::Validation("That would leave the track without a beat".into()));
    }
    if next == beats {
        return Ok(());
    }

    let mut changed = vec![(dat, parsed.with_beat_grid(&next))];
    if let Ok(extended) = rbl_anlz::Anlz::read(&ext) {
        if let Some(bytes) = extended.with_extended_grid_edit(&beats, &next, parsed.grid_offset().unwrap_or(0)) {
            changed.push((ext, bytes));
        }
    }
    let previous: Vec<_> = changed.iter().map(|(path, _)| std::fs::read(path).ok()).collect();
    let put_back = || {
        for ((path, _), bytes) in changed.iter().zip(&previous) {
            if let Some(bytes) = bytes {
                let _ = replace_file(path, bytes);
            }
        }
    };
    for (path, bytes) in &changed {
        if let Err(e) = replace_file(path, bytes) {
            put_back();
            return Err(e.into());
        }
    }
    if let Err(e) = writer.save_grid_revision(track_id, u32::from(tempo_x100(&next))) {
        put_back();
        return Err(rekordbox_err(e));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::analyse::tests::click_track;
    use super::super::tests::fixture;
    use super::super::Edit;
    use super::*;

    /// A fixture with one analysed click track at 120 BPM.
    fn prepared() -> (tempfile::TempDir, Collection, String) {
        let (dir, collection) = fixture();
        let wav = dir.path().join("Click.wav");
        click_track(&wav, 120.0, 20);
        let id = collection.import(&[wav], None).unwrap().track_ids.remove(0);
        collection.analyse(&id, false).unwrap();
        (dir, collection, id)
    }

    fn add(collection: &Collection, id: &str, slot: Option<u8>, position_ms: u32, end_ms: Option<u32>) {
        collection
            .edit(Edit::AddCue { track_id: id.to_owned(), slot, position_ms, end_ms })
            .unwrap();
    }

    #[test]
    fn hot_cues_take_their_own_colour_or_their_pads() {
        use super::HotCueColors::{Cdj, Cold1, Colorful};
        // A cue given a colour keeps it in every mode.
        assert_eq!(Colorful.color_of(21, 0, false), "#3ceb50");
        assert_eq!(Cdj.color_of(42, 3, true), "#e62828");
        // One without takes its pad's: A to H, then the same again.
        assert_eq!(Colorful.color_of(0, 0, false), "#ff376f");
        assert_eq!(Colorful.color_of(0, 9, false), "#45acdb");
        assert_eq!(Colorful.color_of(-1, 2, false), "#7dc13d");
        assert_eq!(Cold1.color_of(0, 0, false), "#50a0ff");
        assert_eq!(Cdj.color_of(0, 0, false), "#28e214");
        assert_eq!(Cdj.color_of(0, 8, false), "#a5e116");
        // A loop without one is orange whatever the mode.
        assert_eq!(Cold1.color_of(0, 5, true), "#ff8c00");

        let settings = r#"<VALUE name="HotCueQuantizeBeat" val="4"/><VALUE name="HotCueColorType" val="2"/>"#;
        assert_eq!(super::HotCueColors::from_settings(settings), Cold1);
        assert_eq!(super::HotCueColors::from_settings("<PROPERTIES/>"), Colorful);
    }

    #[test]
    fn cues_and_loops_are_set_named_and_deleted() {
        let (_dir, collection, id) = prepared();
        assert!(collection.preparation(&id).unwrap().cues.is_empty());

        add(&collection, &id, Some(0), 2_000, None);
        add(&collection, &id, Some(3), 8_000, Some(10_000));
        add(&collection, &id, None, 500, None);
        let cues = collection.preparation(&id).unwrap().cues;
        let seen: Vec<_> = cues.iter().map(|c| (c.slot, c.position_ms, c.end_ms)).collect();
        assert_eq!(seen, [(None, 500, None), (Some(0), 2_000, None), (Some(3), 8_000, Some(10_000))]);

        // A pad holds one cue: setting it again moves it.
        add(&collection, &id, Some(0), 4_000, None);
        let cues = collection.preparation(&id).unwrap().cues;
        let on_a: Vec<_> = cues.iter().filter(|c| c.slot == Some(0)).collect();
        assert_eq!(on_a.len(), 1);
        assert_eq!(on_a[0].position_ms, 4_000);

        collection.edit(Edit::NameCue { cue_id: on_a[0].id.clone(), comment: "Drop".into() }).unwrap();
        let memory = cues.iter().find(|c| c.slot.is_none()).unwrap();
        collection.edit(Edit::DeleteCue { cue_id: memory.id.clone() }).unwrap();
        let cues = collection.preparation(&id).unwrap().cues;
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].comment, "Drop");

        // The same memory cue twice is refused.
        add(&collection, &id, None, 500, None);
        let again = collection.edit(Edit::AddCue { track_id: id.clone(), slot: None, position_ms: 500, end_ms: None });
        assert!(again.is_err());
    }

    #[test]
    fn the_grid_is_nudged_respaced_and_given_a_downbeat() {
        let (_dir, collection, id) = prepared();
        let grid = |collection: &Collection| collection.preparation(&id).unwrap();
        let change =
            |change| collection.edit(Edit::Grid { track_id: id.clone(), change, from_ms: None }).unwrap();
        let before = grid(&collection);
        assert!((before.bpm.unwrap() - 120.0).abs() < 0.5);
        assert!(!before.locked);

        change(GridChange::Nudge { ms: 7 });
        let nudged = grid(&collection);
        assert_eq!(nudged.beats[4].0, before.beats[4].0 + 7);

        // The beat nearest 3.1 s lands on it, and starts a bar.
        change(GridChange::Downbeat { time_ms: 3_100 });
        let aligned = grid(&collection);
        let at = aligned.beats.iter().find(|beat| beat.0 == 3_100).expect("a beat at 3.1 s");
        assert_eq!(at.1, 1);

        change(GridChange::Double);
        let doubled = grid(&collection);
        assert!((doubled.bpm.unwrap() - before.bpm.unwrap() * 2.0).abs() < 0.5);
        assert!(doubled.beats.len() > aligned.beats.len() * 2 - 3);
        change(GridChange::Halve);

        change(GridChange::Tempo { bpm: 125.0 });
        let respaced = grid(&collection);
        assert_eq!(respaced.bpm, Some(125.0));
        assert_eq!(respaced.beats[1].0 - respaced.beats[0].0, 480);
        // The row's BPM follows the grid.
        let page = collection
            .tracks(&super::super::tests::query(super::super::Source::Collection, super::super::Sort::Title), 0, 1000)
            .unwrap();
        assert_eq!(page.rows.iter().find(|r| r.id == id).unwrap().bpm, Some(125.0));

        assert!(collection.edit(Edit::Grid { track_id: id.clone(), change: GridChange::Tempo { bpm: 5.0 }, from_ms: None }).is_err());

        // Undo: a grid goes back exactly as it was read.
        change(GridChange::Restore { beats: before.beats.clone() });
        assert_eq!(grid(&collection).beats, before.beats);

        // A tapped tempo starts a bar where the first tap was.
        change(GridChange::Tap { bpm: 100.0, anchor_ms: 5_000 });
        let tapped = grid(&collection);
        assert_eq!(tapped.bpm, Some(100.0));
        assert_eq!(tapped.beats.iter().find(|beat| beat.0 == 5_000).map(|beat| beat.1), Some(1));

        // From a point on, the beats before it stay where they were.
        change(GridChange::Restore { beats: before.beats.clone() });
        let from = before.beats[16].0;
        collection
            .edit(Edit::Grid { track_id: id.clone(), change: GridChange::Tempo { bpm: 60.0 }, from_ms: Some(from) })
            .unwrap();
        let split = grid(&collection);
        assert_eq!(split.beats[..16], before.beats[..16]);
        assert_eq!(split.beats[18].0 - split.beats[17].0, 1_000);

        // A locked grid is left alone until it's unlocked.
        collection.edit(Edit::LockAnalysis { track_id: id.clone(), locked: true }).unwrap();
        assert!(grid(&collection).locked);
        assert!(collection
            .edit(Edit::Grid { track_id: id.clone(), change: GridChange::Double, from_ms: None })
            .is_err());
        collection.edit(Edit::LockAnalysis { track_id: id.clone(), locked: false }).unwrap();
        assert!(!grid(&collection).locked);
    }

    #[test]
    fn the_waveform_and_preview_audio_cover_the_whole_track() {
        let (dir, collection, id) = prepared();
        // Three forms, each 150 columns a second for 20 seconds.
        let waveform = collection.waveform(&id).unwrap();
        let mut at = 0;
        let mut kinds = Vec::new();
        while at < waveform.len() {
            let (kind, length) = (waveform[at], u32::from_le_bytes(waveform[at + 1..at + 5].try_into().unwrap()));
            let bytes_per_column = match kind {
                7 => 3,
                5 => 2,
                _ => 1,
            };
            let columns = length / bytes_per_column;
            assert!((2_990..=3_010).contains(&columns), "{columns} columns of kind {kind}");
            kinds.push(kind);
            at += 5 + length as usize;
        }
        assert_eq!(kinds, [7, 5, 3]);

        let _ = dir;
        let (mut rate, mut bytes, mut pieces) = (0, 0, 0);
        collection
            .decode_audio(&id, |audio| {
                match audio {
                    Audio::Start { rate: of, frames } => {
                        assert_eq!((bytes, frames), (0, 20 * 44_100), "what it is comes first");
                        rate = of;
                    }
                    Audio::Pcm(pcm) => {
                        bytes += pcm.len();
                        pieces += 1;
                    }
                }
                true
            })
            .unwrap();
        assert_eq!(rate, 44_100);
        assert_eq!(bytes, 20 * 44_100 * 4);
        assert!(pieces > 1, "a short first piece, so playing starts at once");

        // Stopping after the first piece ends the decode.
        let mut pieces = 0;
        collection
            .decode_audio(&id, |audio| {
                pieces += usize::from(matches!(audio, Audio::Pcm(_)));
                pieces == 0
            })
            .unwrap();
        assert_eq!(pieces, 1);
    }

    #[test]
    fn a_waveform_shrinks_to_the_loudest_of_what_each_column_covers() {
        // Three bands: each its own loudest.
        assert_eq!(shrink(7, &[1, 9, 2, 5, 3, 8, 7, 7, 7, 0, 0, 0], 2), [5, 9, 8, 7, 7, 7]);
        // Colour: the tallest column whole (its height is bits 2 to 6 of the second byte).
        assert_eq!(shrink(5, &[0xaa, 4 << 2, 0xbb, 9 << 2, 0xcc, 1 << 2, 0xdd, 0], 2), [0xbb, 9 << 2, 0xcc, 1 << 2]);
        // Blue: the tallest byte, by its low five bits.
        assert_eq!(shrink(3, &[0xe1, 0x05, 0x02, 0x03], 2), [0x05, 0x03]);
        // Nothing is made up for a waveform shorter than the preview.
        assert_eq!(shrink(3, &[1, 2, 3], 400), [1, 2, 3]);
        assert!(shrink(7, &[], 400).is_empty());
    }

    #[test]
    fn previews_carry_a_small_waveform_and_each_tracks_cues() {
        let (dir, collection, id) = prepared();
        let raw = dir.path().join("Raw.wav");
        click_track(&raw, 120.0, 5);
        let unanalysed = collection.import(&[raw], None).unwrap().track_ids.remove(0);
        add(&collection, &id, Some(1), 3_000, None);
        add(&collection, &id, None, 1_000, Some(2_000));

        let asked = [unanalysed.clone(), "gone".to_owned(), id.clone()];
        let previews = collection.previews(&asked, 5).unwrap();
        // In the order asked for, without the track that isn't there.
        assert_eq!(previews.iter().map(|p| p.track_id.as_str()).collect::<Vec<_>>(), [&unanalysed, &id]);
        let [bare, full] = [&previews[0], &previews[1]];
        assert_eq!((bare.kind, bare.columns.len(), bare.cues.len()), (0, 0, 0));
        assert_eq!((full.kind, full.columns.len()), (5, PREVIEW_COLUMNS * 2));
        assert!((19_900..=20_100).contains(&full.duration_ms), "{} ms", full.duration_ms);
        let cues: Vec<_> = full.cues.iter().map(|c| (c.slot, c.position_ms, c.end_ms)).collect();
        assert_eq!(cues, [(None, 1_000, Some(2_000)), (Some(1), 3_000, None)]);

        // The three-band form by default, and when what's asked for isn't one.
        let three_band = &collection.previews(&[id.clone()], 0).unwrap()[0];
        assert_eq!((three_band.kind, three_band.columns.len()), (7, PREVIEW_COLUMNS * 3));
    }

    #[test]
    fn an_unanalysed_track_has_no_grid_to_change() {
        let (dir, collection) = fixture();
        let wav = dir.path().join("Raw.wav");
        click_track(&wav, 120.0, 5);
        let id = collection.import(&[wav], None).unwrap().track_ids.remove(0);
        let prepared = collection.preparation(&id).unwrap();
        assert!(prepared.beats.is_empty() && prepared.bpm.is_none());
        assert!(collection.waveform(&id).unwrap().is_empty());
        assert!(collection.edit(Edit::Grid { track_id: id, change: GridChange::Double, from_ms: None }).is_err());
    }

    /// Reads cues, grids and waveforms rekordbox wrote, and edits a grid and
    /// sets a cue, on a copy of the installed library with copies of its
    /// analysis files; the original is only read.
    /// Run with `cargo test live_prepare -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_prepare() {
        use super::super::tests::query;
        use super::super::{Sort, Source};
        let installed = rbl_db::detect().expect("rekordbox library");
        let real_share = installed.share_root.clone();
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("master.db");
        std::fs::copy(&installed.master_db, &copy).unwrap();
        let share = dir.path().join("share");
        let collection = Collection::new(dir.path().join("backups"));
        collection
            .load_from(rbl_db::LibraryLocation {
                master_db: copy,
                share_root: share.clone(),
                is_real_install: false,
                ..installed
            })
            .unwrap();
        let loaded = collection.loaded().unwrap();
        let page = collection.tracks(&query(Source::Collection, Sort::DateAdded), 0, 10_000).unwrap();
        let (mut with_cues, mut with_waves, mut colour) = (0, 0, 0);
        for track in &page.rows {
            let at = loaded.index.row_of(&track.id).unwrap() as usize;
            let relative = loaded.index.analysis_path.get(at);
            if relative.is_empty() {
                continue;
            }
            for (from, to) in analysis_files(&real_share, relative).iter().zip(analysis_files(&share, relative)) {
                std::fs::create_dir_all(to.parent().unwrap()).unwrap();
                let _ = std::fs::copy(from, to);
            }
            let prepared = collection.preparation(&track.id).unwrap();
            let waveform = collection.waveform(&track.id).unwrap();
            with_cues += usize::from(!prepared.cues.is_empty());
            with_waves += usize::from(!waveform.is_empty());
            colour += usize::from(waveform.first() == Some(&7));
            if !prepared.beats.is_empty() {
                // The grid reaches about as far as the track does.
                let last = prepared.beats.last().unwrap().0;
                assert!(last <= prepared.duration_ms + 2_000, "{}: grid past the end", track.title);
            }
        }
        println!("{} tracks: {with_cues} with cues, {with_waves} with a waveform ({colour} with three bands)", page.rows.len());

        // A screenful of the track list's previews, as scrolling asks for them.
        let screenful: Vec<String> = page.rows.iter().take(60).map(|track| track.id.clone()).collect();
        let started = std::time::Instant::now();
        let previews = collection.previews(&screenful, 7).unwrap();
        println!(
            "{} previews in {:.0} ms: {} with a waveform, {} with cues",
            previews.len(),
            started.elapsed().as_secs_f64() * 1e3,
            previews.iter().filter(|p| p.kind != 0).count(),
            previews.iter().filter(|p| !p.cues.is_empty()).count()
        );
        for preview in &previews {
            assert_eq!(preview.cues, collection.preparation(&preview.track_id).unwrap().cues);
            assert!(preview.cues.iter().all(|cue| cue.position_ms <= preview.duration_ms + 2_000));
        }

        // One of rekordbox's own grids, edited; and a cue set beside its own.
        let track = page
            .rows
            .iter()
            .find(|t| collection.preparation(&t.id).is_ok_and(|p| p.beats.len() > 64 && !p.locked && !p.cues.is_empty()))
            .expect("an analysed track with cues");
        let before = collection.preparation(&track.id).unwrap();
        let started = std::time::Instant::now();
        collection.edit(Edit::Grid { track_id: track.id.clone(), change: GridChange::Nudge { ms: 5 }, from_ms: None }).unwrap();
        println!("a grid edit took {:.2}s", started.elapsed().as_secs_f64());
        collection
            .edit(Edit::AddCue { track_id: track.id.clone(), slot: None, position_ms: before.beats[32].0 + 5, end_ms: None })
            .unwrap();
        let after = collection.preparation(&track.id).unwrap();
        assert_eq!(after.beats.len(), before.beats.len());
        assert_eq!(after.beats[10].0, before.beats[10].0 + 5);
        assert_eq!(after.cues.len(), before.cues.len() + 1);
        println!("{}: {} beats at {:?} BPM, {} cues -> {}", track.title, after.beats.len(), after.bpm, before.cues.len(), after.cues.len());

        let started = std::time::Instant::now();
        let (mut bytes, mut first) = (0, None);
        collection
            .decode_audio(&track.id, |audio| {
                if let Audio::Pcm(pcm) = audio {
                    first.get_or_insert_with(|| started.elapsed());
                    bytes += pcm.len();
                }
                true
            })
            .unwrap();
        println!(
            "player audio: first piece after {:.0} ms, all {:.1} MB in {:.2}s ({})",
            first.unwrap_or_default().as_secs_f64() * 1e3,
            bytes as f64 / 1e6,
            started.elapsed().as_secs_f64(),
            track.format
        );
    }
}
