//! File format detection.
//!
//! Detection order, per spec §9:
//!
//! 1. Magic bytes
//! 2. Container inspection (ISO base media brands, RIFF sub-type)
//! 3. Trusted parser probe — the owning plugin's decoder, during preflight
//! 4. MIME hint
//! 5. Extension, as a weak fallback only
//!
//! The case this exists for: a file named `photo.jpg` whose first bytes are
//! `\x89PNG` **is a PNG**. The extension is evidence, not a verdict, and when
//! the two disagree the user is told.

use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{ConversionError, Result};
use crate::file::FileFormat;

/// Enough for every signature below, including the tar `ustar` marker at
/// offset 257 and ISO-BMFF compatible-brand lists.
const HEADER_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DetectionSource {
    /// A byte signature identified it outright.
    MagicBytes,
    /// A container was recognised and its inner brand or sub-type read.
    Container,
    /// Nothing matched; the extension is all we have.
    Extension,
    /// Nothing matched and the extension means nothing to us either.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Detection {
    pub format: FileFormat,
    /// 1.0 for a byte signature, 0.3 for an extension guess, 0.0 for nothing.
    pub confidence: f32,
    pub source: DetectionSource,
    /// `true` when the extension claims a different format than the bytes do.
    /// The UI must warn; conversion still proceeds using the detected format.
    pub extension_mismatch: bool,
}

impl Detection {
    #[must_use]
    pub fn unknown() -> Self {
        Self {
            format: FileFormat::Unknown,
            confidence: 0.0,
            source: DetectionSource::Unknown,
            extension_mismatch: false,
        }
    }
}

/// Reads the header of `path` and identifies it. Opens the file read-only and
/// reads at most [`HEADER_BYTES`]; it never decodes and never writes.
pub fn detect(path: &Path) -> Result<Detection> {
    crate::paths::ensure_safe_component_bytes(path)?;

    let mut file =
        std::fs::File::open(path).map_err(|err| ConversionError::from_io("open input", &err))?;
    let mut header = [0u8; HEADER_BYTES];
    let read = read_up_to(&mut file, &mut header)
        .map_err(|err| ConversionError::from_io("read input header", &err))?;

    let extension = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase());
    let from_ext = extension.as_deref().and_then(from_extension);

    Ok(
        match detect_header(header.get(..read).unwrap_or_default()) {
            Some(header_format) => {
                // A ZIP might really be an XLSX (or another OOXML container):
                // the signatures are identical, so the members decide. Spec §9's
                // "container inspection" step.
                let format = if header_format == FileFormat::Zip {
                    refine_zip(path).unwrap_or(FileFormat::Zip)
                } else {
                    header_format
                };
                Detection {
                    format,
                    confidence: 1.0,
                    source: if is_container_format(format) {
                        DetectionSource::Container
                    } else {
                        DetectionSource::MagicBytes
                    },
                    // `.jpeg` vs Jpeg is not a mismatch; `.jpg` holding a PNG is.
                    extension_mismatch: from_ext.is_some_and(|claimed| claimed != format),
                }
            }
            None => match from_ext {
                Some(format) => Detection {
                    format,
                    confidence: 0.3,
                    source: DetectionSource::Extension,
                    extension_mismatch: false,
                },
                None => Detection::unknown(),
            },
        },
    )
}

/// `Read::read` may return fewer bytes than asked for without being at EOF.
fn read_up_to(file: &mut impl Read, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        let Some(rest) = buffer.get_mut(filled..) else {
            break;
        };
        match file.read(rest) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(filled)
}

fn at(header: &[u8], offset: usize, needle: &[u8]) -> bool {
    header
        .get(offset..offset.saturating_add(needle.len()))
        .is_some_and(|slice| slice == needle)
}

