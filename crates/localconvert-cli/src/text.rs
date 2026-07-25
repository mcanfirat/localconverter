//! Human text for the terminal.
//!
//! Errors already carry an authored English `detail` (redacted at construction),
//! so the CLI leads with that. Warnings carry only a key, so their prose lives
//! here — and `every_warning_key_has_prose` fails the build if the core starts
//! emitting one this table does not cover, which is what stopped raw
//! identifiers like `warning.image.lossyEncoding` reaching a user's terminal.

/// One line of prose for a warning key.
#[must_use]
pub fn warning(key: &str) -> &'static str {
    match key {
        "warning.destination.overwritten" => "an existing file was replaced",
        "warning.destination.skipped" => "skipped: a file with that name already exists",
        "warning.output.largerThanInput" => "the result came out larger than the original",

        "warning.image.animationFlattened" => {
            "only the first frame was exported; the chosen format cannot store animation"
        }
        "warning.image.extensionMismatch" => {
            "a file's contents did not match its extension; the real format was used"
        }
        "warning.image.lossyEncoding" => "JPEG is a lossy format, so the image was re-compressed",
        "warning.image.metadataRemoved" => "metadata such as camera details and GPS was removed",
        "warning.image.transparencyFlattened" => {
            "transparent areas were filled with the background colour"
        }
        "warning.image.webpLossless" => {
            "WebP was written losslessly, which keeps every pixel but is larger than lossy WebP"
        }

        "warning.media.gifNoAudio" => "GIF has no audio, so the audio track was dropped",
        "warning.media.lossyEncoding" => "the audio was re-compressed, so some quality is lost",
        "warning.media.videoReencoded" => "the video was re-encoded, so some quality is lost",

        "warning.pdf.signatureInvalidated" => {
            "this PDF appears to be signed; editing it invalidates the signature"
        }

        "warning.spreadsheet.coercionKeptAsText" => {
            "some values could not be converted to the requested type and were kept as text"
        }
        "warning.spreadsheet.encodingReplaced" => {
            "some characters could not be decoded and were replaced"
        }
        "warning.spreadsheet.generatedKeys" => {
            "the source had no header row, so JSON keys were generated (Column 1, …)"
        }
        "warning.spreadsheet.jsonFlattened" => {
            "nested JSON values were flattened to text; a table cannot hold nested structure"
        }
        "warning.spreadsheet.oneSheetOnly" => {
            "the workbook has several sheets; only the selected one was converted"
        }
        "warning.spreadsheet.xlsxFeaturesLost" => {
            "formulas, styles, merged cells and extra sheets are not carried into a flat format"
        }

        // Unknown keys fall back to the key itself rather than swallowing the
        // information — but the test below means that should never ship.
        other => {
            debug_assert!(false, "no CLI prose for warning key {other}");
            "see the app for details"
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// Scans the core crate for every `warning.*` key it can emit and fails if
    /// any of them would print as a raw identifier. Mirrors the frontend's
    /// `messages.test.ts`, so neither surface can drift from the engines.
    #[test]
    fn every_warning_key_has_prose() {
        let core = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../localconvert-core/src")
            .canonicalize()
            .expect("core crate sources");

        let mut keys = std::collections::BTreeSet::new();
        for entry in std::fs::read_dir(&core).expect("read core src") {
            let path = entry.expect("dir entry").path();
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read source");
            for (index, _) in source.match_indices("\"warning.") {
                let rest = &source[index + 1..];
                if let Some(end) = rest.find('"') {
                    keys.insert(rest[..end].to_owned());
                }
            }
        }

        assert!(
            keys.len() > 10,
            "expected to find the warning keys, got {keys:?}"
        );
        let missing: Vec<&String> = keys
            .iter()
            .filter(|key| warning(key) == "see the app for details")
            .collect();
        assert!(
            missing.is_empty(),
            "these warning keys would print as raw identifiers: {missing:?}"
        );
    }
}
