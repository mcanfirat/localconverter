//! Job execution pipeline.
//!
//! A job runs in three separate stages, and they are separate on purpose:
//!
//! ```text
//! execute()  -> writes staged output inside the job's temp workspace
//! validate() -> reads that output back and reports on it
//! commit()   -> only now moves it to the user's destination
//! ```
//!
//! Plugins own [`execute`] and [`validate`]; [`commit`] is shared. That is what
//! makes "nothing reaches the user's folder until it has been validated" a
//! property of the pipeline rather than a promise each plugin has to keep.
//!
//! Nothing here knows about Tauri. The caller supplies a progress sink and
//! drives [`crate::job::JobStatus`] transitions, so the same pipeline backs the
//! desktop app, the CLI and the integration tests.

use std::path::PathBuf;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::archive;
use crate::diagnostics;
use crate::error::{ConversionError, ConversionErrorCode, Result};
use crate::image_engine;
use crate::job::{ConversionJob, JobProgress, JobResult, JobWarning, OutputDescriptor};
use crate::media;
use crate::operation::{
    ARCHIVE_CREATE_OPERATION_ID, ARCHIVE_EXTRACT_OPERATION_ID, IMAGE_CONVERT_OPERATION_ID,
    MEDIA_CONVERT_OPERATION_ID, PDF_FROM_IMAGES_OPERATION_ID, PDF_MERGE_OPERATION_ID,
    PDF_PAGES_OPERATION_ID, PDF_REMOVE_METADATA_OPERATION_ID, PDF_SPLIT_OPERATION_ID,
    SELFTEST_OPERATION_ID, SPREADSHEET_CONVERT_OPERATION_ID,
};
use crate::paths;
use crate::pdf;
use crate::spreadsheet;
use crate::validation::ValidationReport;
use crate::workspace::JobWorkspace;

/// Called on every progress update. The desktop app forwards these to the
/// window as events; the CLI will render a bar.
pub type ProgressSink = Arc<dyn Fn(Uuid, JobProgress) + Send + Sync>;

pub struct JobContext {
    pub job_id: Uuid,
    pub workspace: JobWorkspace,
    pub cancel: CancellationToken,
    progress: ProgressSink,
}

impl JobContext {
    #[must_use]
    pub fn new(
        job_id: Uuid,
        workspace: JobWorkspace,
        cancel: CancellationToken,
        progress: ProgressSink,
    ) -> Self {
        Self {
            job_id,
            workspace,
            cancel,
            progress,
        }
    }

    pub fn report(&self, progress: JobProgress) {
        (self.progress)(self.job_id, progress);
    }

    /// Call at every interruptible point in a conversion loop.
    pub fn check_cancelled(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            return Err(ConversionError::cancelled());
        }
        Ok(())
    }
}

/// One finished-but-not-yet-committed file, living in the job's temp workspace.
#[derive(Debug, Clone)]
pub struct StagedOutput {
    pub staged_path: PathBuf,
    /// Name this file will have in the user's destination directory.
    pub file_name: String,
    pub format: String,
    pub size_bytes: u64,
}

/// What [`execute`] hands to [`validate`] and then [`commit`].
#[derive(Debug, Clone, Default)]
pub struct ExecutionOutput {
    pub outputs: Vec<StagedOutput>,
    pub warnings: Vec<JobWarning>,
    pub input_total_bytes: u64,
    /// Set when the output is *supposed* to be bigger — e.g. writing a lossless
    /// PNG from a lossy JPEG. Without this, `commit` would warn "larger than the
    /// original" on a perfectly correct conversion and make it look broken.
    pub size_growth_expected: bool,
}

/// Routes a job to the plugin that owns its operation.
///
/// Each engine phase adds its own arm; there is deliberately no catch-all that
/// "tries something sensible" — an unknown operation is an error, never a guess.
/// `operation::tests::registered_operations_are_all_dispatchable` fails if an
/// operation is advertised without an arm here.
pub async fn execute(job: &ConversionJob, ctx: &JobContext) -> Result<ExecutionOutput> {
    match job.operation_id.as_str() {
        SELFTEST_OPERATION_ID => diagnostics::execute(job, ctx).await,
        IMAGE_CONVERT_OPERATION_ID => image_engine::execute(job, ctx).await,
        ARCHIVE_CREATE_OPERATION_ID | ARCHIVE_EXTRACT_OPERATION_ID => {
            archive::execute(job, ctx).await
        }
        SPREADSHEET_CONVERT_OPERATION_ID => spreadsheet::execute(job, ctx).await,
        PDF_MERGE_OPERATION_ID
        | PDF_PAGES_OPERATION_ID
        | PDF_SPLIT_OPERATION_ID
        | PDF_REMOVE_METADATA_OPERATION_ID
        | PDF_FROM_IMAGES_OPERATION_ID => pdf::execute(job, ctx).await,
        MEDIA_CONVERT_OPERATION_ID => media::execute(job, ctx).await,
        other => Err(unknown_operation(other)),
    }
}

