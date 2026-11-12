//! One beat grid from two detectors, and how far they agreed on it.
//!
//! The signal detector (`rbl-analysis`) fits one tempo to the kick attacks
//! of the whole track: right to the millisecond on a drum machine, and
//! confidently wrong on a drummer who drifts. The model (`beat-this`) hears
//! every beat wherever the tempo goes, but only to the nearest 20 ms frame,
//! and sometimes a few frames early.
//!
//! So the model says which pulse the music has and whether it holds still,
//! and the signal says exactly where each beat is:
//!
//! - Where the model's beats stay on the signal's grid, the grid is kept as
//!   it is. If they sit on its off-beats, or on every other beat, the grid
//!   is moved or re-spaced to the model's pulse and keeps its precision.
//! - Where they wander off it, the grid is built from the model's beats:
//!   each is moved onto the kick attack beside it, and a curve that bends
//!   only slowly is drawn through them, so the grid follows the drummer
//!   without jittering from beat to beat.
//!
//! The confidence is how far the two agreed, as a percentage: 100 is every
//! beat and the bar found alike by both.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use rbl_analysis::attack::AttackMap;
use rbl_analysis::tempo::TempoResult;
use serde::{Deserialize, Serialize};

/// Beats the model must hear before it has an opinion on a track.
const MIN_HEARD: usize = 16;
/// Two beats are the same beat within this, or within a fifth of a beat
/// when that is less: the window beat trackers are scored with.
const SAME_BEAT_SECS: f64 = 0.07;
/// The detectors are on the same pulse when their beats agree this far.
const SAME_PULSE: f64 = 0.85;
/// Another reading of the signal's grid (its off-beats, half or double its
/// tempo) replaces it only when it fits the model's beats this much better.
const CLEARLY_BETTER: f64 = 0.15;
/// Beats in a window when looking for drift, and how far its beats may sit
/// from the grid, by both the model and the attacks, before it has moved.
const DRIFT_WINDOW: usize = 16;
const DRIFT_SECS: f64 = 0.012;
/// Windows in a row that must have moved before the tempo is said to vary.
const DRIFT_RUN: usize = 2;
/// How far from one of the model's beats its kick attack is looked for.
const ATTACK_REACH_SECS: f64 = 0.03;
/// What a beat with no attack beside it weighs against one with.
const UNPLACED_WEIGHT: f64 = 0.2;
/// How stiff the curve through the beats is: the higher, the more beats a
/// change of pace must last to bend it. About half a dozen beats at this.
const STIFFNESS: f64 = 30.0;
/// A stretch whose curve stays this close to a straight line is at one tempo.
const STRAIGHT_SECS: f64 = 0.010;
/// The pace either side of a beat differs by this ratio at a cut from one
/// tempo to another, as a DJ edit makes.
const JUMP_RATIO: f64 = 1.06;
/// The most a stretch's slowest beat may be longer than its fastest.
const WIDEST_RANGE: f64 = 1.4;
/// A beat this share of a beat after the last splits it in two, and gaps
/// between these are two beats with the middle one unheard.
const SPLIT_BEAT: f64 = 0.62;
const TWO_BEATS: (f64, f64) = (1.6, 2.5);
/// Tempos a grid may have, in BPM, however the detectors count it.
const TEMPO_RANGE: std::ops::RangeInclusive<f64> = 40.0..=250.0;
/// How near a simple ratio two paces are to be one tempo counted two ways.
const RECOUNT: f64 = 1.04;
/// A grid sits on an attack that is within this of one of its beats.
const ON_ATTACK_SECS: f64 = 0.010;
/// A grid that follows the tempo replaces one that doesn't only when this
/// much more of it sits on attacks: a share of its beats more, and as many
/// times over.
const TIGHTER_BY: f64 = 0.05;
const TIGHTER_TIMES: f64 = 1.25;
/// The gaps either side of a cut are within this ratio of their side's pace.
const HELD_RATIO: f64 = 1.03;
/// Beats a stretch at its own tempo has to last.
const MIN_STRETCH: usize = 16;
/// Gaps either side of a beat that its local pace is taken from.
const PACE_GAPS: usize = 8;
/// A followed grid is given up on when more than this share of its gaps
/// differ from those around them by this ratio, or the model missed more
/// than this share of its beats.
const UNEVEN_GAP: f64 = 1.1;
const UNEVEN_SHARE: f64 = 0.01;
const MISSED_SHARE: f64 = 0.25;
/// The share of the model's downbeats that must fall on one beat of the bar
/// for it to say where the bar starts.
const BAR_DECIDED: f64 = 0.5;

/// What the detectors disagreed about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Issue {
    /// The model heard too few beats to check the grid against.
    Unheard,
    /// The tempo moves: one tempo doesn't fit the whole track.
    Varies,
    /// They found different tempos, and not a multiple of one another.
    Tempo,
    /// The model's beats were too uneven to make a grid of: no steady pulse.
    Uneven,
    /// One counted two beats for the other's one.
    HalfTime,
    /// They were half a beat apart.
    OffBeat,
    /// They put the start of the bar on different beats.
    Downbeat,
    /// The model itself wasn't sure where the bar starts.
    Bar,
}

/// How a track's grid was decided, for showing beside it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// How far the two detectors agreed on the grid, 0 to 100.
    pub confidence: u8,
    /// The grid changes tempo along the track.
    pub dynamic: bool,
    pub issues: Vec<Issue>,
    /// The tempo each detector found: the signal's one tempo, and the
    /// model's typical one.
    pub signal_bpm: f64,
    pub model_bpm: f64,
}

/// What the user knows of a track that the detectors may not: asked for
/// with an analysis of it.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hint {
    /// Roughly its tempo. Settles which of two counts the beat has (77 or
    /// 154), and keeps the signal detector near it.
    #[serde(default)]
    pub bpm: Option<f64>,
    /// Whether its tempo moves: true to have the grid follow it whatever
    /// the audio says, false to keep one tempo. None leaves it to the
    /// analysis and the setting.
    #[serde(default)]
    pub varies: Option<bool>,
}

impl Hint {
    /// The tempo, when it is one a grid can have.
    pub fn tempo(&self) -> Option<f64> {
        self.bpm.filter(|bpm| TEMPO_RANGE.contains(bpm))
    }
}

/// Whether a grid may change tempo along the track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Follow {
    /// Where both detectors find the tempo moving and the audio bears it out.
    Auto,
    /// Wherever the model's beats can be followed: the user says it moves.
    Always,
    Never,
}

impl Follow {
    /// From the setting, unless the hint says otherwise.
    pub fn from(setting: bool, hint: &Hint) -> Self {
        match hint.varies {
            Some(true) => Self::Always,
            Some(false) => Self::Never,
            None if setting => Self::Auto,
            None => Self::Never,
        }
    }
}

/// Whose grid a track has. The names shown for them are "Blend", "rbxport"
/// and "Beat This"; these are the ones saved, so they stay as they were.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// The one the analysis made of the two detectors.
    #[default]
    Chosen,
    /// The signal detector's (rbxport's) alone: one tempo.
    Signal,
    /// The model's (Beat This's) alone: its beats, followed. A newly
    /// analysed track has it where there is one: see [`Decided::given`].
    Model,
    /// One changed by hand since.
    Hand,
}

/// A beat of the grid, as the analysis files store one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Beat {
    /// 1 to 4; 1 starts a bar.
    pub number: u16,
    pub tempo_x100: u16,
    pub time_ms: u32,
}

/// A whole grid, and what a list says of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grid {
    /// The tempo a list shows: the one tempo, or the typical one.
    pub bpm: f64,
    /// It changes tempo along the track.
    pub dynamic: bool,
    /// (milliseconds, place in the bar, tempo in hundredths of a BPM), as
    /// [`super::GridChange::Restore`] takes them.
    pub beats: Vec<(u32, u8, u16)>,
}

impl Grid {
    fn new(beats: &[Beat], bpm: f64, dynamic: bool) -> Self {
        Self { bpm, dynamic, beats: beats.iter().map(|b| (b.time_ms, b.number.clamp(1, 4) as u8, b.tempo_x100)).collect() }
    }

    /// Carries the grid on at its last tempo to `end_secs`, for a file
    /// longer than the part that was analysed.
    fn extend_to(&mut self, end_secs: f64) {
        let [.., before, last] = self.beats[..] else { return };
        let period = f64::from(last.0 - before.0) / 1e3;
        if period <= 0.0 {
            return;
        }
        let (mut time, mut number) = (f64::from(last.0) / 1e3 + period, last.1);
        while time < end_secs && self.beats.len() < 1_000_000 {
            number = number % 4 + 1;
            self.beats.push(((time * 1e3).round() as u32, number, last.2));
            time += period;
        }
    }
}

/// A grid a track could have instead, in brief.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    pub source: Source,
    pub bpm: f64,
    pub dynamic: bool,
}

/// What is known of a track's grid: how the analysis decided it, whether
/// the user has looked it over, and which grids there are to try.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    /// None for a grid this app didn't make.
    #[serde(default)]
    pub report: Option<Report>,
    /// The user has said the grid is right, or made it so by hand.
    #[serde(default)]
    pub reviewed: bool,
    /// Whose grid the track has now.
    #[serde(default)]
    pub using: Source,
    /// The grids the analysis made, where they differ: the chosen one first.
    #[serde(default)]
    pub choices: Vec<Choice>,
    /// What the analysis was told of the track.
    #[serde(default)]
    pub hint: Hint,
}

/// A file from before there was more to keep than the report.
#[derive(Deserialize)]
#[serde(untagged)]
enum Saved {
    Report(Report),
    State(State),
}

