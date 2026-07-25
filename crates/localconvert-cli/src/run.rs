//! Shared job driver.
//!
//! The CLI runs the exact same three-stage pipeline as the desktop app —
//! `execute` → `validate` → `commit` from `localconvert-core` — against a temp
//! workspace it owns. No conversion logic lives in the CLI; it only builds a
//! `ConversionJob` and reports the result.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use localconvert_core::{
    runner, ConversionError, ConversionJob, FileDescriptor, JobResult, JobWorkspace,
    OverwritePolicy, ProgressStage,
};
use tokio_util::sync::CancellationToken;

pub struct RunOptions {
    pub operation_id: String,
    pub input_paths: Vec<PathBuf>,
    pub output_dir: PathBuf,
    pub overwrite: OverwritePolicy,
    pub options: serde_json::Value,
    pub quiet: bool,
}

/// Probes inputs, builds the job and runs it to completion, returning the
/// result or a domain error. Progress is written to stderr unless `quiet`.
pub async fn run(opts: RunOptions) -> localconvert_core::Result<JobResult> {
    let inputs: Vec<FileDescriptor> = opts
        .input_paths
        .iter()
        .map(|p| FileDescriptor::probe(p))
        .collect::<localconvert_core::Result<_>>()?;

    validate_output_dir(&opts.output_dir)?;

    let job = ConversionJob::new(
        opts.operation_id,
        inputs,
        opts.output_dir.to_string_lossy(),
        opts.overwrite,
        opts.options,
    );

    let temp_root = std::env::temp_dir().join("localconvert");
    std::fs::create_dir_all(&temp_root)
        .map_err(|err| ConversionError::from_io("create temp root", &err))?;
    // No blanket startup cleanup here: each CLI invocation is a separate process,
    // so sweeping all job dirs would delete a *concurrent* invocation's in-flight
    // workspace. The workspace removes itself on drop (RAII), and the long-lived
    // desktop app owns recovery of anything a crash leaves behind.
    let workspace = JobWorkspace::create(&temp_root, job.id)?;
    let cancel = CancellationToken::new();

    let quiet = opts.quiet;
    let progress: runner::ProgressSink = Arc::new(move |_id, progress| {
        if quiet {
            return;
        }
        let label = stage_label(progress.stage);
        match progress.percent {
            Some(pct) => eprint!("\r{label}: {pct:>3.0}%   "),
            None => eprint!("\r{label}…   "),
        }
        use std::io::Write;
        let _ = std::io::stderr().flush();
    });

    let ctx = runner::JobContext::new(job.id, workspace, cancel, progress);

    let execution = runner::execute(&job, &ctx).await?;
    let reports = runner::validate(&job, &ctx, &execution).await?;
    let result = runner::commit(&job, execution, reports, 0)?;

    if !quiet {
        eprintln!("\rdone            ");
    }
    Ok(result)
}

fn validate_output_dir(dir: &Path) -> localconvert_core::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    // Create it if the parent exists; otherwise it is a user mistake.
    std::fs::create_dir_all(dir).map_err(|err| {
        ConversionError::from_io("create output directory", &err)
            .with_message_key("error.destination.unavailable")
    })?;
    Ok(())
}

fn stage_label(stage: ProgressStage) -> &'static str {
    match stage {
        ProgressStage::Queued => "queued",
        ProgressStage::Preparing => "preparing",
        ProgressStage::Running => "working",
        ProgressStage::Validating => "verifying",
        ProgressStage::Finalizing => "saving",
        ProgressStage::Done => "done",
    }
}

/// Resolves `--output`: a directory to write into, plus an optional base name
/// when the user pointed at a filename (`--output out/merged.pdf`).
#[must_use]
pub fn resolve_output(output: Option<&Path>, default_dir: &Path) -> (PathBuf, Option<String>) {
    match output {
        None => (default_dir.to_path_buf(), None),
        Some(path) => {
            // A path with an extension is treated as dir + base name.
            if path.extension().is_some() {
                let dir = path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map_or_else(|| default_dir.to_path_buf(), Path::to_path_buf);
                let name = path.file_stem().map(|s| s.to_string_lossy().into_owned());
                (dir, name)
            } else {
                (path.to_path_buf(), None)
            }
        }
    }
}

/// The exit code and, when `--json`, the machine-readable payload for an error.
#[must_use]
pub fn error_json(err: &ConversionError) -> serde_json::Value {
    serde_json::json!({
        "ok": false,
        "code": format!("{:?}", err.code),
        "messageKey": err.message_key,
        "detail": err.detail,
        "sourceSafe": err.source_safe,
        "partialOutputRemoved": err.partial_output_removed,
    })
}

#[must_use]
pub fn result_json(result: &JobResult) -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "outputs": result.outputs.iter().map(|o| serde_json::json!({
            "path": o.path,
            "displayName": o.display_name,
            "sizeBytes": o.size_bytes,
            "format": o.format,
        })).collect::<Vec<_>>(),
        "warnings": result.warnings.iter().map(|w| &w.message_key).collect::<Vec<_>>(),
        "inputTotalBytes": result.input_total_bytes,
        "outputTotalBytes": result.output_total_bytes,
        "sizeChangePercent": result.size_change_percent,
    })
}

/// Parses an `--overwrite` value.
pub fn parse_policy(value: &str) -> Result<OverwritePolicy, String> {
    match value.to_ascii_lowercase().as_str() {
        "fail" | "stop" => Ok(OverwritePolicy::Fail),
        "rename" => Ok(OverwritePolicy::Rename),
        "skip" => Ok(OverwritePolicy::Skip),
        "overwrite" | "replace" => Ok(OverwritePolicy::Overwrite),
        other => Err(format!(
            "unknown overwrite policy '{other}' (fail|rename|skip|overwrite)"
        )),
    }
}

/// A synthetic descriptor probe used by `convert` to route by detected format.
pub fn detect_format(path: &Path) -> localconvert_core::Result<localconvert_core::FileFormat> {
    Ok(FileDescriptor::probe(path)?.detected_format)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn resolve_output_treats_a_directory_as_a_directory() {
        let (dir, name) = resolve_output(Some(Path::new("out/here")), Path::new("/cwd"));
        assert_eq!(dir, PathBuf::from("out/here"));
        assert_eq!(name, None);
    }

    #[test]
    fn resolve_output_splits_a_filename_into_dir_and_stem() {
        let (dir, name) = resolve_output(Some(Path::new("out/merged.pdf")), Path::new("/cwd"));
        assert_eq!(dir, PathBuf::from("out"));
        assert_eq!(name.as_deref(), Some("merged"));
    }

    #[test]
    fn resolve_output_defaults_to_the_given_dir() {
        let (dir, name) = resolve_output(None, Path::new("/cwd"));
        assert_eq!(dir, PathBuf::from("/cwd"));
        assert_eq!(name, None);
    }

    #[test]
    fn policy_parsing_accepts_aliases() {
        assert_eq!(parse_policy("stop").unwrap(), OverwritePolicy::Fail);
        assert_eq!(parse_policy("REPLACE").unwrap(), OverwritePolicy::Overwrite);
        assert!(parse_policy("nonsense").is_err());
    }
}
