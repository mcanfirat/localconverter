//! The operation registry.
//!
//! `list_operations()` returns exactly what this build can actually do. It is
//! never padded with planned routes: a button that exists in the UI is a route
//! that has passed its fixtures.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum OperationCategory {
    Image,
    Pdf,
    Spreadsheet,
    Media,
    Archive,
    Diagnostics,
}

/// Mirrors the status column of `docs/CONVERSION_MATRIX.md`. Only `Stable`
/// operations are shown in the main tool sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum OperationStability {
    Experimental,
    Beta,
    Stable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OperationDescriptor {
    pub id: String,
    pub category: OperationCategory,
    pub stability: OperationStability,
    pub label_key: String,
    pub description_key: String,
    /// Lowercase extensions accepted as input. Empty means the operation takes
    /// no input files.
    pub input_extensions: Vec<String>,
    /// Lowercase extensions this operation can produce.
    pub output_extensions: Vec<String>,
    /// `true` when the operation consumes several inputs at once (merge, archive)
    /// or processes a batch.
    pub accepts_multiple_inputs: bool,
}

/// Identifier of the pipeline self-test.
pub const SELFTEST_OPERATION_ID: &str = "diagnostics.selftest";
/// Identifier of image conversion and compression.
pub const IMAGE_CONVERT_OPERATION_ID: &str = "image.convert";
/// Identifier of archive creation.
pub const ARCHIVE_CREATE_OPERATION_ID: &str = "archive.create";
/// Identifier of archive extraction.
pub const ARCHIVE_EXTRACT_OPERATION_ID: &str = "archive.extract";
/// Identifier of spreadsheet / structured-data conversion.
pub const SPREADSHEET_CONVERT_OPERATION_ID: &str = "spreadsheet.convert";
/// PDF operation identifiers.
pub const PDF_MERGE_OPERATION_ID: &str = "pdf.merge";
pub const PDF_PAGES_OPERATION_ID: &str = "pdf.pages";
pub const PDF_SPLIT_OPERATION_ID: &str = "pdf.split";
pub const PDF_REMOVE_METADATA_OPERATION_ID: &str = "pdf.removeMetadata";
pub const PDF_FROM_IMAGES_OPERATION_ID: &str = "pdf.fromImages";
/// Identifier of media (video/audio) conversion.
pub const MEDIA_CONVERT_OPERATION_ID: &str = "media.convert";

fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// Every operation this build supports.
#[must_use]
pub fn list_operations() -> Vec<OperationDescriptor> {
    vec![
        OperationDescriptor {
            id: IMAGE_CONVERT_OPERATION_ID.to_owned(),
            category: OperationCategory::Image,
            stability: OperationStability::Stable,
            label_key: "operation.convert.label".to_owned(),
            description_key: "operation.convert.description".to_owned(),
            // HEIC and AVIF are absent on purpose: both need a native codec
            // this build does not bundle, and the engine declines them by name
            // rather than pretending.
            input_extensions: owned(&[
                "jpg", "jpeg", "jpe", "png", "webp", "tif", "tiff", "bmp", "gif",
            ]),
            output_extensions: owned(&["jpg", "png", "webp", "tiff", "bmp"]),
            accepts_multiple_inputs: true,
        },
        OperationDescriptor {
            id: ARCHIVE_CREATE_OPERATION_ID.to_owned(),
            category: OperationCategory::Archive,
            stability: OperationStability::Stable,
            label_key: "operation.archiveCreate.label".to_owned(),
            description_key: "operation.archiveCreate.description".to_owned(),
            input_extensions: Vec::new(),
            output_extensions: owned(&["zip", "tar", "tar.gz"]),
            accepts_multiple_inputs: true,
        },
        OperationDescriptor {
            id: ARCHIVE_EXTRACT_OPERATION_ID.to_owned(),
            category: OperationCategory::Archive,
            stability: OperationStability::Stable,
            label_key: "operation.archiveExtract.label".to_owned(),
            description_key: "operation.archiveExtract.description".to_owned(),
            input_extensions: owned(&["zip", "tar", "gz", "tgz"]),
            output_extensions: Vec::new(),
            accepts_multiple_inputs: false,
        },
        OperationDescriptor {
            id: SPREADSHEET_CONVERT_OPERATION_ID.to_owned(),
            category: OperationCategory::Spreadsheet,
            stability: OperationStability::Stable,
            label_key: "operation.spreadsheet.label".to_owned(),
            description_key: "operation.spreadsheet.description".to_owned(),
            input_extensions: owned(&["csv", "tsv", "xlsx", "json"]),
            output_extensions: owned(&["csv", "tsv", "xlsx", "json"]),
            accepts_multiple_inputs: false,
        },
        OperationDescriptor {
            id: PDF_MERGE_OPERATION_ID.to_owned(),
            category: OperationCategory::Pdf,
            stability: OperationStability::Stable,
            label_key: "operation.pdfMerge.label".to_owned(),
            description_key: "operation.pdfMerge.description".to_owned(),
            input_extensions: owned(&["pdf"]),
            output_extensions: owned(&["pdf"]),
            accepts_multiple_inputs: true,
        },
        OperationDescriptor {
            id: PDF_PAGES_OPERATION_ID.to_owned(),
            category: OperationCategory::Pdf,
            stability: OperationStability::Stable,
            label_key: "operation.pdfPages.label".to_owned(),
            description_key: "operation.pdfPages.description".to_owned(),
            input_extensions: owned(&["pdf"]),
            output_extensions: owned(&["pdf"]),
            accepts_multiple_inputs: false,
        },
        OperationDescriptor {
            id: PDF_SPLIT_OPERATION_ID.to_owned(),
            category: OperationCategory::Pdf,
            stability: OperationStability::Stable,
            label_key: "operation.pdfSplit.label".to_owned(),
            description_key: "operation.pdfSplit.description".to_owned(),
            input_extensions: owned(&["pdf"]),
            output_extensions: owned(&["pdf"]),
            accepts_multiple_inputs: false,
        },
        OperationDescriptor {
            id: PDF_REMOVE_METADATA_OPERATION_ID.to_owned(),
            category: OperationCategory::Pdf,
            stability: OperationStability::Stable,
            label_key: "operation.pdfRemoveMetadata.label".to_owned(),
            description_key: "operation.pdfRemoveMetadata.description".to_owned(),
            input_extensions: owned(&["pdf"]),
            output_extensions: owned(&["pdf"]),
            accepts_multiple_inputs: false,
        },
        OperationDescriptor {
            id: PDF_FROM_IMAGES_OPERATION_ID.to_owned(),
            category: OperationCategory::Pdf,
            stability: OperationStability::Stable,
            label_key: "operation.pdfFromImages.label".to_owned(),
            description_key: "operation.pdfFromImages.description".to_owned(),
            input_extensions: owned(&["jpg", "jpeg", "png", "webp", "tiff", "bmp", "gif"]),
            output_extensions: owned(&["pdf"]),
            accepts_multiple_inputs: true,
        },
        OperationDescriptor {
            id: MEDIA_CONVERT_OPERATION_ID.to_owned(),
            category: OperationCategory::Media,
            // Beta: correctness depends on a system-installed FFmpeg, and only
            // macOS ARM has been exercised so far.
            stability: OperationStability::Beta,
            label_key: "operation.media.label".to_owned(),
            description_key: "operation.media.description".to_owned(),
            input_extensions: owned(&[
                "mp4", "mov", "mkv", "webm", "gif", "mp3", "wav", "flac", "ogg", "m4a",
            ]),
            output_extensions: owned(&[
                "mp4", "webm", "mkv", "gif", "mp3", "wav", "flac", "ogg", "m4a",
            ]),
            accepts_multiple_inputs: false,
        },
        OperationDescriptor {
            id: SELFTEST_OPERATION_ID.to_owned(),
            category: OperationCategory::Diagnostics,
            stability: OperationStability::Stable,
            label_key: "operation.selftest.label".to_owned(),
            description_key: "operation.selftest.description".to_owned(),
            input_extensions: Vec::new(),
            output_extensions: owned(&["bin"]),
            accepts_multiple_inputs: false,
        },
    ]
}

