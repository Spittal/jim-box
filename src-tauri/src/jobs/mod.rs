mod pipeline;
mod ranking;
mod youtube;

pub use pipeline::{cancel, start, start_candidate};
pub use youtube::start as start_youtube;

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::persistence;
use crate::state::{now_ms, Job, JobStatus, TrackMetadata};

/// How many jobs may search/download at once. Soulseek servers penalize
/// search flooding, so extra jobs wait in `Queued`. Jobs waiting to search
/// again don't hold a slot.
const MAX_CONCURRENT_JOBS: usize = 3;

/// Completed jobs kept across launches, newest first.
const MAX_COMPLETED_JOBS: usize = 2000;
/// The newest completed jobs keep every source found for them, to pick
/// another from; older ones keep the one they used. A job's sources are
/// most of what it takes to save.
const MAX_DETAILED_JOBS: usize = 100;
/// Changes are saved together, this long after the first of them.
const SAVE_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

/// What a download was asked for and where its file is: what following a
/// playlist needs of each job.
#[derive(Debug, Clone)]
pub struct Fetch {
    pub request: crate::state::TrackRequest,
    pub output_path: Option<std::path::PathBuf>,
}

/// All download jobs, newest first. Every change is emitted to the frontend as
/// `job-updated`; status changes are also persisted.
pub struct JobStore {
    jobs: Mutex<Vec<Job>>,
    tasks: Mutex<HashMap<Uuid, JoinHandle<()>>>,
    slots: Arc<Semaphore>,
    /// A save is on its way.
    saving: std::sync::atomic::AtomicBool,
}