/// Pure signature matching, split out so it can be fuzzed and unit-tested
/// without touching the filesystem.
#[must_use]
pub fn detect_header(header: &[u8]) -> Option<FileFormat> {
    // --- images -----------------------------------------------------------
    if at(header, 0, b"\x89PNG\r\n\x1a\n") {
        return Some(FileFormat::Png);
    }
    if at(header, 0, b"\xFF\xD8\xFF") {
        return Some(FileFormat::Jpeg);
    }
    if at(header, 0, b"GIF87a") || at(header, 0, b"GIF89a") {
        return Some(FileFormat::Gif);
    }
    if at(header, 0, b"BM") {
        return Some(FileFormat::Bmp);
    }
    if at(header, 0, b"II\x2A\x00") || at(header, 0, b"MM\x00\x2A") {
        return Some(FileFormat::Tiff);
    }

    // --- RIFF containers: the sub-type at offset 8 decides -----------------
    if at(header, 0, b"RIFF") {
        if at(header, 8, b"WEBP") {
            return Some(FileFormat::WebP);
        }
        if at(header, 8, b"WAVE") {
            return Some(FileFormat::Wav);
        }
    }

    // --- ISO base media: `ftyp` box, then the brand ------------------------
    if at(header, 4, b"ftyp") {
        if let Some(format) = iso_brand(header) {
            return Some(format);
        }
    }

    // --- documents and archives -------------------------------------------
    if at(header, 0, b"%PDF-") {
        return Some(FileFormat::Pdf);
    }
    if at(header, 0, b"\x1F\x8B") {
        // A gzip member. Whether it wraps a tar needs decompression, which is
        // Phase 2's job; TarGz is the overwhelmingly common case for us.
        return Some(FileFormat::TarGz);
    }
    if at(header, 0, b"PK\x03\x04") || at(header, 0, b"PK\x05\x06") {
        // XLSX is a ZIP containing `xl/workbook.xml`. Reading the central
        // directory to tell them apart is Phase 4's job.
        return Some(FileFormat::Zip);
    }
    if at(header, 257, b"ustar") {
        return Some(FileFormat::Tar);
    }

    // --- audio -------------------------------------------------------------
    if at(header, 0, b"fLaC") {
        return Some(FileFormat::Flac);
    }
    if at(header, 0, b"OggS") {
        return Some(FileFormat::Ogg);
    }
    if at(header, 0, b"ID3") {
        return Some(FileFormat::Mp3);
    }
    // MPEG audio frame sync: 11 set bits. `FF FE` and `FF FF` are excluded
    // because those are UTF-16 byte-order marks, which collide with the loose
    // frame-sync pattern and are overwhelmingly more likely than a bare MP3
    // frame that happens to start a file.
    if let (Some(&a), Some(&b)) = (header.first(), header.get(1)) {
        if a == 0xFF && (b & 0xE0) == 0xE0 && b != 0xFE && b != 0xFF {
            return Some(FileFormat::Mp3);
        }
    }

    // --- Matroska / WebM ---------------------------------------------------
    if at(header, 0, b"\x1A\x45\xDF\xA3") {
        // Both use EBML; the DocType string appears within the first bytes.
        if find(header, b"webm").is_some() {
            return Some(FileFormat::WebM);
        }
        return Some(FileFormat::Mkv);
    }

    None
}

/// Reads the major brand and the compatible-brand list of an ISO-BMFF file.
fn iso_brand(header: &[u8]) -> Option<FileFormat> {
    let major = header.get(8..12)?;
    if let Some(format) = brand_to_format(major) {
        return Some(format);
    }
    // Compatible brands run from offset 16 in 4-byte entries. `mif1` as a major
    // brand is ambiguous, so the list is what disambiguates HEIC from AVIF.
    let mut offset: usize = 16;
    while let Some(brand) = header.get(offset..offset.saturating_add(4)) {
        if let Some(format) = brand_to_format(brand) {
            return Some(format);
        }
        offset = offset.saturating_add(4);
        if offset > 64 {
            break;
        }
    }
    None
}

fn brand_to_format(brand: &[u8]) -> Option<FileFormat> {
    Some(match brand {
        b"avif" | b"avis" => FileFormat::Avif,
        b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" => FileFormat::Heic,
        b"qt  " => FileFormat::Mov,
        b"M4A " => FileFormat::M4a,
        b"isom" | b"iso2" | b"mp41" | b"mp42" | b"avc1" | b"dash" => FileFormat::Mp4,
        _ => return None,
    })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len().saturating_sub(needle.len()))
        .find(|&i| haystack.get(i..i.saturating_add(needle.len())) == Some(needle))
}

/// Distinguishes an OOXML container (XLSX) from a plain ZIP by its members.
/// An XLSX always contains `xl/workbook.xml`; a `.docx`/`.pptx` would contain
/// `word/`/`ppt/` instead, so those stay ZIP for now. Returns `None` on any
/// read error, leaving the caller to treat it as a plain ZIP.
fn refine_zip(path: &Path) -> Option<FileFormat> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    for i in 0..archive.len() {
        let Ok(entry) = archive.by_index(i) else {
            continue;
        };
        if entry.name() == "xl/workbook.xml" {
            return Some(FileFormat::Xlsx);
        }
    }
    Some(FileFormat::Zip)
}

