//! The complete IPC surface.
//!
//! This list *is* the security boundary. There is no generic "read file",
//! "write file" or "run command" command, and there never will be: the frontend
//! can start an operation the registry knows about, cancel one, and read job
//! state. Every path it supplies is validated here before it reaches the core.

use std::path::{Path, PathBuf};

use localconvert_core::image_engine::{ImageBatchPreflight, ImageOptions};
use localconvert_core::{
    image_engine, operation, ConversionError, ConversionErrorCode, ConversionJob, FileDescriptor,
    OperationDescriptor, StartJobRequest,
};
use serde::Serialize;
use tauri::{AppHandle, State};
use ts_rs::TS;
use uuid::Uuid;

use crate::state::AppState;

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppInfo {
    pub version: String,
    pub platform: String,
    pub arch: String,
    /// Always true. Asserted by a test, surfaced in the About page, and the
    /// reason there is no HTTP client anywhere in the dependency tree.
    pub offline_only: bool,
}

#[tauri::command]
#[must_use]
pub fn app_info() -> AppInfo {
    AppInfo {
        version: localconvert_core::VERSION.to_owned(),
        platform: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        offline_only: true,
    }
}

/// Exactly the operations this build can perform. Never padded with planned ones.
#[tauri::command]
#[must_use]
pub fn list_operations() -> Vec<OperationDescriptor> {
    operation::list_operations()
}

/// Whether a system FFmpeg/FFprobe pair is available, so the UI can explain the
/// media tab's state instead of only failing when a job runs.
#[tauri::command]
#[must_use]
pub fn media_available() -> bool {
    localconvert_core::media::is_available()
}

#[tauri::command]
#[must_use]
pub fn list_jobs(state: State<'_, AppState>) -> Vec<ConversionJob> {
    state.jobs()
}

#[tauri::command]
pub fn get_job(state: State<'_, AppState>, job_id: Uuid) -> Result<ConversionJob, ConversionError> {
    state.job(job_id).ok_or_else(|| {
        ConversionError::new(ConversionErrorCode::InvalidInput, "unknown job id")
            .with_message_key("error.job.unknown")
    })
}

#[tauri::command]
pub fn cancel_job(state: State<'_, AppState>, job_id: Uuid) -> Result<(), ConversionError> {
    state.cancel(job_id)
}

#[tauri::command]
#[must_use]
pub fn clear_completed_jobs(state: State<'_, AppState>) -> Vec<ConversionJob> {
    state.remove_terminal_jobs();
    state.jobs()
}

/// Inspects a selection of images and reports what converting them would cost:
/// dimensions, transparency, animation, files that cannot be handled at all.
///
/// The UI calls this whenever the selection or the options change, so a user is
/// told about transparency loss *before* pressing the button rather than by a
/// warning afterwards.
#[tauri::command]
pub fn preflight_images(
    input_paths: Vec<String>,
    options: serde_json::Value,
) -> Result<ImageBatchPreflight, ConversionError> {
    let options = ImageOptions::parse(&options)?;
    let inputs = input_paths
        .iter()
        .map(|path| FileDescriptor::probe(Path::new(path)))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(image_engine::preflight_batch(&inputs, &options))
}

/// Validates the request, then queues it. Returns the queued job immediately;
/// everything after that arrives as `job-updated` events.
#[tauri::command]
pub fn start_job(
    app: AppHandle,
    state: State<'_, AppState>,
    request: StartJobRequest,
) -> Result<ConversionJob, ConversionError> {
    let descriptor = operation::find_operation(&request.operation_id).ok_or_else(|| {
        ConversionError::new(
            ConversionErrorCode::InvalidInput,
            format!("unknown operation: {}", request.operation_id),
        )
        .with_message_key("error.operation.unknown")
    })?;

    let output_directory = validate_output_directory(&request.output_directory)?;

    if !descriptor.accepts_multiple_inputs && request.input_paths.len() > 1 {
        return Err(ConversionError::new(
            ConversionErrorCode::InvalidInput,
            "this operation takes a single input",
        )
        .with_message_key("error.input.tooMany"));
    }

    let input_files = request
        .input_paths
        .iter()
        .map(|path| FileDescriptor::probe(Path::new(path)))
        .collect::<Result<Vec<_>, _>>()?;

    let job = ConversionJob::new(
        request.operation_id,
        input_files,
        output_directory.to_string_lossy(),
        request.overwrite_policy,
        request.options,
    );

    tracing::info!(job_id = %job.id, operation = %job.operation_id, "job queued");
    Ok(state.spawn(&app, job))
}

/// The destination must be an existing, writable directory. Checking here means
/// a job never fails halfway through because the folder was never usable.
fn validate_output_directory(raw: &str) -> Result<PathBuf, ConversionError> {
    let path = Path::new(raw);
    localconvert_core::paths::ensure_safe_component_bytes(path)?;

    let metadata = std::fs::metadata(path).map_err(|err| {
        ConversionError::from_io("stat output directory", &err)
            .with_message_key("error.destination.unavailable")
    })?;
    if !metadata.is_dir() {
        return Err(ConversionError::new(
            ConversionErrorCode::DestinationUnavailable,
            "output destination is not a directory",
        )
        .with_message_key("error.destination.notADirectory"));
    }
    path.canonicalize()
        .map_err(|err| ConversionError::from_io("canonicalize output directory", &err))
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

    #[test]
    fn app_info_reports_the_workspace_version() {
        let info = app_info();
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(info.offline_only);
    }

    #[test]
    fn a_missing_output_directory_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let err =
            validate_output_directory(&dir.path().join("nope").to_string_lossy()).unwrap_err();
        assert_eq!(err.code, ConversionErrorCode::DestinationUnavailable);
    }

    #[test]
    fn a_file_is_not_an_acceptable_output_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-a-dir.txt");
        std::fs::write(&file, b"x").unwrap();

        let err = validate_output_directory(&file.to_string_lossy()).unwrap_err();
        assert_eq!(err.message_key, "error.destination.notADirectory");
    }

    #[test]
    fn a_real_directory_is_accepted_and_canonicalized() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("out");
        std::fs::create_dir_all(&nested).unwrap();

        let resolved = validate_output_directory(&nested.join(".").to_string_lossy()).unwrap();
        assert!(resolved.is_absolute());
        assert!(resolved.ends_with("out"));
    }

    #[test]
    fn the_ipc_surface_exposes_no_generic_filesystem_or_shell_command() {
        // The generated handler list is the security contract. If someone adds
        // a passthrough command, this test is the tripwire.
        let source = include_str!("lib.rs");
        for forbidden in ["shell", "spawn_process", "read_file", "write_file", "exec"] {
            assert!(
                !source.contains(&format!("commands::{forbidden}")),
                "generic {forbidden} command must not be registered"
            );
        }
    }

    #[test]
    fn the_window_capability_grants_no_dangerous_permissions() {
        let capability = include_str!("../capabilities/default.json");
        for forbidden in ["fs:", "shell:", "process:", "http:", "opener:"] {
            assert!(
                !capability.contains(forbidden),
                "capability must not grant `{forbidden}` permissions"
            );
        }
    }
}