/// What is known of a library's grids, by track id, kept in a file of the
/// app's own: rekordbox's library has nowhere for it. The grids a track
/// could have instead are beside it, a file a track, as they are big.
#[derive(Default)]
pub struct States {
    file: Option<PathBuf>,
    by_track: RwLock<HashMap<String, State>>,
}

impl States {
    /// What is saved in `file`; nothing when it isn't there or can't be read.
    pub fn open(file: PathBuf) -> Self {
        let saved: HashMap<String, Saved> = std::fs::read(&file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        let by_track = saved
            .into_iter()
            .map(|(id, saved)| match saved {
                Saved::State(state) => (id, state),
                Saved::Report(report) => (id, State { report: Some(report), ..State::default() }),
            })
            .collect();
        Self { file: Some(file), by_track: RwLock::new(by_track) }
    }

    pub fn get(&self, track_id: &str) -> Option<State> {
        self.by_track.read().expect("grids lock poisoned").get(track_id).cloned()
    }

    /// Where each track sorts by how sure its grid is: its confidence, and
    /// past every confidence once reviewed. Tracks with neither have none.
    pub fn ranks(&self) -> HashMap<String, u8> {
        self.by_track
            .read()
            .expect("grids lock poisoned")
            .iter()
            .filter_map(|(id, state)| {
                let rank = if state.reviewed { Some(u8::MAX) } else { state.report.as_ref().map(|r| r.confidence) };
                rank.map(|rank| (id.clone(), rank))
            })
            .collect()
    }

    fn change(&self, track_id: &str, change: impl FnOnce(&mut State)) {
        change(self.by_track.write().expect("grids lock poisoned").entry(track_id.to_owned()).or_default());
        self.save();
    }

    /// A track was analysed: its grid is `using`'s (see [`Decided::given`]),
    /// not yet looked at.
    pub fn analysed(&self, track_id: &str, decided: &Decided, using: Source, hint: Hint) {
        let mut grids = vec![(Source::Chosen, Grid::new(&decided.beats, decided.bpm, decided.report.dynamic))];
        grids.extend(decided.others.iter().cloned());
        let choices = if grids.len() > 1 {
            grids.iter().map(|(source, grid)| Choice { source: *source, bpm: grid.bpm, dynamic: grid.dynamic }).collect()
        } else {
            Vec::new()
        };
        if let Some(file) = self.choices_file(track_id) {
            let saved = if choices.is_empty() {
                match std::fs::remove_file(&file) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                    _ => Ok(()),
                }
            } else {
                serde_json::to_vec(&grids).map_err(std::io::Error::other).and_then(|bytes| super::analyse::replace_file(&file, &bytes))
            };
            if let Err(e) = saved {
                tracing::warn!("Couldn't save the grids to choose from to {}: {e}", file.display());
            }
        }
        self.change(track_id, |state| *state = State { report: Some(decided.report.clone()), reviewed: false, using, choices, hint });
    }

    /// One of the grids the analysis made for a track.
    pub fn choice(&self, track_id: &str, source: Source) -> Option<Grid> {
        let grids: Vec<(Source, Grid)> = serde_json::from_slice(&std::fs::read(self.choices_file(track_id)?).ok()?).ok()?;
        grids.into_iter().find(|(of, _)| *of == source).map(|(_, grid)| grid)
    }

    /// The track was given another of its grids to try.
    pub fn switched(&self, track_id: &str, source: Source) {
        self.change(track_id, |state| {
            state.using = source;
            state.reviewed = false;
        });
    }

    /// The grid was changed by hand, which is looking it over.
    pub fn edited(&self, track_id: &str) {
        self.change(track_id, |state| {
            state.using = Source::Hand;
            state.reviewed = true;
        });
    }

    pub fn review(&self, track_id: &str, reviewed: bool) {
        self.change(track_id, |state| state.reviewed = reviewed);
    }

    /// `<library>.grids.json` keeps the states; `<library>.grids/` the grids.
    fn choices_file(&self, track_id: &str) -> Option<PathBuf> {
        let safe: String = track_id.chars().filter(char::is_ascii_alphanumeric).collect();
        Some(self.file.as_ref()?.with_extension("").join(format!("{safe}.json")))
    }

    fn save(&self) {
        let Some(file) = &self.file else { return };
        let saved = serde_json::to_vec(&*self.by_track.read().expect("grids lock poisoned"))
            .map_err(std::io::Error::other)
            .and_then(|bytes| super::analyse::replace_file(file, &bytes));
        if let Err(e) = saved {
            tracing::warn!("Couldn't save the grids' states to {}: {e}", file.display());
        }
    }
}

#[derive(Debug, Clone)]
pub struct Decided {
    pub beats: Vec<Beat>,
    /// The tempo a list shows: the one tempo, or the typical one.
    pub bpm: f64,
    pub report: Report,
    /// Each detector's own grid, where it isn't the one decided on.
    pub others: Vec<(Source, Grid)>,
}

impl Decided {
    /// The grid a track is given, and whose it is: the model's own where
    /// it made one apart from the grid decided on, as it is the one more
    /// often right when the two differ. One that changes tempo is passed
    /// over when grids may not.
    pub fn given(&self, following: Follow) -> (Source, Grid) {
        self.others
            .iter()
            .find(|(source, grid)| *source == Source::Model && (following != Follow::Never || !grid.dynamic))
            .cloned()
            .unwrap_or_else(|| (Source::Chosen, Grid::new(&self.beats, self.bpm, self.report.dynamic)))
    }

    /// Carries the grid on at its last tempo to `end_secs`, for a file
    /// longer than the part that was analysed.
    pub fn extend_to(&mut self, end_secs: f64) {
        let mut grid = Grid::new(&self.beats, self.bpm, self.report.dynamic);
        grid.extend_to(end_secs);
        self.beats = grid.beats.iter().map(|&(time_ms, number, tempo_x100)| Beat { number: u16::from(number), tempo_x100, time_ms }).collect();
        for (_, other) in &mut self.others {
            other.extend_to(end_secs);
        }
    }
}

/// Beats at `times`, the bar starting on the `first_bar`th, and the tempo a
/// list shows for them. A grid at one tempo has one figure on every beat:
/// its whole length over its gaps.
fn laid_out(times: &[f64], first_bar: usize, dynamic: bool) -> (Vec<Beat>, f64) {
    let even = match times {
        [first, .., last] if last > first => (last - first) / (times.len() - 1) as f64,
        _ => 0.0,
    };
    let figure = |gap: f64| if gap > 0.0 { (6000.0 / gap).round().clamp(0.0, f64::from(u16::MAX)) as u16 } else { 0 };
    let beats = times
        .iter()
        .enumerate()
        .map(|(i, &time)| {
            let gap = if i + 1 < times.len() { times[i + 1] - time } else { time - times[i.saturating_sub(1)] };
            Beat {
                number: ((i + 4 - first_bar % 4) % 4 + 1) as u16,
                tempo_x100: figure(if dynamic { gap } else { even }),
                time_ms: (time * 1e3).round().max(0.0) as u32,
            }
        })
        .collect();
    let typical = if dynamic { median(&mut times.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>()).unwrap_or(even) } else { even };
    (beats, if typical > 0.0 { (6000.0 / typical).round() / 100.0 } else { 0.0 })
}

fn median(values: &mut [f64]) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    values.get(values.len() / 2).copied()
}

/// How far the nearest of `sorted` is from `time`, signed: positive is later.
fn offset_to_nearest(sorted: &[f64], time: f64) -> Option<f64> {
    let after = sorted.partition_point(|&t| t < time);
    [after.checked_sub(1), (after < sorted.len()).then_some(after)]
        .into_iter()
        .flatten()
        .map(|i| sorted[i] - time)
        .min_by(|a, b| a.abs().total_cmp(&b.abs()))
}

fn same_beat(period: f64) -> f64 {
    SAME_BEAT_SECS.min(period * 0.2)
}

/// How well a grid and the model's beats agree: the share of the model's
/// beats that are on the grid, and of the grid's beats the model heard,
/// as their harmonic mean.
///
/// Grid beats in a stretch the model heard nothing in, a breakdown or a
/// quiet intro, don't count against it: only those between two of its beats
/// less than `2.5` beats apart do, by the slower of the two paces.
fn agreement(grid: &[f64], heard: &[f64], period: f64, heard_period: f64) -> f64 {
    let within = same_beat(period);
    let listening = 2.5 * period.max(heard_period);
    let on_grid = heard.iter().filter(|&&t| offset_to_nearest(grid, t).is_some_and(|o| o.abs() <= within)).count();
    let (mut listened, mut found) = (0usize, 0usize);
    for &beat in grid {
        let after = heard.partition_point(|&t| t < beat);
        let surrounded = after > 0 && after < heard.len() && heard[after] - heard[after - 1] < listening;
        let on_one = offset_to_nearest(heard, beat).is_some_and(|o| o.abs() <= within);
        if surrounded || on_one {
            listened += 1;
            found += usize::from(on_one);
        }
    }
    if heard.is_empty() || listened == 0 {
        return 0.0;
    }
    let (precision, recall) = (on_grid as f64 / heard.len() as f64, found as f64 / listened as f64);
    if precision + recall == 0.0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    }
}

/// Every `phase + k · period` from the start of the file to `end`.
fn comb(period: f64, phase: f64, end: f64) -> Vec<f64> {
    let first = phase - (phase / period).floor() * period;
    (0..).map(|k| first + f64::from(k) * period).take_while(|&t| t < end).take(1_000_000).collect()
}