impl JobStore {
    /// Loads saved jobs. Anything that was mid-flight when the app closed is
    /// marked failed so it can be retried.
    pub fn load(app: &AppHandle) -> Self {
        let mut jobs = persistence::load_jobs(app);
        // Nothing lists finished downloads to clear by hand: cancelled ones
        // go, and only the newest completed ones stay, as the record of
        // where a track's file came from.
        let mut completed = 0;
        jobs.retain(|j| match j.status {
            JobStatus::Cancelled => false,
            JobStatus::Completed => {
                completed += 1;
                completed <= MAX_COMPLETED_JOBS
            }
            _ => true,
        });
        for job in jobs.iter_mut().filter(|j| j.status == JobStatus::Completed).skip(MAX_DETAILED_JOBS) {
            let used = job.active_candidate.filter(|&i| i < job.candidates.len()).map(|i| job.candidates.swap_remove(i));
            job.active_candidate = used.as_ref().map(|_| 0);
            job.candidates = used.into_iter().collect();
            job.searches.clear();
        }
        for job in jobs.iter_mut().filter(|j| !j.status.is_finished()) {
            job.status = JobStatus::Failed;
            job.error = Some("Interrupted when the app closed".into());
            job.progress = None;
            job.transfer_id = None;
            job.retry_at = None;
        }
        Self {
            jobs: Mutex::new(jobs),
            tasks: Mutex::new(HashMap::new()),
            slots: Arc::new(Semaphore::new(MAX_CONCURRENT_JOBS)),
            saving: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn list(&self) -> Vec<Job> {
        self.jobs.lock().expect("jobs lock poisoned").clone()
    }

    /// Looks at the jobs where they are: `list` copies every one, with all its sources.
    pub fn read<R>(&self, look: impl FnOnce(&[Job]) -> R) -> R {
        look(&self.jobs.lock().expect("jobs lock poisoned"))
    }

    pub fn fetches(&self) -> Vec<Fetch> {
        self.read(|jobs| jobs.iter().map(|j| Fetch { request: j.request.clone(), output_path: j.output_path.clone() }).collect())
    }

    /// Notes how far a transfer has got. Told to the frontend as
    /// `job-progress`, which carries only that: these come every second.
    pub fn set_progress(&self, app: &AppHandle, id: Uuid, progress: Option<crate::state::TransferProgress>) -> AppResult<()> {
        {
            let mut jobs = self.jobs.lock().expect("jobs lock poisoned");
            let job = jobs.iter_mut().find(|j| j.id == id).ok_or(AppError::JobNotFound(id))?;
            if job.progress == progress {
                return Ok(());
            }
            job.progress = progress.clone();
        }
        let _ = app.emit("job-progress", serde_json::json!({ "id": id, "progress": progress }));
        Ok(())
    }

    pub fn get(&self, id: Uuid) -> AppResult<Job> {
        self.jobs
            .lock()
            .expect("jobs lock poisoned")
            .iter()
            .find(|j| j.id == id)
            .cloned()
            .ok_or(AppError::JobNotFound(id))
    }

    pub fn insert(&self, app: &AppHandle, job: Job) {
        self.jobs.lock().expect("jobs lock poisoned").insert(0, job.clone());
        let _ = app.emit("job-updated", &job);
        self.persist(app);
    }

    /// Mutates a job, stamps `updated_at` and notifies the frontend.
    pub fn update<R>(&self, app: &AppHandle, id: Uuid, f: impl FnOnce(&mut Job) -> R) -> AppResult<R> {
        let (result, job, status_changed) = {
            let mut jobs = self.jobs.lock().expect("jobs lock poisoned");
            let job = jobs.iter_mut().find(|j| j.id == id).ok_or(AppError::JobNotFound(id))?;
            let before = job.status;
            let result = f(job);
            job.updated_at = now_ms();
            (result, job.clone(), before != job.status)
        };
        let _ = app.emit("job-updated", &job);
        // Progress ticks are frequent; only persist meaningful transitions.
        if status_changed {
            self.persist(app);
        }
        Ok(result)
    }

    pub fn remove(&self, app: &AppHandle, id: Uuid) -> AppResult<()> {
        if self.is_running(id) {
            return Err(AppError::Validation("Cancel the download before removing it".into()));
        }
        self.jobs.lock().expect("jobs lock poisoned").retain(|j| j.id != id);
        let _ = app.emit("job-removed", id);
        self.persist(app);
        Ok(())
    }

    pub fn has_running(&self) -> bool {
        self.tasks
            .lock()
            .expect("tasks lock poisoned")
            .values()
            .any(|task| !task.inner().is_finished())
    }

    /// Follows downloads' files after a run of them moved, each from the
    /// first path to the second; saved once.
    pub fn paths_moved(&self, app: &AppHandle, moved: &[(std::path::PathBuf, std::path::PathBuf)]) {
        let to: HashMap<&Path, &Path> = moved.iter().map(|(from, to)| (from.as_path(), to.as_path())).collect();
        let changed: Vec<Job> = {
            let mut jobs = self.jobs.lock().expect("jobs lock poisoned");
            jobs.iter_mut()
                .filter_map(|job| {
                    let new = *to.get(job.output_path.as_deref()?)?;
                    job.output_path = Some(new.to_path_buf());
                    Some(job.clone())
                })
                .collect()
        };
        if changed.is_empty() {
            return;
        }
        for job in &changed {
            let _ = app.emit("job-updated", job);
        }
        self.persist(app);
    }

    /// The finished download that produced `path`, if any.
    pub fn find_by_output(&self, path: &Path) -> Option<Job> {
        self.read(|jobs| jobs.iter().find(|j| j.status == JobStatus::Completed && j.output_path.as_deref() == Some(path)).cloned())
    }

    /// Follows a download's file after the library re-tags and moves it.
    pub fn file_moved(&self, app: &AppHandle, from: &Path, to: &Path, metadata: &TrackMetadata) {
        let moved: Vec<Uuid> =
            self.read(|jobs| jobs.iter().filter(|j| j.output_path.as_deref() == Some(from)).map(|j| j.id).collect());
        if moved.is_empty() {
            return;
        }
        for id in moved {
            let _ = self.update(app, id, |j| {
                j.output_path = Some(to.to_path_buf());
                j.metadata = Some(metadata.clone());
            });
        }
        self.persist(app);
    }

    pub fn is_running(&self, id: Uuid) -> bool {
        self.tasks
            .lock()
            .expect("tasks lock poisoned")
            .get(&id)
            .is_some_and(|task| !task.inner().is_finished())
    }

    /// Waits for one of the concurrency slots; the job holds it while it
    /// searches or downloads.
    async fn acquire_slot(&self) -> OwnedSemaphorePermit {
        self.slots.clone().acquire_owned().await.expect("job semaphore closed")
    }

    /// Runs `work` for a job. The work takes a concurrency slot itself.
    fn spawn<F>(&self, id: Uuid, work: F) -> AppResult<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut tasks = self.tasks.lock().expect("tasks lock poisoned");
        if tasks.get(&id).is_some_and(|task| !task.inner().is_finished()) {
            return Err(AppError::Validation("This download is already running".into()));
        }
        tasks.retain(|_, task| !task.inner().is_finished());
        let task = tauri::async_runtime::spawn(work);
        tasks.insert(id, task);
        Ok(())
    }

    /// Stops a job's task. Returns whether one was running.
    fn abort(&self, id: Uuid) -> bool {
        match self.tasks.lock().expect("tasks lock poisoned").remove(&id) {
            Some(task) => {
                task.abort();
                true
            }
            None => false,
        }
    }

    /// Saves the jobs soon, off this thread: a run of changes is one save.
    fn persist(&self, app: &AppHandle) {
        use std::sync::atomic::Ordering;
        if self.saving.swap(true, Ordering::SeqCst) {
            return;
        }
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(SAVE_DELAY).await;
            let _ = tauri::async_runtime::spawn_blocking(move || tauri::Manager::state::<JobStore>(&app).save_now(&app)).await;
        });
    }

    /// Saves the jobs as they are now: what `persist` comes to, and the last thing done at exit.
    pub fn save_now(&self, app: &AppHandle) {
        // Cleared first: a change made while this writes asks for another save.
        self.saving.store(false, std::sync::atomic::Ordering::SeqCst);
        self.read(|jobs| persistence::save_jobs(app, jobs));
    }
}
