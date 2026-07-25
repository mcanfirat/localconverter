//! Stable exit codes.
//!
//! These are part of the CLI's contract — scripts branch on them, so they do not
//! change without a version bump. Documented in `docs/CLI.md`.

use localconvert_core::ConversionErrorCode;

#[allow(dead_code)] // documented for completeness; success uses ExitCode::SUCCESS
pub const SUCCESS: i32 = 0;
/// Usage error: bad arguments, unknown option.
pub const USAGE: i32 = 1;
/// The input could not be used: unsupported format, corrupt, not found.
pub const INPUT: i32 = 2;
/// The conversion ran but its output failed verification, or the engine failed.
pub const CONVERSION: i32 = 3;
/// A required external tool (FFmpeg) is missing.
pub const TOOL_MISSING: i32 = 4;
/// Interrupted.
pub const CANCELLED: i32 = 130;

/// Maps a domain error onto its stable exit code.
#[must_use]
pub fn code_for(code: ConversionErrorCode) -> i32 {
    use ConversionErrorCode as E;
    match code {
        E::UnsupportedFormat
        | E::UnsupportedCodec
        | E::InvalidInput
        | E::CorruptedInput
        | E::PasswordRequired
        | E::InvalidPassword => INPUT,
        E::ToolMissing | E::ToolChecksumMismatch => TOOL_MISSING,
        E::Cancelled => CANCELLED,
        E::OutputValidationFailed
        | E::OutputLargerThanInput
        | E::ProcessFailed
        | E::ProcessTimedOut
        | E::ArchiveUnsafe => CONVERSION,
        E::InsufficientDiskSpace
        | E::InsufficientMemory
        | E::PermissionDenied
        | E::DestinationUnavailable
        | E::InternalError => CONVERSION,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_missing_and_cancelled_have_their_own_codes() {
        assert_eq!(code_for(ConversionErrorCode::ToolMissing), TOOL_MISSING);
        assert_eq!(code_for(ConversionErrorCode::Cancelled), CANCELLED);
    }

    #[test]
    fn input_problems_map_to_the_input_code() {
        assert_eq!(code_for(ConversionErrorCode::UnsupportedFormat), INPUT);
        assert_eq!(code_for(ConversionErrorCode::CorruptedInput), INPUT);
    }

    #[test]
    fn validation_failure_maps_to_the_conversion_code() {
        assert_eq!(
            code_for(ConversionErrorCode::OutputValidationFailed),
            CONVERSION
        );
    }
}
