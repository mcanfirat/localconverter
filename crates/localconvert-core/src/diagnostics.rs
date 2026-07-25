//! Pipeline self-test.
//!
//! This is **not** a converter and is never advertised as one. It is a real,
//! permanent diagnostic that drives the whole machine end to end — temp
//! workspace, chunked progress, cancellation, independent read-back, output
//! validation, conflict policy, atomic commit, cleanup — so Phase 0's "a job
//! runs and reports progress" acceptance criterion is met without a placeholder
//! conversion existing anywhere in the codebase.
//!
//! It stays after the engines land: it is how a user or a support request
//! answers "is the job pipeline itself healthy on this machine?"

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use ts_rs::TS;

use crate::error::{ConversionError, ConversionErrorCode, Result};
use crate::job::{ConversionJob, JobProgress, ProgressStage};
use crate::runner::{ExecutionOutput, JobContext, StagedOutput};
use crate::validation::{basic_output_checks, OutputMetadata, ValidationCheck, ValidationReport};

pub const OUTPUT_FILE_NAME: &str = "localconvert-selftest.bin";
const CHUNK_BYTES: usize = 64 * 1024;
const DEFAULT_BYTES: u64 = 1024 * 1024;
const MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SelftestOptions {
    /// How many bytes to write. Bounded so a mistyped option cannot fill the disk.
    #[ts(type = "number")]
    pub size_bytes: u64,
}

impl Default for SelftestOptions {
    fn default() -> Self {
        Self {
            size_bytes: DEFAULT_BYTES,
        }
    }
}

impl SelftestOptions {
    /// Out-of-range values are rejected, not silently clamped: the spec forbids
    /// quietly changing what the user asked for.
    pub fn parse(options: &serde_json::Value) -> Result<Self> {
        let parsed: Self = if options.is_null() {
            Self::default()
        } else {
            serde_json::from_value(options.clone()).map_err(|err| {
                ConversionError::new(
                    ConversionErrorCode::InvalidInput,
                    format!("invalid selftest options: {err}"),
                )
                .with_message_key("error.options.invalid")
            })?
        };

        if parsed.size_bytes == 0 || parsed.size_bytes > MAX_BYTES {
            return Err(ConversionError::new(
                ConversionErrorCode::InvalidInput,
                format!("sizeBytes must be between 1 and {MAX_BYTES}"),
            )
            .with_message_key("error.options.outOfRange"));
        }
        Ok(parsed)
    }
}

/// Deterministic pattern. Chosen so that a truncated, zero-filled or
/// block-shifted output cannot accidentally verify.
fn expected_byte(offset: u64) -> u8 {
    // ponytail: a cheap mix is enough to catch truncation, zero-fill and
    // duplicated blocks. Hash the content instead if subtle bit rot ever matters.
    (offset.wrapping_mul(31).wrapping_add(7) & 0xFF) as u8
}

/// Writes the pattern into the job's temp workspace. Nothing reaches the user's
/// destination here — that is [`crate::runner::commit`]'s job, after validation.
pub async fn execute(job: &ConversionJob, ctx: &JobContext) -> Result<ExecutionOutput> {
    let options = SelftestOptions::parse(&job.options)?;

    ctx.report(JobProgress::indeterminate(
        ProgressStage::Preparing,
        "progress.preparing",
    ));
    ctx.check_cancelled()?;

    let staged_path = ctx.workspace.staging_path(OUTPUT_FILE_NAME)?;
    write_pattern(ctx, &staged_path, options.size_bytes).await?;

    Ok(ExecutionOutput {
        outputs: vec![StagedOutput {
            staged_path,
            file_name: OUTPUT_FILE_NAME.to_owned(),
            format: "bin".to_owned(),
            size_bytes: options.size_bytes,
        }],
        warnings: Vec::new(),
        input_total_bytes: 0,
        size_growth_expected: false,
    })
}

/// Re-reads the staged file with a completely separate code path from the
/// writer and compares every byte — this operation's equivalent of "a second
/// independent parser can open the output".
pub async fn validate(
    job: &ConversionJob,
    ctx: &JobContext,
    execution: &ExecutionOutput,
) -> Result<Vec<ValidationReport>> {
    ctx.report(JobProgress::indeterminate(
        ProgressStage::Validating,
        "progress.validating",
    ));

    let mut reports = Vec::with_capacity(execution.outputs.len());
    for staged in &execution.outputs {
        let mut checks = basic_output_checks(&job.input_files, &staged.staged_path, None);
        checks.push(verify_pattern(&staged.staged_path, staged.size_bytes).await?);

        reports.push(ValidationReport::from_checks(
            "application/octet-stream",
            checks,
            OutputMetadata {
                size_bytes: staged.size_bytes,
                properties: serde_json::json!({ "pattern": "localconvert-selftest-v1" }),
            },
        ));
    }
    Ok(reports)
}

