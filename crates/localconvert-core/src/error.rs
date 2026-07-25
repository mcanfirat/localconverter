//! Structured domain errors.
//!
//! Every failure the user can observe is one of these. The UI never sees a raw
//! `io::Error` string, and diagnostics that a user may paste into a bug report
//! have filesystem paths redacted first (see [`redact`]).

use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Stable machine-readable failure taxonomy. These values are part of the CLI
/// contract (`--json`) and must not be renamed without a version bump.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ConversionErrorCode {
    UnsupportedFormat,
    UnsupportedCodec,
    InvalidInput,
    CorruptedInput,
    PasswordRequired,
    InvalidPassword,
    InsufficientDiskSpace,
    InsufficientMemory,
    PermissionDenied,
    DestinationUnavailable,
    ToolMissing,
    ToolChecksumMismatch,
    ProcessFailed,
    ProcessTimedOut,
    Cancelled,
    OutputValidationFailed,
    OutputLargerThanInput,
    ArchiveUnsafe,
    InternalError,
}

impl ConversionErrorCode {
    /// Default i18n key for this code. Call sites may override it with
    /// something more specific via [`ConversionError::with_message_key`].
    #[must_use]
    pub fn message_key(self) -> &'static str {
        match self {
            Self::UnsupportedFormat => "error.unsupportedFormat",
            Self::UnsupportedCodec => "error.unsupportedCodec",
            Self::InvalidInput => "error.invalidInput",
            Self::CorruptedInput => "error.corruptedInput",
            Self::PasswordRequired => "error.passwordRequired",
            Self::InvalidPassword => "error.invalidPassword",
            Self::InsufficientDiskSpace => "error.insufficientDiskSpace",
            Self::InsufficientMemory => "error.insufficientMemory",
            Self::PermissionDenied => "error.permissionDenied",
            Self::DestinationUnavailable => "error.destinationUnavailable",
            Self::ToolMissing => "error.toolMissing",
            Self::ToolChecksumMismatch => "error.toolChecksumMismatch",
            Self::ProcessFailed => "error.processFailed",
            Self::ProcessTimedOut => "error.processTimedOut",
            Self::Cancelled => "error.cancelled",
            Self::OutputValidationFailed => "error.outputValidationFailed",
            Self::OutputLargerThanInput => "error.outputLargerThanInput",
            Self::ArchiveUnsafe => "error.archiveUnsafe",
            Self::InternalError => "error.internalError",
        }
    }
}

/// A single error type rather than one enum variant per code: the code *is* the
/// discriminant, and the extra fields are the ones the UI must render to satisfy
/// the "what happened / is my source safe / was partial output removed" contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
#[error("{code:?}: {detail}")]
pub struct ConversionError {
    pub code: ConversionErrorCode,
    /// i18n key resolved by the frontend into a user-facing explanation.
    pub message_key: String,
    /// Technical detail, shown only behind the "technical details" toggle.
    /// Already redacted — see [`redact`].
    pub detail: String,
    /// `true` when the input files are known to be byte-for-byte untouched.
    pub source_safe: bool,
    /// `true` when an invalid or partial output was deleted or quarantined.
    pub partial_output_removed: bool,
}

impl ConversionError {
    #[must_use]
    pub fn new(code: ConversionErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            message_key: code.message_key().to_owned(),
            detail: redact(&detail.into()),
            // Sources are only ever read, so the default is the honest one. Any
            // code path that could touch an input must clear this explicitly.
            source_safe: true,
            partial_output_removed: false,
        }
    }

    #[must_use]
    pub fn internal(detail: impl Into<String>) -> Self {
        Self::new(ConversionErrorCode::InternalError, detail)
    }

    #[must_use]
    pub fn cancelled() -> Self {
        Self::new(ConversionErrorCode::Cancelled, "job cancelled by user")
            .with_partial_output_removed(true)
    }

    #[must_use]
    pub fn with_message_key(mut self, key: impl Into<String>) -> Self {
        self.message_key = key.into();
        self
    }

    #[must_use]
    pub fn with_partial_output_removed(mut self, removed: bool) -> Self {
        self.partial_output_removed = removed;
        self
    }

    #[must_use]
    pub fn with_source_safe(mut self, safe: bool) -> Self {
        self.source_safe = safe;
        self
    }

    /// Maps an `io::Error` onto the closest domain code. Used at every
    /// filesystem boundary so no raw io error escapes the core.
    #[must_use]
    pub fn from_io(context: &str, err: &std::io::Error) -> Self {
        use std::io::ErrorKind;
        let code = match err.kind() {
            ErrorKind::PermissionDenied => ConversionErrorCode::PermissionDenied,
            ErrorKind::NotFound => ConversionErrorCode::DestinationUnavailable,
            ErrorKind::StorageFull => ConversionErrorCode::InsufficientDiskSpace,
            ErrorKind::TimedOut => ConversionErrorCode::ProcessTimedOut,
            _ => ConversionErrorCode::InternalError,
        };
        Self::new(code, format!("{context}: {err}"))
    }
}

