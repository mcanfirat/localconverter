//! Output validation.
//!
//! No job reaches [`JobStatus::Completed`](crate::job::JobStatus::Completed)
//! without a passing [`ValidationReport`]. Plugins implement
//! [`OutputValidator`] with format-specific checks (magic bytes, second-parser
//! open, dimensions, page count, duration); the universal checks that apply to
//! every route regardless of format live in [`basic_output_checks`].

use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{ConversionError, ConversionErrorCode, Result};
use crate::file::FileDescriptor;
use crate::job::OutputDescriptor;

/// One named assertion about an output file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ValidationCheck {
    /// Stable identifier, e.g. `output.exists`, `image.dimensionsMatch`.
    pub id: String,
    pub passed: bool,
    pub detail: Option<String>,
}

impl ValidationCheck {
    #[must_use]
    pub fn passed(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            passed: true,
            detail: None,
        }
    }

    #[must_use]
    pub fn failed(id: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            passed: false,
            detail: Some(crate::error::redact(&detail.into())),
        }
    }
}

/// Format-specific facts read back off the finished file. Kept as free-form
/// JSON rather than a union of every media type: each plugin owns its own
/// shape, and the UI renders whatever keys it recognises.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OutputMetadata {
    #[ts(type = "number")]
    pub size_bytes: u64,
    #[ts(type = "Record<string, unknown>")]
    pub properties: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ValidationReport {
    pub valid: bool,
    pub detected_format: String,
    pub warnings: Vec<String>,
    pub checks: Vec<ValidationCheck>,
    pub output_metadata: OutputMetadata,
}

impl ValidationReport {
    /// Builds a report from its checks. `valid` is *derived*, never set by
    /// hand, so a plugin cannot accidentally report success alongside a failed
    /// check.
    #[must_use]
    pub fn from_checks(
        detected_format: impl Into<String>,
        checks: Vec<ValidationCheck>,
        output_metadata: OutputMetadata,
    ) -> Self {
        let valid = checks.iter().all(|check| check.passed);
        Self {
            valid,
            detected_format: detected_format.into(),
            warnings: Vec::new(),
            checks,
            output_metadata,
        }
    }

    #[must_use]
    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }

    #[must_use]
    pub fn failed_checks(&self) -> Vec<&ValidationCheck> {
        self.checks.iter().filter(|c| !c.passed).collect()
    }

    /// Converts a failing report into the error the job will carry.
    pub fn into_error_if_invalid(self) -> Result<Self> {
        if self.valid {
            return Ok(self);
        }
        let failed = self
            .failed_checks()
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        Err(ConversionError::new(
            ConversionErrorCode::OutputValidationFailed,
            format!("failed checks: {failed}"),
        ))
    }
}

/// The checks that apply to every conversion route, from spec §4.1: the output
/// exists, is non-empty, is not the input file, and matches the name the job
/// promised.
///
/// Plugins call this first and append their format-specific checks.
pub fn basic_output_checks(
    inputs: &[FileDescriptor],
    output_path: &Path,
    expected_extension: Option<&str>,
) -> Vec<ValidationCheck> {
    let mut checks = Vec::new();

    let metadata = std::fs::metadata(output_path);
    match &metadata {
        Ok(_) => checks.push(ValidationCheck::passed("output.exists")),
        Err(err) => checks.push(ValidationCheck::failed("output.exists", format!("{err}"))),
    }

    match &metadata {
        Ok(meta) if meta.len() > 0 => checks.push(ValidationCheck::passed("output.nonEmpty")),
        Ok(meta) => checks.push(ValidationCheck::failed(
            "output.nonEmpty",
            format!("output is {} bytes", meta.len()),
        )),
        Err(_) => checks.push(ValidationCheck::failed("output.nonEmpty", "output missing")),
    }

    // "The output does not overwrite the source unless explicitly approved."
    // Compared canonically so a symlinked or relative input still matches.
    let canonical_output = output_path.canonicalize().ok();
    let collides = inputs.iter().any(|input| {
        let canonical_input = Path::new(&input.path).canonicalize().ok();
        match (&canonical_output, &canonical_input) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    });
    if collides {
        checks.push(ValidationCheck::failed(
            "output.distinctFromInput",
            "output path resolves to one of the input files",
        ));
    } else {
        checks.push(ValidationCheck::passed("output.distinctFromInput"));
    }

    if let Some(expected) = expected_extension {
        let actual = final_extension_of(output_path);
        if actual.as_deref() == Some(expected) {
            checks.push(ValidationCheck::passed("output.extensionMatches"));
        } else {
            checks.push(ValidationCheck::failed(
                "output.extensionMatches",
                format!("expected .{expected}, got .{}", actual.unwrap_or_default()),
            ));
        }
    }

    checks
}

