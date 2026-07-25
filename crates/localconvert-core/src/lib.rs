//! LocalConvert core.
//!
//! Owns the domain contracts, the job state machine, safe path handling,
//! temporary workspace management and the output-validation framework. It has
//! no dependency on Tauri, so the same code backs the desktop app, the CLI and
//! every integration test.
//!
//! Three invariants this crate exists to enforce:
//!
//! 1. **Source files are only ever read.** Nothing here opens an input for
//!    writing, and [`validation::basic_output_checks`] fails any job whose
//!    output resolves to one of its inputs.
//! 2. **No job reports success without validation.** The state machine in
//!    [`job::JobStatus::can_transition_to`] admits `Completed` only from
//!    `Validating`.
//! 3. **Cleanup cannot escape.** Every deletion is fenced by a canonical
//!    containment check against the LocalConvert temp root.

#![doc(html_no_source)]

pub mod archive;
pub mod detect;
pub mod diagnostics;
pub mod error;
pub mod file;
pub mod image_engine;
pub mod job;
pub mod media;
pub mod operation;
pub mod paths;
pub mod pdf;
pub mod runner;
pub mod spreadsheet;
pub mod validation;
pub mod workspace;

pub use archive::{ArchiveFormat, CreateOptions};
pub use detect::{detect, Detection, DetectionSource};
pub use error::{ConversionError, ConversionErrorCode, Result};
pub use file::{FileDescriptor, FileFormat, FileMetadata};
pub use image_engine::{ImageOptions, ImageOutputFormat, ImagePreflight};
pub use job::{
    ConversionJob, JobProgress, JobResult, JobStatus, JobWarning, OutputDescriptor, ProgressStage,
    StartJobRequest,
};
pub use media::{MediaFormat, MediaOptions, MediaPreset};
pub use operation::{list_operations, OperationDescriptor, SELFTEST_OPERATION_ID};
pub use paths::OverwritePolicy;
pub use pdf::parse_page_ranges;
pub use runner::{
    commit, execute, validate, ExecutionOutput, JobContext, ProgressSink, StagedOutput,
};
pub use spreadsheet::{ColumnType, SpreadsheetOptions, TabularFormat};
pub use validation::{OutputValidator, ValidationCheck, ValidationReport};
pub use workspace::{cleanup_stale, JobWorkspace};

/// Version of the desktop application and the CLI, from the workspace manifest.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