/// Strips user-identifying path prefixes out of text destined for a diagnostics
/// bundle. The home directory is the realistic leak (it contains the account
/// name); temp roots live under it on every supported platform.
///
/// ponytail: prefix substitution only. If diagnostics ever include engine
/// stderr with paths in exotic encodings, swap in a component-wise scrubber.
#[must_use]
pub fn redact(text: &str) -> String {
    let Some(home) = home_dir() else {
        return text.to_owned();
    };
    let Some(home) = home.to_str() else {
        return text.to_owned();
    };
    if home.is_empty() {
        return text.to_owned();
    }
    text.replace(home, "<home>")
}

fn home_dir() -> Option<std::path::PathBuf> {
    // std::env::home_dir is deprecated for its Windows behaviour; the env vars
    // below are what every platform actually sets.
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return Some(std::path::PathBuf::from(home));
        }
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        if !profile.is_empty() {
            return Some(std::path::PathBuf::from(profile));
        }
    }
    None
}

/// Convenience for reporting a path in an error without leaking its parents.
#[must_use]
pub fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || "<unnamed>".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    )
}

pub type Result<T> = std::result::Result<T, ConversionError>;

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
    fn every_code_has_a_distinct_message_key() {
        let codes = [
            ConversionErrorCode::UnsupportedFormat,
            ConversionErrorCode::UnsupportedCodec,
            ConversionErrorCode::InvalidInput,
            ConversionErrorCode::CorruptedInput,
            ConversionErrorCode::PasswordRequired,
            ConversionErrorCode::InvalidPassword,
            ConversionErrorCode::InsufficientDiskSpace,
            ConversionErrorCode::InsufficientMemory,
            ConversionErrorCode::PermissionDenied,
            ConversionErrorCode::DestinationUnavailable,
            ConversionErrorCode::ToolMissing,
            ConversionErrorCode::ToolChecksumMismatch,
            ConversionErrorCode::ProcessFailed,
            ConversionErrorCode::ProcessTimedOut,
            ConversionErrorCode::Cancelled,
            ConversionErrorCode::OutputValidationFailed,
            ConversionErrorCode::OutputLargerThanInput,
            ConversionErrorCode::ArchiveUnsafe,
            ConversionErrorCode::InternalError,
        ];
        let keys: std::collections::HashSet<_> = codes.iter().map(|c| c.message_key()).collect();
        assert_eq!(keys.len(), codes.len(), "message keys must be unique");
    }

    #[test]
    fn detail_is_redacted_on_construction() {
        let home = home_dir().expect("test host must have HOME or USERPROFILE");
        let leaky = format!("failed to open {}/Documents/secret.heic", home.display());
        let err = ConversionError::internal(leaky);
        assert!(!err.detail.contains(&*home.to_string_lossy()));
        assert!(err.detail.contains("<home>/Documents/secret.heic"));
    }

    #[test]
    fn cancelled_reports_partial_output_removed_and_safe_source() {
        let err = ConversionError::cancelled();
        assert_eq!(err.code, ConversionErrorCode::Cancelled);
        assert!(err.source_safe);
        assert!(err.partial_output_removed);
    }

    #[test]
    fn io_errors_map_to_domain_codes() {
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert_eq!(
            ConversionError::from_io("write", &denied).code,
            ConversionErrorCode::PermissionDenied
        );
        let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert_eq!(
            ConversionError::from_io("open", &missing).code,
            ConversionErrorCode::DestinationUnavailable
        );
    }

    #[test]
    fn display_name_never_leaks_parent_directories() {
        let path = Path::new("/Users/someone/private/holiday.heic");
        assert_eq!(display_name(path), "holiday.heic");
    }
}
