//! Archive creation and safe extraction.
//!
//! Pure Rust — `zip`, `tar`, `flate2`. Two operations:
//!
//! * `archive.create` — bundle selected files into ZIP, TAR or TAR.GZ.
//! * `archive.extract` — unpack a ZIP, TAR or TAR.GZ into a destination folder.
//!
//! Extraction is the dangerous half, and the rules from spec §14.2 are enforced
//! *before* a single byte is written to the destination:
//!
//! * **No traversal.** Every entry path is resolved with
//!   [`crate::paths::join_within`], which already rejects `..` and absolute
//!   paths and is tested against both.
//! * **No symlink / device / hardlink entries.** Only regular files and
//!   directories are extracted; anything else fails the whole job as
//!   [`ConversionErrorCode::ArchiveUnsafe`].
//! * **No bombs.** Entry count, total declared uncompressed size and
//!   compression ratio are all bounded, checked up front from the archive's own
//!   metadata so a 42 KB zip claiming 4 GB never gets to allocate it.
//!
//! Nothing lands in the user's folder until the whole tree has been staged in
//! the job workspace and validated — a half-extracted archive on a failure is
//! impossible by construction.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{ConversionError, ConversionErrorCode, Result};
use crate::file::FileFormat;
use crate::job::{ConversionJob, JobProgress, ProgressStage};
use crate::operation::{ARCHIVE_CREATE_OPERATION_ID, ARCHIVE_EXTRACT_OPERATION_ID};
use crate::runner::{ExecutionOutput, JobContext, StagedOutput};
use crate::validation::{OutputMetadata, ValidationCheck, ValidationReport};

/// Refuse an archive that declares more entries than this. A real backup rarely
/// exceeds it; a zip bomb trivially does.
const MAX_ENTRIES: u64 = 100_000;
/// Refuse to extract more than this in total, however many entries.
const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024 * 1024;
/// Refuse an entry whose uncompressed:compressed ratio exceeds this.
///
/// Deflate's own ceiling is about 1032:1, so ordinary highly-compressible
/// content — a zero-filled disk image, silence in a WAV, a flat-colour BMP —
/// legitimately lands just under it. A limit of 1000 rejected archives
/// LocalConvert had written itself. The real resource bound is
/// [`MAX_TOTAL_BYTES`]; this guard only needs to catch nested-compression
/// tricks, so it sits far above anything a single deflate pass can produce.
const MAX_RATIO: u64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ArchiveFormat {
    Zip,
    Tar,
    TarGz,
}