async fn write_pattern(ctx: &JobContext, staged: &std::path::Path, total: u64) -> Result<()> {
    let mut file = tokio::fs::File::create(staged)
        .await
        .map_err(|err| ConversionError::from_io("create staged output", &err))?;

    let mut written: u64 = 0;
    let mut buffer = vec![0u8; CHUNK_BYTES];

    while written < total {
        // Checked per chunk, so a cancel lands within one 64 KiB write rather
        // than at the end of the job.
        if ctx.cancel.is_cancelled() {
            drop(file);
            let removed = tokio::fs::remove_file(staged).await.is_ok();
            return Err(ConversionError::cancelled().with_partial_output_removed(removed));
        }

        let len = usize::try_from(total - written)
            .unwrap_or(CHUNK_BYTES)
            .min(CHUNK_BYTES);
        for (index, slot) in buffer.iter_mut().take(len).enumerate() {
            *slot = expected_byte(written.saturating_add(index as u64));
        }
        file.write_all(buffer.get(..len).unwrap_or_default())
            .await
            .map_err(|err| ConversionError::from_io("write staged output", &err))?;

        written = written.saturating_add(len as u64);
        ctx.report(JobProgress::counted(
            ProgressStage::Running,
            written,
            total,
            "progress.writing",
        ));
    }

    file.flush()
        .await
        .map_err(|err| ConversionError::from_io("flush staged output", &err))?;
    file.sync_all()
        .await
        .map_err(|err| ConversionError::from_io("sync staged output", &err))?;
    Ok(())
}

