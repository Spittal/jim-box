//! Analysing a track the way rekordbox does: tempo, beat grid, key and
//! waveforms worked out from the audio (rbxport's `rbl-analysis`), written
//! as the three analysis files rekordbox and CDJs read (`rbl-anlz`), and
//! registered on the track's row.
//!
//! The audio is decoded and analysed before the library is opened, so the
//! database is held only for the moment the result is saved.

use std::path::{Path, PathBuf};

use rbl_db::write::{AnalysisWrite, Writer};
use serde::Serialize;

use super::{rekordbox_err, Collection};
use crate::error::{AppError, AppResult};

/// How much of a file is kept in memory for the tempo and key, in seconds.
/// The rest of a longer file is still decoded for its waveforms, and the
/// grid carried on to the end at the last tempo.
const HEAD_SECS: usize = 1800;

/// Whether the index's `analysed` column says the track has been analysed.
/// The index keeps only whether `djmdContent.Analysed` is non-zero.
pub(super) fn is_analysed(analysed: u8) -> bool {
    analysed != 0
}

/// What analysing a track found.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Analysed {
    pub track_id: String,
    pub bpm: f64,
    /// Empty when no key was found.
    pub key: String,
    pub beats: usize,
}

/// A file's analysis, before it's saved anywhere.
struct Computed {
    beats: Vec<rbl_anlz::Beat>,
    waves: rbl_anlz::RekordboxWaves,
    bpm_x100: u32,
    key: Option<String>,
    length_secs: u32,
}

fn compute(path: &Path, follow_tempo_changes: bool) -> AppResult<Computed> {
    let undecodable = |e: rbl_audio::AudioError| AppError::Rekordbox(format!("The file couldn't be decoded: {e}"));
    let mut stream = rbl_audio::MonoStream::open(path).map_err(undecodable)?;
    let mut waves = rbl_analysis::wave_creator::WaveCreator::new(stream.sample_rate());
    if let Some(frames) = stream.expected_frames() {
        waves.reserve(frames);
    }
    let cap = HEAD_SECS * stream.sample_rate() as usize;
    let mut head: Vec<f32> = Vec::new();
    let mut total = 0usize;
    while let Some((mono, stereo)) = stream.next_frames() {
        waves.push(stereo);
        total += mono.len();
        let room = cap.saturating_sub(head.len());
        head.extend_from_slice(&mono[..mono.len().min(room)]);
    }
    if head.is_empty() {
        return Err(undecodable(rbl_audio::AudioError::Unsupported("decoded no samples".into())));
    }

    let rate = stream.sample_rate().max(1);
    let preset = if follow_tempo_changes {
        rbl_analysis::AnalysisPreset::Rbxport
    } else {
        rbl_analysis::AnalysisPreset::Rekordbox
    };
    let mut analysis = rbl_analysis::analyse_with(&head, rate, preset.options());
    let duration = total as f64 / f64::from(rate);
    if total > head.len() {
        analysis.tempo.extend_to(duration);
    }

    let w = waves.finish();
    Ok(Computed {
        beats: analysis
            .tempo
            .beats
            .iter()
            .map(|b| rbl_anlz::Beat { beat_number: b.beat_number, tempo_x100: b.tempo_x100, time_ms: b.time_ms })
            .collect(),
        waves: rbl_anlz::RekordboxWaves {
            pwav: w.pwav,
            pwv2: w.pwv2,
            pwvc: w.pwvc,
            pwv3: w.pwv3,
            pwv4: w.pwv4,
            pwv5: w.pwv5,
            pwv6: w.pwv6,
            pwv7: w.pwv7,
        },
        bpm_x100: (analysis.tempo.bpm * 100.0).round().clamp(0.0, 100_000.0) as u32,
        key: analysis.key.map(|k| k.name),
        length_secs: duration.round() as u32,
    })
}

impl Collection {
    /// Of `track_ids`, the ones with no analysis yet.
    pub fn unanalysed(&self, track_ids: &[String]) -> Vec<String> {
        let Ok(loaded) = self.loaded_or_read() else { return Vec::new() };
        track_ids
            .iter()
            .filter(|id| {
                loaded.index.row_of(id).is_some_and(|row| !is_analysed(loaded.index.analysed[row as usize]))
            })
            .cloned()
            .collect()
    }

    /// Every track with a file and no analysis yet.
    pub fn all_unanalysed(&self) -> Vec<String> {
        let Ok(loaded) = self.loaded_or_read() else { return Vec::new() };
        let index = &loaded.index;
        (0..index.len())
            .filter(|&row| !is_analysed(index.analysed[row]) && !index.folder_path.get(row).is_empty())
            .map(|row| index.ids[row].to_string())
            .collect()
    }