/// Solves `(W + λ·DᵀD) y = W x` for `y`, where `D` takes second
/// differences: the sequence nearest `x` by `weights` whose pace changes
/// least. A beat with no weight is filled in from its neighbours.
fn smooth(x: &[f64], weights: &[f64], stiffness: f64) -> Vec<f64> {
    let n = x.len();
    if n < 3 {
        return x.to_vec();
    }
    // The three bands of the symmetric matrix: the diagonal and the two above.
    let (mut a0, mut a1, mut a2) = (weights.to_vec(), vec![0.0; n], vec![0.0; n]);
    for i in 0..n - 2 {
        // Row i of D is [1, -2, 1] at i, i + 1, i + 2.
        a0[i] += stiffness;
        a0[i + 1] += 4.0 * stiffness;
        a0[i + 2] += stiffness;
        a1[i] -= 2.0 * stiffness;
        a1[i + 1] -= 2.0 * stiffness;
        a2[i] += stiffness;
    }
    // LDLᵀ with L unit lower triangular: e is its first band, f its second.
    let (mut d, mut e, mut f) = (vec![0.0; n], vec![0.0; n], vec![0.0; n]);
    for i in 0..n {
        let mut pivot = a0[i];
        if i >= 1 {
            pivot -= e[i - 1] * e[i - 1] * d[i - 1];
        }
        if i >= 2 {
            pivot -= f[i - 2] * f[i - 2] * d[i - 2];
        }
        d[i] = pivot;
        if i + 1 < n {
            let mut below = a1[i];
            if i >= 1 {
                below -= f[i - 1] * e[i - 1] * d[i - 1];
            }
            e[i] = below / pivot;
        }
        if i + 2 < n {
            f[i] = a2[i] / pivot;
        }
    }
    let mut y: Vec<f64> = x.iter().zip(weights).map(|(x, w)| x * w).collect();
    for i in 0..n {
        if i >= 1 {
            y[i] -= e[i - 1] * y[i - 1];
        }
        if i >= 2 {
            y[i] -= f[i - 2] * y[i - 2];
        }
    }
    for i in 0..n {
        y[i] /= d[i];
    }
    for i in (0..n).rev() {
        if i + 1 < n {
            y[i] -= e[i] * y[i + 1];
        }
        if i + 2 < n {
            y[i] -= f[i] * y[i + 2];
        }
    }
    y
}

/// The weighted least-squares line through `(index, time)`: its time at
/// index 0 and its period.
fn line(points: &[(f64, f64, f64)]) -> Option<(f64, f64)> {
    let total: f64 = points.iter().map(|p| p.2).sum();
    if total <= 0.0 {
        return None;
    }
    let mean_k = points.iter().map(|p| p.0 * p.2).sum::<f64>() / total;
    let mean_t = points.iter().map(|p| p.1 * p.2).sum::<f64>() / total;
    let spread: f64 = points.iter().map(|p| p.2 * (p.0 - mean_k).powi(2)).sum();
    if spread <= 0.0 {
        return None;
    }
    let period = points.iter().map(|p| p.2 * (p.0 - mean_k) * (p.1 - mean_t)).sum::<f64>() / spread;
    Some((mean_t - period * mean_k, period))
}

/// The model's beats at one count of the beat throughout.
///
/// The model sometimes counts a stretch at twice or half the pace of the
/// rest: the same pulse, heard two ways. Against `reference`, the seconds
/// a beat lasts (the user's hint, or the model's own usual gap), a beat
/// that splits a beat in two is dropped and a gap two beats long gets its
/// middle beat back. The reference follows the music as it drifts, and a
/// change of tempo that isn't a doubling carries it along.
fn folded(heard: &[f64], reference: f64) -> Vec<f64> {
    if reference <= 0.0 {
        return heard.to_vec();
    }
    let mut out: Vec<f64> = Vec::with_capacity(heard.len());
    let mut recent: std::collections::VecDeque<f64> = std::collections::VecDeque::with_capacity(PACE_GAPS);
    let mut pace = reference;
    for (i, &beat) in heard.iter().enumerate() {
        let Some(&last) = out.last() else {
            out.push(beat);
            continue;
        };
        let ratio = (beat - last) / pace;
        // Short of a whole beat, and the next one heard lands nearer to
        // one: this is a beat between beats, at whatever count.
        let next_is_nearer = heard.get(i + 1).is_some_and(|next| ((next - last) / pace - 1.0).abs() < (ratio - 1.0).abs());
        if ratio < SPLIT_BEAT || (ratio < 1.0 && next_is_nearer) {
            continue;
        }
        let spans = if (TWO_BEATS.0..TWO_BEATS.1).contains(&ratio) { 2.0 } else { 1.0 };
        if spans == 2.0 {
            out.push((last + beat) / 2.0);
        }
        out.push(beat);
        // Longer gaps are a breakdown, and say nothing of the pace.
        if ratio < TWO_BEATS.1 {
            if recent.len() == PACE_GAPS {
                recent.pop_front();
            }
            recent.push_back((beat - last) / spans);
            if recent.len() >= PACE_GAPS / 2 {
                pace = median(&mut recent.iter().copied().collect::<Vec<_>>()).unwrap_or(pace);
            }
        }
    }
    out
}

/// The model's beats, counted: each with its place in the sequence, so a
/// beat it missed leaves a hole and one it heard twice is dropped.
fn counted(heard: &[f64]) -> Vec<(usize, f64)> {
    let gaps: Vec<f64> = heard.windows(2).map(|w| w[1] - w[0]).collect();
    let pace = |from: usize, to: usize| median(&mut gaps[from.min(gaps.len())..to.min(gaps.len())].to_vec());
    // An extra beat splits one gap in two.
    let mut kept: Vec<f64> = Vec::with_capacity(heard.len());
    for (i, &beat) in heard.iter().enumerate() {
        let around = pace(i.saturating_sub(PACE_GAPS), i + PACE_GAPS).unwrap_or(0.0);
        let extra = match (kept.last(), heard.get(i + 1)) {
            (Some(&before), Some(&after)) => beat - before < 0.6 * around && ((after - before) / around - 1.0).abs() < 0.2,
            _ => false,
        };
        if !extra {
            kept.push(beat);
        }
    }
    let gaps: Vec<f64> = kept.windows(2).map(|w| w[1] - w[0]).collect();
    let pace = |from: usize, to: usize| median(&mut gaps[from.min(gaps.len())..to.min(gaps.len())].to_vec());
    let mut out = Vec::with_capacity(kept.len());
    let mut index = 0usize;
    for (i, &beat) in kept.iter().enumerate() {
        if i > 0 {
            let gap = gaps[i - 1];
            // Across a change of tempo the gap is one beat of the pace
            // before it or of the pace after: whichever makes it nearer a
            // whole number of beats is the one to count it in.
            let steps = [pace(i.saturating_sub(1 + PACE_GAPS), i - 1), pace(i, i + PACE_GAPS)]
                .into_iter()
                .flatten()
                .filter(|&p| p > 0.0)
                .map(|p| gap / p)
                .min_by(|a, b| (a - a.round()).abs().total_cmp(&(b - b.round()).abs()))
                .map_or(1, |beats| beats.round().max(1.0) as usize);
            index += steps.min(4096);
        }
        out.push((index, beat));
    }
    out
}

/// Where the tempo cuts from one to another: the places in `counted` a new
/// stretch starts at, the first always 0.
fn stretches(counted: &[(usize, f64)]) -> Option<Vec<usize>> {
    let pace_at: Vec<f64> = counted.windows(2).map(|w| (w[1].1 - w[0].1) / (w[1].0 - w[0].0) as f64).collect();
    let pace = |from: usize, to: usize| median(&mut pace_at[from.min(pace_at.len())..to.min(pace_at.len())].to_vec());
    let change = |i: usize| match (pace(i.saturating_sub(PACE_GAPS), i), pace(i, i + PACE_GAPS)) {
        (Some(before), Some(after)) if i >= PACE_GAPS && i + PACE_GAPS <= pace_at.len() => (after / before).ln().abs(),
        _ => 0.0,
    };
    let mut starts = vec![0];
    let mut i = PACE_GAPS;
    while i + PACE_GAPS <= pace_at.len() {
        if change(i) <= JUMP_RATIO.ln() {
            i += 1;
            continue;
        }
        // The medians notice a few beats early and stay changed a few
        // beats late. The cut is where the gaps themselves change sides:
        // the split that leaves each gap nearest its own side's pace.
        let (Some(before), Some(after)) = (pace(i.saturating_sub(PACE_GAPS), i), pace(i + PACE_GAPS, i + 2 * PACE_GAPS).or(pace(i, i + PACE_GAPS)))
        else {
            break;
        };
        let (from, to) = (i.saturating_sub(PACE_GAPS), (i + 2 * PACE_GAPS).min(pace_at.len()));
        let cost = |cut: usize| -> f64 {
            (from..to).map(|k| (pace_at[k] / if k < cut { before } else { after }).ln().abs()).sum()
        };
        let cut = (from + 1..to).min_by(|&a, &b| cost(a).total_cmp(&cost(b))).unwrap_or(i);
        // A few beats at another pace are a fill or a stumble, not a
        // section: each side of a cut has to last.
        let since = cut - starts.last().copied().unwrap_or(0);
        // And each side is a tempo, held: its gaps within a few percent of
        // one another. Beats that scatter are the model losing its place.
        let held = |from: usize, to: usize, around: f64| {
            let mut off: Vec<f64> = pace_at[from.min(pace_at.len())..to.min(pace_at.len())].iter().map(|g| (g / around).ln().abs()).collect();
            median(&mut off).is_some_and(|typical| typical < HELD_RATIO.ln())
        };
        let steady = held(cut.saturating_sub(PACE_GAPS), cut, before) && held(cut, cut + PACE_GAPS, after);
        if steady && since >= MIN_STRETCH && pace_at.len() - cut >= MIN_STRETCH {
            // Twice or half the pace, or three beats for two, is the same
            // tempo counted another way: the model changing its mind, which
            // no grid should follow.
            if [2.0, 0.5, 1.5, 2.0 / 3.0, 4.0 / 3.0, 0.75].iter().any(|r| (after / before / r).ln().abs() < RECOUNT.ln()) {
                return None;
            }
            starts.push(cut);
        }
        i = cut.max(i) + PACE_GAPS;
    }
    Some(starts)
}