/// Reads the staged output back and reports on it. Runs before anything is
/// moved to the user's destination.
pub async fn validate(
    job: &ConversionJob,
    ctx: &JobContext,
    execution: &ExecutionOutput,
) -> Result<Vec<ValidationReport>> {
    match job.operation_id.as_str() {
        SELFTEST_OPERATION_ID => diagnostics::validate(job, ctx, execution).await,
        IMAGE_CONVERT_OPERATION_ID => image_engine::validate(job, ctx, execution).await,
        ARCHIVE_CREATE_OPERATION_ID | ARCHIVE_EXTRACT_OPERATION_ID => {
            archive::validate(job, ctx, execution).await
        }
        SPREADSHEET_CONVERT_OPERATION_ID => spreadsheet::validate(job, ctx, execution).await,
        PDF_MERGE_OPERATION_ID
        | PDF_PAGES_OPERATION_ID
        | PDF_SPLIT_OPERATION_ID
        | PDF_REMOVE_METADATA_OPERATION_ID
        | PDF_FROM_IMAGES_OPERATION_ID => pdf::validate(job, ctx, execution).await,
        MEDIA_CONVERT_OPERATION_ID => media::validate(job, ctx, execution).await,
        other => Err(unknown_operation(other)),
    }
}

fn unknown_operation(id: &str) -> ConversionError {
    ConversionError::new(
        ConversionErrorCode::InvalidInput,
        format!("unknown operation: {id}"),
    )
    .with_message_key("error.operation.unknown")
}

/// Moves validated output to the user's destination and assembles the result.
///
/// Refuses outright if any report is invalid, deleting the staged output first
/// so an unusable file is never left where the user might find it. The source
/// is untouched either way.
pub fn commit(
    job: &ConversionJob,
    execution: ExecutionOutput,
    reports: Vec<ValidationReport>,
    elapsed_ms: u64,
) -> Result<JobResult> {
    if reports.iter().any(|report| !report.valid) {
        let failed: Vec<&str> = reports
            .iter()
            .flat_map(ValidationReport::failed_checks)
            .map(|check| check.id.as_str())
            .collect();
        let removed = discard(&execution);
        return Err(ConversionError::new(
            ConversionErrorCode::OutputValidationFailed,
            format!("failed checks: {}", failed.join(", ")),
        )
        .with_partial_output_removed(removed));
    }

    let destination_dir = std::path::Path::new(&job.output_directory);
    let mut outputs = Vec::with_capacity(execution.outputs.len());
    let mut warnings = execution.warnings.clone();
    let mut output_total_bytes = 0u64;

    // ponytail: a failure partway through a multi-output job leaves the earlier
    // files committed. Phase 0 produces one file; revisit when PDF split lands,
    // which is the first operation that can emit many.
    for staged in &execution.outputs {
        let Some(destination) =
            paths::resolve_destination(destination_dir, &staged.file_name, job.overwrite_policy)?
        else {
            warnings.push(JobWarning::new("warning.destination.skipped"));
            continue;
        };

        // "Source files are never modified" outranks every overwrite policy.
        // Without this, an operation whose output keeps the input's name (PDF
        // remove-metadata, image PNG→PNG) run with `Overwrite` into the source's
        // own folder would destroy the original. Checked here rather than in one
        // engine so it holds for every route, present and future.
        if resolves_to_an_input(&destination, &job.input_files) {
            let removed = discard(&execution);
            return Err(ConversionError::new(
                ConversionErrorCode::DestinationUnavailable,
                "refusing to write the result on top of the source file",
            )
            .with_message_key("error.destination.wouldOverwriteSource")
            .with_partial_output_removed(removed));
        }

        let overwriting = destination.exists();
        paths::commit_output(&staged.staged_path, &destination, overwriting)?;
        if overwriting {
            warnings.push(JobWarning::new("warning.destination.overwritten"));
        }

        output_total_bytes = output_total_bytes.saturating_add(staged.size_bytes);
        outputs.push(OutputDescriptor {
            path: destination.to_string_lossy().into_owned(),
            display_name: staged.file_name.clone(),
            size_bytes: staged.size_bytes,
            format: staged.format.clone(),
        });
    }

    // "The application warns when compression increases file size." Only when the
    // growth is actually a surprise — see `size_growth_expected`.
    if !execution.size_growth_expected
        && execution.input_total_bytes > 0
        && output_total_bytes > execution.input_total_bytes
    {
        warnings.push(JobWarning::new("warning.output.largerThanInput"));
    }

    Ok(JobResult {
        outputs,
        warnings,
        validation_reports: reports,
        elapsed_ms,
        input_total_bytes: execution.input_total_bytes,
        output_total_bytes,
        size_change_percent: JobResult::size_change_percent(
            execution.input_total_bytes,
            output_total_bytes,
        ),
    })
}