    /// A track's title, for telling the user about it.
    pub fn title_of(&self, track_id: &str) -> Option<String> {
        let loaded = self.loaded().ok()?;
        let row = loaded.index.row_of(track_id)?;
        Some(loaded.index.title.get(row as usize).to_owned())
    }

    /// Analyses a track and saves the result, replacing any analysis it had.
    /// Its cues are kept. Blocking, for seconds.
    pub fn analyse(&self, track_id: &str, follow_tempo_changes: bool) -> AppResult<Analysed> {
        let loaded = self.loaded_or_read()?;
        let row = loaded
            .index
            .row_of(track_id)
            .ok_or_else(|| AppError::Rekordbox("That track is no longer in the collection".into()))?
            as usize;
        // Before the decode, which is seconds of work for the same refusal.
        if loaded.location.is_real_install && rbl_db::is_rekordbox_running() {
            return Err(AppError::Rekordbox("rekordbox is running. Quit it before analysing.".into()));
        }
        let path = PathBuf::from(loaded.index.folder_path.get(row));
        if !path.is_file() {
            return Err(AppError::Rekordbox(format!("Its file is missing: {}", path.display())));
        }
        drop(loaded);

        let computed = compute(&path, follow_tempo_changes)?;
        self.write(|writer, index| save(writer, index, track_id, &path, &computed))?;
        Ok(Analysed {
            track_id: track_id.to_owned(),
            bpm: f64::from(computed.bpm_x100) / 100.0,
            key: computed.key.unwrap_or_default(),
            beats: computed.beats.len(),
        })
    }
}

/// Writes the analysis files into rekordbox's share folder, then registers
/// them on the track. The files go first: a row naming files that aren't
/// there shows as analysed with nothing to draw.
fn save(
    writer: &mut Writer,
    index: &rbl_index::Library,
    track_id: &str,
    audio: &Path,
    computed: &Computed,
) -> AppResult<()> {
    let row = index
        .row_of(track_id)
        .ok_or_else(|| AppError::Rekordbox("That track is no longer in the collection".into()))?
        as usize;
    let locked = super::prepare::is_locked(writer.library(), track_id)?;
    if locked {
        return Err(AppError::Rekordbox("Its analysis is locked in rekordbox".into()));
    }
    if let Err(e) = writer.import_artwork(track_id) {
        tracing::warn!("Couldn't take the cover from {}: {e}", audio.display());
    }

    // Where the row already points, or where rekordbox would put them.
    let current = index.analysis_path.get(row);
    let relative = if current.is_empty() {
        writer.analysis_data_path_for(track_id).map_err(rekordbox_err)?
    } else {
        current.to_owned()
    };
    let dat = rbl_anlz::resolve(&writer.library().location().share_root, &relative);
    let paths = [dat.clone(), rbl_anlz::sibling(&dat, "EXT"), rbl_anlz::sibling(&dat, "2EX")];
    let previous = paths.each_ref().map(|path| std::fs::read(path).ok());
    // What rekordbox put in the old files and this can't work out (cues,
    // phrases) is carried into the new ones.
    let existing = paths.each_ref().map(|path| rbl_anlz::Anlz::read(path).ok());
    let files = rbl_anlz::author_with_rekordbox_waves(
        &rbl_anlz::share_tree_ppth(&audio.to_string_lossy()),
        &computed.beats,
        &computed.waves,
        rbl_anlz::Existing { dat: existing[0].as_ref(), ext: existing[1].as_ref(), two_ex: existing[2].as_ref() },
    );

    let put_back = |paths: &[PathBuf; 3]| {
        for (path, bytes) in paths.iter().zip(&previous) {
            let _ = match bytes {
                Some(bytes) => replace_file(path, bytes),
                None => std::fs::remove_file(path),
            };
        }
    };
    for (path, bytes) in paths.iter().zip([&files.dat, &files.ext, &files.two_ex]) {
        if let Err(e) = replace_file(path, bytes) {
            put_back(&paths);
            return Err(e.into());
        }
    }
    let registered = register(writer, index, row, track_id, audio, computed, &relative);
    if registered.is_err() {
        put_back(&paths);
    }
    registered
}

fn register(
    writer: &mut Writer,
    index: &rbl_index::Library,
    row: usize,
    track_id: &str,
    audio: &Path,
    computed: &Computed,
    analysis_path: &str,
) -> AppResult<()> {
    // A key the track already has, from its tags or from rekordbox, is the
    // user's: many are in a notation of their choosing (12A, not Dbm).
    let key = computed.key.as_deref().filter(|_| index.key_name(row as u32).is_empty());
    if let Some(key) = key {
        writer.ensure_detected_key(key).map_err(rekordbox_err)?;
    }
    refresh_file_info(writer, index, row, track_id, audio)?;
    writer
        .set_analysis(
            track_id,
            &AnalysisWrite {
                bpm_x100: computed.bpm_x100,
                key,
                analysis_path,
                length_sec: Some(computed.length_secs),
            },
        )
        .map_err(rekordbox_err)?;
    Ok(())
}