/// Validation runs against staged output, which carries the `.partial` suffix
/// added by [`crate::workspace::JobWorkspace::staging_path`]. The extension that
/// matters is the one the committed file will have, so that suffix is peeled off
/// first — otherwise every route would "fail" for producing `.partial`.
fn final_extension_of(path: &Path) -> Option<String> {
    let without_suffix = if path.extension().is_some_and(|ext| ext == "partial") {
        path.file_stem().map(Path::new)
    } else {
        Some(path)
    };
    without_suffix
        .and_then(Path::extension)
        .map(|ext| ext.to_string_lossy().to_lowercase())
}

/// Implemented by every conversion plugin.
///
/// Deviation from the spec sketch: `options` is the job's already-validated
/// `serde_json::Value` rather than a `ConversionOptions` struct, because option
/// shapes are per-operation and owned by the plugin that declares the schema.
pub trait OutputValidator {
    fn validate(
        &self,
        inputs: &[FileDescriptor],
        output: &OutputDescriptor,
        options: &serde_json::Value,
    ) -> Result<ValidationReport>;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn descriptor(path: &Path) -> FileDescriptor {
        FileDescriptor::probe(path).unwrap()
    }

    #[test]
    fn valid_is_derived_from_the_checks() {
        let ok = ValidationReport::from_checks(
            "png",
            vec![ValidationCheck::passed("a"), ValidationCheck::passed("b")],
            OutputMetadata::default(),
        );
        assert!(ok.valid);

        let bad = ValidationReport::from_checks(
            "png",
            vec![
                ValidationCheck::passed("a"),
                ValidationCheck::failed("b", "nope"),
            ],
            OutputMetadata::default(),
        );
        assert!(!bad.valid);
        assert_eq!(bad.failed_checks().len(), 1);
    }

    #[test]
    fn an_invalid_report_becomes_an_output_validation_error() {
        let report = ValidationReport::from_checks(
            "png",
            vec![ValidationCheck::failed("output.nonEmpty", "0 bytes")],
            OutputMetadata::default(),
        );
        let err = report.into_error_if_invalid().unwrap_err();
        assert_eq!(err.code, ConversionErrorCode::OutputValidationFailed);
        assert!(err.detail.contains("output.nonEmpty"));
    }

    #[test]
    fn missing_output_fails_the_basic_checks() {
        let dir = tempfile::tempdir().unwrap();
        let checks = basic_output_checks(&[], &dir.path().join("nope.png"), Some("png"));
        let report = ValidationReport::from_checks("png", checks, OutputMetadata::default());
        assert!(!report.valid);
    }

    #[test]
    fn empty_output_fails_even_though_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("empty.png");
        std::fs::write(&out, b"").unwrap();

        let checks = basic_output_checks(&[], &out, Some("png"));
        let report = ValidationReport::from_checks("png", checks, OutputMetadata::default());
        assert!(!report.valid);
        assert!(report
            .failed_checks()
            .iter()
            .any(|c| c.id == "output.nonEmpty"));
    }

    #[test]
    fn writing_over_the_input_is_caught() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("photo.png");
        std::fs::write(&source, b"pixels").unwrap();

        let checks = basic_output_checks(&[descriptor(&source)], &source, Some("png"));
        let report = ValidationReport::from_checks("png", checks, OutputMetadata::default());
        assert!(!report.valid);
        assert!(report
            .failed_checks()
            .iter()
            .any(|c| c.id == "output.distinctFromInput"));
    }

    #[test]
    fn the_staging_suffix_is_peeled_off_before_the_extension_is_judged() {
        assert_eq!(
            final_extension_of(Path::new("a/photo.jpg.partial")).as_deref(),
            Some("jpg")
        );
        assert_eq!(
            final_extension_of(Path::new("a/photo.jpg")).as_deref(),
            Some("jpg")
        );
        assert_eq!(
            final_extension_of(Path::new("a/archive.tar.gz")).as_deref(),
            Some("gz")
        );
        assert_eq!(final_extension_of(Path::new("a/README")), None);
    }

    #[test]
    fn a_staged_output_passes_the_extension_check() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("result.png.partial");
        std::fs::write(&staged, b"data").unwrap();

        let checks = basic_output_checks(&[], &staged, Some("png"));
        let report = ValidationReport::from_checks("png", checks, OutputMetadata::default());
        assert!(report.valid, "unexpected: {:?}", report.failed_checks());
    }

    #[test]
    fn wrong_extension_is_caught() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("result.jpg");
        std::fs::write(&out, b"data").unwrap();

        let checks = basic_output_checks(&[], &out, Some("png"));
        let report = ValidationReport::from_checks("png", checks, OutputMetadata::default());
        assert!(!report.valid);
        assert!(report
            .failed_checks()
            .iter()
            .any(|c| c.id == "output.extensionMatches"));
    }

    #[test]
    fn a_good_output_passes_every_basic_check() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("in.heic");
        let out = dir.path().join("out.png");
        std::fs::write(&source, b"source").unwrap();
        std::fs::write(&out, b"converted").unwrap();

        let checks = basic_output_checks(&[descriptor(&source)], &out, Some("png"));
        let report = ValidationReport::from_checks("png", checks, OutputMetadata::default());
        assert!(
            report.valid,
            "unexpected failures: {:?}",
            report.failed_checks()
        );
    }
}
