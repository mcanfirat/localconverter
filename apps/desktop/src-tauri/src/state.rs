//! Job registry and scheduler.
//!
//! Holds every job this session knows about, the cancellation token for each
//! running one, and the permit that bounds how many run at once. The UI process
//! never runs conversion work: [`spawn`] hands it to a Tokio worker.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use localconvert_core::{
    runner, ConversionError, ConversionErrorCode, ConversionJob, JobProgress, JobStatus,
    JobWorkspace,
};
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Single event channel: the whole job is re-sent whenever anything about it
/// changes, and the frontend replaces its copy by id. One event type instead of
/// separate progress/status/complete channels, so the UI cannot end up showing
/// a stale status next to fresh progress.
pub const JOB_UPDATED_EVENT: &str = "job-updated";

/// Phase 0 runs one job at a time. Per-category limits (heavy media 1, images
/// scaled to CPU) arrive with the engines that need them.
const MAX_CONCURRENT_JOBS: usize = 1;

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

pub struct Inner {
    temp_root: PathBuf,
    jobs: Mutex<HashMap<Uuid, ConversionJob>>,
    cancels: Mutex<HashMap<Uuid, CancellationToken>>,
    permits: Arc<Semaphore>,
}

impl AppState {
    #[must_use]
    pub fn new(temp_root: PathBuf) -> Self {
        Self(Arc::new(Inner {
            temp_root,
            jobs: Mutex::new(HashMap::new()),
            cancels: Mutex::new(HashMap::new()),
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT_JOBS)),
        }))
    }

    #[must_use]
    pub fn temp_root(&self) -> &Path {
        &self.0.temp_root
    }

    /// Jobs, newest first.
    #[must_use]
    pub fn jobs(&self) -> Vec<ConversionJob> {
        let mut jobs: Vec<ConversionJob> = self
            .0
            .jobs
            .lock()
            .map(|guard| guard.values().cloned().collect())
            .unwrap_or_default();
        jobs.sort_by_key(|job| std::cmp::Reverse(job.created_at));
        jobs
    }

    #[must_use]
    pub fn job(&self, id: Uuid) -> Option<ConversionJob> {
        self.0
            .jobs
            .lock()
            .ok()
            .and_then(|guard| guard.get(&id).cloned())
    }

    /// Ids of jobs that have not reached a terminal status. Used by startup
    /// cleanup so a running job's workspace is never swept.
    #[must_use]
    pub fn active_job_ids(&self) -> HashSet<Uuid> {
        self.0
            .jobs
            .lock()
            .map(|guard| {
                guard
                    .values()
                    .filter(|job| !job.status.is_terminal())
                    .map(|job| job.id)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn remove_terminal_jobs(&self) {
        if let Ok(mut guard) = self.0.jobs.lock() {
            guard.retain(|_, job| !job.status.is_terminal());
        }
    }

    pub fn cancel(&self, id: Uuid) -> localconvert_core::Result<()> {
        let token = self
            .0
            .cancels
            .lock()
            .ok()
            .and_then(|guard| guard.get(&id).cloned());
        match token {
            Some(token) => {
                token.cancel();
                Ok(())
            }
            None => Err(ConversionError::new(
                ConversionErrorCode::InvalidInput,
                "no running job with that id",
            )
            .with_message_key("error.job.notRunning")),
        }
    }

    fn store(&self, job: &ConversionJob) {
        if let Ok(mut guard) = self.0.jobs.lock() {
            guard.insert(job.id, job.clone());
        }
    }

    fn emit<R: Runtime>(&self, app: &AppHandle<R>, job: &ConversionJob) {
        self.store(job);
        if let Err(err) = app.emit(JOB_UPDATED_EVENT, job) {
            tracing::warn!(job_id = %job.id, error = %err, "failed to emit job update");
        }
    }

    /// Registers a queued job and starts it on a worker task.
    pub fn spawn<R: Runtime>(&self, app: &AppHandle<R>, job: ConversionJob) -> ConversionJob {
        let cancel = CancellationToken::new();
        if let Ok(mut guard) = self.0.cancels.lock() {
            guard.insert(job.id, cancel.clone());
        }
        self.emit(app, &job);

        let state = self.clone();
        let app = app.clone();
        let permits = Arc::clone(&self.0.permits);
        let queued = job.clone();

        tauri::async_runtime::spawn(async move {
            let mut job = queued;
            // Bounds concurrency; a cancel while queued is honoured immediately.
            let _permit = tokio::select! {
                permit = permits.acquire_owned() => permit,
                () = cancel.cancelled() => {
                    finish(&state, &app, &mut job, Err(ConversionError::cancelled()));
                    return;
                }
            };

            let outcome = drive(&state, &app, &mut job, &cancel).await;
            finish(&state, &app, &mut job, outcome);
        });

        job
    }
}

/// Walks the job through the state machine. Every `?` here lands in
/// [`finish`], which is the single place a job becomes Failed or Cancelled.
async fn drive<R: Runtime>(
    state: &AppState,
    app: &AppHandle<R>,
    job: &mut ConversionJob,
    cancel: &CancellationToken,
) -> localconvert_core::Result<localconvert_core::JobResult> {
    let started = std::time::Instant::now();

    job.transition_to(JobStatus::Preparing)?;
    job.set_progress(JobProgress::indeterminate(
        localconvert_core::ProgressStage::Preparing,
        "progress.preparing",
    ));
    state.emit(app, job);

    let workspace = JobWorkspace::create(state.temp_root(), job.id)?;

    let progress_app = app.clone();
    let progress_state = state.clone();
    let ctx = runner::JobContext::new(
        job.id,
        workspace,
        cancel.clone(),
        Arc::new(move |job_id, progress| {
            // Merge progress into the stored snapshot, then re-emit the job.
            if let Some(mut snapshot) = progress_state.job(job_id) {
                snapshot.set_progress(progress);
                progress_state.emit(&progress_app, &snapshot);
            }
        }),
    );

    job.transition_to(JobStatus::Running)?;
    state.emit(app, job);
    let execution = runner::execute(job, &ctx).await?;

    job.transition_to(JobStatus::Validating)?;
    state.emit(app, job);
    let reports = runner::validate(job, &ctx, &execution).await?;

    // Committing is a real stage — moving a large file across volumes is not
    // instant — so the user sees it rather than a stalled validation bar.
    ctx.report(JobProgress::indeterminate(
        localconvert_core::ProgressStage::Finalizing,
        "progress.finalizing",
    ));

    let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    runner::commit(job, execution, reports, elapsed)
}

fn finish<R: Runtime>(
    state: &AppState,
    app: &AppHandle<R>,
    job: &mut ConversionJob,
    outcome: localconvert_core::Result<localconvert_core::JobResult>,
) {
    let transition = match outcome {
        Ok(result) => job.complete(result),
        Err(err) => {
            tracing::warn!(job_id = %job.id, code = ?err.code, "job failed");
            job.fail(err)
        }
    };
    if let Err(err) = transition {
        tracing::error!(job_id = %job.id, error = %err, "job ended in an illegal state");
    }

    if let Ok(mut guard) = state.0.cancels.lock() {
        guard.remove(&job.id);
    }
    state.emit(app, job);
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use localconvert_core::OverwritePolicy;

    fn job() -> ConversionJob {
        ConversionJob::new(
            localconvert_core::SELFTEST_OPERATION_ID,
            Vec::new(),
            "/tmp",
            OverwritePolicy::Fail,
            serde_json::Value::Null,
        )
    }

    #[test]
    fn active_ids_exclude_terminal_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().to_path_buf());

        let running = job();
        let mut done = job();
        done.transition_to(JobStatus::Preparing).unwrap();
        done.fail(ConversionError::internal("boom")).unwrap();

        state.store(&running);
        state.store(&done);

        let active = state.active_job_ids();
        assert!(active.contains(&running.id));
        assert!(!active.contains(&done.id));
    }

    #[test]
    fn cancelling_an_unknown_job_is_an_error_not_a_silent_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().to_path_buf());
        assert!(state.cancel(Uuid::new_v4()).is_err());
    }

    #[test]
    fn clearing_completed_jobs_keeps_the_live_ones() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().to_path_buf());

        let running = job();
        let mut done = job();
        done.transition_to(JobStatus::Preparing).unwrap();
        done.fail(ConversionError::cancelled()).unwrap();
        state.store(&running);
        state.store(&done);

        state.remove_terminal_jobs();
        let remaining = state.jobs();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, running.id);
    }

    #[test]
    fn jobs_are_listed_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().to_path_buf());

        let mut older = job();
        older.created_at = chrono::Utc::now() - chrono::Duration::seconds(60);
        let newer = job();
        state.store(&older);
        state.store(&newer);

        assert_eq!(state.jobs()[0].id, newer.id);
    }
}