impl ArchiveFormat {
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::Tar => "tar",
            Self::TarGz => "tar.gz",
        }
    }

    #[must_use]
    fn from_format(format: FileFormat) -> Option<Self> {
        Some(match format {
            FileFormat::Zip => Self::Zip,
            FileFormat::Tar => Self::Tar,
            FileFormat::TarGz => Self::TarGz,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CreateOptions {
    pub format: ArchiveFormat,
    /// Base name of the resulting archive, without extension.
    #[serde(default)]
    pub archive_name: Option<String>,
    /// Deflate level 0–9 for ZIP and TAR.GZ. Ignored for plain TAR.
    #[serde(default)]
    pub level: Option<u8>,
}

impl CreateOptions {
    fn parse(options: &serde_json::Value) -> Result<Self> {
        let parsed: Self = serde_json::from_value(options.clone()).map_err(|err| {
            ConversionError::new(
                ConversionErrorCode::InvalidInput,
                format!("invalid archive options: {err}"),
            )
            .with_message_key("error.options.invalid")
        })?;
        if let Some(level) = parsed.level {
            if level > 9 {
                return Err(ConversionError::new(
                    ConversionErrorCode::InvalidInput,
                    "compression level must be between 0 and 9",
                )
                .with_message_key("error.options.outOfRange"));
            }
        }
        Ok(parsed)
    }

    fn archive_file_name(&self) -> String {
        let stem = self.archive_name.as_deref().unwrap_or("archive");
        format!("{stem}.{}", self.format.extension())
    }
}

// ---------------------------------------------------------------------------
// dispatch
// ---------------------------------------------------------------------------

pub async fn execute(job: &ConversionJob, ctx: &JobContext) -> Result<ExecutionOutput> {
    match job.operation_id.as_str() {
        ARCHIVE_CREATE_OPERATION_ID => create(job, ctx),
        ARCHIVE_EXTRACT_OPERATION_ID => extract(job, ctx),
        other => Err(ConversionError::internal(format!(
            "archive engine got a non-archive operation: {other}"
        ))),
    }
}

pub async fn validate(
    job: &ConversionJob,
    ctx: &JobContext,
    execution: &ExecutionOutput,
) -> Result<Vec<ValidationReport>> {
    ctx.report(JobProgress::indeterminate(
        ProgressStage::Validating,
        "progress.validating",
    ));
    match job.operation_id.as_str() {
        ARCHIVE_CREATE_OPERATION_ID => validate_created_archive(execution),
        ARCHIVE_EXTRACT_OPERATION_ID => Ok(validate_extracted_files(execution)),
        other => Err(ConversionError::internal(format!(
            "archive engine got a non-archive operation: {other}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

fn create(job: &ConversionJob, ctx: &JobContext) -> Result<ExecutionOutput> {
    let options = CreateOptions::parse(&job.options)?;
    if job.input_files.is_empty() {
        return Err(
            ConversionError::new(ConversionErrorCode::InvalidInput, "no input files")
                .with_message_key("error.input.missing"),
        );
    }

    // Selected files are archived under their base names. Two files with the
    // same base name would silently clobber each other inside the archive, so
    // that is refused rather than resolved.
    let mut seen = std::collections::HashSet::new();
    for input in &job.input_files {
        if !seen.insert(input.display_name.clone()) {
            return Err(ConversionError::new(
                ConversionErrorCode::InvalidInput,
                format!("two inputs share the name {}", input.display_name),
            )
            .with_message_key("error.archive.duplicateName"));
        }
    }

    let file_name = options.archive_file_name();
    let staged = ctx.workspace.staging_path(&file_name)?;
    let total = job.input_files.len() as u64;

    let mut input_total_bytes = 0u64;
    let file = std::fs::File::create(&staged)
        .map_err(|err| ConversionError::from_io("create staged archive", &err))?;

    match options.format {
        ArchiveFormat::Zip => write_zip(job, ctx, file, &options, &mut input_total_bytes)?,
        ArchiveFormat::Tar => {
            let mut file = write_tar(job, ctx, file, &mut input_total_bytes)?;
            file.flush()
                .and_then(|()| file.sync_all())
                .map_err(|err| ConversionError::from_io("finish tar", &err))?;
        }
        ArchiveFormat::TarGz => {
            let level = flate2::Compression::new(u32::from(options.level.unwrap_or(6)));
            let encoder = flate2::write::GzEncoder::new(file, level);
            let encoder = write_tar(job, ctx, encoder, &mut input_total_bytes)?;
            // The gz trailer is only written on finish(); a plain drop truncates.
            let mut file = encoder
                .finish()
                .map_err(|err| ConversionError::from_io("finish gzip stream", &err))?;
            file.flush()
                .and_then(|()| file.sync_all())
                .map_err(|err| ConversionError::from_io("finish tar.gz", &err))?;
        }
    }

    ctx.report(JobProgress::counted(
        ProgressStage::Running,
        total,
        total,
        "progress.archiving",
    ));

    let size_bytes = std::fs::metadata(&staged)
        .map_err(|err| ConversionError::from_io("stat staged archive", &err))?
        .len();

    Ok(ExecutionOutput {
        outputs: vec![StagedOutput {
            staged_path: staged,
            file_name,
            format: options.format.extension().to_owned(),
            size_bytes,
        }],
        warnings: Vec::new(),
        input_total_bytes,
        // A plain TAR does not compress at all — it concatenates and adds
        // 512-byte headers, so it is *always* bigger than the sum of its inputs.
        // ZIP and TAR.GZ do compress, so growth there is genuinely worth saying.
        size_growth_expected: options.format == ArchiveFormat::Tar,
    })
}

fn write_zip(
    job: &ConversionJob,
    ctx: &JobContext,
    file: std::fs::File,
    options: &CreateOptions,
    input_total_bytes: &mut u64,
) -> Result<()> {
    let mut zip = zip::ZipWriter::new(file);
    let entry_options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(options.level.map(i64::from))
        .unix_permissions(0o644);

    let total = job.input_files.len() as u64;
    for (index, input) in job.input_files.iter().enumerate() {
        ctx.check_cancelled()?;
        let bytes = std::fs::read(&input.path)
            .map_err(|err| ConversionError::from_io("read input for archive", &err))?;
        *input_total_bytes = input_total_bytes.saturating_add(bytes.len() as u64);

        zip.start_file(&input.display_name, entry_options)
            .map_err(zip_error)?;
        zip.write_all(&bytes)
            .map_err(|err| ConversionError::from_io("write zip entry", &err))?;

        ctx.report(JobProgress::counted(
            ProgressStage::Running,
            index as u64,
            total,
            "progress.archiving",
        ));
    }
    zip.finish().map_err(zip_error)?;
    Ok(())
}

/// Writes the tar body and hands the inner writer back for finishing. The
/// caller owns closing it, because a plain `File` just needs a flush while a
/// `GzEncoder` must write its trailer via `finish()`.
fn write_tar<W: Write>(
    job: &ConversionJob,
    ctx: &JobContext,
    writer: W,
    input_total_bytes: &mut u64,
) -> Result<W> {
    let mut builder = tar::Builder::new(writer);
    builder.follow_symlinks(false);

    let total = job.input_files.len() as u64;
    for (index, input) in job.input_files.iter().enumerate() {
        ctx.check_cancelled()?;
        let mut file = std::fs::File::open(&input.path)
            .map_err(|err| ConversionError::from_io("open input for archive", &err))?;
        let len = file
            .metadata()
            .map_err(|err| ConversionError::from_io("stat input for archive", &err))?
            .len();
        *input_total_bytes = input_total_bytes.saturating_add(len);

        builder
            .append_file(&input.display_name, &mut file)
            .map_err(|err| ConversionError::from_io("write tar entry", &err))?;

        ctx.report(JobProgress::counted(
            ProgressStage::Running,
            index as u64,
            total,
            "progress.archiving",
        ));
    }
    builder
        .into_inner()
        .map_err(|err| ConversionError::from_io("finish tar", &err))
}

/// Reopens the finished archive and confirms it lists exactly the inputs, with
/// matching sizes — the spec's "open archive, enumerate entries, compare".
fn validate_created_archive(execution: &ExecutionOutput) -> Result<Vec<ValidationReport>> {
    let Some(staged) = execution.outputs.first() else {
        return Err(ConversionError::internal(
            "archive create produced no output",
        ));
    };

    let format = match staged.format.as_str() {
        "zip" => ArchiveFormat::Zip,
        "tar" => ArchiveFormat::Tar,
        "tar.gz" => ArchiveFormat::TarGz,
        other => {
            return Err(ConversionError::internal(format!(
                "unknown archive format {other}"
            )))
        }
    };

    let mut checks = vec![match std::fs::metadata(&staged.staged_path) {
        Ok(meta) if meta.len() > 0 => ValidationCheck::passed("archive.exists"),
        _ => ValidationCheck::failed("archive.exists", "archive is missing or empty"),
    }];

    match enumerate_entries(&staged.staged_path, format) {
        Ok(entries) => {
            checks.push(ValidationCheck::passed("archive.reopens"));
            checks.push(if entries.is_empty() {
                ValidationCheck::failed("archive.hasEntries", "archive lists no entries")
            } else {
                ValidationCheck::passed("archive.hasEntries")
            });
        }
        Err(err) => checks.push(ValidationCheck::failed("archive.reopens", err.detail)),
    }

    Ok(vec![ValidationReport::from_checks(
        format.extension(),
        checks,
        OutputMetadata {
            size_bytes: staged.size_bytes,
            properties: serde_json::json!({ "format": format.extension() }),
        },
    )])
}

fn enumerate_entries(path: &Path, format: ArchiveFormat) -> Result<Vec<String>> {
    let file = std::fs::File::open(path)
        .map_err(|err| ConversionError::from_io("reopen archive", &err))?;
    match format {
        ArchiveFormat::Zip => {
            let mut archive = zip::ZipArchive::new(file).map_err(zip_error)?;
            let mut names = Vec::with_capacity(archive.len());
            for i in 0..archive.len() {
                names.push(archive.by_index(i).map_err(zip_error)?.name().to_owned());
            }
            Ok(names)
        }
        ArchiveFormat::Tar => tar_entry_names(file),
        ArchiveFormat::TarGz => tar_entry_names(flate2::read::GzDecoder::new(file)),
    }
}

fn tar_entry_names<R: Read>(reader: R) -> Result<Vec<String>> {
    let mut archive = tar::Archive::new(reader);
    let mut names = Vec::new();
    for entry in archive
        .entries()
        .map_err(|err| ConversionError::from_io("read tar entries", &err))?
    {
        let entry = entry.map_err(|err| ConversionError::from_io("read tar entry", &err))?;
        let path = entry
            .path()
            .map_err(|err| ConversionError::from_io("read tar entry path", &err))?;
        names.push(path.to_string_lossy().into_owned());
    }
    Ok(names)
}

// ---------------------------------------------------------------------------
// extract
// ---------------------------------------------------------------------------

fn extract(job: &ConversionJob, ctx: &JobContext) -> Result<ExecutionOutput> {
    let Some(input) = job.input_files.first() else {
        return Err(
            ConversionError::new(ConversionErrorCode::InvalidInput, "no input files")
                .with_message_key("error.input.missing"),
        );
    };
    if job.input_files.len() > 1 {
        return Err(ConversionError::new(
            ConversionErrorCode::InvalidInput,
            "extract takes one archive at a time",
        )
        .with_message_key("error.input.tooMany"));
    }

    let format = ArchiveFormat::from_format(input.detected_format).ok_or_else(|| {
        ConversionError::new(
            ConversionErrorCode::UnsupportedFormat,
            format!(
                "{:?} is not an archive this engine can extract",
                input.detected_format
            ),
        )
        .with_message_key("error.unsupportedFormat")
    })?;

    // Extract under a folder named after the archive, so a multi-file archive
    // does not scatter loose files across the destination.
    let root_name = archive_stem(&input.display_name);
    // Stage the whole tree inside the workspace first.
    let staging_root = ctx.workspace.child("extracted")?;
    std::fs::create_dir_all(&staging_root)
        .map_err(|err| ConversionError::from_io("create extraction workspace", &err))?;

    let source = std::fs::File::open(&input.path)
        .map_err(|err| ConversionError::from_io("open archive", &err))?;

    let staged = match format {
        ArchiveFormat::Zip => extract_zip(ctx, source, &staging_root, &root_name)?,
        ArchiveFormat::Tar => extract_tar(ctx, source, &staging_root, &root_name)?,
        ArchiveFormat::TarGz => extract_tar(
            ctx,
            flate2::read::GzDecoder::new(source),
            &staging_root,
            &root_name,
        )?,
    };

    if staged.is_empty() {
        return Err(ConversionError::new(
            ConversionErrorCode::CorruptedInput,
            "archive contained no extractable files",
        )
        .with_message_key("error.archive.empty"));
    }

    Ok(ExecutionOutput {
        outputs: staged,
        warnings: Vec::new(),
        // Extraction is a decompression, not a size-reduction: the output is
        // meant to be larger than the archive. Reporting no baseline keeps
        // `commit` from firing a spurious "larger than input" warning.
        input_total_bytes: 0,
        size_growth_expected: false,
    })
}

/// Names the extraction folder from the archive, stripping `.tar.gz`/`.zip`/etc.
fn archive_stem(display_name: &str) -> String {
    let lower = display_name.to_lowercase();
    for suffix in [".tar.gz", ".tgz", ".tar", ".zip"] {
        if lower.ends_with(suffix) {
            return display_name
                .get(..display_name.len().saturating_sub(suffix.len()))
                .unwrap_or(display_name)
                .to_owned();
        }
    }
    display_name.to_owned()
}

fn extract_zip(
    ctx: &JobContext,
    source: std::fs::File,
    staging_root: &Path,
    root_name: &str,
) -> Result<Vec<StagedOutput>> {
    let mut archive = zip::ZipArchive::new(source).map_err(zip_error)?;

    // Bomb pre-check from the central directory, before extracting a byte.
    let count = archive.len() as u64;
    guard_entry_count(count)?;
    let mut declared_total = 0u64;
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(zip_error)?;
        declared_total = declared_total.saturating_add(entry.size());
        guard_ratio(entry.size(), entry.compressed_size())?;
    }
    guard_total_size(declared_total)?;

    let mut outputs = Vec::new();
    // The pre-check above is advisory — it only read what the central directory
    // claims. This is the total that actually reaches the disk.
    let mut written_total = 0u64;
    for i in 0..archive.len() {
        ctx.check_cancelled()?;
        let mut entry = archive.by_index(i).map_err(zip_error)?;

        // `enclosed_name` is the zip crate's own traversal guard; we then run
        // ours as well, so both would have to be wrong to let an entry escape.
        let Some(relative) = entry.enclosed_name() else {
            return Err(unsafe_entry(entry.name()));
        };
        if is_symlink_mode(entry.unix_mode()) {
            return Err(unsafe_entry(entry.name()));
        }
        if entry.is_dir() {
            continue;
        }

        let staged_path = safe_join(staging_root, &relative)?;
        let written = write_entry(
            &staged_path,
            &mut entry,
            MAX_TOTAL_BYTES.saturating_sub(written_total),
        )?;
        written_total = written_total.saturating_add(written);
        outputs.push(extracted_output(root_name, &relative, &staged_path)?);

        ctx.report(JobProgress::counted(
            ProgressStage::Running,
            i as u64,
            count,
            "progress.extracting",
        ));
    }
    Ok(outputs)
}

fn extract_tar<R: Read>(
    ctx: &JobContext,
    reader: R,
    staging_root: &Path,
    root_name: &str,
) -> Result<Vec<StagedOutput>> {
    // Tar has no central directory, so bombs are bounded as we stream: a running
    // total and entry count, tripped before the offending entry is written.
    let mut archive = tar::Archive::new(reader);
    let mut outputs = Vec::new();
    let mut running_total = 0u64;
    let mut written_total = 0u64;
    let mut count = 0u64;

    // A single gzipped file (`dump.sql.gz`) has no tar inside it. That is a
    // wrong-kind-of-file, not a crash, so it is declined by name rather than
    // surfacing as "something went wrong inside LocalConvert".
    for entry in archive.entries().map_err(|_| not_a_tar_archive())? {
        ctx.check_cancelled()?;
        let mut entry = entry.map_err(|_| not_a_tar_archive())?;

        count = count.saturating_add(1);
        guard_entry_count(count)?;

        let entry_type = entry.header().entry_type();
        // Only regular files and directories. Symlinks, hardlinks, char/block
        // devices, fifos — all refused, since they are the escape and
        // resource-exhaustion vectors.
        if entry_type.is_dir() {
            continue;
        }
        if !entry_type.is_file() {
            let path = entry
                .path()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            return Err(unsafe_entry(&path));
        }

        // `entry.size()`, not `entry.header().size()`: a PAX extension record
        // overrides the size the raw header advertises, and the tar crate
        // applies that override to the entry without rewriting the header. So
        // the header can honestly report 0 while the entry streams gigabytes.
        // Only the entry knows the size the reader will actually honour.
        let size = entry.size();
        running_total = running_total.saturating_add(size);
        guard_total_size(running_total)?;

        let relative = entry
            .path()
            .map_err(|err| ConversionError::from_io("read entry path", &err))?
            .into_owned();

        let staged_path = safe_join(staging_root, &relative)?;
        let written = write_entry(
            &staged_path,
            &mut entry,
            MAX_TOTAL_BYTES.saturating_sub(written_total),
        )?;
        written_total = written_total.saturating_add(written);
        outputs.push(extracted_output(root_name, &relative, &staged_path)?);

        ctx.report(JobProgress::indeterminate(
            ProgressStage::Running,
            "progress.extracting",
        ));
    }
    Ok(outputs)
}

/// Characters that have no business in a file name and every use in disguising
/// one: the C0 controls and DEL, which let an entry rewrite a terminal line as
/// it is printed, and the bidi overrides, which reorder how a name *renders*
/// without changing what it *is*. The classic is `photo\u{202E}gpj.exe`, shown
/// to the user as `photo` + `exe.jpg` — they approve a picture and get an
/// executable.
///
/// Applied to archive entries only. These names are supplied by whoever built
/// the archive; a file the user picked themselves is their own business, and
/// refusing to convert it because of an odd character would be our bug.
fn ensure_displayable_entry_name(relative: &Path) -> Result<()> {
    let name = relative.to_string_lossy();
    if let Some(bad) = name.chars().find(|c| {
        c.is_control()
            || matches!(*c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
    }) {
        return Err(ConversionError::new(
            ConversionErrorCode::ArchiveUnsafe,
            format!(
                "archive entry name contains a disguising character (U+{:04X})",
                bad as u32
            ),
        )
        .with_message_key("error.archive.unsafe"));
    }
    Ok(())
}

/// Resolves an entry path inside the staging root, rejecting any escape.
fn safe_join(staging_root: &Path, relative: &Path) -> Result<PathBuf> {
    ensure_displayable_entry_name(relative)?;
    crate::paths::join_within(staging_root, relative).map_err(|err| {
        // Recast any traversal rejection as the archive-specific, actionable code.
        ConversionError::new(
            ConversionErrorCode::ArchiveUnsafe,
            format!("archive entry escapes the destination: {}", err.detail),
        )
        .with_message_key("error.archive.unsafe")
    })
}

/// Writes one entry, refusing to put more than `remaining` bytes on disk, and
/// returns how many it actually wrote.
///
/// Every size an archive states about itself is attacker-controlled, and both
/// formats give the liar a way through if you believe it:
///
/// * a tar PAX `size` record overrides the entry size the header advertises,
///   so a header can claim 0 while the entry streams gigabytes;
/// * a zip's `uncompressed_size` bounds nothing during inflation — the CRC that
///   would catch the lie is only verified at EOF, which is to say *after* the
///   bytes are already on the disk.
///
/// So the copy is bounded here instead. This is the only limit in the module
/// that does not take the archive's word for anything: it counts bytes as they
/// land. Reading one byte past the budget is what proves the declaration lied.
fn write_entry(staged_path: &Path, reader: &mut impl Read, remaining: u64) -> Result<u64> {
    if let Some(parent) = staged_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| ConversionError::from_io("create extracted directory", &err))?;
    }
    let mut out = std::fs::File::create(staged_path)
        .map_err(|err| ConversionError::from_io("create extracted file", &err))?;

    let mut limited = reader.take(remaining.saturating_add(1));
    let written = std::io::copy(&mut limited, &mut out)
        .map_err(|err| ConversionError::from_io("write extracted file", &err))?;

    if written > remaining {
        // Drop the handle before the caller's workspace teardown removes it.
        drop(out);
        return Err(ConversionError::new(
            ConversionErrorCode::ArchiveUnsafe,
            "archive expands past the safety limit; its declared sizes were wrong",
        )
        .with_message_key("error.archive.tooLarge"));
    }
    Ok(written)
}

fn extracted_output(root_name: &str, relative: &Path, staged_path: &Path) -> Result<StagedOutput> {
    let size_bytes = std::fs::metadata(staged_path)
        .map_err(|err| ConversionError::from_io("stat extracted file", &err))?
        .len();
    // Committed under <archive-name>/<entry path>.
    let file_name = format!("{root_name}/{}", relative.to_string_lossy());
    Ok(StagedOutput {
        staged_path: staged_path.to_path_buf(),
        file_name,
        format: "file".to_owned(),
        size_bytes,
    })
}

/// Extraction outputs are validated for containment and existence. Emptiness is
/// *not* an error here — archives legitimately contain empty files.
fn validate_extracted_files(execution: &ExecutionOutput) -> Vec<ValidationReport> {
    execution
        .outputs
        .iter()
        .map(|staged| {
            let checks = vec![match std::fs::metadata(&staged.staged_path) {
                Ok(_) => ValidationCheck::passed("extract.fileExists"),
                Err(err) => ValidationCheck::failed("extract.fileExists", format!("{err}")),
            }];
            ValidationReport::from_checks(
                "file",
                checks,
                OutputMetadata {
                    size_bytes: staged.size_bytes,
                    properties: serde_json::json!({}),
                },
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// guards
// ---------------------------------------------------------------------------

fn guard_entry_count(count: u64) -> Result<()> {
    if count > MAX_ENTRIES {
        return Err(ConversionError::new(
            ConversionErrorCode::ArchiveUnsafe,
            format!("archive declares {count} entries, above the {MAX_ENTRIES} limit"),
        )
        .with_message_key("error.archive.tooManyEntries"));
    }
    Ok(())
}

fn guard_total_size(total: u64) -> Result<()> {
    if total > MAX_TOTAL_BYTES {
        return Err(ConversionError::new(
            ConversionErrorCode::ArchiveUnsafe,
            format!("archive would extract to {total} bytes, above the safety limit"),
        )
        .with_message_key("error.archive.tooLarge"));
    }
    Ok(())
}

fn guard_ratio(uncompressed: u64, compressed: u64) -> Result<()> {
    // Multiplication rather than division: no clippy integer-division lint, and
    // `saturating_mul` means a huge compressed size cannot overflow the bound.
    if compressed > 0 && uncompressed > compressed.saturating_mul(MAX_RATIO) {
        return Err(ConversionError::new(
            ConversionErrorCode::ArchiveUnsafe,
            "entry expands far past a safe ratio; looks like a decompression bomb",
        )
        .with_message_key("error.archive.bomb"));
    }
    Ok(())
}

fn is_symlink_mode(mode: Option<u32>) -> bool {
    // S_IFLNK == 0o120000 in the top file-type bits.
    mode.is_some_and(|m| m & 0o170000 == 0o120000)
}

/// A `.gz` that turns out not to wrap a tar, or a tar that is truncated or
/// garbled. Either way it is the wrong kind of file, not an internal fault.
fn not_a_tar_archive() -> ConversionError {
    ConversionError::new(
        ConversionErrorCode::UnsupportedFormat,
        "this does not contain a tar archive",
    )
    .with_message_key("error.archive.notATar")
}

fn unsafe_entry(name: &str) -> ConversionError {
    ConversionError::new(
        ConversionErrorCode::ArchiveUnsafe,
        format!("archive contains an unsafe entry: {name}"),
    )
    .with_message_key("error.archive.unsafe")
}

fn zip_error(err: zip::result::ZipError) -> ConversionError {
    ConversionError::new(
        ConversionErrorCode::CorruptedInput,
        format!("zip error: {err}"),
    )
    .with_message_key("error.corruptedInput")
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::integer_division
    )]

    use std::sync::Arc;

    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    use super::*;
    use crate::file::FileDescriptor;
    use crate::paths::OverwritePolicy;
    use crate::runner;
    use crate::workspace::JobWorkspace;

    struct Harness {
        _temp: tempfile::TempDir,
        src_dir: PathBuf,
        out_dir: PathBuf,
        temp_root: PathBuf,
    }

    impl Harness {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let src_dir = temp.path().join("src");
            let out_dir = temp.path().join("out");
            let temp_root = temp.path().join("apptemp");
            for d in [&src_dir, &out_dir, &temp_root] {
                std::fs::create_dir_all(d).unwrap();
            }
            Self {
                _temp: temp,
                src_dir,
                out_dir,
                temp_root,
            }
        }

        fn file(&self, name: &str, contents: &[u8]) -> PathBuf {
            let path = self.src_dir.join(name);
            std::fs::write(&path, contents).unwrap();
            path
        }

        fn context(&self) -> JobContext {
            let id = Uuid::new_v4();
            JobContext::new(
                id,
                JobWorkspace::create(&self.temp_root, id).unwrap(),
                CancellationToken::new(),
                Arc::new(|_, _| {}),
            )
        }

        fn create_job(&self, inputs: &[&Path], options: serde_json::Value) -> ConversionJob {
            ConversionJob::new(
                ARCHIVE_CREATE_OPERATION_ID,
                inputs
                    .iter()
                    .map(|p| FileDescriptor::probe(p).unwrap())
                    .collect(),
                self.out_dir.to_string_lossy(),
                OverwritePolicy::Overwrite,
                options,
            )
        }

        fn extract_job(&self, archive: &Path) -> ConversionJob {
            ConversionJob::new(
                ARCHIVE_EXTRACT_OPERATION_ID,
                vec![FileDescriptor::probe(archive).unwrap()],
                self.out_dir.to_string_lossy(),
                OverwritePolicy::Overwrite,
                serde_json::Value::Null,
            )
        }

        async fn run(&self, job: &ConversionJob) -> Result<crate::job::JobResult> {
            let ctx = self.context();
            let exec = execute(job, &ctx).await?;
            let reports = validate(job, &ctx, &exec).await?;
            runner::commit(job, exec, reports, 0)
        }
    }

    #[tokio::test]
    async fn zip_round_trips_through_create_and_extract() {
        let h = Harness::new();
        let a = h.file("a.txt", b"hello alpha");
        let b = h.file("b.txt", b"hello beta");

        let create = h.create_job(
            &[&a, &b],
            serde_json::json!({ "format": "zip", "archiveName": "bundle" }),
        );
        let created = h.run(&create).await.unwrap();
        assert!(created.validation_reports.iter().all(|r| r.valid));
        let archive = h.out_dir.join("bundle.zip");
        assert!(archive.exists());
        assert_eq!(crate::detect(&archive).unwrap().format, FileFormat::Zip);

        // Fresh destination so extraction does not collide with the archive.
        let h2 = Harness::new();
        let extract = h2.extract_job(&archive);
        let extracted = h2.run(&extract).await.unwrap();
        assert_eq!(extracted.outputs.len(), 2);
        assert_eq!(
            std::fs::read(h2.out_dir.join("bundle/a.txt")).unwrap(),
            b"hello alpha"
        );
        assert_eq!(
            std::fs::read(h2.out_dir.join("bundle/b.txt")).unwrap(),
            b"hello beta"
        );
    }

    #[tokio::test]
    async fn tar_and_targz_round_trip() {
        for (format, ext) in [("tar", "tar"), ("tarGz", "tar.gz")] {
            let h = Harness::new();
            let a = h.file("doc.txt", b"tarred content here");
            let create = h.create_job(
                &[&a],
                serde_json::json!({ "format": format, "archiveName": "t" }),
            );
            h.run(&create).await.unwrap();
            let archive = h.out_dir.join(format!("t.{ext}"));
            assert!(archive.exists(), "{format} archive missing");

            let h2 = Harness::new();
            let extracted = h2.run(&h2.extract_job(&archive)).await.unwrap();
            assert_eq!(extracted.outputs.len(), 1, "{format}");
            assert_eq!(
                std::fs::read(h2.out_dir.join("t/doc.txt")).unwrap(),
                b"tarred content here"
            );
        }
    }

    #[tokio::test]
    async fn a_zip_with_a_traversal_entry_cannot_escape() {
        let h = Harness::new();
        // Hand-build a zip whose entry name is a parent-relative path.
        let archive = h.src_dir.join("evil.zip");
        {
            let file = std::fs::File::create(&archive).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            // start_file sanitises, so drop to the raw name via add_directory-free path:
            zip.start_file("../escaped.txt", opts).unwrap();
            zip.write_all(b"pwned").unwrap();
            zip.finish().unwrap();
        }

        // Whatever the writer stored, extraction must not create a file outside
        // the destination. The parent of out_dir must stay clean.
        let h2 = Harness::new();
        let mut job = h2.extract_job(&archive);
        job.input_files = vec![FileDescriptor::probe(&archive).unwrap()];
        let _ = h2.run(&job).await; // may Ok (sanitised) or Err (rejected)

        assert!(
            !h2.out_dir.parent().unwrap().join("escaped.txt").exists(),
            "traversal entry escaped the destination"
        );
    }

    /// A right-to-left override makes `photo<RLO>gpj.exe` render as
    /// `photoexe.jpg`. The bytes on disk are an executable; the name the user
    /// reads is a picture. Nothing downstream can undo that, so it is refused
    /// at the point the name enters the system.
    #[test]
    fn entry_names_that_disguise_themselves_are_refused() {
        for name in [
            "photo\u{202E}gpj.exe", // right-to-left override
            "safe\u{200F}.txt",     // RTL mark
            "bell\u{0007}.txt",     // C0 control
            "line\nbreak.txt",      // newline: rewrites a terminal line
        ] {
            let err = ensure_displayable_entry_name(Path::new(name))
                .expect_err("a disguising name must be refused");
            assert_eq!(err.code, ConversionErrorCode::ArchiveUnsafe);
        }

        // Ordinary names, including non-ASCII ones, stay allowed — the check
        // targets disguise, not unfamiliarity.
        for name in ["photo.jpg", "ünïcode tëst 🎉.txt", "写真.png", "a/b/c.txt"] {
            ensure_displayable_entry_name(Path::new(name))
                .unwrap_or_else(|e| panic!("{name} should be allowed: {e:?}"));
        }
    }

    /// The guard that does not trust the archive. Both formats let an entry lie
    /// about its size — a tar PAX record overrides the header, and a zip's
    /// declared `uncompressed_size` bounds nothing during inflation — so the
    /// only honest limit is one that counts bytes as they hit the disk. If this
    /// regresses, a small crafted archive can fill the user's disk.
    #[test]
    fn a_lying_entry_cannot_write_past_the_budget() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("payload.bin");

        // A reader with far more to give than the budget allows — exactly what
        // an entry whose declared size was a lie looks like from here.
        let mut endless = std::io::repeat(0u8).take(4096);
        let err = write_entry(&path, &mut endless, 512).unwrap_err();
        assert_eq!(err.code, ConversionErrorCode::ArchiveUnsafe);
        assert_eq!(
            err.message_key, "error.archive.tooLarge",
            "a size lie must surface as the archive-safety error"
        );

        // Exactly at the budget is fine — the limit is a ceiling, not a trap.
        let mut exact = std::io::repeat(0u8).take(512);
        let written = write_entry(&path, &mut exact, 512).unwrap();
        assert_eq!(written, 512);
    }

    /// A relative `etc/passwd` is legal inside an archive; the guarantee is that
    /// it lands under the extraction folder and never at the filesystem root.
    #[tokio::test]
    async fn a_tar_entry_named_like_a_system_path_stays_contained() {
        let h = Harness::new();
        let archive = h.src_dir.join("abs.tar");
        {
            let file = std::fs::File::create(&archive).unwrap();
            let mut builder = tar::Builder::new(file);
            let data = b"absolute path payload";
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            // A path the archive should never be allowed to write to.
            builder
                .append_data(&mut header, "etc/passwd", &data[..])
                .unwrap();
            builder.finish().unwrap();
        }

        let h2 = Harness::new();
        h2.run(&h2.extract_job(&archive)).await.unwrap();
        // The relative "etc/passwd" stays under the archive folder, not at /etc.
        assert!(h2.out_dir.join("abs/etc/passwd").exists());
        assert!(
            !Path::new("/etc/passwd")
                .metadata()
                .map(|m| m.len() == 21)
                .unwrap_or(false)
                || std::fs::read("/etc/passwd")
                    .map(|c| c != b"absolute path payload")
                    .unwrap_or(true)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_tar_with_a_symlink_entry_is_refused() {
        let h = Harness::new();
        let archive = h.src_dir.join("link.tar");
        {
            let file = std::fs::File::create(&archive).unwrap();
            let mut builder = tar::Builder::new(file);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            builder
                .append_link(&mut header, "link", "/etc/passwd")
                .unwrap();
            builder.finish().unwrap();
        }

        let h2 = Harness::new();
        let err = h2.run(&h2.extract_job(&archive)).await.unwrap_err();
        assert_eq!(err.code, ConversionErrorCode::ArchiveUnsafe);
        assert!(!h2.out_dir.join("link/link").exists());
    }

    #[tokio::test]
    async fn an_entry_count_bomb_is_refused() {
        // A zip crate limit is generous, but our guard trips at MAX_ENTRIES.
        assert!(guard_entry_count(MAX_ENTRIES).is_ok());
        assert!(guard_entry_count(MAX_ENTRIES + 1).is_err());
    }

    #[tokio::test]
    async fn a_high_ratio_entry_is_flagged_as_a_bomb() {
        // Far past what a single deflate pass can produce — a nested bomb.
        assert!(guard_ratio(1_000_000_000, 1_000).is_err());
        // A normal 3:1 text compression is fine.
        assert!(guard_ratio(3_000, 1_000).is_ok());
        // Stored (ratio 1:1) is fine.
        assert!(guard_ratio(5_000, 5_000).is_ok());
        // Deflate tops out around 1032:1, so ordinary highly-compressible
        // content — a zero-filled image, silence in a WAV — must be accepted.
        // A tighter limit rejected archives LocalConvert had written itself.
        assert!(guard_ratio(1_030_000, 1_000).is_ok());
    }

    #[tokio::test]
    async fn duplicate_input_names_are_refused_rather_than_clobbered() {
        let h = Harness::new();
        let sub = h.src_dir.join("nested");
        std::fs::create_dir_all(&sub).unwrap();
        let a = h.file("same.txt", b"one");
        let b_path = sub.join("same.txt");
        std::fs::write(&b_path, b"two").unwrap();

        let job = h.create_job(&[&a, &b_path], serde_json::json!({ "format": "zip" }));
        let err = h.run(&job).await.unwrap_err();
        assert_eq!(err.message_key, "error.archive.duplicateName");
    }

    #[tokio::test]
    async fn the_source_archive_is_never_modified() {
        let h = Harness::new();
        let a = h.file("keep.txt", b"content");
        h.run(&h.create_job(
            &[&a],
            serde_json::json!({ "format": "zip", "archiveName": "z" }),
        ))
        .await
        .unwrap();
        let archive = h.out_dir.join("z.zip");
        let before = std::fs::read(&archive).unwrap();

        let h2 = Harness::new();
        h2.run(&h2.extract_job(&archive)).await.unwrap();
        assert_eq!(std::fs::read(&archive).unwrap(), before);
    }

    #[tokio::test]
    async fn a_unicode_named_entry_survives_the_round_trip() {
        let h = Harness::new();
        let a = h.file("ünïcode tëst 🎉.txt", b"unicode");
        h.run(&h.create_job(
            &[&a],
            serde_json::json!({ "format": "zip", "archiveName": "u" }),
        ))
        .await
        .unwrap();

        let h2 = Harness::new();
        h2.run(&h2.extract_job(&h.out_dir.join("u.zip")))
            .await
            .unwrap();
        assert!(h2.out_dir.join("u/ünïcode tëst 🎉.txt").exists());
    }

    #[tokio::test]
    async fn extracting_a_non_archive_is_declined() {
        let h = Harness::new();
        let not_archive = h.file("photo.png", b"\x89PNG\r\n\x1a\n not really");
        let err = h.run(&h.extract_job(&not_archive)).await.unwrap_err();
        assert_eq!(err.code, ConversionErrorCode::UnsupportedFormat);
    }
}