/// A grid that follows the model's beats: every beat's time, in order, the
/// share of them the model heard itself and an attack placed, and whether
/// the whole of it came out at one tempo.
struct Followed {
    times: Vec<f64>,
    placed: f64,
    one_tempo: bool,
}

fn follow(heard: &[f64], attacks: &AttackMap, end_secs: f64) -> Option<Followed> {
    let counted = counted(heard);
    if counted.len() < MIN_HEARD {
        return None;
    }
    let starts = stretches(&counted)?;
    let mut times: Vec<f64> = Vec::new();
    let (mut placed, mut straight) = (0usize, true);
    for (s, &start) in starts.iter().enumerate() {
        let stretch = &counted[start..starts.get(s + 1).copied().unwrap_or(counted.len())];
        let (first, last) = (stretch[0].0, stretch[stretch.len() - 1].0);
        let n = last - first + 1;
        let (mut x, mut weights) = (vec![0.0; n], vec![0.0; n]);
        for &(index, beat) in stretch {
            let attack = attacks.attack_within(beat, ATTACK_REACH_SECS);
            placed += usize::from(attack.is_some());
            x[index - first] = attack.map_or(beat, |a| a.secs);
            weights[index - first] = if attack.is_some() { 1.0 } else { UNPLACED_WEIGHT };
        }
        let origin = x[0];
        x.iter_mut().for_each(|t| *t -= origin);
        let mut curve = smooth(&x, &weights, STIFFNESS);
        let points: Vec<(f64, f64, f64)> = (0..n).map(|k| (k as f64, x[k], weights[k])).collect();
        match line(&points) {
            Some((at_zero, period)) if (0..n).all(|k| (curve[k] - (at_zero + period * k as f64)).abs() < STRAIGHT_SECS) => {
                curve = (0..n).map(|k| at_zero + period * k as f64).collect();
            }
            _ => straight = false,
        }
        // A drummer drifts by a few percent and a ramp by tens; a curve
        // that ranges further is following beats that aren't there.
        let (fastest, slowest) = curve.windows(2).map(|w| w[1] - w[0]).fold((f64::MAX, 0.0_f64), |(low, high), gap| (low.min(gap), high.max(gap)));
        if fastest <= 0.0 || slowest / fastest > WIDEST_RANGE {
            return None;
        }
        // Stretches meet where the model's beats do; a curve can't run
        // back over the one before.
        let floor = times.last().copied().unwrap_or(f64::NEG_INFINITY);
        times.extend(curve.into_iter().map(|t| t + origin).filter(|&t| t > floor + 0.05));
    }
    if times.len() < 2 {
        return None;
    }
    // Music with no steady pulse (a drum solo, cut-up breaks) gives beats
    // no curve follows: gaps that keep leaping from their neighbours'. A
    // grid through those is worse than one tempo.
    let gaps: Vec<f64> = times.windows(2).map(|w| w[1] - w[0]).collect();
    let leaps = (0..gaps.len())
        .filter(|&i| {
            let around = median(&mut gaps[i.saturating_sub(PACE_GAPS)..(i + PACE_GAPS).min(gaps.len())].to_vec()).unwrap_or(gaps[i]);
            (gaps[i] / around).ln().abs() > UNEVEN_GAP.ln()
        })
        .count();
    let holes = 1.0 - counted.len() as f64 / (counted[counted.len() - 1].0 - counted[0].0 + 1) as f64;
    if leaps as f64 > UNEVEN_SHARE * gaps.len() as f64 || holes > MISSED_SHARE {
        return None;
    }
    // On at the pace of each end, to the ends of the file.
    let lead = times[1] - times[0];
    let mut before = Vec::new();
    let mut time = times[0] - lead;
    while lead > 0.0 && time >= 0.0 && before.len() < 100_000 {
        before.push(time);
        time -= lead;
    }
    before.reverse();
    let tail = times[times.len() - 1] - times[times.len() - 2];
    let mut time = times[times.len() - 1] + tail;
    before.append(&mut times);
    let mut times = before;
    while tail > 0.0 && time < end_secs && times.len() < 1_000_000 {
        times.push(time);
        time += tail;
    }
    Some(Followed { times, placed: placed as f64 / counted.len() as f64, one_tempo: straight && starts.len() == 1 })
}

/// The share of a grid's beats between `from` and `to` seconds that have a
/// kick attack on them: how well it sits on the audio, whoever made it.
fn on_attacks(grid: &[f64], attacks: &AttackMap, from: f64, to: f64) -> f64 {
    let within: Vec<f64> = grid.iter().copied().filter(|t| (from..=to).contains(t)).collect();
    if within.is_empty() {
        return 0.0;
    }
    within.iter().filter(|&&t| attacks.attack_within(t, ON_ATTACK_SECS).is_some()).count() as f64 / within.len() as f64
}

/// Whether a grid at one tempo holds for the whole track: false when, for
/// stretches at a time, both the model's beats and the kick attacks sit to
/// the same side of it. The model alone doesn't say so: on a drum machine
/// it can be three frames early for a whole section.
fn holds(grid: &[f64], heard: &[f64], attacks: &AttackMap, period: f64) -> bool {
    let within = same_beat(period);
    let mut run = 0;
    for window in grid.chunks(DRIFT_WINDOW / 2).collect::<Vec<_>>().windows(2) {
        let beats = window[0].iter().chain(window[1]);
        let mut by_model: Vec<f64> = beats.clone().filter_map(|&b| offset_to_nearest(heard, b).filter(|o| o.abs() <= within)).collect();
        let mut by_attack: Vec<f64> =
            beats.filter_map(|&b| attacks.attack_within(b, same_beat(period).min(0.045)).map(|a| a.secs - b)).collect();
        let enough = DRIFT_WINDOW * 3 / 8;
        let moved = match (median(&mut by_model), median(&mut by_attack)) {
            (Some(model), Some(attack)) if by_model.len() >= enough && by_attack.len() >= enough => {
                model.abs() > DRIFT_SECS && attack.abs() > DRIFT_SECS && model.signum() == attack.signum()
            }
            _ => false,
        };
        run = if moved { run + 1 } else { 0 };
        if run >= DRIFT_RUN {
            return false;
        }
    }
    true
}

/// Which beat of the bar, 0 to 3 counting from the grid's first, the
/// model's downbeats are on, and the share of them that are.
fn bar_by_model(grid: &[f64], downbeats: &[f64], period: f64) -> Option<(usize, f64)> {
    let mut votes = [0usize; 4];
    for &downbeat in downbeats {
        let nearest = grid.partition_point(|&t| t < downbeat);
        let index = [nearest.checked_sub(1), (nearest < grid.len()).then_some(nearest)]
            .into_iter()
            .flatten()
            .min_by(|&a, &b| (grid[a] - downbeat).abs().total_cmp(&(grid[b] - downbeat).abs()));
        if let Some(index) = index.filter(|&i| (grid[i] - downbeat).abs() <= same_beat(period)) {
            votes[index % 4] += 1;
        }
    }
    let total: usize = votes.iter().sum();
    let best = (0..4).max_by_key(|&i| votes[i])?;
    (total >= 8).then(|| (best, votes[best] as f64 / total as f64))
}

