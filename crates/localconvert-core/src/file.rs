//! Input file description.
//!
//! Format comes from [`crate::detect`], which reads the file's bytes. The
//! extension is only consulted when no signature matches, and when the two
//! disagree `extension_mismatch` is set so the user can be warned.

use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::error::{ConversionError, Result};

/// Formats LocalConvert intends to handle. Present in full from Phase 0 so the
/// conversion matrix, the UI and the CLI share one vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FileFormat {
    // Images
    Jpeg,
    Png,
    #[serde(rename = "webp")]
    WebP,
    Avif,
    Heic,
    Tiff,
    Bmp,
    Gif,
    // Documents
    Pdf,
    // Structured data
    Csv,
    Tsv,
    Json,
    Xlsx,
    // Media
    Mp4,
    Mov,
    Mkv,
    #[serde(rename = "webm")]
    WebM,
    Mp3,
    Wav,
    Flac,
    Ogg,
    M4a,
    // Archives
    Zip,
    Tar,
    TarGz,
    /// Not yet identified. The only value Phase 0 ever produces.
    Unknown,
}

impl FileFormat {
    /// Canonical lowercase extension, used when naming outputs.
    #[must_use]
    pub fn extension(self) -> Option<&'static str> {
        Some(match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::WebP => "webp",
            Self::Avif => "avif",
            Self::Heic => "heic",
            Self::Tiff => "tiff",
            Self::Bmp => "bmp",
            Self::Gif => "gif",
            Self::Pdf => "pdf",
            Self::Csv => "csv",
            Self::Tsv => "tsv",
            Self::Json => "json",
            Self::Xlsx => "xlsx",
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
            Self::Mkv => "mkv",
            Self::WebM => "webm",
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::Ogg => "ogg",
            Self::M4a => "m4a",
            Self::Zip => "zip",
            Self::Tar => "tar",
            Self::TarGz => "tar.gz",
            Self::Unknown => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileMetadata {
    pub modified_at: Option<String>,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileDescriptor {
    pub id: Uuid,
    pub path: String,
    /// File name only — the UI never renders a full path, so a screenshot of a
    /// job cannot leak the user's directory structure.
    pub display_name: String,
    #[ts(type = "number")]
    pub size_bytes: u64,
    pub extension: Option<String>,
    /// Extension-derived MIME hint from the OS, if any. A hint, never a decision.
    pub declared_mime: Option<String>,
    pub detected_format: FileFormat,
    /// 1.0 for a byte signature, 0.3 for an extension guess, 0.0 for nothing.
    pub detection_confidence: f32,
    /// `true` when the extension claims a format the bytes contradict.
    pub extension_mismatch: bool,
    pub metadata: FileMetadata,
}

impl FileDescriptor {
    /// Reads the filesystem facts about `path` and identifies its format from
    /// its leading bytes. Decodes nothing, and never modifies the file.
    pub fn probe(path: &Path) -> Result<Self> {
        crate::paths::ensure_safe_component_bytes(path)?;

        let metadata = std::fs::metadata(path).map_err(|err| {
            // A missing *input* is a bad-input error, not a destination problem —
            // `from_io` would otherwise map NotFound to DestinationUnavailable,
            // which is the wrong story (and the wrong CLI exit code) for an input.
            if err.kind() == std::io::ErrorKind::NotFound {
                ConversionError::new(
                    crate::error::ConversionErrorCode::InvalidInput,
                    format!("input file not found: {}", crate::error::display_name(path)),
                )
                .with_message_key("error.input.notFound")
            } else {
                ConversionError::from_io("read input metadata", &err)
            }
        })?;

        if !metadata.is_file() {
            return Err(ConversionError::new(
                crate::error::ConversionErrorCode::InvalidInput,
                "input is not a regular file",
            )
            .with_message_key("error.input.notAFile"));
        }

        let modified_at = metadata
            .modified()
            .ok()
            .map(|time| chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339());

        let detection = crate::detect::detect(path)?;

        Ok(Self {
            id: Uuid::new_v4(),
            path: path.to_string_lossy().into_owned(),
            display_name: crate::error::display_name(path),
            size_bytes: metadata.len(),
            extension: path
                .extension()
                .map(|ext| ext.to_string_lossy().to_lowercase()),
            declared_mime: None,
            detected_format: detection.format,
            detection_confidence: detection.confidence,
            extension_mismatch: detection.extension_mismatch,
            metadata: FileMetadata {
                modified_at,
                read_only: metadata.permissions().readonly(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn probe_reads_size_name_and_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ünïcode tëst.HEIC");
        std::fs::write(&path, b"0123456789").unwrap();

        let fd = FileDescriptor::probe(&path).unwrap();
        assert_eq!(fd.size_bytes, 10);
        assert_eq!(
            fd.detected_format,
            FileFormat::Heic,
            "falls back to the extension"
        );
        assert_eq!(fd.display_name, "ünïcode tëst.HEIC");
        assert_eq!(fd.extension.as_deref(), Some("heic"));
    }

    #[test]
    fn probe_trusts_the_bytes_over_the_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.jpg");
        // PNG magic bytes behind a .jpg extension — the case the spec calls out.
        std::fs::write(&path, b"\x89PNG\r\n\x1a\n").unwrap();

        let fd = FileDescriptor::probe(&path).unwrap();
        assert_eq!(fd.detected_format, FileFormat::Png);
        assert_eq!(fd.detection_confidence, 1.0);
        assert!(fd.extension_mismatch, "the user must be warned");
    }

    #[test]
    fn probe_rejects_directories() {
        let dir = tempfile::tempdir().unwrap();
        assert!(FileDescriptor::probe(dir.path()).is_err());
    }

    #[test]
    fn probe_leaves_the_source_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.bin");
        std::fs::write(&path, b"original bytes").unwrap();
        let before = std::fs::read(&path).unwrap();

        let _ = FileDescriptor::probe(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn every_known_format_has_an_extension() {
        for format in [FileFormat::Jpeg, FileFormat::TarGz, FileFormat::Xlsx] {
            assert!(format.extension().is_some());
        }
        assert!(FileFormat::Unknown.extension().is_none());
    }
}