/// Brings the row's format, bitrate and size up to date when the file isn't
/// the one they were read from: a lossless copy that took an MP3's place.
/// `set_analysis`, straight after, stamps the row as changed.
fn refresh_file_info(
    writer: &Writer,
    index: &rbl_index::Library,
    row: usize,
    track_id: &str,
    audio: &Path,
) -> AppResult<()> {
    let Some(file_type) = rbl_db::import::file_type(audio) else { return Ok(()) };
    let size = std::fs::metadata(audio)?.len();
    if file_type == i64::from(index.file_type[row]) && size == index.file_size[row] {
        return Ok(());
    }
    let tags = rbl_db::import::read_tags(audio).map_err(rekordbox_err)?;
    writer
        .library()
        .connection()
        .execute(
            "UPDATE djmdContent SET FileType = ?1, BitRate = ?2, SampleRate = ?3, FileSize = ?4, BitDepth = ?5
             WHERE ID = ?6 AND rb_local_deleted = 0",
            (
                file_type,
                i64::from(tags.bitrate),
                i64::from(tags.sample_rate),
                i64::try_from(tags.file_size).unwrap_or(i64::MAX),
                i64::from(tags.bit_depth),
                track_id,
            ),
        )
        .map_err(rekordbox_err)?;
    Ok(())
}

/// Writes a file beside its final name and renames it into place, so a
/// reader never sees half of one.
pub(super) fn replace_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
pub(super) mod tests {
    use super::super::tests::{fixture, query};
    use super::super::{Sort, Source};
    use super::*;

