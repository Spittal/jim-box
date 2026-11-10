//! Files on their way into rekordbox's collection. rekordbox's library can
//! only be changed while rekordbox is closed, so arrivals wait in a saved
//! queue and are taken in at the next chance: when another file arrives, or
//! when the collection is opened.
//!
//! Emits `collection-changed` whenever the collection or the queue changes.

use serde::Serialize;
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

/// The queue's state, for the frontend.
#[derive(Debug, Clone, Default, Serialize)]
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
}

impl ArrivalQueue {
    pub fn load(app: &AppHandle) -> Self {
        Self { waiting: Mutex::new(persistence::load_arrivals(app)), error: Mutex::new(None) }
    }

    pub fn waiting(&self) -> Waiting {
        Waiting {
            count: self.waiting.lock().expect("arrivals lock poisoned").len(),
            error: self.error.lock().expect("arrivals lock poisoned").clone(),
        }
    }

    fn push(&self, app: &AppHandle, arrivals: Vec<Arrival>) {
        let mut waiting = self.waiting.lock().expect("arrivals lock poisoned");
        for arrival in arrivals {
            if !waiting.contains(&arrival) {
                waiting.push(arrival);
            }
        }
        persistence::save_arrivals(app, &waiting);
    }

    fn is_empty(&self) -> bool {
        self.waiting.lock().expect("arrivals lock poisoned").is_empty()
    }

    fn take(&self) -> Vec<Arrival> {
        std::mem::take(&mut *self.waiting.lock().expect("arrivals lock poisoned"))
    }
}

/// Queues files for the collection and tries to take the queue in.
pub async fn arrived(app: &AppHandle, arrivals: Vec<Arrival>) {
    if arrivals.is_empty() {
        return;
    }
    app.state::<ArrivalQueue>().push(app, arrivals);
    flush(app).await;
}

/// Takes everything waiting into the collection, if rekordbox allows it.
/// What can't go in now stays queued; a file that has gone is dropped.
pub async fn flush(app: &AppHandle) {
    let queue = app.state::<ArrivalQueue>();
    let batch = queue.take();
    if batch.is_empty() {
        return;
    }
    let (work_app, work) = (app.clone(), batch.clone());
    let result = library::blocking(move || work_app.state::<Collection>().take_in(&work)).await;
    let error = match result {
        Ok(outcomes) => {
            let taken = outcomes.iter().filter(|o| o.is_ok()).count();
            info!("Added {taken} of {} files to the rekordbox collection", batch.len());
            for (arrival, outcome) in batch.iter().zip(&outcomes) {
                if let Err(e) = outcome {
                    warn!("Couldn't add {} to the collection: {e}", arrival.path.display());
                }
            }
            analyse_arrivals(app, &batch, &outcomes).await;
            // Keep what failed for a reason that may pass; a missing file won't.
            let retry: Vec<Arrival> = batch
                .into_iter()
                .zip(&outcomes)
                .filter(|(arrival, outcome)| outcome.is_err() && arrival.path.is_file())
                .map(|(arrival, _)| arrival)
                .collect();
            let error = outcomes.into_iter().find_map(Result::err).filter(|_| !retry.is_empty());
            queue.push(app, retry);
            error
        }
        Err(e) => {
            info!("{} files wait to be added to the rekordbox collection: {e}", batch.len());
            queue.push(app, batch);
            Some(e.to_string())
        }
    };
    *queue.error.lock().expect("arrivals lock poisoned") = error;
    // `push` saved only when something was put back.
    persistence::save_arrivals(app, &queue.waiting.lock().expect("arrivals lock poisoned"));
    let _ = app.emit("collection-changed", queue.waiting());
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
    analysis::enqueue(app, replaced, true).await;
}

/// Takes waiting files in soon after rekordbox closes, for the app's lifetime.
pub async fn retry_when_rekordbox_closes(app: AppHandle) {
    loop {
        tokio::time::sleep(RETRY_EVERY).await;
        if !app.state::<ArrivalQueue>().is_empty() && !rbl_db::is_rekordbox_running() {
            flush(&app).await;
        }
    }
}