/// Decides a track's grid from what each detector found in it.
///
/// `signal` is the signal detector's grid at one tempo, `heard` the model's
/// beats, `attacks` the kick attacks of `samples`. `following` says whether
/// the grid may change tempo; when it may not, the report still says where
/// the music does. `hint_bpm` is roughly the tempo, when the user gave it.
pub fn decide(
    signal: &TempoResult,
    heard: &beat_this::Tracked,
    attacks: &AttackMap,
    samples: &[f32],
    sample_rate: u32,
    following: Follow,
    hint_bpm: Option<f64>,
) -> Decided {
    let end_secs = samples.len() as f64 / f64::from(sample_rate.max(1));
    let signal_beats: Vec<Beat> =
        signal.beats.iter().map(|b| Beat { number: b.beat_number.clamp(1, 4), tempo_x100: b.tempo_x100, time_ms: b.time_ms }).collect();
    // The model's beats at one count: the hinted one, or its own usual one.
    let usual = median(&mut heard.beats.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>()).unwrap_or(0.0);
    let model_beats = folded(&heard.beats, hint_bpm.map_or(usual, |bpm| 60.0 / bpm));
    let mut gaps: Vec<f64> = model_beats.windows(2).map(|w| w[1] - w[0]).collect();
    let model_period = median(&mut gaps).unwrap_or(0.0);
    let model_bpm = if model_period > 0.0 { (6000.0 / model_period).round() / 100.0 } else { 0.0 };
    let report = |confidence: f64, dynamic: bool, issues: Vec<Issue>| Report {
        confidence: (confidence.clamp(0.0, 1.0) * 100.0).round() as u8,
        dynamic,
        issues,
        signal_bpm: (signal.bpm * 100.0).round() / 100.0,
        model_bpm,
    };

    // The signal's one tempo: its longest stretch, as a short opening one
    // can stand before it.
    let main = signal.segments.iter().max_by(|a, b| (a.to_secs - a.from_secs).total_cmp(&(b.to_secs - b.from_secs))).copied();
    if model_beats.len() < MIN_HEARD || model_period <= 0.0 {
        // One detector's word for it.
        let report = report(0.5 * signal.confidence.max(0.2), false, vec![Issue::Unheard]);
        return Decided { beats: signal_beats, bpm: signal.bpm, report, others: Vec::new() };
    }

    let mut issues = Vec::new();
    // The grid at one tempo that best fits the model's beats, if any does:
    // its beats, period, and what the signal numbered its bar from.
    let mut constant: Option<(Vec<f64>, f64, Option<usize>)> = None;
    let mut pulse = 0.0;
    if let Some(main) = main.filter(|m| m.period_secs > 0.0) {
        let (period, phase) = (main.period_secs, main.phase_secs);
        let own: Vec<f64> = signal_beats.iter().map(|b| f64::from(b.time_ms) / 1e3).collect();
        let as_found = agreement(&own, &model_beats, period, model_period);
        let readings = [
            (Issue::OffBeat, period, phase + period / 2.0),
            (Issue::HalfTime, period * 2.0, phase),
            (Issue::HalfTime, period * 2.0, phase + period),
            (Issue::HalfTime, period / 2.0, phase),
        ];
        let best = readings
            .into_iter()
            .filter(|(_, period, _)| TEMPO_RANGE.contains(&(60.0 / period)))
            .map(|(issue, period, phase)| {
                let grid = comb(period, phase, end_secs);
                (agreement(&grid, &model_beats, period, model_period), issue, period, grid)
            })
            .max_by(|a, b| a.0.total_cmp(&b.0));
        pulse = as_found;
        let first_bar = signal_beats.iter().position(|b| b.number == 1).map(|i| i % 4);
        constant = Some((own, period, first_bar));
        if let Some((fit, issue, period, grid)) = best.filter(|b| b.0 >= SAME_PULSE && b.0 > as_found + CLEARLY_BETTER) {
            // The model's pulse, on the signal's attacks.
            issues.push(issue);
            pulse = fit;
            constant = Some((grid, period, None));
        }
    }

    let same_pulse = pulse >= SAME_PULSE;
    // Whether the grid is made of the model's beats rather than the signal's.
    let mut from_model = false;
    // The model's own grid, when it made one that wasn't used.
    let mut unused: Option<Followed> = None;
    let steady = following != Follow::Always && same_pulse && constant.as_ref().is_some_and(|(grid, period, _)| holds(grid, &model_beats, attacks, *period));
    let reread = issues.iter().any(|i| matches!(i, Issue::OffBeat | Issue::HalfTime));

    // The grid's beats, the period its tolerance is taken from, how far the
    // detectors agreed on the beats, the signal's bar if it still applies,
    // and whether it follows the tempo.
    let (times, period, mut confidence, signal_bar, dynamic) = match (constant, steady) {
        (Some((grid, period, bar)), true) => {
            // A grid re-read to the model's pulse is its word against the
            // signal's: no better than even.
            (grid, period, if reread { 0.5 * pulse } else { pulse }, bar, false)
        }
        (constant, _) => {
            // A grid that follows the tempo has to earn its place: on a
            // track made on a grid the one tempo already sits on the
            // attacks, and nothing that bends can sit on them much better.
            let (from, to) = (model_beats[0], model_beats[model_beats.len() - 1]);
            let followed = follow(&model_beats, attacks, end_secs);
            let followable = followed.is_some();
            unused = followed.as_ref().map(|f| Followed { times: f.times.clone(), ..*f });
            let earned = followed.filter(|followed| {
                following == Follow::Always
                    || constant.as_ref().map_or(true, |(grid, _, _)| {
                    let (bent, straight) = (on_attacks(&followed.times, attacks, from, to), on_attacks(grid, attacks, from, to));
                    bent >= (straight + TIGHTER_BY).max(straight * TIGHTER_TIMES)
                })
            });
            match (earned, constant) {
                (Some(followed), constant) if following != Follow::Never || constant.is_none() => {
                    if followed.one_tempo {
                        // One tempo after all, and not the signal's.
                        issues.push(Issue::Tempo);
                    } else {
                        issues.push(Issue::Varies);
                    }
                    // The model's count of the beat, which may be half or
                    // twice the signal's.
                    let ratio = main.map_or(1.0, |m| model_period / m.period_secs);
                    if (ratio / 2.0).ln().abs() < RECOUNT.ln() || (ratio * 2.0).ln().abs() < RECOUNT.ln() {
                        issues.push(Issue::HalfTime);
                    }
                    let fit = agreement(&followed.times, &model_beats, model_period, model_period);
                    // Nothing checks the model here but the attacks its
                    // beats were placed on.
                    let confidence = fit * followed.placed.max(0.25);
                    from_model = true;
                    unused = None;
                    (followed.times, model_period, confidence, None, !followed.one_tempo)
                }
                (earned, Some((grid, period, bar))) => {
                    match (earned.is_some(), same_pulse, followable) {
                        // The tempo moves, and following is turned off.
                        (true, _, _) => issues.push(Issue::Varies),
                        // The same beats, and the one tempo sits on the
                        // audio as well as anything: nothing to report.
                        (false, true, _) => {}
                        // Different beats, and the audio doesn't say whose.
                        (false, false, true) => issues.push(Issue::Tempo),
                        (false, false, false) => issues.push(Issue::Uneven),
                    }
                    (grid, period, if reread { 0.5 * pulse } else { pulse }, bar, false)
                }
                (_, None) => {
                    return Decided { beats: signal_beats, bpm: signal.bpm, report: report(0.0, false, vec![Issue::Unheard]), others: Vec::new() };
                }
            }
        }
    };

    // Where the bar starts: the model's word when it has one, checked
    // against the signal's.
    let by_signal = signal_bar.or_else(|| {
        let phase = rbl_analysis::downbeat::grid_phase(samples, sample_rate, &times);
        let nearest = times.partition_point(|&t| t < phase.downbeat_secs - period / 2.0);
        (times.len() >= 8).then_some(nearest % 4)
    });
    let by_model = bar_by_model(&times, &heard.downbeats, period);
    let first_bar = match (by_model, by_signal) {
        (Some((model, share)), signal) if share >= BAR_DECIDED => {
            if signal.is_some_and(|signal| signal != model) {
                issues.push(Issue::Downbeat);
                // The surer the model, the less the signal's word against
                // it counts: its bar is a rule of thumb for dance music.
                confidence *= (0.6 + 0.75 * (share - BAR_DECIDED)).min(0.9);
            }
            model
        }
        (_, signal) => {
            issues.push(Issue::Bar);
            confidence *= 0.8;
            signal.unwrap_or(0)
        }
    };

    let (beats, bpm) = laid_out(&times, first_bar, dynamic);
    // A grid kept as the signal made it keeps the signal's figures for its
    // tempo too, rather than each gap rounded to the millisecond.
    let untouched = !from_model && !reread && beats.len() == signal_beats.len();
    let (beats, bpm) = if untouched {
        (beats.iter().zip(&signal_beats).map(|(ours, theirs)| Beat { number: ours.number, ..*theirs }).collect(), signal.bpm)
    } else {
        (beats, bpm)
    };

    // Each detector's own grid, to try where it isn't this one.
    let chosen = Grid::new(&beats, bpm, dynamic);
    let mut others = Vec::new();
    let own = Grid::new(&signal_beats, signal.bpm, false);
    if !own.beats.is_empty() && own.beats != chosen.beats {
        others.push((Source::Signal, own));
    }
    if let Some(followed) = unused {
        let bar = bar_by_model(&followed.times, &heard.downbeats, model_period).map_or(0, |(bar, _)| bar);
        let (beats, bpm) = laid_out(&followed.times, bar, !followed.one_tempo);
        let own = Grid::new(&beats, bpm, !followed.one_tempo);
        if own.beats != chosen.beats {
            others.push((Source::Model, own));
        }
    }
    Decided { beats, bpm, report: report(confidence, dynamic, issues), others }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curve_is_a_line_through_a_line() {
        let x: Vec<f64> = (0..40).map(|k| 0.5 * f64::from(k)).collect();
        let y = smooth(&x, &vec![1.0; 40], STIFFNESS);
        for (a, b) in x.iter().zip(&y) {
            assert!((a - b).abs() < 1e-9);
        }
        // A missing beat is filled in on the line, and a late one pulled back.
        let mut weights = vec![1.0; 40];
        weights[10] = 0.0;
        let mut bent = x.clone();
        bent[10] = 99.0;
        bent[20] += 0.03;
        let y = smooth(&bent, &weights, STIFFNESS);
        assert!((y[10] - 5.0).abs() < 1e-3, "{}", y[10]);
        assert!((y[20] - 10.0).abs() < 0.01, "{}", y[20]);
    }

    #[test]
    fn beats_are_counted_through_misses_and_extras() {
        let mut heard: Vec<f64> = (0..40).map(|k| 0.5 * f64::from(k)).collect();
        heard.remove(20);
        heard.remove(20);
        heard.insert(10, 4.74);
        let counted = counted(&heard);
        assert_eq!(counted.len(), 38);
        assert_eq!(counted[0], (0, 0.0));
        assert_eq!(counted.last().unwrap().0, 39);
        assert!(counted.iter().all(|&(index, time)| (time - 0.5 * index as f64).abs() < 1e-9));
    }

    #[test]
    fn a_cut_to_another_tempo_starts_a_stretch() {
        // 60 beats at 120, then 60 at 150.
        let mut heard: Vec<f64> = (0..60).map(|k| 0.5 * f64::from(k)).collect();
        heard.extend((0..60).map(|k| 30.0 + 0.4 * f64::from(k)));
        let counted = counted(&heard);
        assert_eq!(counted.len(), 120);
        assert_eq!(counted.last().unwrap().0, 119);
        assert_eq!(stretches(&counted), Some(vec![0, 60]));
    }

    #[test]
    fn agreement_tells_the_same_pulse_from_its_off_beat_and_its_half() {
        let grid = comb(0.5, 0.1, 120.0);
        let on: Vec<f64> = grid.iter().map(|t| t + 0.02).collect();
        assert!(agreement(&grid, &on, 0.5, 0.5) > 0.99);
        let off: Vec<f64> = grid.iter().map(|t| t + 0.25).collect();
        assert!(agreement(&grid, &off, 0.5, 0.5) < 0.05);
        let half: Vec<f64> = grid.iter().step_by(2).copied().collect();
        let fit = agreement(&grid, &half, 0.5, 1.0);
        assert!((0.6..0.7).contains(&fit), "{fit}");
        // And a grid at twice the model's pace is no better for having a
        // beat near each of the model's.
        let double = comb(0.25, 0.1, 120.0);
        let fit = agreement(&double, &grid, 0.25, 0.5);
        assert!((0.6..0.7).contains(&fit), "{fit}");
        // A breakdown the model heard nothing in doesn't count against it.
        let with_a_gap: Vec<f64> = grid.iter().copied().filter(|t| !(40.0..70.0).contains(t)).collect();
        assert!(agreement(&grid, &with_a_gap, 0.5, 0.5) > 0.99);
    }

    const RATE: u32 = 22_050;

    /// A click, with the snap a kick's attack has, at each of `beats`.
    fn clicks(beats: &[f64], secs: f64) -> Vec<f32> {
        let mut samples = vec![0.0_f32; (secs * f64::from(RATE)) as usize];
        for &beat in beats {
            let start = (beat * f64::from(RATE)).round() as usize;
            for i in 0..220.min(samples.len().saturating_sub(start)) {
                let t = i as f32 / RATE as f32;
                samples[start + i] += (std::f32::consts::TAU * 3000.0 * t).sin() * (-t * 600.0).exp() * 0.8;
            }
        }
        samples
    }

    /// The signal detector's answer: one tempo, numbered from its first beat.
    fn one_tempo(period: f64, phase: f64, secs: f64) -> TempoResult {
        let segments = vec![rbl_analysis::tempo::Segment { from_secs: 0.0, to_secs: secs, period_secs: period, phase_secs: phase }];
        TempoResult {
            bpm: 60.0 / period,
            confidence: 1.0,
            first_beat_secs: phase,
            beats: rbl_analysis::tempo::beats_of(&segments, 0),
            segments,
        }
    }

    /// The model's answer: the beats to its 20 ms frames, every fourth a downbeat.
    fn heard(beats: &[f64], first_bar: usize) -> beat_this::Tracked {
        let beats: Vec<f64> = beats.iter().map(|b| (b * 50.0).round() / 50.0).collect();
        let downbeats = beats.iter().copied().skip(first_bar).step_by(4).collect();
        beat_this::Tracked { beats, downbeats, logits: Vec::new() }
    }

    fn decided(signal: &TempoResult, model: &beat_this::Tracked, truth: &[f64], secs: f64, follow_tempo: bool) -> Decided {
        let samples = clicks(truth, secs);
        let attacks = AttackMap::new(&samples, RATE, rbl_analysis::attack::AttackOptions::default());
        decide(signal, model, &attacks, &samples, RATE, if follow_tempo { Follow::Auto } else { Follow::Never }, None)
    }

    /// How far the grid's beat nearest each true beat is from it, at worst.
    fn worst_miss(decided: &Decided, truth: &[f64]) -> f64 {
        let grid: Vec<f64> = decided.beats.iter().map(|b| f64::from(b.time_ms) / 1e3).collect();
        truth.iter().map(|&t| offset_to_nearest(&grid, t).map_or(f64::MAX, f64::abs)).fold(0.0, f64::max)
    }

    #[test]
    fn detectors_that_agree_keep_the_signals_grid_and_are_sure() {
        let truth = comb(0.5, 0.25, 120.0);
        let signal = one_tempo(0.5, 0.25, 120.0);
        let decided = decided(&signal, &heard(&truth, 0), &truth, 120.0, true);
        assert_eq!(decided.report.confidence, 100);
        assert!(!decided.report.dynamic && decided.report.issues.is_empty(), "{:?}", decided.report);
        assert_eq!(decided.beats.len(), signal.beats.len());
        assert!(decided.beats.iter().zip(&signal.beats).all(|(ours, theirs)| ours.time_ms == theirs.time_ms && ours.tempo_x100 == 12000));
        assert_eq!(decided.bpm, 120.0);
    }

    #[test]
    fn the_models_bar_wins_and_costs_some_confidence() {
        let truth = comb(0.5, 0.25, 120.0);
        let signal = one_tempo(0.5, 0.25, 120.0);
        // The signal numbers from its first beat; the model hears the bar
        // start two beats on.
        let decided = decided(&signal, &heard(&truth, 2), &truth, 120.0, true);
        assert_eq!(decided.report.issues, [Issue::Downbeat]);
        assert_eq!(decided.report.confidence, 90);
        assert_eq!(decided.beats.iter().position(|b| b.number == 1), Some(2));
    }

    #[test]
    fn a_grid_on_the_off_beat_is_moved_onto_the_models_beats() {
        let truth = comb(0.5, 0.25, 120.0);
        let signal = one_tempo(0.5, 0.0, 120.0);
        let decided = decided(&signal, &heard(&truth, 0), &truth, 120.0, true);
        assert!(decided.report.issues.contains(&Issue::OffBeat), "{:?}", decided.report);
        assert!(decided.report.confidence <= 50);
        assert!(worst_miss(&decided, &truth) < 0.002, "{}", worst_miss(&decided, &truth));
        assert_eq!(decided.bpm, 120.0);
    }

    #[test]
    fn a_signal_counting_double_is_halved_to_the_models_count() {
        let truth = comb(0.8, 0.2, 120.0);
        let signal = one_tempo(0.4, 0.2, 120.0);
        let decided = decided(&signal, &heard(&truth, 0), &truth, 120.0, true);
        assert!(decided.report.issues.contains(&Issue::HalfTime), "{:?}", decided.report);
        assert_eq!(decided.bpm, 75.0);
        assert!((decided.beats.len() as i64 - truth.len() as i64).abs() <= 1);
    }

    /// A drummer who speeds up and settles back: 120 BPM, up to 126 and down again.
    fn drifting(secs: f64) -> Vec<f64> {
        let mut beats = vec![0.25];
        while let Some(&last) = beats.last() {
            let bpm = 120.0 + 6.0 * (std::f64::consts::TAU * last / 60.0).sin().max(0.0);
            if last + 60.0 / bpm >= secs {
                break;
            }
            beats.push(last + 60.0 / bpm);
        }
        beats
    }

    #[test]
    fn a_drifting_drummer_is_followed_beat_by_beat() {
        let truth = drifting(120.0);
        let signal = one_tempo(120.0 / truth.len() as f64, 0.25, 120.0);
        let decided = decided(&signal, &heard(&truth, 0), &truth, 120.0, true);
        assert!(decided.report.dynamic, "{:?}", decided.report);
        // (Identical clicks have no bar for the signal to find, so it may
        // also disagree with the model about that.)
        assert!(decided.report.issues.contains(&Issue::Varies), "{:?}", decided.report);
        // Every click has a beat within a few milliseconds, though the
        // model placed them only to the nearest 20.
        assert!(worst_miss(&decided, &truth) < 0.006, "{}", worst_miss(&decided, &truth));
        assert!((decided.beats.len() as i64 - truth.len() as i64).abs() <= 1);
        let tempos: Vec<u16> = decided.beats.iter().map(|b| b.tempo_x100).collect();
        assert!(tempos.iter().all(|t| (11_900..=12_700).contains(t)), "{:?}", tempos.iter().min().zip(tempos.iter().max()));
        assert!(tempos.iter().any(|&t| t > 12_500));
        assert_eq!(decided.beats.iter().position(|b| b.number == 1), Some(0));
    }

    #[test]
    fn one_tempo_the_signal_got_wrong_is_the_models_and_still_one_tempo() {
        // 125 BPM, which the signal took for 126.5.
        let truth = comb(0.48, 0.3, 120.0);
        let signal = one_tempo(60.0 / 126.5, 0.3, 120.0);
        let decided = decided(&signal, &heard(&truth, 0), &truth, 120.0, true);
        assert!(!decided.report.dynamic, "{:?}", decided.report);
        assert!(decided.report.issues.contains(&Issue::Tempo), "{:?}", decided.report);
        assert_eq!(decided.bpm, 125.0);
        assert!(decided.beats.iter().all(|b| b.tempo_x100 == 12_500));
        assert!(worst_miss(&decided, &truth) < 0.003, "{}", worst_miss(&decided, &truth));
    }

    #[test]
    fn with_following_off_the_grid_stays_at_one_tempo_and_says_it_moves() {
        let truth = drifting(120.0);
        let signal = one_tempo(120.0 / truth.len() as f64, 0.25, 120.0);
        let decided = decided(&signal, &heard(&truth, 0), &truth, 120.0, false);
        assert!(!decided.report.dynamic);
        assert!(decided.report.issues.contains(&Issue::Varies), "{:?}", decided.report);
        assert!(decided.report.confidence < 85);
        assert_eq!(decided.beats.len(), signal.beats.len());
    }

    #[test]
    fn a_cut_to_another_tempo_is_two_straight_stretches() {
        let mut truth: Vec<f64> = (0..120).map(|k| 0.25 + 0.5 * f64::from(k)).collect();
        truth.extend((0..140).map(|k| 60.25 + 0.4 * f64::from(k)));
        let signal = one_tempo(0.5, 0.25, 116.0);
        let decided = decided(&signal, &heard(&truth, 0), &truth, 116.5, true);
        assert!(decided.report.dynamic, "{:?}", decided.report);
        assert!(worst_miss(&decided, &truth) < 0.004, "{}", worst_miss(&decided, &truth));
        let mut tempos: Vec<u16> = decided.beats.iter().map(|b| b.tempo_x100).collect();
        tempos.dedup();
        // 120, the one gap across the cut, then 150.
        assert!(tempos.len() <= 4 && tempos[0] == 12_000 && tempos[tempos.len() - 1] == 15_000, "{tempos:?}");
    }

    #[test]
    fn a_model_that_heard_nothing_leaves_the_signals_grid_unsure() {
        let truth = comb(0.5, 0.25, 60.0);
        let signal = one_tempo(0.5, 0.25, 60.0);
        let decided = decided(&signal, &beat_this::Tracked::default(), &truth, 60.0, true);
        assert_eq!(decided.report.issues, [Issue::Unheard]);
        assert_eq!(decided.report.confidence, 50);
        assert_eq!(decided.beats.len(), signal.beats.len());
    }

    #[test]
    fn a_grid_carries_on_to_the_end_of_a_longer_file() {
        let truth = comb(0.5, 0.25, 60.0);
        let mut decided = decided(&one_tempo(0.5, 0.25, 60.0), &heard(&truth, 0), &truth, 60.0, true);
        let before = decided.beats.len();
        decided.extend_to(90.0);
        assert_eq!(decided.beats.len(), before + 60);
        let last = decided.beats[decided.beats.len() - 1];
        assert_eq!((last.time_ms, last.tempo_x100), (89_750, 12_000));
        assert!(decided.beats.windows(2).all(|w| w[1].number == w[0].number % 4 + 1));
    }

    #[test]
    fn beats_counted_two_ways_are_folded_to_one() {
        // 120 BPM, with ten beats heard at double time and ten at half.
        let steady: Vec<f64> = (0..120).map(|k| 0.5 * f64::from(k)).collect();
        let mut heard = steady.clone();
        heard.extend((40..50).map(|k| 0.5 * f64::from(k) + 0.25));
        heard.retain(|t| !((35.0..40.0).contains(t) && ((t / 0.5).round() as i64) % 2 == 1));
        heard.sort_by(f64::total_cmp);
        assert_eq!(folded(&heard, 0.5), steady);
        // Told it is 240, every beat gets one between; told 60, every other goes.
        assert_eq!(folded(&steady, 0.25).len(), 239);
        assert_eq!(folded(&steady, 1.0).len(), 60);
        // A cut to a tempo that isn't a doubling is left alone.
        let mut cut: Vec<f64> = (0..40).map(|k| 0.5 * f64::from(k)).collect();
        cut.extend((0..40).map(|k| 20.0 + 0.36 * f64::from(k)));
        assert_eq!(folded(&cut, 0.5), cut);
    }

    #[test]
    fn a_hint_settles_the_count_and_a_stretch_at_double_time_is_followed_through() {
        let truth = drifting(120.0);
        // The model doubles up for a few bars in the middle.
        let mut doubled = truth.clone();
        doubled.extend(truth.windows(2).skip(100).take(16).map(|w| (w[0] + w[1]) / 2.0));
        doubled.sort_by(f64::total_cmp);
        let model = heard(&doubled, 0);
        let model = beat_this::Tracked { downbeats: truth.iter().copied().step_by(4).map(|b| (b * 50.0).round() / 50.0).collect(), ..model };
        let signal = one_tempo(120.0 / truth.len() as f64, 0.25, 120.0);
        let samples = clicks(&truth, 120.0);
        let attacks = AttackMap::new(&samples, RATE, rbl_analysis::attack::AttackOptions::default());

        let followed = decide(&signal, &model, &attacks, &samples, RATE, Follow::Auto, None);
        assert!(followed.report.dynamic, "{:?}", followed.report);
        assert!((followed.beats.len() as i64 - truth.len() as i64).abs() <= 1, "{} beats", followed.beats.len());
        assert!(worst_miss(&followed, &truth) < 0.006, "{}", worst_miss(&followed, &truth));

        // Hinted at about half the tempo, the grid has every other beat.
        let halved = decide(&signal, &model, &attacks, &samples, RATE, Follow::Auto, Some(61.0));
        assert!((halved.beats.len() as i64 - truth.len() as i64 / 2).abs() <= 1, "{} beats", halved.beats.len());
        assert!((halved.bpm - 61.5).abs() < 2.0, "{}", halved.bpm);
    }

    #[test]
    fn told_the_tempo_moves_the_grid_follows_without_the_audio_bearing_it_out() {
        // A drummer who drifts over clicks too faint to place a beat on:
        // left to itself the analysis keeps one tempo.
        let truth = drifting(120.0);
        let signal = one_tempo(120.0 / truth.len() as f64, 0.25, 120.0);
        let silence = vec![0.0_f32; 120 * RATE as usize];
        let attacks = AttackMap::new(&silence, RATE, rbl_analysis::attack::AttackOptions::default());
        let model = heard(&truth, 0);
        let left = decide(&signal, &model, &attacks, &silence, RATE, Follow::Auto, None);
        assert!(!left.report.dynamic, "{:?}", left.report);
        let told = decide(&signal, &model, &attacks, &silence, RATE, Follow::Always, None);
        assert!(told.report.dynamic && told.report.issues.contains(&Issue::Varies), "{:?}", told.report);
        // To the model's 20 ms frames, with nothing to place beats on.
        assert!(worst_miss(&told, &truth) < 0.02, "{}", worst_miss(&told, &truth));
        // And told it doesn't, it stays put.
        assert_eq!(Follow::from(true, &Hint { bpm: None, varies: Some(false) }), Follow::Never);
        assert_eq!(Follow::from(false, &Hint { bpm: None, varies: Some(true) }), Follow::Always);
        assert_eq!(Follow::from(true, &Hint::default()), Follow::Auto);
    }

    #[test]
    fn each_detectors_own_grid_is_kept_to_try() {
        // Agreed: nothing else to try.
        let truth = comb(0.5, 0.25, 120.0);
        let agreed = decided(&one_tempo(0.5, 0.25, 120.0), &heard(&truth, 0), &truth, 120.0, true);
        assert!(agreed.others.is_empty());

        // Followed: the signal's one tempo is the other.
        let truth = drifting(120.0);
        let signal = one_tempo(120.0 / truth.len() as f64, 0.25, 120.0);
        let followed = decided(&signal, &heard(&truth, 0), &truth, 120.0, true);
        let [(Source::Signal, other)] = &followed.others[..] else { panic!("{:?}", followed.others.iter().map(|o| o.0).collect::<Vec<_>>()) };
        assert!(!other.dynamic && other.beats.len() == signal.beats.len());

        // Kept at one tempo: the model's is the other, and goes on with it
        // to the end of a longer file.
        let mut kept = decided(&signal, &heard(&truth, 0), &truth, 120.0, false);
        let [(Source::Model, other)] = &kept.others[..] else { panic!("{:?}", kept.others.iter().map(|o| o.0).collect::<Vec<_>>()) };
        assert!(other.dynamic);
        // The model's own is the grid a track is given, unless it changes
        // tempo and grids may not.
        assert_eq!(kept.given(Follow::Auto), (Source::Model, other.clone()));
        assert_eq!(kept.given(Follow::Never).0, Source::Chosen);
        assert_eq!(followed.given(Follow::Auto).0, Source::Chosen);
        let before = other.beats.len();
        kept.extend_to(150.0);
        assert!(kept.others[0].1.beats.len() > before + 50);
        assert!(kept.others[0].1.beats.windows(2).all(|w| w[1].0 > w[0].0));
    }

    #[test]
    fn a_grids_state_is_kept_between_runs() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("libraries").join("lib.grids.json");
        let truth = drifting(120.0);
        let signal = one_tempo(120.0 / truth.len() as f64, 0.25, 120.0);
        let followed = decided(&signal, &heard(&truth, 0), &truth, 120.0, true);

        let states = States::open(file.clone());
        assert_eq!(states.get("7"), None);
        states.analysed("7", &followed, Source::Chosen, Hint { bpm: Some(124.0), varies: None });
        let state = States::open(file.clone()).get("7").unwrap();
        assert_eq!(state.report.as_ref(), Some(&followed.report));
        assert!(!state.reviewed && state.using == Source::Chosen);
        assert_eq!(state.hint.bpm, Some(124.0));
        assert_eq!(state.choices.iter().map(|c| c.source).collect::<Vec<_>>(), [Source::Chosen, Source::Signal]);
        assert!(state.choices[0].dynamic && !state.choices[1].dynamic);
        assert_eq!(states.ranks()["7"], followed.report.confidence);

        // The grids themselves are beside it, to switch between.
        let other = states.choice("7", Source::Signal).unwrap();
        assert_eq!(other.beats.len(), signal.beats.len());
        assert_eq!(states.choice("7", Source::Chosen).unwrap().beats.len(), followed.beats.len());
        assert_eq!(states.choice("7", Source::Model), None);

        states.switched("7", Source::Signal);
        states.review("7", true);
        let state = States::open(file.clone()).get("7").unwrap();
        assert!(state.reviewed && state.using == Source::Signal);
        assert_eq!(states.ranks()["7"], u8::MAX);
        // Trying another is not having looked at it yet.
        states.switched("7", Source::Chosen);
        assert!(!states.get("7").unwrap().reviewed);

        // A grid from elsewhere, changed by hand, is reviewed with no report.
        states.edited("8");
        let state = States::open(file.clone()).get("8").unwrap();
        assert!(state.reviewed && state.using == Source::Hand && state.report.is_none());

        // Analysing again starts over, and a grid both agree on has no others.
        let truth = comb(0.5, 0.25, 60.0);
        states.analysed("7", &decided(&one_tempo(0.5, 0.25, 60.0), &heard(&truth, 0), &truth, 60.0, true), Source::Chosen, Hint::default());
        let state = states.get("7").unwrap();
        assert!(!state.reviewed && state.choices.is_empty());
        assert_eq!(states.choice("7", Source::Chosen), None);
    }

    #[test]
    fn a_file_of_reports_alone_still_reads() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lib.grids.json");
        std::fs::write(&file, r#"{"7":{"confidence":72,"dynamic":true,"issues":["varies","downbeat"],"signalBpm":126.68,"modelBpm":125.0}}"#).unwrap();
        let state = States::open(file).get("7").unwrap();
        assert_eq!(state.report.unwrap().issues, [Issue::Varies, Issue::Downbeat]);
        assert!(!state.reviewed && state.choices.is_empty());
    }

    /// What the two detectors make of real files, one path a line in
    /// `GRID_PROBE`: `cargo test grid_probe -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn grid_probe() {
        let paths = std::env::var("GRID_PROBE").expect("GRID_PROBE");
        for path in paths.lines().filter(|line| !line.trim().is_empty()) {
            let path = std::path::Path::new(path.trim());
            let Ok(audio) = rbl_audio::decode_mono(path, Some(1800.0)) else {
                println!("{}: undecodable", path.display());
                continue;
            };
            let started = std::time::Instant::now();
            // `GRID_HINT=125` and `GRID_FOLLOW=always`, as the user could say.
            let hint: Option<f64> = std::env::var("GRID_HINT").ok().and_then(|bpm| bpm.parse().ok());
            let following = if std::env::var("GRID_FOLLOW").is_ok_and(|f| f == "always") { Follow::Always } else { Follow::Auto };
            let mut options = rbl_analysis::AnalysisPreset::Rekordbox.options();
            if let Some(bpm) = hint {
                (options.tempo.min_bpm, options.tempo.max_bpm, options.tempo.prior_centre) = (bpm * 0.92, bpm * 1.08, bpm);
            }
            let signal = rbl_analysis::analyse_grid_with(&audio.samples, audio.sample_rate, options);
            let heard = beat_this::track(&audio.samples, audio.sample_rate, 4);
            let attacks = AttackMap::new(&audio.samples, audio.sample_rate, options.attacks);
            let decided = decide(&signal, &heard, &attacks, &audio.samples, audio.sample_rate, following, hint);
            let r = &decided.report;
            let tempos: Vec<f64> = decided.beats.iter().map(|b| f64::from(b.tempo_x100) / 100.0).collect();
            let (low, high) = tempos.iter().fold((f64::MAX, f64::MIN), |(l, h), &t| (l.min(t), h.max(t)));
            let mut steps: Vec<f64> = tempos.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
            println!("    beat-to-beat tempo step: typical {:.2} BPM, largest {:.2}", median(&mut steps).unwrap_or(0.0), steps.last().copied().unwrap_or(0.0));
            println!(
                "{:>3}% {} {:7.2} BPM (signal {:.2}, model {:.2}; {low:.1}..{high:.1}) {} beats, first bar at {:?} ms, {:?} [{:.1}s] {}",
                r.confidence,
                if r.dynamic { "dynamic " } else { "constant" },
                decided.bpm,
                r.signal_bpm,
                r.model_bpm,
                decided.beats.len(),
                decided.beats.iter().find(|b| b.number == 1).map(|b| b.time_ms),
                r.issues,
                started.elapsed().as_secs_f64(),
                path.file_stem().unwrap().to_string_lossy(),
            );
        }
    }

    /// Grids for a sample of the installed library beside the ones it has,
    /// which rekordbox or a hand made. Nothing is written.
    /// `GRID_SAMPLE=40 cargo test library_probe -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn library_probe() {
        use super::super::tests::query;
        use super::super::{Collection, Sort, Source};
        let installed = rbl_db::detect().expect("rekordbox library");
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("master.db");
        std::fs::copy(&installed.master_db, &copy).unwrap();
        let collection = Collection::new(dir.path().join("backups"));
        collection.load_from(rbl_db::LibraryLocation { master_db: copy, is_real_install: false, ..installed }).unwrap();
        let page = collection.tracks(&query(Source::Collection, Sort::Title), 0, 100_000).unwrap();
        let wanted: usize = std::env::var("GRID_SAMPLE").ok().and_then(|n| n.parse().ok()).unwrap_or(24);
        let skip: usize = std::env::var("GRID_SKIP").ok().and_then(|n| n.parse().ok()).unwrap_or(0);
        let matching = std::env::var("GRID_MATCH").unwrap_or_default().to_lowercase();
        let matching: Vec<&str> = matching.split('|').filter(|m| !m.is_empty()).collect();
        let rows: Vec<_> = page
            .rows
            .iter()
            .filter(|r| r.analysed && std::path::Path::new(&r.path).is_file())
            .filter(|r| matching.is_empty() || matching.iter().any(|m| r.title.to_lowercase().contains(m)))
            .collect();
        println!("{} analysed tracks with files; sampling {wanted}", rows.len());
        let (mut close, mut total, mut same_bar, mut scores) = (0, 0, 0, Vec::new());
        for track in rows.iter().skip(skip).step_by((rows.len() / wanted.max(1)).max(1)).take(wanted) {
            let Ok(existing) = collection.preparation(&track.id) else { continue };
            let Ok(audio) = rbl_audio::decode_mono(std::path::Path::new(&track.path), Some(1800.0)) else { continue };
            if existing.beats.len() < 32 {
                continue;
            }
            let options = rbl_analysis::AnalysisPreset::Rekordbox.options();
            let signal = rbl_analysis::analyse_grid_with(&audio.samples, audio.sample_rate, options);
            let heard = beat_this::track(&audio.samples, audio.sample_rate, 4);
            let attacks = AttackMap::new(&audio.samples, audio.sample_rate, options.attacks);
            let decided = decide(&signal, &heard, &attacks, &audio.samples, audio.sample_rate, Follow::Auto, None);
            let theirs: Vec<f64> = existing.beats.iter().map(|b| f64::from(b.0) / 1e3).collect();
            let grid_of = |beats: &[Beat]| beats.iter().map(|b| f64::from(b.time_ms) / 1e3).collect::<Vec<f64>>();
            let near = |ours: &[f64], within: f64| {
                ours.iter().filter(|&&t| offset_to_nearest(&theirs, t).is_some_and(|o| o.abs() <= within)).count() as f64 / ours.len().max(1) as f64
            };
            let ours = grid_of(&decided.beats);
            let only_signal = grid_of(&signal.beats.iter().map(|b| Beat { number: b.beat_number, tempo_x100: b.tempo_x100, time_ms: b.time_ms }).collect::<Vec<_>>());
            let mut tempos: Vec<u16> = existing.beats.iter().map(|b| b.2).collect();
            tempos.dedup();
            let bar = |beats: &[(f64, u16)]| beats.iter().find(|b| b.1 == 1).map(|b| b.0);
            let their_bar = bar(&existing.beats.iter().map(|b| (f64::from(b.0) / 1e3, u16::from(b.1))).collect::<Vec<_>>());
            let our_bar = bar(&decided.beats.iter().map(|b| (f64::from(b.time_ms) / 1e3, b.number)).collect::<Vec<_>>());
            let period = 60.0 / decided.bpm.max(1.0);
            let bars_agree = match (their_bar, our_bar) {
                (Some(a), Some(b)) => {
                    let beats = (a - b) / period / 4.0;
                    (beats - beats.round()).abs() < 0.1
                }
                _ => false,
            };
            let r = &decided.report;
            total += 1;
            close += usize::from(near(&ours, 0.02) > 0.9);
            same_bar += usize::from(bars_agree);
            scores.push((r.confidence, near(&ours, 0.02)));
            let (from, to) = (heard.beats.first().copied().unwrap_or(0.0), heard.beats.last().copied().unwrap_or(0.0));
            let followed = follow(&heard.beats, &attacks, audio.duration_secs());
            println!(
                "    on attacks: ours {:.2} signal {:.2} theirs {:.2} followed {:?}",
                on_attacks(&ours, &attacks, from, to),
                on_attacks(&only_signal, &attacks, from, to),
                on_attacks(&theirs, &attacks, from, to),
                followed.map(|f| (on_attacks(&f.times, &attacks, from, to) * 100.0).round() / 100.0),
            );
            println!(
                "{:>3}% {} {:7.2} | theirs {:7.2} x{:<3} | within 20 ms: ours {:>3.0}% signal alone {:>3.0}% | bar {} | {:?} | {} - {}",
                r.confidence,
                if r.dynamic { "dyn" } else { "one" },
                decided.bpm,
                existing.bpm.unwrap_or(0.0),
                tempos.len(),
                100.0 * near(&ours, 0.02),
                100.0 * near(&only_signal, 0.02),
                if bars_agree { "same" } else { "DIFF" },
                r.issues,
                track.artist,
                track.title,
            );
        }
        println!("{close} of {total} within 20 ms of the existing grid on nine beats in ten; {same_bar} with the same bar");
    }
}
