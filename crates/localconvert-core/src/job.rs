//! Job model and state machine.
//!
//! The state machine is the single place that decides whether a job may move
//! from one status to the next. Nothing else mutates `ConversionJob::status`,
//! which is what makes "a job that reported Completed really did pass
//! validation" an invariant rather than a convention.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::error::{ConversionError, ConversionErrorCode, Result};
use crate::file::FileDescriptor;
use crate::paths::OverwritePolicy;
use crate::validation::ValidationReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum JobStatus {
    Queued,
    Preparing,
    Running,
    Validating,
    Completed,
    CompletedWithWarnings,
    Failed,
    Cancelled,
}

impl JobStatus {
    /// Terminal statuses accept no further transitions.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::CompletedWithWarnings | Self::Failed | Self::Cancelled
        )
    }

    /// `true` when the job finished without producing a usable output.
    #[must_use]
    pub fn is_failure(self) -> bool {
        matches!(self, Self::Failed | Self::Cancelled)
    }

    /// The allowed transition table.
    ///
    /// Note what is *absent*: nothing reaches `Completed` except from
    /// `Validating`. A conversion cannot be declared successful without having
    /// gone through output validation first.
    #[must_use]
    pub fn can_transition_to(self, next: Self) -> bool {
        use JobStatus::{
            Cancelled, Completed, CompletedWithWarnings, Failed, Preparing, Queued, Running,
            Validating,
        };
        match (self, next) {
            (Queued, Preparing)
            | (Preparing, Running)
            | (Running, Validating)
            | (Validating, Completed | CompletedWithWarnings) => true,
            // Failure and cancellation may interrupt any non-terminal stage.
            (Queued | Preparing | Running | Validating, Failed | Cancelled) => true,
            _ => false,
        }
    }
}

/// Coarse stage shown next to the progress bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ProgressStage {
    Queued,
    Preparing,
    Running,
    Validating,
    Finalizing,
    Done,
}

/// Honest progress.
///
/// `percent` is `None` whenever the underlying engine cannot report real
/// progress; the UI then shows an indeterminate bar and the stage label. There
/// is deliberately no synthetic "estimated" percentage anywhere in the codebase.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobProgress {
    pub stage: ProgressStage,
    #[ts(type = "number | null")]
    pub completed_units: Option<u64>,
    #[ts(type = "number | null")]
    pub total_units: Option<u64>,
    pub percent: Option<f32>,
    pub message_key: String,
}

impl JobProgress {
    #[must_use]
    pub fn indeterminate(stage: ProgressStage, message_key: impl Into<String>) -> Self {
        Self {
            stage,
            completed_units: None,
            total_units: None,
            percent: None,
            message_key: message_key.into(),
        }
    }