    /// A 16-bit stereo WAV with a click on every beat over a quiet tone.
    pub(in super::super) fn click_track(path: &Path, bpm: f64, secs: u32) {
        const RATE: u32 = 44_100;
        let frames = RATE * secs;
        let beat = f64::from(RATE) * 60.0 / bpm;
        let mut data = Vec::with_capacity(frames as usize * 4);
        for n in 0..frames {
            let since_beat = f64::from(n) % beat;
            let click = if since_beat < 400.0 {
                (1.0 - since_beat / 400.0) * (since_beat * 0.35).sin() * 0.9
            } else {
                0.0
            };
            let tone = (f64::from(n) * 220.0 * std::f64::consts::TAU / f64::from(RATE)).sin() * 0.05;
            let sample = ((click + tone).clamp(-1.0, 1.0) * f64::from(i16::MAX)) as i16;
            data.extend_from_slice(&sample.to_le_bytes());
            data.extend_from_slice(&sample.to_le_bytes());
        }
        let mut wav = Vec::with_capacity(data.len() + 44);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&RATE.to_le_bytes());
        wav.extend_from_slice(&(RATE * 4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
        wav.extend_from_slice(&data);
        std::fs::write(path, wav).unwrap();
    }

    fn row(collection: &Collection, id: &str) -> super::super::TrackRow {
        let page = collection.tracks(&query(Source::Collection, Sort::Title), 0, 10_000).unwrap();
        page.rows.into_iter().find(|r| r.id == id).expect("track listed")
    }

    #[test]
    fn analysis_is_saved_where_rekordbox_reads_it() {
        let (dir, collection) = fixture();
        let wav = dir.path().join("Click 128.wav");
        click_track(&wav, 128.0, 30);
        let id = collection.import(&[wav], None).unwrap().track_ids.remove(0);
        assert_eq!(collection.unanalysed(&[id.clone()]), [id.clone()]);

        let found = collection.analyse(&id, false).unwrap();
        assert!((found.bpm - 128.0).abs() < 0.5, "bpm {}", found.bpm);
        assert!(found.beats > 55, "{} beats", found.beats);

        let track = row(&collection, &id);
        assert!(track.analysed);
        assert!((track.bpm.unwrap() - 128.0).abs() < 0.5);
        assert_eq!(track.duration_secs, 30);
        assert!(collection.unanalysed(&[id.clone()]).is_empty());

        // The three files are in the share folder, and the grid reads back.
        let loaded = collection.loaded().unwrap();
        let at = loaded.index.row_of(&id).unwrap() as usize;
        let dat = rbl_anlz::resolve(&loaded.location.share_root, loaded.index.analysis_path.get(at));
        for file in [dat.clone(), rbl_anlz::sibling(&dat, "EXT"), rbl_anlz::sibling(&dat, "2EX")] {
            assert!(file.is_file(), "{} is missing", file.display());
        }
        let grid = rbl_anlz::Anlz::read(&dat).unwrap().beat_grid().expect("a beat grid");
        assert_eq!(grid.len(), found.beats);
        let leftovers = std::fs::read_dir(dat.parent().unwrap()).unwrap().count();
        assert_eq!(leftovers, 3, "no temporary files left behind");
    }

    #[test]
    fn analysing_again_replaces_the_files_in_place() {
        let (dir, collection) = fixture();
        let wav = dir.path().join("Click.wav");
        click_track(&wav, 120.0, 20);
        let id = collection.import(&[wav.clone()], None).unwrap().track_ids.remove(0);
        collection.analyse(&id, false).unwrap();
        let path_of = |collection: &Collection| {
            let loaded = collection.loaded().unwrap();
            let at = loaded.index.row_of(&id).unwrap() as usize;
            loaded.index.analysis_path.get(at).to_owned()
        };
        let first = path_of(&collection);

        click_track(&wav, 140.0, 20);
        let again = collection.analyse(&id, true).unwrap();
        assert!((again.bpm - 140.0).abs() < 0.5, "bpm {}", again.bpm);
        assert_eq!(path_of(&collection), first);
    }

    #[test]
    fn a_file_that_is_not_audio_is_refused_and_nothing_changes() {
        let (dir, collection) = fixture();
        let wav = dir.path().join("Broken.wav");
        click_track(&wav, 120.0, 5);
        let id = collection.import(&[wav.clone()], None).unwrap().track_ids.remove(0);
        std::fs::write(&wav, b"not audio at all").unwrap();
        assert!(collection.analyse(&id, false).is_err());
        assert!(!row(&collection, &id).analysed);
    }

    /// Analyses a few tracks of a copy of the installed library, with the
    /// analysis files going to a temporary share folder; the original is only
    /// read. Run with `cargo test live_analysis -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_analysis() {
        let installed = rbl_db::detect().expect("rekordbox library");
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("master.db");
        std::fs::copy(&installed.master_db, &copy).unwrap();
        let collection = Collection::new(dir.path().join("backups"));
        collection
            .load_from(rbl_db::LibraryLocation {
                master_db: copy,
                share_root: dir.path().join("share"),
                is_real_install: false,
                ..installed
            })
            .unwrap();

        // rekordbox's own answer beside this one's, for tracks it analysed.
        let page = collection.tracks(&query(Source::Collection, Sort::DateAdded), 0, 10_000).unwrap();
        let sample: Vec<_> = page
            .rows
            .iter()
            .filter(|r| r.bpm.is_some() && Path::new(&r.path).is_file())
            .step_by(page.rows.len().max(8) / 8)
            .take(8)
            .collect();
        // rekordbox's own analysis files, copied to where the copy looks for
        // them, so the new ones are authored over real ones.
        let loaded = collection.loaded().unwrap();
        let files_of = |id: &str, share: &Path| {
            let at = loaded.index.row_of(id).unwrap() as usize;
            let dat = rbl_anlz::resolve(share, loaded.index.analysis_path.get(at));
            [dat.clone(), rbl_anlz::sibling(&dat, "EXT"), rbl_anlz::sibling(&dat, "2EX")]
        };
        let sections = |file: &Path| -> Vec<(String, Vec<u8>)> {
            rbl_anlz::Anlz::read(file)
                .map(|anlz| anlz.sections.iter().map(|s| (format!("{:?}", s.tag), s.payload.clone())).collect())
                .unwrap_or_default()
        };
        let real_share = rbl_db::detect().unwrap().share_root;
        for track in sample {
            let (real, ours) = (files_of(&track.id, &real_share), files_of(&track.id, &loaded.location.share_root));
            for (from, to) in real.iter().zip(&ours) {
                std::fs::create_dir_all(to.parent().unwrap()).unwrap();
                let _ = std::fs::copy(from, to);
            }
            let before: Vec<_> = ours.iter().map(|f| sections(f)).collect();
            let started = std::time::Instant::now();
            let result = collection.analyse(&track.id, false);
            // What rekordbox wrote and this can't: cue lists and phrases.
            let kept = ours.iter().zip(&before).all(|(file, before)| {
                let after = sections(file);
                before
                    .iter()
                    .filter(|(tag, _)| tag.contains("[80, 67, 79") || tag.contains("[80, 83, 83, 73]"))
                    .all(|section| after.contains(section))
            });
            assert!(kept, "{}: rekordbox's cue or phrase sections changed", track.title);
            match result {
                Ok(found) => println!(
                    "{:6.2} {:4} (rekordbox {:6.2} {:4})  {:.1}s  {} {}",
                    found.bpm,
                    found.key,
                    track.bpm.unwrap_or(0.0),
                    track.key,
                    started.elapsed().as_secs_f64(),
                    track.format,
                    track.title,
                ),
                Err(e) => println!("FAILED {}: {e}", track.title),
            }
        }
    }
}
