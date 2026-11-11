//! The analysis queue: tracks waiting for their tempo, beat grid, key and
//! waveforms to be worked out, a few at a time in the background.
//!
//! Emits `analysis-progress` as the queue moves, and `collection-changed`
//! after each track so lists show its BPM and key.

use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tracing::{info, warn};

use crate::ingest::ArrivalQueue;
use crate::library;
use crate::rekordbox::{Collection, GridHint};
use crate::state::SettingsState;

/// Tracks analysed at once. Each holds its decoded audio in memory (about
/// 10 MB a minute), and saving them takes turns anyway.
fn workers() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get() / 2).clamp(1, 4)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub track_id: String,
    pub title: String,
    pub error: String,
}

/// The current batch, for the frontend. A batch runs from the first track
/// queued until the queue is empty.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisProgress {
    pub active: bool,
    pub total: usize,
    pub done: usize,
    /// Tracks being analysed right now.
    pub running: Vec<String>,
    /// Tracks waiting their turn, in order.
    pub queued: Vec<String>,
    pub failed: Vec<Failure>,
    /// Why the batch ended early, when it did.
    pub stopped: Option<String>,
}

#[derive(Default)]
struct Queue {
    /// Each with what the user said of it, if anything.
    pending: VecDeque<(String, GridHint)>,
    workers: usize,
    progress: AnalysisProgress,
}

#[derive(Default)]
pub struct AnalysisQueue(Mutex<Queue>);

impl AnalysisQueue {
    pub fn progress(&self) -> AnalysisProgress {
        let queue = self.0.lock().expect("analysis lock poisoned");
        let mut progress = queue.progress.clone();
        progress.queued = queue.pending.iter().map(|(id, _)| id.clone()).collect();
        progress
    }
}

fn emit(app: &AppHandle) {
    let _ = app.emit("analysis-progress", app.state::<AnalysisQueue>().progress());
}

/// Queues tracks for analysis and starts on them. With `again` false, tracks
/// that already have an analysis are left out. Returns how many were queued.
///
/// A grid the user has reviewed is only analysed over when its track is
/// asked for on its own: a batch leaves it be.
pub async fn enqueue(app: &AppHandle, track_ids: Vec<String>, again: bool) -> usize {
    enqueue_with(app, track_ids, again, GridHint::default()).await
}

/// [`enqueue`], with what the user knows of the tracks' tempo.
pub async fn enqueue_with(app: &AppHandle, track_ids: Vec<String>, again: bool, hint: GridHint) -> usize {
    let track_ids = if again {
        let reviewed = if track_ids.len() > 1 { app.state::<Collection>().reviewed(&track_ids) } else { Vec::new() };
        track_ids.into_iter().filter(|id| !reviewed.contains(id)).collect()
    } else {
        let reader = app.clone();
        library::blocking(move || Ok(reader.state::<Collection>().unanalysed(&track_ids)))
            .await
            .unwrap_or_default()
    };
    let (queued, start) = {
        let state = app.state::<AnalysisQueue>();
        let mut queue = state.0.lock().expect("analysis lock poisoned");
        if queue.workers == 0 {
            queue.progress = AnalysisProgress::default();
        }
        let mut queued = 0;
        for id in track_ids {
            if !queue.pending.iter().any(|(waiting, _)| *waiting == id) && !queue.progress.running.contains(&id) {
                queue.pending.push_back((id, hint));
                queued += 1;
            }
        }
        queue.progress.total += queued;
        let start = workers().min(queue.pending.len()).saturating_sub(queue.workers);
        queue.workers += start;
        queue.progress.active = queue.workers > 0;
        (queued, start)
    };
    for _ in 0..start {
        tauri::async_runtime::spawn(work(app.clone()));
    }
    emit(app);
    queued
}

/// Drops everything still waiting; tracks being analysed finish.
pub fn cancel(app: &AppHandle) {
    stop(app, None);
    emit(app);
}

fn stop(app: &AppHandle, why: Option<String>) {
    let state = app.state::<AnalysisQueue>();
    let mut queue = state.0.lock().expect("analysis lock poisoned");
    let dropped = queue.pending.len();
    queue.pending.clear();
    queue.progress.total -= dropped.min(queue.progress.total);
    if dropped > 0 && why.is_some() {
        queue.progress.stopped = why;
    }
}

/// Takes tracks off the queue until it's empty.
async fn work(app: AppHandle) {
    loop {
        // rekordbox being opened ends the batch: every track would be refused.
        if rbl_db::is_rekordbox_running() {
            stop(&app, Some("rekordbox was opened. Quit it and analyse again.".into()));
        }
        let next = {
            let state = app.state::<AnalysisQueue>();
            let mut queue = state.0.lock().expect("analysis lock poisoned");
            match queue.pending.pop_front() {
                Some((id, hint)) => {
                    queue.progress.running.push(id.clone());
                    Some((id, hint))
                }
                None => {
                    queue.workers -= 1;
                    queue.progress.active = queue.workers > 0;
                    None
                }
            }
        };
        let Some((track_id, hint)) = next else {
            emit(&app);
            return;
        };
        emit(&app);

        let follow = app.state::<SettingsState>().snapshot().dynamic_grids;
        let (worker, id) = (app.clone(), track_id.clone());
        let result = library::blocking(move || worker.state::<Collection>().analyse_with(&id, follow, &hint)).await;
        {
            let state = app.state::<AnalysisQueue>();
            let mut queue = state.0.lock().expect("analysis lock poisoned");
            queue.progress.running.retain(|id| id != &track_id);
            queue.progress.done += 1;
            match &result {
                Ok(found) => info!(
                    "Analysed track {track_id}: {:.2} BPM, {}, grid {}% sure{} {:?}",
                    found.bpm,
                    found.key,
                    found.grid.confidence,
                    if found.grid.dynamic { ", following the tempo" } else { "" },
                    found.grid.issues
                ),
                Err(e) => {
                    let title = app.state::<Collection>().title_of(&track_id).unwrap_or_default();
                    warn!("Couldn't analyse {title} ({track_id}): {e}");
                    queue.progress.failed.push(Failure { track_id: track_id.clone(), title, error: e.to_string() });
                }
            }
        }
        emit(&app);
        let _ = app.emit("collection-changed", app.state::<ArrivalQueue>().waiting());
    }
}
