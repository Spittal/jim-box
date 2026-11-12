//! Files on their way into rekordbox's collection. rekordbox's library can
//! only be changed while rekordbox is closed, so arrivals wait in a saved
//! queue and are taken in at the next chance: when another file arrives, or
//! when the collection is opened.
//!
//! Emits `collection-changed` whenever the collection or the queue changes.

use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tracing::{info, warn};

use crate::analysis;
use crate::library;
use crate::persistence;
use crate::rekordbox::{Arrival, Collection};
use crate::state::SettingsState;

/// How often to check whether rekordbox has closed while files wait.
const RETRY_EVERY: Duration = Duration::from_secs(20);

/// Times a file is tried on the timer before it's left for the user to ask
/// again: one that can't be read, or whose playlist can't be made, never goes in.
const MAX_ATTEMPTS: u32 = 3;

/// The queue's state, for the frontend.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Waiting {
    /// Files waiting to be added to the collection.
    pub count: usize,
    /// Why the last attempt to add them didn't work.
    pub error: Option<String>,
}

pub struct ArrivalQueue {
    waiting: Mutex<Vec<Arrival>>,
    error: Mutex<Option<String>>,
    /// How often each waiting file has been refused, by its path.
    attempts: Mutex<HashMap<PathBuf, u32>>,
    /// Held while the queue is taken in: one attempt at a time.
    flushing: tokio::sync::Mutex<()>,
}

impl ArrivalQueue {
    pub fn load(app: &AppHandle) -> Self {
        Self {
            waiting: Mutex::new(persistence::load_arrivals(app)),
            error: Mutex::new(None),
            attempts: Mutex::default(),
            flushing: tokio::sync::Mutex::default(),
        }
    }

    pub fn waiting(&self) -> Waiting {
        Waiting {
            count: self.waiting.lock().expect("arrivals lock poisoned").len(),
            error: self.error.lock().expect("arrivals lock poisoned").clone(),
        }
    }

    fn push(&self, app: &AppHandle, arrivals: Vec<Arrival>) {
        let mut waiting = self.waiting.lock().expect("arrivals lock poisoned");
        let before = waiting.len();
        for arrival in arrivals {
            if !waiting.contains(&arrival) {
                waiting.push(arrival);
            }
        }
        if waiting.len() != before {
            persistence::save_arrivals(app, &waiting);
        }
    }

    fn snapshot(&self) -> Vec<Arrival> {
        self.waiting.lock().expect("arrivals lock poisoned").clone()
    }

    /// Whether any waiting file is still worth trying unasked.
    fn any_due(&self) -> bool {
        let attempts = self.attempts.lock().expect("arrivals lock poisoned");
        self.waiting
            .lock()
            .expect("arrivals lock poisoned")
            .iter()
            .any(|arrival| attempts.get(&arrival.path).copied().unwrap_or(0) < MAX_ATTEMPTS)
    }

    /// Drops the files an attempt dealt with from the queue, and saves it.
    fn settle(&self, app: &AppHandle, done: &[Arrival]) {
        let mut waiting = self.waiting.lock().expect("arrivals lock poisoned");
        waiting.retain(|arrival| !done.contains(arrival));
        persistence::save_arrivals(app, &waiting);
        let mut attempts = self.attempts.lock().expect("arrivals lock poisoned");
        attempts.retain(|path, _| waiting.iter().any(|arrival| &arrival.path == path));
    }
}

/// Queues files for the collection and tries to take the queue in.
pub async fn arrived(app: &AppHandle, arrivals: Vec<Arrival>) {
    if arrivals.is_empty() {
        return;
    }
    queue(app, arrivals);
    flush(app).await;
}

/// Queues files for the collection, and saves the queue, without taking it
/// in yet: for a run of files that [`flush`] takes in together at the end.
pub fn queue(app: &AppHandle, arrivals: Vec<Arrival>) {
    app.state::<ArrivalQueue>().push(app, arrivals);
}

/// Takes everything waiting into the collection, if rekordbox allows it.
/// What can't go in now stays queued; a file that has gone is dropped.
/// A file stays in the saved queue until it is dealt with, so stopping
/// part-way loses none.
pub async fn flush(app: &AppHandle) {
    let queue = app.state::<ArrivalQueue>();
    let _flushing = queue.flushing.lock().await;
    let batch = queue.snapshot();
    if batch.is_empty() {
        return;
    }
    let before = queue.waiting();
    let (work_app, work) = (app.clone(), batch.clone());
    let result = library::blocking(move || work_app.state::<Collection>().take_in(&work)).await;
    let (error, taken) = match result {
        Ok(outcomes) => {
            let taken = outcomes.iter().filter(|o| o.is_ok()).count();
            info!("Added {taken} of {} files to the rekordbox collection", batch.len());
            analyse_arrivals(app, &batch, &outcomes).await;
            // Keep what failed for a reason that may pass; a missing file won't.
            let mut done = Vec::new();
            let mut error = None;
            for (arrival, outcome) in batch.into_iter().zip(outcomes) {
                match outcome {
                    Err(e) if arrival.path.is_file() => {
                        warn!("Couldn't add {} to the collection: {e}", arrival.path.display());
                        *queue.attempts.lock().expect("arrivals lock poisoned").entry(arrival.path.clone()).or_default() += 1;
                        error.get_or_insert(e);
                    }
                    Err(e) => {
                        warn!("Couldn't add {} to the collection, and it's gone: {e}", arrival.path.display());
                        done.push(arrival);
                    }
                    Ok(_) => done.push(arrival),
                }
            }
            queue.settle(app, &done);
            (error, taken)
        }
        Err(e) => {
            info!("{} files wait to be added to the rekordbox collection: {e}", batch.len());
            (Some(e.to_string()), 0)
        }
    };
    *queue.error.lock().expect("arrivals lock poisoned") = error;
    // Only when there is something new to show: each of these has every list read again.
    let now = queue.waiting();
    if taken > 0 || now != before {
        let _ = app.emit("collection-changed", now);
    }
}

/// Queues what just arrived for analysis: new tracks, and tracks whose audio
/// was replaced by a file of another format, since the old grid and
/// waveforms were made from different audio.
async fn analyse_arrivals(app: &AppHandle, batch: &[Arrival], outcomes: &[Result<String, String>]) {
    if !app.state::<SettingsState>().snapshot().analyse_new_tracks {
        return;
    }
    let (mut new, mut replaced) = (Vec::new(), Vec::new());
    for (arrival, outcome) in batch.iter().zip(outcomes) {
        let Ok(id) = outcome else { continue };
        let reformatted = arrival
            .replaces
            .as_deref()
            .is_some_and(|old| library::extension(old) != library::extension(&arrival.path));
        if reformatted { &mut replaced } else { &mut new }.push(id.clone());
    }
    analysis::enqueue(app, new, false).await;
    analysis::enqueue_replaced(app, replaced).await;
}

/// Takes waiting files in soon after rekordbox closes, for the app's lifetime.
pub async fn retry_when_rekordbox_closes(app: AppHandle) {
    loop {
        tokio::time::sleep(RETRY_EVERY).await;
        if app.state::<ArrivalQueue>().any_due() && !crate::rekordbox::is_running() {
            flush(&app).await;
        }
    }
}