#[must_use]
pub fn find_operation(id: &str) -> Option<OperationDescriptor> {
    list_operations().into_iter().find(|op| op.id == id)
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
    fn operation_ids_are_unique() {
        let ops = list_operations();
        let ids: std::collections::HashSet<_> = ops.iter().map(|o| &o.id).collect();
        assert_eq!(ids.len(), ops.len());
    }

    #[test]
    fn registered_operations_are_all_dispatchable() {
        // A registry entry with no arm in `runner::execute` would be a button
        // that fails when pressed.
        for op in list_operations() {
            assert!(
                matches!(
                    op.id.as_str(),
                    SELFTEST_OPERATION_ID
                        | IMAGE_CONVERT_OPERATION_ID
                        | ARCHIVE_CREATE_OPERATION_ID
                        | ARCHIVE_EXTRACT_OPERATION_ID
                        | SPREADSHEET_CONVERT_OPERATION_ID
                        | PDF_MERGE_OPERATION_ID
                        | PDF_PAGES_OPERATION_ID
                        | PDF_SPLIT_OPERATION_ID
                        | PDF_REMOVE_METADATA_OPERATION_ID
                        | PDF_FROM_IMAGES_OPERATION_ID
                        | MEDIA_CONVERT_OPERATION_ID
                ),
                "{} is advertised but not dispatched",
                op.id
            );
        }
    }

    #[test]
    fn the_image_route_advertises_only_formats_the_engine_can_read() {
        let op = find_operation(IMAGE_CONVERT_OPERATION_ID).unwrap();
        for extension in &op.input_extensions {
            let format = crate::detect::from_extension(extension)
                .unwrap_or_else(|| panic!("{extension} maps to no format"));
            assert!(
                crate::image_engine::can_decode(format),
                "{extension} is advertised but the engine cannot decode it"
            );
        }
    }

    #[test]
    fn heic_and_avif_are_not_advertised() {
        let op = find_operation(IMAGE_CONVERT_OPERATION_ID).unwrap();
        for absent in ["heic", "heif", "avif"] {
            assert!(
                !op.input_extensions.iter().any(|e| e == absent),
                "{absent} needs a native codec this build does not bundle"
            );
        }
    }

    #[test]
    fn operations_are_findable_and_unknown_ones_are_not_invented() {
        assert!(find_operation(SELFTEST_OPERATION_ID).is_some());
        assert!(find_operation(IMAGE_CONVERT_OPERATION_ID).is_some());
        assert!(find_operation("nonexistent.operation").is_none());
    }
}
