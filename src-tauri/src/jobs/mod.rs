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

/// All download jobs, newest first. Every change is emitted to the frontend as
/// `job-updated`; status changes are also persisted.
pub struct JobStore {
    jobs: Mutex<Vec<Job>>,
    tasks: Mutex<HashMap<Uuid, JoinHandle<()>>>,
    slots: Arc<Semaphore>,
}

impl JobStore {
    /// Loads saved jobs. Anything that was mid-flight when the app closed is
    /// marked failed so it can be retried.
    pub fn load(app: &AppHandle) -> Self {
        let mut jobs = persistence::load_jobs(app);
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
        }
    }

    pub fn list(&self) -> Vec<Job> {
        self.jobs.lock().expect("jobs lock poisoned").clone()
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

    /// Removes every finished job except failures that can still be retried.
    pub fn clear_finished(&self, app: &AppHandle) {
        let removed: Vec<Uuid> = {
            let mut jobs = self.jobs.lock().expect("jobs lock poisoned");
            let removed = jobs
                .iter()
                .filter(|j| matches!(j.status, JobStatus::Completed | JobStatus::Cancelled))
                .map(|j| j.id)
                .collect();
            jobs.retain(|j| !matches!(j.status, JobStatus::Completed | JobStatus::Cancelled));
            removed
        };
        for id in removed {
            let _ = app.emit("job-removed", id);
        }
        self.persist(app);
    }

    pub fn has_running(&self) -> bool {
        self.tasks
            .lock()
            .expect("tasks lock poisoned")
            .values()
            .any(|task| !task.inner().is_finished())
    }

    /// Points finished jobs' output paths at the library's new location.
    pub fn rebase_output_paths(&self, app: &AppHandle, old_root: &Path, new_root: &Path) {
        {
            let mut jobs = self.jobs.lock().expect("jobs lock poisoned");
            for job in jobs.iter_mut() {
                if let Some(rest) = job.output_path.as_ref().and_then(|p| p.strip_prefix(old_root).ok()) {
                    job.output_path = Some(new_root.join(rest));
                }
            }
        }
        for job in self.list() {
            let _ = app.emit("job-updated", &job);
        }
        self.persist(app);
    }

    /// The finished download that produced `path`, if any.
    pub fn find_by_output(&self, path: &Path) -> Option<Job> {
        self.list()
            .into_iter()
            .find(|j| j.status == JobStatus::Completed && j.output_path.as_deref() == Some(path))
    }

    /// Follows a download's file after the library re-tags and moves it.
    pub fn file_moved(&self, app: &AppHandle, from: &Path, to: &Path, metadata: &TrackMetadata) {
        let moved: Vec<Uuid> = self
            .list()
            .into_iter()
            .filter(|j| j.output_path.as_deref() == Some(from))
            .map(|j| j.id)
            .collect();
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

    fn persist(&self, app: &AppHandle) {
        persistence::save_jobs(app, &self.list());
    }
}