/// `true` when `destination` is the same file as one of the job's inputs.
/// Compared canonically, so a relative path, a symlink or a `..` detour all
/// still match.
fn resolves_to_an_input(destination: &std::path::Path, inputs: &[crate::FileDescriptor]) -> bool {
    let Ok(destination) = destination.canonicalize() else {
        // It does not exist yet, so it cannot be an existing input file.
        return false;
    };
    inputs.iter().any(|input| {
        std::path::Path::new(&input.path)
            .canonicalize()
            .is_ok_and(|input| input == destination)
    })
}

/// Deletes staged output. Returns `true` when everything that existed was
/// removed, which is what the error reports back to the user.
pub fn discard(execution: &ExecutionOutput) -> bool {
    execution.outputs.iter().all(|staged| {
        !staged.staged_path.exists() || std::fs::remove_file(&staged.staged_path).is_ok()
    })
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
    use crate::paths::OverwritePolicy;
    use crate::validation::{OutputMetadata, ValidationCheck};

    fn context(temp: &std::path::Path, cancel: CancellationToken) -> JobContext {
        let id = Uuid::new_v4();
        JobContext::new(
            id,
            JobWorkspace::create(temp, id).unwrap(),
            cancel,
            Arc::new(|_, _| {}),
        )
    }

    fn job(out_dir: &std::path::Path, policy: OverwritePolicy) -> ConversionJob {
        ConversionJob::new(
            SELFTEST_OPERATION_ID,
            Vec::new(),
            out_dir.to_string_lossy(),
            policy,
            serde_json::Value::Null,
        )
    }

    fn staged(ctx: &JobContext, name: &str, bytes: &[u8]) -> ExecutionOutput {
        let path = ctx.workspace.staging_path(name).unwrap();
        std::fs::write(&path, bytes).unwrap();
        ExecutionOutput {
            outputs: vec![StagedOutput {
                staged_path: path,
                file_name: name.to_owned(),
                format: "bin".to_owned(),
                size_bytes: bytes.len() as u64,
            }],
            warnings: Vec::new(),
            input_total_bytes: 0,
            size_growth_expected: false,
        }
    }

    fn report(valid: bool) -> ValidationReport {
        let check = if valid {
            ValidationCheck::passed("test.check")
        } else {
            ValidationCheck::failed("test.check", "nope")
        };
        ValidationReport::from_checks("bin", vec![check], OutputMetadata::default())
    }

    #[tokio::test]
    async fn unknown_operations_are_rejected_not_guessed() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path(), CancellationToken::new());
        let mut j = job(temp.path(), OverwritePolicy::Fail);
        // Deliberately a route that does not exist yet.
        j.operation_id = "nonexistent.operation".to_owned();

        let err = execute(&j, &ctx).await.unwrap_err();
        assert_eq!(err.code, ConversionErrorCode::InvalidInput);
        assert_eq!(err.message_key, "error.operation.unknown");
    }

    #[test]
    fn a_cancelled_token_short_circuits_the_context() {
        let temp = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let ctx = context(temp.path(), cancel.clone());

        assert!(ctx.check_cancelled().is_ok());
        cancel.cancel();
        assert_eq!(
            ctx.check_cancelled().unwrap_err().code,
            ConversionErrorCode::Cancelled
        );
    }

    #[test]
    fn progress_reaches_the_sink_with_the_job_id() {
        let temp = tempfile::tempdir().unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink_seen = Arc::clone(&seen);
        let id = Uuid::new_v4();
        let ctx = JobContext::new(
            id,
            JobWorkspace::create(temp.path(), id).unwrap(),
            CancellationToken::new(),
            Arc::new(move |job_id, progress: JobProgress| {
                if let Ok(mut guard) = sink_seen.lock() {
                    guard.push((job_id, progress));
                }
            }),
        );

        ctx.report(JobProgress::counted(
            crate::job::ProgressStage::Running,
            1,
            2,
            "progress.running",
        ));

        let guard = seen.lock().unwrap();
        assert_eq!(guard.len(), 1);
        assert_eq!(guard[0].0, id);
        assert_eq!(guard[0].1.percent, Some(50.0));
    }

    #[test]
    fn commit_refuses_invalid_output_and_deletes_it() {
        let temp = tempfile::tempdir().unwrap();
        let out_dir = temp.path().join("out");
        std::fs::create_dir_all(&out_dir).unwrap();
        let ctx = context(temp.path(), CancellationToken::new());
        let execution = staged(&ctx, "result.bin", b"garbage");
        let staged_path = execution.outputs[0].staged_path.clone();

        let err = commit(
            &job(&out_dir, OverwritePolicy::Fail),
            execution,
            vec![report(false)],
            1,
        )
        .unwrap_err();

        assert_eq!(err.code, ConversionErrorCode::OutputValidationFailed);
        assert!(err.partial_output_removed);
        assert!(!staged_path.exists(), "invalid output must be deleted");
        assert!(
            !out_dir.join("result.bin").exists(),
            "invalid output must never reach the destination"
        );
    }

    #[test]
    fn commit_moves_validated_output_to_the_destination() {
        let temp = tempfile::tempdir().unwrap();
        let out_dir = temp.path().join("out");
        std::fs::create_dir_all(&out_dir).unwrap();
        let ctx = context(temp.path(), CancellationToken::new());
        let execution = staged(&ctx, "result.bin", b"good bytes");

        let result = commit(
            &job(&out_dir, OverwritePolicy::Fail),
            execution,
            vec![report(true)],
            5,
        )
        .unwrap();

        assert_eq!(result.outputs.len(), 1);
        assert_eq!(
            std::fs::read(out_dir.join("result.bin")).unwrap(),
            b"good bytes"
        );
        assert_eq!(result.output_total_bytes, 10);
    }

    #[test]
    fn commit_warns_when_the_output_is_larger_than_the_input() {
        let temp = tempfile::tempdir().unwrap();
        let out_dir = temp.path().join("out");
        std::fs::create_dir_all(&out_dir).unwrap();
        let ctx = context(temp.path(), CancellationToken::new());
        let mut execution = staged(&ctx, "result.bin", b"0123456789");
        execution.input_total_bytes = 4;

        let result = commit(
            &job(&out_dir, OverwritePolicy::Fail),
            execution,
            vec![report(true)],
            1,
        )
        .unwrap();

        assert!(result.output_grew());
        assert!(result
            .warnings
            .iter()
            .any(|w| w.message_key == "warning.output.largerThanInput"));
        assert_eq!(result.size_change_percent, 150.0);
    }

    #[test]
    fn commit_honours_the_skip_policy_without_writing() {
        let temp = tempfile::tempdir().unwrap();
        let out_dir = temp.path().join("out");
        std::fs::create_dir_all(&out_dir).unwrap();
        std::fs::write(out_dir.join("result.bin"), b"existing").unwrap();
        let ctx = context(temp.path(), CancellationToken::new());
        let execution = staged(&ctx, "result.bin", b"replacement");

        let result = commit(
            &job(&out_dir, OverwritePolicy::Skip),
            execution,
            vec![report(true)],
            1,
        )
        .unwrap();

        assert!(result.outputs.is_empty());
        assert!(result
            .warnings
            .iter()
            .any(|w| w.message_key == "warning.destination.skipped"));
        assert_eq!(
            std::fs::read(out_dir.join("result.bin")).unwrap(),
            b"existing"
        );
    }

    #[test]
    fn commit_refuses_to_clobber_under_the_fail_policy() {
        let temp = tempfile::tempdir().unwrap();
        let out_dir = temp.path().join("out");
        std::fs::create_dir_all(&out_dir).unwrap();
        std::fs::write(out_dir.join("result.bin"), b"precious").unwrap();
        let ctx = context(temp.path(), CancellationToken::new());
        let execution = staged(&ctx, "result.bin", b"replacement");

        let err = commit(
            &job(&out_dir, OverwritePolicy::Fail),
            execution,
            vec![report(true)],
            1,
        )
        .unwrap_err();

        assert_eq!(err.code, ConversionErrorCode::DestinationUnavailable);
        assert_eq!(
            std::fs::read(out_dir.join("result.bin")).unwrap(),
            b"precious"
        );
    }
}