fn is_container_format(format: FileFormat) -> bool {
    matches!(
        format,
        FileFormat::WebP
            | FileFormat::Wav
            | FileFormat::Avif
            | FileFormat::Heic
            | FileFormat::Mp4
            | FileFormat::Mov
            | FileFormat::M4a
            | FileFormat::Mkv
            | FileFormat::WebM
    )
}

/// The weak fallback. Only consulted when no signature matched.
#[must_use]
pub fn from_extension(extension: &str) -> Option<FileFormat> {
    Some(match extension {
        "jpg" | "jpeg" | "jpe" => FileFormat::Jpeg,
        "png" => FileFormat::Png,
        "webp" => FileFormat::WebP,
        "avif" => FileFormat::Avif,
        "heic" | "heif" => FileFormat::Heic,
        "tif" | "tiff" => FileFormat::Tiff,
        "bmp" => FileFormat::Bmp,
        "gif" => FileFormat::Gif,
        "pdf" => FileFormat::Pdf,
        "csv" => FileFormat::Csv,
        "tsv" => FileFormat::Tsv,
        "json" => FileFormat::Json,
        "xlsx" => FileFormat::Xlsx,
        "mp4" | "m4v" => FileFormat::Mp4,
        "mov" => FileFormat::Mov,
        "mkv" => FileFormat::Mkv,
        "webm" => FileFormat::WebM,
        "mp3" => FileFormat::Mp3,
        "wav" => FileFormat::Wav,
        "flac" => FileFormat::Flac,
        "ogg" | "oga" => FileFormat::Ogg,
        "m4a" | "aac" => FileFormat::M4a,
        "zip" => FileFormat::Zip,
        "tar" => FileFormat::Tar,
        "gz" | "tgz" => FileFormat::TarGz,
        _ => return None,
    })
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

    fn header_of(bytes: &[u8]) -> Option<FileFormat> {
        detect_header(bytes)
    }

    #[test]
    fn image_signatures_are_recognised() {
        assert_eq!(
            header_of(b"\x89PNG\r\n\x1a\n\x00\x00"),
            Some(FileFormat::Png)
        );
        assert_eq!(header_of(b"\xFF\xD8\xFF\xE0JFIF"), Some(FileFormat::Jpeg));
        assert_eq!(header_of(b"GIF89a\x00\x00"), Some(FileFormat::Gif));
        assert_eq!(header_of(b"BM\x36\x00\x00\x00"), Some(FileFormat::Bmp));
        assert_eq!(header_of(b"II\x2A\x00\x08\x00"), Some(FileFormat::Tiff));
        assert_eq!(header_of(b"MM\x00\x2A\x00\x08"), Some(FileFormat::Tiff));
    }

    #[test]
    fn riff_subtype_separates_webp_from_wav() {
        assert_eq!(
            header_of(b"RIFF\x00\x00\x00\x00WEBPVP8 "),
            Some(FileFormat::WebP)
        );
        assert_eq!(
            header_of(b"RIFF\x00\x00\x00\x00WAVEfmt "),
            Some(FileFormat::Wav)
        );
        // A RIFF container we do not know is not guessed at.
        assert_eq!(header_of(b"RIFF\x00\x00\x00\x00AVI LIST"), None);
    }

    #[test]
    fn iso_brands_separate_avif_heic_and_mp4() {
        assert_eq!(
            header_of(b"\x00\x00\x00\x18ftypavif\x00\x00\x00\x00avifmif1"),
            Some(FileFormat::Avif)
        );
        assert_eq!(
            header_of(b"\x00\x00\x00\x18ftypheic\x00\x00\x00\x00heicmif1"),
            Some(FileFormat::Heic)
        );
        assert_eq!(
            header_of(b"\x00\x00\x00\x18ftypisom\x00\x00\x02\x00isomiso2"),
            Some(FileFormat::Mp4)
        );
        assert_eq!(
            header_of(b"\x00\x00\x00\x14ftypqt  \x00\x00\x02\x00qt  "),
            Some(FileFormat::Mov)
        );
    }

    #[test]
    fn an_ambiguous_major_brand_is_resolved_from_compatible_brands() {
        // `mif1` alone says only "some HEIF-family file"; the list decides.
        let avif = b"\x00\x00\x00\x1Cftypmif1\x00\x00\x00\x00mif1avif";
        assert_eq!(header_of(avif), Some(FileFormat::Avif));
    }

    #[test]
    fn documents_archives_and_audio_are_recognised() {
        assert_eq!(header_of(b"%PDF-1.7\n"), Some(FileFormat::Pdf));
        assert_eq!(header_of(b"PK\x03\x04\x14\x00"), Some(FileFormat::Zip));
        assert_eq!(header_of(b"\x1F\x8B\x08\x00"), Some(FileFormat::TarGz));
        assert_eq!(header_of(b"fLaC\x00\x00\x00\x22"), Some(FileFormat::Flac));
        assert_eq!(header_of(b"OggS\x00\x02"), Some(FileFormat::Ogg));
        assert_eq!(header_of(b"ID3\x04\x00"), Some(FileFormat::Mp3));
        assert_eq!(header_of(b"\xFF\xFB\x90\x00"), Some(FileFormat::Mp3));
    }

    #[test]
    fn tar_is_found_by_its_offset_257_marker() {
        let mut header = vec![0u8; 512];
        header[257..262].copy_from_slice(b"ustar");
        assert_eq!(header_of(&header), Some(FileFormat::Tar));
    }

    #[test]
    fn matroska_doctype_separates_mkv_from_webm() {
        let mut webm = b"\x1A\x45\xDF\xA3\x01\x00\x00\x00".to_vec();
        webm.extend_from_slice(b"\x42\x82\x84webm");
        assert_eq!(header_of(&webm), Some(FileFormat::WebM));

        let mut mkv = b"\x1A\x45\xDF\xA3\x01\x00\x00\x00".to_vec();
        mkv.extend_from_slice(b"\x42\x82\x88matroska");
        assert_eq!(header_of(&mkv), Some(FileFormat::Mkv));
    }

    #[test]
    fn nothing_is_guessed_from_an_unrecognised_header() {
        assert_eq!(header_of(b"this is just some text"), None);
        assert_eq!(header_of(b""), None);
        assert_eq!(header_of(b"\x00"), None);
    }

    #[test]
    fn short_and_truncated_headers_do_not_panic() {
        for len in 0..24usize {
            let _ = header_of(&vec![0xFFu8; len]);
            let _ = header_of(&b"\x00\x00\x00\x18ftypavif"[..len.min(12)]);
        }
    }

    #[test]
    fn a_png_named_jpg_is_detected_as_png_and_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holiday.jpg");
        std::fs::write(&path, b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR").unwrap();

        let detection = detect(&path).unwrap();
        assert_eq!(detection.format, FileFormat::Png);
        assert_eq!(detection.confidence, 1.0);
        assert_eq!(detection.source, DetectionSource::MagicBytes);
        assert!(
            detection.extension_mismatch,
            "the user must be warned that the extension lies"
        );
    }

    #[test]
    fn jpg_and_jpeg_extensions_are_not_a_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.jpg", "b.jpeg", "c.JPG"] {
            let path = dir.path().join(name);
            std::fs::write(&path, b"\xFF\xD8\xFF\xE0\x00\x10JFIF").unwrap();
            let detection = detect(&path).unwrap();
            assert_eq!(detection.format, FileFormat::Jpeg);
            assert!(!detection.extension_mismatch, "{name}");
        }
    }

    #[test]
    fn extension_is_a_weak_fallback_not_a_verdict() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.csv");
        std::fs::write(&path, b"id,name\n1,ada\n").unwrap();

        let detection = detect(&path).unwrap();
        assert_eq!(detection.format, FileFormat::Csv);
        assert_eq!(detection.source, DetectionSource::Extension);
        assert!(
            detection.confidence < 0.5,
            "an extension guess must not claim high confidence"
        );
    }

    #[test]
    fn an_unknown_file_stays_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mystery.qqq");
        std::fs::write(&path, b"nothing recognisable here").unwrap();

        let detection = detect(&path).unwrap();
        assert_eq!(detection.format, FileFormat::Unknown);
        assert_eq!(detection.source, DetectionSource::Unknown);
        assert_eq!(detection.confidence, 0.0);
    }

    #[test]
    fn an_empty_file_is_unknown_rather_than_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.png");
        std::fs::write(&path, b"").unwrap();

        // No bytes to match, so the extension is all there is.
        let detection = detect(&path).unwrap();
        assert_eq!(detection.format, FileFormat::Png);
        assert_eq!(detection.source, DetectionSource::Extension);
    }

    #[test]
    fn detection_never_modifies_the_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.png");
        let bytes = b"\x89PNG\r\n\x1a\n original content".to_vec();
        std::fs::write(&path, &bytes).unwrap();

        let _ = detect(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}