async fn verify_pattern(staged: &std::path::Path, expected_len: u64) -> Result<ValidationCheck> {
    let bytes = tokio::fs::read(staged)
        .await
        .map_err(|err| ConversionError::from_io("read back staged output", &err))?;

    if bytes.len() as u64 != expected_len {
        return Ok(ValidationCheck::failed(
            "selftest.contentMatches",
            format!("expected {expected_len} bytes, read {}", bytes.len()),
        ));
    }
    if let Some((offset, _)) = bytes
        .iter()
        .enumerate()
        .find(|(offset, byte)| **byte != expected_byte(*offset as u64))
    {
        return Ok(ValidationCheck::failed(
            "selftest.contentMatches",
            format!("byte mismatch at offset {offset}"),
        ));
    }
    Ok(ValidationCheck::passed("selftest.contentMatches"))
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use std::sync::{Arc, Mutex};

    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    use super::*;
    use crate::paths::OverwritePolicy;
    use crate::runner;
    use crate::workspace::JobWorkspace;

    struct Harness {
        _temp: tempfile::TempDir,
        out_dir: std::path::PathBuf,
        temp_root: std::path::PathBuf,
        progress: Arc<Mutex<Vec<JobProgress>>>,
    }

    impl Harness {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let out_dir = temp.path().join("out");
            let temp_root = temp.path().join("apptemp");
            std::fs::create_dir_all(&out_dir).unwrap();
            std::fs::create_dir_all(&temp_root).unwrap();
            Self {
                _temp: temp,
                out_dir,
                temp_root,
                progress: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn job(&self, size: u64, policy: OverwritePolicy) -> ConversionJob {
            ConversionJob::new(
                crate::operation::SELFTEST_OPERATION_ID,
                Vec::new(),
                self.out_dir.to_string_lossy(),
                policy,
                serde_json::json!({ "sizeBytes": size }),
            )
        }

        fn context(&self, cancel: CancellationToken) -> JobContext {
            let id = Uuid::new_v4();
            let sink = Arc::clone(&self.progress);
            JobContext::new(
                id,
                JobWorkspace::create(&self.temp_root, id).unwrap(),
                cancel,
                Arc::new(move |_, progress| {
                    if let Ok(mut guard) = sink.lock() {
                        guard.push(progress);
                    }
                }),
            )
        }

        fn stages(&self) -> Vec<ProgressStage> {
            self.progress
                .lock()
                .map(|g| g.iter().map(|p| p.stage).collect())
                .unwrap_or_default()
        }
    }

    /// Drives the same three stages the desktop app drives.
    async fn run(job: &ConversionJob, ctx: &JobContext) -> Result<crate::job::JobResult> {
        let execution = runner::execute(job, ctx).await?;
        let reports = runner::validate(job, ctx, &execution).await?;
        runner::commit(job, execution, reports, 0)
    }

    #[tokio::test]
    async fn selftest_writes_verifies_and_commits() {
        let h = Harness::new();
        let job = h.job(200_000, OverwritePolicy::Fail);
        let ctx = h.context(CancellationToken::new());

        let result = run(&job, &ctx).await.unwrap();

        assert_eq!(result.outputs.len(), 1);
        assert_eq!(result.output_total_bytes, 200_000);
        assert!(result.validation_reports[0].valid);

        let out = h.out_dir.join(OUTPUT_FILE_NAME);
        assert_eq!(std::fs::metadata(&out).unwrap().len(), 200_000);
        assert_eq!(
            std::fs::read_dir(ctx.workspace.path()).unwrap().count(),
            0,
            "the workspace must be empty once the output has been committed"
        );
    }

    #[tokio::test]
    async fn progress_is_real_and_monotonic() {
        let h = Harness::new();
        let job = h.job(200_000, OverwritePolicy::Fail);
        let ctx = h.context(CancellationToken::new());
        run(&job, &ctx).await.unwrap();

        let counted: Vec<f32> = h
            .progress
            .lock()
            .unwrap()
            .iter()
            .filter_map(|p| p.percent)
            .collect();
        assert!(
            counted.len() >= 3,
            "expected chunked progress, got {counted:?}"
        );
        assert!(
            counted.windows(2).all(|w| w[1] >= w[0]),
            "progress went backwards"
        );
        assert_eq!(counted.last().copied(), Some(100.0));

        let stages = h.stages();
        assert_eq!(stages.first(), Some(&ProgressStage::Preparing));
        assert!(stages.contains(&ProgressStage::Running));
        assert!(stages.contains(&ProgressStage::Validating));
    }

    #[tokio::test]
    async fn cancellation_removes_partial_output_and_writes_nothing_to_the_destination() {
        let h = Harness::new();
        let job = h.job(8 * 1024 * 1024, OverwritePolicy::Fail);
        let cancel = CancellationToken::new();
        let ctx = h.context(cancel.clone());
        cancel.cancel();

        let err = run(&job, &ctx).await.unwrap_err();
        assert_eq!(err.code, ConversionErrorCode::Cancelled);
        assert!(err.source_safe);
        assert!(!h.out_dir.join(OUTPUT_FILE_NAME).exists());
        assert_eq!(std::fs::read_dir(ctx.workspace.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn fail_policy_refuses_to_clobber_an_existing_file() {
        let h = Harness::new();
        let existing = h.out_dir.join(OUTPUT_FILE_NAME);
        std::fs::write(&existing, b"do not touch me").unwrap();

        let err = run(
            &h.job(4096, OverwritePolicy::Fail),
            &h.context(CancellationToken::new()),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code, ConversionErrorCode::DestinationUnavailable);
        assert_eq!(std::fs::read(&existing).unwrap(), b"do not touch me");
    }

    #[tokio::test]
    async fn rename_policy_produces_a_numbered_sibling() {
        let h = Harness::new();
        std::fs::write(h.out_dir.join(OUTPUT_FILE_NAME), b"existing").unwrap();

        let result = run(
            &h.job(4096, OverwritePolicy::Rename),
            &h.context(CancellationToken::new()),
        )
        .await
        .unwrap();

        assert!(result.outputs[0]
            .path
            .contains("localconvert-selftest (1).bin"));
        assert_eq!(
            std::fs::read(h.out_dir.join(OUTPUT_FILE_NAME)).unwrap(),
            b"existing"
        );
    }

    #[tokio::test]
    async fn overwrite_policy_warns_that_it_overwrote() {
        let h = Harness::new();
        std::fs::write(h.out_dir.join(OUTPUT_FILE_NAME), b"old").unwrap();

        let result = run(
            &h.job(4096, OverwritePolicy::Overwrite),
            &h.context(CancellationToken::new()),
        )
        .await
        .unwrap();

        assert!(result
            .warnings
            .iter()
            .any(|w| w.message_key == "warning.destination.overwritten"));
        assert_eq!(
            std::fs::metadata(h.out_dir.join(OUTPUT_FILE_NAME))
                .unwrap()
                .len(),
            4096
        );
    }

    #[tokio::test]
    async fn skip_policy_writes_nothing_and_says_so() {
        let h = Harness::new();
        std::fs::write(h.out_dir.join(OUTPUT_FILE_NAME), b"keep").unwrap();

        let result = run(
            &h.job(4096, OverwritePolicy::Skip),
            &h.context(CancellationToken::new()),
        )
        .await
        .unwrap();

        assert!(result.outputs.is_empty());
        assert!(result
            .warnings
            .iter()
            .any(|w| w.message_key == "warning.destination.skipped"));
        assert_eq!(
            std::fs::read(h.out_dir.join(OUTPUT_FILE_NAME)).unwrap(),
            b"keep"
        );
    }

    #[tokio::test]
    async fn a_unicode_destination_directory_works() {
        let h = Harness::new();
        let unicode_dir = h.out_dir.join("öutput 📁 dïr");
        std::fs::create_dir_all(&unicode_dir).unwrap();

        let mut job = h.job(4096, OverwritePolicy::Fail);
        job.output_directory = unicode_dir.to_string_lossy().into_owned();

        run(&job, &h.context(CancellationToken::new()))
            .await
            .unwrap();
        assert!(unicode_dir.join(OUTPUT_FILE_NAME).exists());
    }

    #[tokio::test]
    async fn a_read_only_destination_fails_without_leaving_anything_behind() {
        let h = Harness::new();
        let locked = h.out_dir.join("locked");
        std::fs::create_dir_all(&locked).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        }
        #[cfg(not(unix))]
        {
            let mut perms = std::fs::metadata(&locked).unwrap().permissions();
            perms.set_readonly(true);
            std::fs::set_permissions(&locked, perms).unwrap();
        }

        let mut job = h.job(4096, OverwritePolicy::Fail);
        job.output_directory = locked.to_string_lossy().into_owned();
        let ctx = h.context(CancellationToken::new());

        // Windows honours directory read-only flags inconsistently; assert the
        // invariant that holds everywhere instead of the specific error code.
        if let Ok(result) = run(&job, &ctx).await {
            assert_eq!(result.outputs.len(), 1);
        } else {
            assert!(!locked.join(OUTPUT_FILE_NAME).exists());
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn options_are_validated_not_clamped() {
        assert_eq!(
            SelftestOptions::parse(&serde_json::Value::Null)
                .unwrap()
                .size_bytes,
            DEFAULT_BYTES
        );
        assert!(SelftestOptions::parse(&serde_json::json!({ "sizeBytes": 0 })).is_err());
        assert!(
            SelftestOptions::parse(&serde_json::json!({ "sizeBytes": MAX_BYTES + 1 })).is_err()
        );
        assert!(SelftestOptions::parse(&serde_json::json!({ "sizeBytes": "big" })).is_err());
    }

    #[tokio::test]
    async fn verification_catches_a_truncated_output() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("truncated.bin");
        std::fs::write(&path, vec![0u8; 10]).unwrap();

        assert!(!verify_pattern(&path, 20).await.unwrap().passed);
    }

    #[tokio::test]
    async fn verification_catches_a_corrupted_byte() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corrupt.bin");
        let mut bytes: Vec<u8> = (0..64u64).map(expected_byte).collect();
        bytes[32] ^= 0xFF;
        std::fs::write(&path, &bytes).unwrap();

        let check = verify_pattern(&path, 64).await.unwrap();
        assert!(!check.passed);
        assert!(check.detail.unwrap_or_default().contains("offset 32"));
    }

    #[tokio::test]
    async fn verification_passes_on_a_good_output() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("good.bin");
        let bytes: Vec<u8> = (0..1000u64).map(expected_byte).collect();
        std::fs::write(&path, &bytes).unwrap();

        assert!(verify_pattern(&path, 1000).await.unwrap().passed);
    }

    #[tokio::test]
    async fn an_output_that_fails_validation_never_reaches_the_destination() {
        let h = Harness::new();
        let job = h.job(4096, OverwritePolicy::Fail);
        let ctx = h.context(CancellationToken::new());

        let execution = runner::execute(&job, &ctx).await.unwrap();
        // Corrupt the staged file behind validation's back.
        std::fs::write(&execution.outputs[0].staged_path, vec![0u8; 4096]).unwrap();

        let reports = runner::validate(&job, &ctx, &execution).await.unwrap();
        assert!(!reports[0].valid);

        let err = runner::commit(&job, execution, reports, 0).unwrap_err();
        assert_eq!(err.code, ConversionErrorCode::OutputValidationFailed);
        assert!(err.partial_output_removed);
        assert!(!h.out_dir.join(OUTPUT_FILE_NAME).exists());
    }
}