    /// Derives the percentage from real counted units. A zero total stays
    /// indeterminate rather than reporting a made-up 0% or 100%.
    #[must_use]
    pub fn counted(
        stage: ProgressStage,
        completed: u64,
        total: u64,
        message_key: impl Into<String>,
    ) -> Self {
        let percent = if total == 0 {
            None
        } else {
            let ratio = (completed.min(total) as f64) / (total as f64);
            Some((ratio * 100.0) as f32)
        };
        Self {
            stage,
            completed_units: Some(completed),
            total_units: Some(total),
            percent,
            message_key: message_key.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobWarning {
    /// i18n key, e.g. `warning.image.transparencyLost`.
    pub message_key: String,
    pub detail: Option<String>,
}

impl JobWarning {
    #[must_use]
    pub fn new(message_key: impl Into<String>) -> Self {
        Self {
            message_key: message_key.into(),
            detail: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OutputDescriptor {
    pub path: String,
    pub display_name: String,
    #[ts(type = "number")]
    pub size_bytes: u64,
    pub format: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobResult {
    pub outputs: Vec<OutputDescriptor>,
    pub warnings: Vec<JobWarning>,
    pub validation_reports: Vec<ValidationReport>,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
    #[ts(type = "number")]
    pub input_total_bytes: u64,
    #[ts(type = "number")]
    pub output_total_bytes: u64,
    pub size_change_percent: f64,
}

impl JobResult {
    /// Negative means the output got smaller. A zero-byte input reports no
    /// change rather than dividing by zero.
    #[must_use]
    pub fn size_change_percent(input_total_bytes: u64, output_total_bytes: u64) -> f64 {
        if input_total_bytes == 0 {
            return 0.0;
        }
        let delta = output_total_bytes as f64 - input_total_bytes as f64;
        delta / (input_total_bytes as f64) * 100.0
    }

    /// `true` when "compression" made the file bigger — the spec requires the
    /// UI to say so instead of quietly reporting a negative saving.
    #[must_use]
    pub fn output_grew(&self) -> bool {
        self.output_total_bytes > self.input_total_bytes
    }
}

/// What the frontend sends to start work. Deliberately narrow: no command
/// strings, no engine arguments, no arbitrary paths beyond the selected inputs
/// and one destination directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StartJobRequest {
    pub operation_id: String,
    pub input_paths: Vec<String>,
    pub output_directory: String,
    pub overwrite_policy: OverwritePolicy,
    /// Operation-specific options, validated against the operation's schema by
    /// the owning plugin before execution.
    pub options: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConversionJob {
    pub id: Uuid,
    pub operation_id: String,
    pub input_files: Vec<FileDescriptor>,
    pub output_directory: String,
    pub overwrite_policy: OverwritePolicy,
    pub options: serde_json::Value,
    pub status: JobStatus,
    pub progress: JobProgress,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub result: Option<JobResult>,
    pub error: Option<ConversionError>,
}

impl ConversionJob {
    #[must_use]
    pub fn new(
        operation_id: impl Into<String>,
        input_files: Vec<FileDescriptor>,
        output_directory: impl Into<String>,
        overwrite_policy: OverwritePolicy,
        options: serde_json::Value,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            operation_id: operation_id.into(),
            input_files,
            output_directory: output_directory.into(),
            overwrite_policy,
            options,
            status: JobStatus::Queued,
            progress: JobProgress::indeterminate(ProgressStage::Queued, "progress.queued"),
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            result: None,
            error: None,
        }
    }

    /// The only way to change a job's status.
    pub fn transition_to(&mut self, next: JobStatus) -> Result<()> {
        if !self.status.can_transition_to(next) {
            return Err(ConversionError::new(
                ConversionErrorCode::InternalError,
                format!("illegal job transition {:?} -> {:?}", self.status, next),
            ));
        }
        if next == JobStatus::Preparing && self.started_at.is_none() {
            self.started_at = Some(Utc::now());
        }
        if next.is_terminal() {
            self.completed_at = Some(Utc::now());
        }
        self.status = next;
        Ok(())
    }

    /// Attaches the outcome and moves to the matching terminal status.
    /// Warnings decide between `Completed` and `CompletedWithWarnings`.
    pub fn complete(&mut self, result: JobResult) -> Result<()> {
        let next = if result.warnings.is_empty() {
            JobStatus::Completed
        } else {
            JobStatus::CompletedWithWarnings
        };
        self.transition_to(next)?;
        self.progress = JobProgress::counted(ProgressStage::Done, 1, 1, "progress.done");
        self.result = Some(result);
        Ok(())
    }

    pub fn fail(&mut self, error: ConversionError) -> Result<()> {
        let next = if error.code == ConversionErrorCode::Cancelled {
            JobStatus::Cancelled
        } else {
            JobStatus::Failed
        };
        self.transition_to(next)?;
        self.error = Some(error);
        Ok(())
    }

    pub fn set_progress(&mut self, progress: JobProgress) {
        self.progress = progress;
    }

    #[must_use]
    pub fn elapsed_ms(&self) -> Option<u64> {
        let started = self.started_at?;
        let ended = self.completed_at.unwrap_or_else(Utc::now);
        u64::try_from((ended - started).num_milliseconds()).ok()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    const ALL: [JobStatus; 8] = [
        JobStatus::Queued,
        JobStatus::Preparing,
        JobStatus::Running,
        JobStatus::Validating,
        JobStatus::Completed,
        JobStatus::CompletedWithWarnings,
        JobStatus::Failed,
        JobStatus::Cancelled,
    ];

    fn job() -> ConversionJob {
        ConversionJob::new(
            "diagnostics.selftest",
            Vec::new(),
            "/tmp/out",
            OverwritePolicy::Fail,
            serde_json::Value::Null,
        )
    }

    #[test]
    fn happy_path_walks_every_stage() {
        let mut j = job();
        assert_eq!(j.status, JobStatus::Queued);
        for next in [
            JobStatus::Preparing,
            JobStatus::Running,
            JobStatus::Validating,
            JobStatus::Completed,
        ] {
            j.transition_to(next).unwrap();
            assert_eq!(j.status, next);
        }
    }

    #[test]
    fn terminal_states_accept_nothing() {
        for terminal in ALL.iter().copied().filter(|s| s.is_terminal()) {
            for next in ALL {
                assert!(
                    !terminal.can_transition_to(next),
                    "{terminal:?} must not transition to {next:?}"
                );
            }
        }
    }

    #[test]
    fn completion_requires_passing_through_validation() {
        for from in ALL {
            for done in [JobStatus::Completed, JobStatus::CompletedWithWarnings] {
                assert_eq!(
                    from.can_transition_to(done),
                    from == JobStatus::Validating,
                    "{from:?} -> {done:?}"
                );
            }
        }
    }

    #[test]
    fn stages_cannot_be_skipped() {
        let mut j = job();
        let err = j.transition_to(JobStatus::Running).unwrap_err();
        assert_eq!(err.code, ConversionErrorCode::InternalError);
        assert_eq!(
            j.status,
            JobStatus::Queued,
            "a rejected transition changes nothing"
        );
    }

    #[test]
    fn failure_and_cancellation_interrupt_any_live_stage() {
        for from in ALL.iter().copied().filter(|s| !s.is_terminal()) {
            assert!(from.can_transition_to(JobStatus::Failed));
            assert!(from.can_transition_to(JobStatus::Cancelled));
        }
    }

    #[test]
    fn timestamps_are_recorded_at_the_right_edges() {
        let mut j = job();
        assert!(j.started_at.is_none() && j.completed_at.is_none());

        j.transition_to(JobStatus::Preparing).unwrap();
        assert!(j.started_at.is_some());
        assert!(j.completed_at.is_none());

        j.transition_to(JobStatus::Failed).unwrap();
        assert!(j.completed_at.is_some());
        assert!(j.elapsed_ms().is_some());
    }

    #[test]
    fn warnings_route_to_completed_with_warnings() {
        let mut j = job();
        for next in [
            JobStatus::Preparing,
            JobStatus::Running,
            JobStatus::Validating,
        ] {
            j.transition_to(next).unwrap();
        }
        j.complete(JobResult {
            outputs: Vec::new(),
            warnings: vec![JobWarning::new("warning.destination.overwritten")],
            validation_reports: Vec::new(),
            elapsed_ms: 1,
            input_total_bytes: 10,
            output_total_bytes: 5,
            size_change_percent: -50.0,
        })
        .unwrap();
        assert_eq!(j.status, JobStatus::CompletedWithWarnings);
    }

    #[test]
    fn cancellation_error_lands_in_cancelled_not_failed() {
        let mut j = job();
        j.transition_to(JobStatus::Preparing).unwrap();
        j.fail(ConversionError::cancelled()).unwrap();
        assert_eq!(j.status, JobStatus::Cancelled);

        let mut k = job();
        k.transition_to(JobStatus::Preparing).unwrap();
        k.fail(ConversionError::internal("boom")).unwrap();
        assert_eq!(k.status, JobStatus::Failed);
    }

    #[test]
    fn progress_stays_indeterminate_when_the_total_is_unknown() {
        let p = JobProgress::indeterminate(ProgressStage::Running, "progress.running");
        assert!(p.percent.is_none());

        let zero = JobProgress::counted(ProgressStage::Running, 0, 0, "progress.running");
        assert!(
            zero.percent.is_none(),
            "0/0 must not report a fabricated percentage"
        );
    }

    #[test]
    fn counted_progress_is_clamped_and_correct() {
        let half = JobProgress::counted(ProgressStage::Running, 5, 10, "k");
        assert_eq!(half.percent, Some(50.0));

        let over = JobProgress::counted(ProgressStage::Running, 20, 10, "k");
        assert_eq!(over.percent, Some(100.0), "percent must never exceed 100");
    }

    #[test]
    fn size_change_reports_growth_and_shrinkage_honestly() {
        assert_eq!(JobResult::size_change_percent(1000, 500), -50.0);
        assert_eq!(JobResult::size_change_percent(1000, 1500), 50.0);
        assert_eq!(
            JobResult::size_change_percent(0, 100),
            0.0,
            "an empty input must not divide by zero"
        );
    }

    #[test]
    fn output_grew_is_detected() {
        let bigger = JobResult {
            outputs: Vec::new(),
            warnings: Vec::new(),
            validation_reports: Vec::new(),
            elapsed_ms: 0,
            input_total_bytes: 100,
            output_total_bytes: 140,
            size_change_percent: 40.0,
        };
        assert!(bigger.output_grew());
    }
}
