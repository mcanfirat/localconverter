//! Image conversion and compression.
//!
//! Pure Rust — no bundled binaries, no linked C libraries, nothing spawned.
//! That is why HEIC and AVIF are absent: both need a native codec, which is a
//! packaging and licensing decision rather than a coding one. They are declined
//! explicitly here rather than silently producing something else.
//!
//! The rules this module exists to keep, all from spec §4.2 and §10:
//!
//! * Transparency is never dropped silently. Converting an image with alpha to
//!   JPEG **fails** unless the caller supplied a background colour.
//! * Animation is never flattened silently. A multi-frame source warns that
//!   only the first frame was exported.
//! * EXIF orientation is baked into the pixels and the tag is not carried over,
//!   so the output cannot be rotated twice by a viewer that reads it.
//! * Metadata removal is reported when the source actually had metadata.
//! * The output is decoded again, by a separate parser, before it is accepted.
//!
//! ponytail: this lives in `localconvert-core` rather than a `plugin-image`
//! crate. One engine does not need a registry and a dyn-dispatch boundary;
//! extract it when the second engine (archives, Phase 2) arrives and the
//! dispatch in `runner` has two real arms to justify one.

use std::io::Cursor;
use std::path::Path;

use image::{DynamicImage, ImageFormat as ImgFormat, ImageReader};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{ConversionError, ConversionErrorCode, Result};
use crate::file::{FileDescriptor, FileFormat};
use crate::job::{ConversionJob, JobProgress, JobWarning, ProgressStage};
use crate::runner::{ExecutionOutput, JobContext, StagedOutput};
use crate::validation::{basic_output_checks, OutputMetadata, ValidationCheck, ValidationReport};

/// Refuse to decode anything whose declared dimensions would allocate more than
/// this. A 16-byte header can claim 60000×60000, which is 14 GB of RGBA.
const MAX_PIXELS: u64 = 100_000_000;

/// Formats this engine can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ImageOutputFormat {
    Jpeg,
    Png,
    #[serde(rename = "webp")]
    WebP,
    Tiff,
    Bmp,
}

impl ImageOutputFormat {
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::WebP => "webp",
            Self::Tiff => "tiff",
            Self::Bmp => "bmp",
        }
    }

    #[must_use]
    fn image_format(self) -> ImgFormat {
        match self {
            Self::Jpeg => ImgFormat::Jpeg,
            Self::Png => ImgFormat::Png,
            Self::WebP => ImgFormat::WebP,
            Self::Tiff => ImgFormat::Tiff,
            Self::Bmp => ImgFormat::Bmp,
        }
    }

    /// Whether the encoder keeps an alpha channel.
    #[must_use]
    pub fn supports_alpha(self) -> bool {
        !matches!(self, Self::Jpeg)
    }

    /// Whether the `quality` option means anything.
    ///
    /// WebP is encoded losslessly: the pure-Rust encoder has no lossy mode. It
    /// is honest about that rather than accepting a quality value and ignoring
    /// it — see `error.image.qualityNotApplicable`.
    #[must_use]
    pub fn is_lossy(self) -> bool {
        matches!(self, Self::Jpeg)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "mode"
)]
#[ts(export)]
pub enum ResizeSpec {
    /// Scale down to fit inside the box, preserving aspect ratio. Never upscales.
    Fit { max_width: u32, max_height: u32 },
    /// Exact dimensions, aspect ratio not preserved.
    Exact { width: u32, height: u32 },
}

/// Background painted underneath transparent pixels when alpha cannot survive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Background {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImageOptions {
    pub target_format: ImageOutputFormat,
    /// 1–100. Only meaningful for lossy formats; rejected for the others.
    pub quality: Option<u8>,
    pub resize: Option<ResizeSpec>,
    /// Required when the source has transparency and the target cannot keep it.
    pub background: Option<Background>,
}

impl ImageOptions {
    pub fn parse(options: &serde_json::Value) -> Result<Self> {
        let parsed: Self = serde_json::from_value(options.clone()).map_err(|err| {
            ConversionError::new(
                ConversionErrorCode::InvalidInput,
                format!("invalid image options: {err}"),
            )
            .with_message_key("error.options.invalid")
        })?;

        if let Some(quality) = parsed.quality {
            if !(1..=100).contains(&quality) {
                return Err(ConversionError::new(
                    ConversionErrorCode::InvalidInput,
                    "quality must be between 1 and 100",
                )
                .with_message_key("error.options.outOfRange"));
            }
            if !parsed.target_format.is_lossy() {
                return Err(ConversionError::new(
                    ConversionErrorCode::InvalidInput,
                    format!(
                        "{} is encoded losslessly; a quality setting would be ignored",
                        parsed.target_format.extension()
                    ),
                )
                .with_message_key("error.image.qualityNotApplicable"));
            }
        }

        match parsed.resize {
            Some(ResizeSpec::Fit {
                max_width,
                max_height,
            }) if max_width == 0 || max_height == 0 => {
                return Err(zero_dimension());
            }
            Some(ResizeSpec::Exact { width, height }) if width == 0 || height == 0 => {
                return Err(zero_dimension());
            }
            _ => {}
        }

        Ok(parsed)
    }
}

fn zero_dimension() -> ConversionError {
    ConversionError::new(
        ConversionErrorCode::InvalidInput,
        "resize dimensions must be greater than zero",
    )
    .with_message_key("error.options.outOfRange")
}

/// What a conversion will cost the user, computed before it runs so the UI can
/// explain it and the user can decline. Spec §4.3.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImagePreflight {
    pub source_width: u32,
    pub source_height: u32,
    pub has_alpha: bool,
    pub is_animated: bool,
    pub has_metadata: bool,
    /// The target cannot keep this image's transparency, so a background must
    /// be chosen before the job can start.
    pub background_required: bool,
    pub warnings: Vec<JobWarning>,
}

/// One file's contribution to the preview shown before a batch runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImageFilePreflight {
    pub display_name: String,
    pub detected_format: FileFormat,
    pub extension_mismatch: bool,
    pub width: u32,
    pub height: u32,
    #[ts(type = "number")]
    pub size_bytes: u64,
    pub has_alpha: bool,
    pub is_animated: bool,
    /// Set when this particular file cannot be converted. Reported per file so
    /// one HEIC among twenty PNGs does not blank the whole preview.
    pub error_message_key: Option<String>,
}

/// What the UI shows before the user commits to a batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImageBatchPreflight {
    pub files: Vec<ImageFilePreflight>,
    /// Deduplicated union across the batch — twenty files losing transparency
    /// is one sentence, not twenty.
    pub warnings: Vec<JobWarning>,
    pub background_required: bool,
    pub convertible_count: u32,
    pub unsupported_count: u32,
}

/// Inspects a whole selection without converting anything.
///
/// Deliberately infallible: an unreadable or unsupported file becomes a row
/// with `error_message_key` set, so the user can see which file is the problem
/// and drop it, instead of getting one error for the whole selection.
#[must_use]
pub fn preflight_batch(inputs: &[FileDescriptor], options: &ImageOptions) -> ImageBatchPreflight {
    let mut files = Vec::with_capacity(inputs.len());
    let mut warnings = Vec::new();
    let mut background_required = false;
    let mut unsupported = 0u32;

    for input in inputs {
        match preflight(input, options) {
            Ok(report) => {
                background_required |= report.background_required;
                warnings.extend(report.warnings.iter().cloned());
                files.push(ImageFilePreflight {
                    display_name: input.display_name.clone(),
                    detected_format: input.detected_format,
                    extension_mismatch: input.extension_mismatch,
                    width: report.source_width,
                    height: report.source_height,
                    size_bytes: input.size_bytes,
                    has_alpha: report.has_alpha,
                    is_animated: report.is_animated,
                    error_message_key: None,
                });
            }
            Err(err) => {
                unsupported = unsupported.saturating_add(1);
                files.push(ImageFilePreflight {
                    display_name: input.display_name.clone(),
                    detected_format: input.detected_format,
                    extension_mismatch: input.extension_mismatch,
                    width: 0,
                    height: 0,
                    size_bytes: input.size_bytes,
                    has_alpha: false,
                    is_animated: false,
                    error_message_key: Some(err.message_key),
                });
            }
        }
    }

    let convertible = u32::try_from(files.len()).unwrap_or(u32::MAX) - unsupported;
    ImageBatchPreflight {
        files,
        warnings: dedupe_warnings(warnings),
        background_required,
        convertible_count: convertible,
        unsupported_count: unsupported,
    }
}

/// Formats this engine can read.
#[must_use]
pub fn can_decode(format: FileFormat) -> bool {
    matches!(
        format,
        FileFormat::Jpeg
            | FileFormat::Png
            | FileFormat::WebP
            | FileFormat::Tiff
            | FileFormat::Bmp
            | FileFormat::Gif
    )
}

fn decoder_error(format: FileFormat) -> ConversionError {
    match format {
        // Both need a native codec. Declining is the honest answer; producing
        // a different format, or a broken file, is not.
        FileFormat::Heic => ConversionError::new(
            ConversionErrorCode::UnsupportedFormat,
            "HEIC requires the libheif codec, which this build does not bundle",
        )
        .with_message_key("error.image.heicUnsupported"),
        FileFormat::Avif => ConversionError::new(
            ConversionErrorCode::UnsupportedFormat,
            "AVIF requires a native AV1 decoder, which this build does not bundle",
        )
        .with_message_key("error.image.avifUnsupported"),
        other => ConversionError::new(
            ConversionErrorCode::UnsupportedFormat,
            format!("{other:?} is not an image format this engine can read"),
        )
        .with_message_key("error.unsupportedFormat"),
    }
}

/// Inspects the source without doing the conversion.
pub fn preflight(input: &FileDescriptor, options: &ImageOptions) -> Result<ImagePreflight> {
    let path = Path::new(&input.path);
    if !can_decode(input.detected_format) {
        return Err(decoder_error(input.detected_format));
    }

    let (source_width, source_height) = read_dimensions(path)?;
    guard_pixel_count(source_width, source_height)?;

    let has_alpha = format_may_have_alpha(input.detected_format) && decode_has_alpha(path)?;
    let is_animated = count_frames_up_to_two(path, input.detected_format) > 1;
    let has_metadata = read_exif(path).is_some();
    let background_required = has_alpha && !options.target_format.supports_alpha();

    let mut warnings = Vec::new();
    if input.extension_mismatch {
        warnings.push(JobWarning::new("warning.image.extensionMismatch"));
    }
    if is_animated {
        warnings.push(JobWarning::new("warning.image.animationFlattened"));
    }
    if has_alpha && !options.target_format.supports_alpha() {
        warnings.push(JobWarning::new("warning.image.transparencyFlattened"));
    }
    if has_metadata {
        warnings.push(JobWarning::new("warning.image.metadataRemoved"));
    }
    if options.target_format.is_lossy() {
        warnings.push(JobWarning::new("warning.image.lossyEncoding"));
    }
    if options.target_format == ImageOutputFormat::WebP {
        warnings.push(JobWarning::new("warning.image.webpLossless"));
    }

    Ok(ImagePreflight {
        source_width,
        source_height,
        has_alpha,
        is_animated,
        has_metadata,
        background_required,
        warnings,
    })
}

pub async fn execute(job: &ConversionJob, ctx: &JobContext) -> Result<ExecutionOutput> {
    let options = ImageOptions::parse(&job.options)?;
    let total = job.input_files.len();

    if job.input_files.is_empty() {
        return Err(
            ConversionError::new(ConversionErrorCode::InvalidInput, "no input files")
                .with_message_key("error.input.missing"),
        );
    }

    let mut outputs = Vec::with_capacity(total);
    let mut warnings = Vec::new();
    let mut input_total_bytes = 0u64;

    for (index, input) in job.input_files.iter().enumerate() {
        ctx.check_cancelled()?;
        ctx.report(JobProgress::counted(
            ProgressStage::Running,
            index as u64,
            total as u64,
            "progress.converting",
        ));

        let preflight = preflight(input, &options)?;
        if preflight.background_required && options.background.is_none() {
            return Err(ConversionError::new(
                ConversionErrorCode::InvalidInput,
                "this image has transparency and the target format cannot keep it; \
                 choose a background colour",
            )
            .with_message_key("error.image.backgroundRequired"));
        }
        warnings.extend(preflight.warnings.iter().cloned());

        let staged = convert_one(ctx, input, &options)?;
        input_total_bytes = input_total_bytes.saturating_add(input.size_bytes);
        outputs.push(staged);
    }

    ctx.report(JobProgress::counted(
        ProgressStage::Running,
        total as u64,
        total as u64,
        "progress.converting",
    ));

    Ok(ExecutionOutput {
        outputs,
        warnings: dedupe_warnings(warnings),
        input_total_bytes,
        // Writing a lossless format (PNG/TIFF/BMP/lossless-WebP) from a lossy
        // source is *expected* to be several times bigger — that is what
        // "lossless" costs. Flagging it here keeps `commit` from raising a
        // "larger than the original" warning on a perfectly correct job.
        size_growth_expected: !options.target_format.is_lossy(),
    })
}

/// The same warning from twenty batched files is one warning, not twenty.
fn dedupe_warnings(warnings: Vec<JobWarning>) -> Vec<JobWarning> {
    let mut seen = std::collections::HashSet::new();
    warnings
        .into_iter()
        .filter(|w| seen.insert(w.message_key.clone()))
        .collect()
}

fn convert_one(
    ctx: &JobContext,
    input: &FileDescriptor,
    options: &ImageOptions,
) -> Result<StagedOutput> {
    let path = Path::new(&input.path);

    let mut image = decode(path, input.detected_format)?;
    let needs_flattening =
        !options.target_format.supports_alpha() && has_visible_transparency(&image);
    ctx.check_cancelled()?;

    // Orientation first: resizing before rotating would apply the box to the
    // wrong axis for a 90°-rotated photo.
    if let Some(orientation) = read_exif_orientation(path) {
        image = apply_orientation(image, orientation);
    }

    if let Some(resize) = options.resize {
        image = apply_resize(image, resize);
    }

    // Only images that actually use transparency need a background. An opaque
    // RGBA PNG has nothing to lose, so demanding a colour would be a false alarm.
    if needs_flattening {
        let Some(background) = options.background else {
            // Unreachable via `execute`, which checks preflight first; kept so
            // the invariant holds for any other caller too.
            return Err(ConversionError::new(
                ConversionErrorCode::InvalidInput,
                "background required to flatten transparency",
            )
            .with_message_key("error.image.backgroundRequired"));
        };
        image = flatten_onto(&image, background);
    }

    ctx.check_cancelled()?;

    let file_name = output_file_name(&input.display_name, options.target_format);
    let staged_path = ctx.workspace.staging_path(&file_name)?;
    encode(&image, options, &staged_path)?;

    let size_bytes = std::fs::metadata(&staged_path)
        .map_err(|err| ConversionError::from_io("stat encoded output", &err))?
        .len();

    Ok(StagedOutput {
        staged_path,
        file_name,
        format: options.target_format.extension().to_owned(),
        size_bytes,
    })
}

/// `holiday.heic` → `holiday.jpg`. Replaces the extension rather than appending.
fn output_file_name(display_name: &str, target: ImageOutputFormat) -> String {
    let stem = match display_name.rfind('.') {
        Some(index) if index > 0 => display_name.get(..index).unwrap_or(display_name),
        _ => display_name,
    };
    format!("{stem}.{}", target.extension())
}

fn decode(path: &Path, format: FileFormat) -> Result<DynamicImage> {
    if !can_decode(format) {
        return Err(decoder_error(format));
    }

    let (width, height) = read_dimensions(path)?;
    guard_pixel_count(width, height)?;

    let reader = ImageReader::open(path)
        .map_err(|err| ConversionError::from_io("open image", &err))?
        .with_guessed_format()
        .map_err(|err| ConversionError::from_io("read image header", &err))?;

    reader.decode().map_err(|err| {
        ConversionError::new(
            ConversionErrorCode::CorruptedInput,
            format!("could not decode image: {err}"),
        )
        .with_message_key("error.corruptedInput")
    })
}

/// Reads dimensions from the header only, so a hostile file cannot make us
/// allocate gigabytes before we have decided whether to.
fn read_dimensions(path: &Path) -> Result<(u32, u32)> {
    let size = imagesize::size(path).map_err(|err| {
        ConversionError::new(
            ConversionErrorCode::CorruptedInput,
            format!("could not read image dimensions: {err}"),
        )
        .with_message_key("error.corruptedInput")
    })?;
    let width = u32::try_from(size.width).unwrap_or(u32::MAX);
    let height = u32::try_from(size.height).unwrap_or(u32::MAX);
    Ok((width, height))
}

fn guard_pixel_count(width: u32, height: u32) -> Result<()> {
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    if pixels > MAX_PIXELS {
        return Err(ConversionError::new(
            ConversionErrorCode::InsufficientMemory,
            format!("image is {width}×{height}, above the {MAX_PIXELS}-pixel limit"),
        )
        .with_message_key("error.image.tooLarge"));
    }
    Ok(())
}

fn format_may_have_alpha(format: FileFormat) -> bool {
    matches!(
        format,
        FileFormat::Png | FileFormat::WebP | FileFormat::Gif | FileFormat::Tiff | FileFormat::Avif
    )
}

/// A format *permitting* alpha does not mean this file *uses* it. Painting a
/// background under a fully opaque PNG, or demanding the user pick one, would
/// be a false alarm — so the pixels are checked.
fn decode_has_alpha(path: &Path) -> Result<bool> {
    let image = ImageReader::open(path)
        .map_err(|err| ConversionError::from_io("open image", &err))?
        .with_guessed_format()
        .map_err(|err| ConversionError::from_io("read image header", &err))?
        .decode()
        .map_err(|err| {
            ConversionError::new(
                ConversionErrorCode::CorruptedInput,
                format!("could not decode image: {err}"),
            )
            .with_message_key("error.corruptedInput")
        })?;

    Ok(has_visible_transparency(&image))
}

/// A colour type *permitting* alpha is not the same as a pixel *using* it.
fn has_visible_transparency(image: &DynamicImage) -> bool {
    image.color().has_alpha() && image.to_rgba8().pixels().any(|pixel| pixel.0[3] < 255)
}

/// Counts up to two frames — all we need to answer "is it animated?" without
/// decoding a 900-frame GIF.
fn count_frames_up_to_two(path: &Path, format: FileFormat) -> usize {
    use image::AnimationDecoder;

    let Ok(file) = std::fs::File::open(path) else {
        return 1;
    };
    let reader = std::io::BufReader::new(file);

    match format {
        FileFormat::Gif => match image::codecs::gif::GifDecoder::new(reader) {
            Ok(decoder) => decoder.into_frames().take(2).count(),
            Err(_) => 1,
        },
        FileFormat::WebP => match image::codecs::webp::WebPDecoder::new(reader) {
            Ok(decoder) => {
                if decoder.has_animation() {
                    2
                } else {
                    1
                }
            }
            Err(_) => 1,
        },
        _ => 1,
    }
}

fn read_exif(path: &Path) -> Option<exif::Exif> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    exif::Reader::new().read_from_container(&mut reader).ok()
}

/// EXIF orientation, 1–8. `None` when absent or already upright.
fn read_exif_orientation(path: &Path) -> Option<u32> {
    let exif = read_exif(path)?;
    let field = exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)?;
    let value = field.value.get_uint(0)?;
    (2..=8).contains(&value).then_some(value)
}

/// Bakes the orientation into the pixels. The output carries no EXIF at all, so
/// a viewer that reads the tag cannot rotate it a second time.
fn apply_orientation(image: DynamicImage, orientation: u32) -> DynamicImage {
    match orientation {
        2 => image.fliph(),
        3 => image.rotate180(),
        4 => image.flipv(),
        5 => image.rotate90().fliph(),
        6 => image.rotate90(),
        7 => image.rotate270().fliph(),
        8 => image.rotate270(),
        _ => image,
    }
}

fn apply_resize(image: DynamicImage, resize: ResizeSpec) -> DynamicImage {
    match resize {
        ResizeSpec::Fit {
            max_width,
            max_height,
        } => {
            // Never upscale: enlarging invents detail the source does not have.
            if image.width() <= max_width && image.height() <= max_height {
                image
            } else {
                image.resize(max_width, max_height, image::imageops::FilterType::Lanczos3)
            }
        }
        ResizeSpec::Exact { width, height } => {
            image.resize_exact(width, height, image::imageops::FilterType::Lanczos3)
        }
    }
}

fn flatten_onto(image: &DynamicImage, background: Background) -> DynamicImage {
    let source = image.to_rgba8();
    let mut out = image::RgbImage::new(source.width(), source.height());

    for (x, y, pixel) in source.enumerate_pixels() {
        let [r, g, b, a] = pixel.0;
        let alpha = f32::from(a) / 255.0;
        let blend = |fg: u8, bg: u8| -> u8 {
            (f32::from(fg) * alpha + f32::from(bg) * (1.0 - alpha)).round() as u8
        };
        out.put_pixel(
            x,
            y,
            image::Rgb([
                blend(r, background.r),
                blend(g, background.g),
                blend(b, background.b),
            ]),
        );
    }
    DynamicImage::ImageRgb8(out)
}

fn encode(image: &DynamicImage, options: &ImageOptions, staged: &Path) -> Result<()> {
    let mut buffer = Cursor::new(Vec::new());

    match options.target_format {
        ImageOutputFormat::Jpeg => {
            // JPEG has no alpha channel at all; by here the image is already
            // flattened, but to_rgb8 makes that explicit for the encoder.
            let quality = options.quality.unwrap_or(85);
            let rgb = image.to_rgb8();
            let mut encoder =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buffer, quality);
            encoder
                .encode_image(&DynamicImage::ImageRgb8(rgb))
                .map_err(encode_error)?;
        }
        other => {
            image
                .write_to(&mut buffer, other.image_format())
                .map_err(encode_error)?;
        }
    }

    std::fs::write(staged, buffer.get_ref())
        .map_err(|err| ConversionError::from_io("write encoded image", &err))
}

fn encode_error(err: image::ImageError) -> ConversionError {
    ConversionError::new(
        ConversionErrorCode::ProcessFailed,
        format!("could not encode image: {err}"),
    )
    .with_message_key("error.image.encodeFailed")
}

/// Reads the finished file back and checks it against what was asked for.
///
/// The dimension check uses `imagesize`, a header-only parser with no code in
/// common with the encoder — the spec's "second independent parser".
pub async fn validate(
    job: &ConversionJob,
    ctx: &JobContext,
    execution: &ExecutionOutput,
) -> Result<Vec<ValidationReport>> {
    let options = ImageOptions::parse(&job.options)?;
    ctx.report(JobProgress::indeterminate(
        ProgressStage::Validating,
        "progress.validating",
    ));

    let mut reports = Vec::with_capacity(execution.outputs.len());

    for (staged, input) in execution.outputs.iter().zip(job.input_files.iter()) {
        ctx.check_cancelled()?;
        let mut checks = basic_output_checks(
            &job.input_files,
            &staged.staged_path,
            Some(options.target_format.extension()),
        );

        checks.push(magic_bytes_check(
            &staged.staged_path,
            options.target_format,
        ));

        let independent = imagesize::size(&staged.staged_path);
        checks.push(match &independent {
            Ok(_) => ValidationCheck::passed("image.independentParserOpens"),
            Err(err) => ValidationCheck::failed("image.independentParserOpens", format!("{err}")),
        });

        match decode_output(&staged.staged_path) {
            Ok(decoded) => {
                checks.push(ValidationCheck::passed("image.decodes"));

                if let Ok(size) = &independent {
                    let matches = size.width == decoded.width() as usize
                        && size.height == decoded.height() as usize;
                    checks.push(if matches {
                        ValidationCheck::passed("image.dimensionsAgree")
                    } else {
                        ValidationCheck::failed(
                            "image.dimensionsAgree",
                            format!(
                                "header says {}×{}, decoder says {}×{}",
                                size.width,
                                size.height,
                                decoded.width(),
                                decoded.height()
                            ),
                        )
                    });
                }

                checks.push(expected_dimensions_check(input, &options, &decoded));

                if !options.target_format.supports_alpha() {
                    checks.push(if decoded.color().has_alpha() {
                        ValidationCheck::failed(
                            "image.alphaRemoved",
                            "output still carries an alpha channel",
                        )
                    } else {
                        ValidationCheck::passed("image.alphaRemoved")
                    });
                }

                reports.push(ValidationReport::from_checks(
                    options.target_format.extension(),
                    checks,
                    OutputMetadata {
                        size_bytes: staged.size_bytes,
                        properties: serde_json::json!({
                            "width": decoded.width(),
                            "height": decoded.height(),
                            "hasAlpha": decoded.color().has_alpha(),
                        }),
                    },
                ));
            }
            Err(err) => {
                checks.push(ValidationCheck::failed("image.decodes", err.detail));
                reports.push(ValidationReport::from_checks(
                    options.target_format.extension(),
                    checks,
                    OutputMetadata {
                        size_bytes: staged.size_bytes,
                        properties: serde_json::json!({}),
                    },
                ));
            }
        }
    }

    Ok(reports)
}

fn decode_output(path: &Path) -> std::result::Result<DynamicImage, ConversionError> {
    ImageReader::open(path)
        .map_err(|err| ConversionError::from_io("reopen output", &err))?
        .with_guessed_format()
        .map_err(|err| ConversionError::from_io("read output header", &err))?
        .decode()
        .map_err(|err| {
            ConversionError::new(
                ConversionErrorCode::OutputValidationFailed,
                format!("output does not decode: {err}"),
            )
        })
}

fn magic_bytes_check(path: &Path, target: ImageOutputFormat) -> ValidationCheck {
    let Ok(bytes) = std::fs::read(path) else {
        return ValidationCheck::failed("image.magicBytes", "could not read output");
    };
    let header = bytes.get(..512.min(bytes.len())).unwrap_or_default();
    let detected = crate::detect::detect_header(header);

    let expected = match target {
        ImageOutputFormat::Jpeg => FileFormat::Jpeg,
        ImageOutputFormat::Png => FileFormat::Png,
        ImageOutputFormat::WebP => FileFormat::WebP,
        ImageOutputFormat::Tiff => FileFormat::Tiff,
        ImageOutputFormat::Bmp => FileFormat::Bmp,
    };

    if detected == Some(expected) {
        ValidationCheck::passed("image.magicBytes")
    } else {
        ValidationCheck::failed(
            "image.magicBytes",
            format!("expected {expected:?}, header says {detected:?}"),
        )
    }
}

/// The output must have exactly the dimensions the options imply — including
/// the axis swap when EXIF orientation rotated the image by 90°.
fn expected_dimensions_check(
    input: &FileDescriptor,
    options: &ImageOptions,
    decoded: &DynamicImage,
) -> ValidationCheck {
    let path = Path::new(&input.path);
    let Ok((mut width, mut height)) = read_dimensions(path) else {
        return ValidationCheck::failed("image.expectedDimensions", "source dimensions unreadable");
    };

    if matches!(read_exif_orientation(path), Some(5..=8)) {
        std::mem::swap(&mut width, &mut height);
    }

    let (expected_width, expected_height) = match options.resize {
        None => (width, height),
        Some(ResizeSpec::Exact { width, height }) => (width, height),
        Some(ResizeSpec::Fit {
            max_width,
            max_height,
        }) => {
            if width <= max_width && height <= max_height {
                (width, height)
            } else {
                fit_dimensions(width, height, max_width, max_height)
            }
        }
    };

    if decoded.width() == expected_width && decoded.height() == expected_height {
        ValidationCheck::passed("image.expectedDimensions")
    } else {
        ValidationCheck::failed(
            "image.expectedDimensions",
            format!(
                "expected {expected_width}×{expected_height}, got {}×{}",
                decoded.width(),
                decoded.height()
            ),
        )
    }
}

/// Mirrors `image::DynamicImage::resize`'s aspect-preserving arithmetic.
fn fit_dimensions(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    let wratio = f64::from(max_width) / f64::from(width);
    let hratio = f64::from(max_height) / f64::from(height);
    let ratio = wratio.min(hratio);

    let scale = |value: u32| -> u32 {
        let scaled = (f64::from(value) * ratio).round() as u64;
        u32::try_from(scaled.max(1)).unwrap_or(u32::MAX)
    };
    (scale(width), scale(height))
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        // Fixture pixel maths: integer division is exactly what is wanted here.
        clippy::integer_division
    )]

    use std::sync::Arc;

    use image::{GenericImageView, Rgba, RgbaImage};
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    use super::*;
    use crate::paths::OverwritePolicy;
    use crate::runner;
    use crate::workspace::JobWorkspace;

    struct Fixtures {
        dir: tempfile::TempDir,
    }

    impl Fixtures {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
            }
        }

        fn path(&self, name: &str) -> std::path::PathBuf {
            self.dir.path().join(name)
        }

        /// Opaque RGB gradient — the ordinary case.
        fn gradient(&self, name: &str, w: u32, h: u32, format: ImgFormat) -> std::path::PathBuf {
            let mut img = image::RgbImage::new(w, h);
            for (x, y, pixel) in img.enumerate_pixels_mut() {
                *pixel = image::Rgb([(x * 255 / w.max(1)) as u8, (y * 255 / h.max(1)) as u8, 128]);
            }
            let path = self.path(name);
            DynamicImage::ImageRgb8(img)
                .save_with_format(&path, format)
                .unwrap();
            path
        }

        /// Half transparent, so alpha handling is actually exercised.
        fn with_alpha(&self, name: &str, w: u32, h: u32) -> std::path::PathBuf {
            let mut img = RgbaImage::new(w, h);
            for (x, _y, pixel) in img.enumerate_pixels_mut() {
                *pixel = if x < w / 2 {
                    Rgba([255, 0, 0, 255])
                } else {
                    Rgba([0, 0, 255, 0])
                };
            }
            let path = self.path(name);
            img.save_with_format(&path, ImgFormat::Png).unwrap();
            path
        }

        fn animated_gif(&self, name: &str) -> std::path::PathBuf {
            use image::codecs::gif::GifEncoder;
            use image::Frame;

            let path = self.path(name);
            let file = std::fs::File::create(&path).unwrap();
            let mut encoder = GifEncoder::new(file);
            for shade in [0u8, 128, 255] {
                let frame = RgbaImage::from_pixel(8, 8, Rgba([shade, shade, shade, 255]));
                encoder.encode_frame(Frame::new(frame)).unwrap();
            }
            drop(encoder);
            path
        }
    }

    fn descriptor(path: &Path) -> FileDescriptor {
        FileDescriptor::probe(path).unwrap()
    }

    struct Harness {
        _temp: tempfile::TempDir,
        out_dir: std::path::PathBuf,
        temp_root: std::path::PathBuf,
    }

    impl Harness {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let out_dir = temp.path().join("out");
            let temp_root = temp.path().join("apptemp");
            std::fs::create_dir_all(&out_dir).unwrap();
            std::fs::create_dir_all(&temp_root).unwrap();
            Self {
                _temp: temp,
                out_dir,
                temp_root,
            }
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

        fn job(&self, inputs: &[&Path], options: serde_json::Value) -> ConversionJob {
            ConversionJob::new(
                crate::operation::IMAGE_CONVERT_OPERATION_ID,
                inputs.iter().map(|p| descriptor(p)).collect(),
                self.out_dir.to_string_lossy(),
                OverwritePolicy::Rename,
                options,
            )
        }

        async fn run(&self, job: &ConversionJob) -> Result<crate::job::JobResult> {
            let ctx = self.context();
            let execution = execute(job, &ctx).await?;
            let reports = validate(job, &ctx, &execution).await?;
            runner::commit(job, execution, reports, 0)
        }
    }

    #[tokio::test]
    async fn png_to_jpeg_produces_a_real_jpeg() {
        let f = Fixtures::new();
        let src = f.gradient("photo.png", 64, 48, ImgFormat::Png);
        let h = Harness::new();

        let job = h.job(
            &[&src],
            serde_json::json!({ "targetFormat": "jpeg", "quality": 85 }),
        );
        let result = h.run(&job).await.unwrap();

        assert_eq!(result.outputs.len(), 1);
        let out = h.out_dir.join("photo.jpg");
        assert!(out.exists());

        // Independently: the bytes really are a JPEG of the right size.
        assert_eq!(
            crate::detect::detect(&out).unwrap().format,
            FileFormat::Jpeg
        );
        let size = imagesize::size(&out).unwrap();
        assert_eq!((size.width, size.height), (64, 48));
        assert!(result.validation_reports[0].valid);
    }

    #[tokio::test]
    async fn jpeg_to_png_round_trips_dimensions() {
        let f = Fixtures::new();
        let src = f.gradient("shot.jpg", 40, 30, ImgFormat::Jpeg);
        let h = Harness::new();

        let job = h.job(&[&src], serde_json::json!({ "targetFormat": "png" }));
        h.run(&job).await.unwrap();

        let out = h.out_dir.join("shot.png");
        assert_eq!(crate::detect::detect(&out).unwrap().format, FileFormat::Png);
        assert_eq!(imagesize::size(&out).unwrap().width, 40);
    }

    #[tokio::test]
    async fn png_to_bmp_is_pixel_exact() {
        let f = Fixtures::new();
        let src = f.gradient("exact.png", 16, 16, ImgFormat::Png);
        let h = Harness::new();

        let job = h.job(&[&src], serde_json::json!({ "targetFormat": "bmp" }));
        h.run(&job).await.unwrap();

        let before = image::open(&src).unwrap().to_rgb8();
        let after = image::open(h.out_dir.join("exact.bmp")).unwrap().to_rgb8();
        assert_eq!(
            before.as_raw(),
            after.as_raw(),
            "a lossless route must not change a single pixel"
        );
    }

    #[tokio::test]
    async fn converting_transparency_to_jpeg_without_a_background_is_refused() {
        let f = Fixtures::new();
        let src = f.with_alpha("logo.png", 32, 32);
        let h = Harness::new();

        let job = h.job(&[&src], serde_json::json!({ "targetFormat": "jpeg" }));
        let err = h.run(&job).await.unwrap_err();

        assert_eq!(err.message_key, "error.image.backgroundRequired");
        assert!(
            std::fs::read_dir(&h.out_dir).unwrap().next().is_none(),
            "nothing may be written when the job is refused"
        );
    }

    #[tokio::test]
    async fn an_explicit_background_flattens_transparency() {
        let f = Fixtures::new();
        let src = f.with_alpha("logo.png", 32, 32);
        let h = Harness::new();

        let job = h.job(
            &[&src],
            serde_json::json!({
                "targetFormat": "jpeg",
                "background": { "r": 255, "g": 255, "b": 255 }
            }),
        );
        let result = h.run(&job).await.unwrap();

        let out = image::open(h.out_dir.join("logo.jpg")).unwrap().to_rgb8();
        // Right half was fully transparent, so it becomes the background.
        let right = out.get_pixel(24, 16).0;
        assert!(
            right.iter().all(|&c| c > 240),
            "transparent area should be white, got {right:?}"
        );
        assert!(result
            .warnings
            .iter()
            .any(|w| w.message_key == "warning.image.transparencyFlattened"));
    }

    #[tokio::test]
    async fn a_fully_opaque_png_does_not_demand_a_background() {
        let f = Fixtures::new();
        // RGBA colour type, but every pixel opaque.
        let path = f.path("opaque.png");
        RgbaImage::from_pixel(16, 16, Rgba([10, 20, 30, 255]))
            .save_with_format(&path, ImgFormat::Png)
            .unwrap();
        let h = Harness::new();

        let job = h.job(&[&path], serde_json::json!({ "targetFormat": "jpeg" }));
        assert!(
            h.run(&job).await.is_ok(),
            "an opaque image has no transparency to lose"
        );
    }

    #[tokio::test]
    async fn an_animated_source_warns_that_only_one_frame_is_exported() {
        let f = Fixtures::new();
        let src = f.animated_gif("loop.gif");
        let h = Harness::new();

        let job = h.job(
            &[&src],
            serde_json::json!({ "targetFormat": "jpeg", "background": {"r":0,"g":0,"b":0} }),
        );
        let result = h.run(&job).await.unwrap();

        assert!(result
            .warnings
            .iter()
            .any(|w| w.message_key == "warning.image.animationFlattened"));
    }

    #[tokio::test]
    async fn fit_resize_preserves_aspect_ratio_and_never_upscales() {
        let f = Fixtures::new();
        let src = f.gradient("big.png", 200, 100, ImgFormat::Png);
        let h = Harness::new();

        let job = h.job(
            &[&src],
            serde_json::json!({
                "targetFormat": "png",
                "resize": { "mode": "fit", "maxWidth": 50, "maxHeight": 50 }
            }),
        );
        h.run(&job).await.unwrap();
        let size = imagesize::size(h.out_dir.join("big.png")).unwrap();
        assert_eq!((size.width, size.height), (50, 25));

        // Asking for a box larger than the source leaves it alone.
        let h2 = Harness::new();
        let job2 = h2.job(
            &[&src],
            serde_json::json!({
                "targetFormat": "png",
                "resize": { "mode": "fit", "maxWidth": 500, "maxHeight": 500 }
            }),
        );
        h2.run(&job2).await.unwrap();
        let size2 = imagesize::size(h2.out_dir.join("big.png")).unwrap();
        assert_eq!((size2.width, size2.height), (200, 100));
    }

    #[tokio::test]
    async fn exact_resize_uses_the_requested_dimensions() {
        let f = Fixtures::new();
        let src = f.gradient("stretch.png", 60, 60, ImgFormat::Png);
        let h = Harness::new();

        let job = h.job(
            &[&src],
            serde_json::json!({
                "targetFormat": "png",
                "resize": { "mode": "exact", "width": 33, "height": 11 }
            }),
        );
        h.run(&job).await.unwrap();
        let size = imagesize::size(h.out_dir.join("stretch.png")).unwrap();
        assert_eq!((size.width, size.height), (33, 11));
    }

    #[tokio::test]
    async fn lower_quality_produces_a_smaller_jpeg() {
        let f = Fixtures::new();
        let src = f.gradient("q.png", 200, 200, ImgFormat::Png);

        let mut sizes = Vec::new();
        for quality in [95u8, 40] {
            let h = Harness::new();
            let job = h.job(
                &[&src],
                serde_json::json!({ "targetFormat": "jpeg", "quality": quality }),
            );
            let result = h.run(&job).await.unwrap();
            sizes.push(result.output_total_bytes);
        }
        assert!(
            sizes[1] < sizes[0],
            "quality 40 should be smaller than 95, got {sizes:?}"
        );
    }

    #[test]
    fn quality_is_rejected_where_it_would_be_ignored() {
        let err = ImageOptions::parse(&serde_json::json!({
            "targetFormat": "png", "quality": 50
        }))
        .unwrap_err();
        assert_eq!(err.message_key, "error.image.qualityNotApplicable");

        assert!(ImageOptions::parse(&serde_json::json!({
            "targetFormat": "jpeg", "quality": 50
        }))
        .is_ok());
    }

    #[test]
    fn out_of_range_options_are_rejected() {
        for bad in [
            serde_json::json!({ "targetFormat": "jpeg", "quality": 0 }),
            serde_json::json!({ "targetFormat": "jpeg", "quality": 101 }),
            serde_json::json!({ "targetFormat": "png",
                "resize": { "mode": "exact", "width": 0, "height": 10 } }),
            serde_json::json!({ "targetFormat": "png",
                "resize": { "mode": "fit", "maxWidth": 10, "maxHeight": 0 } }),
        ] {
            assert!(ImageOptions::parse(&bad).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn heic_and_avif_are_declined_explicitly() {
        for (name, bytes, key) in [
            (
                "photo.heic",
                b"\x00\x00\x00\x18ftypheic\x00\x00\x00\x00heicmif1".as_slice(),
                "error.image.heicUnsupported",
            ),
            (
                "photo.avif",
                b"\x00\x00\x00\x18ftypavif\x00\x00\x00\x00avifmif1".as_slice(),
                "error.image.avifUnsupported",
            ),
        ] {
            let f = Fixtures::new();
            let path = f.path(name);
            std::fs::write(&path, bytes).unwrap();
            let h = Harness::new();

            let job = h.job(&[&path], serde_json::json!({ "targetFormat": "jpeg" }));
            let err = h.run(&job).await.unwrap_err();
            assert_eq!(err.message_key, key);
            assert_eq!(err.code, ConversionErrorCode::UnsupportedFormat);
        }
    }

    #[tokio::test]
    async fn a_corrupted_image_fails_without_producing_output() {
        let f = Fixtures::new();
        let path = f.path("broken.png");
        // Valid PNG signature and IHDR, then garbage.
        let mut bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 16, 0, 0, 0, 16, 8, 6, 0, 0, 0]);
        bytes.extend_from_slice(b"garbage garbage garbage");
        std::fs::write(&path, &bytes).unwrap();
        let h = Harness::new();

        let job = h.job(&[&path], serde_json::json!({ "targetFormat": "jpeg" }));
        assert!(h.run(&job).await.is_err());
        assert!(std::fs::read_dir(&h.out_dir).unwrap().next().is_none());
    }

    #[tokio::test]
    async fn a_wrong_extension_source_converts_and_warns() {
        let f = Fixtures::new();
        // A real PNG named .jpg — the spec's headline case.
        let src = f.gradient("mislabelled.jpg", 24, 24, ImgFormat::Png);
        let h = Harness::new();

        let job = h.job(&[&src], serde_json::json!({ "targetFormat": "png" }));
        let result = h.run(&job).await.unwrap();

        assert!(result
            .warnings
            .iter()
            .any(|w| w.message_key == "warning.image.extensionMismatch"));
        assert!(h.out_dir.join("mislabelled.png").exists());
    }

    #[tokio::test]
    async fn batch_conversion_produces_one_output_per_input() {
        let f = Fixtures::new();
        let a = f.gradient("one.png", 20, 20, ImgFormat::Png);
        let b = f.gradient("two.png", 30, 30, ImgFormat::Png);
        let c = f.gradient("three.png", 40, 40, ImgFormat::Png);
        let h = Harness::new();

        let job = h.job(&[&a, &b, &c], serde_json::json!({ "targetFormat": "bmp" }));
        let result = h.run(&job).await.unwrap();

        assert_eq!(result.outputs.len(), 3);
        for name in ["one.bmp", "two.bmp", "three.bmp"] {
            assert!(h.out_dir.join(name).exists(), "{name} missing");
        }
        assert_eq!(result.validation_reports.len(), 3);
        assert!(result.validation_reports.iter().all(|r| r.valid));
    }

    #[tokio::test]
    async fn a_unicode_filename_survives_conversion() {
        let f = Fixtures::new();
        let src = f.gradient("ünïcode tëst 🎉.png", 16, 16, ImgFormat::Png);
        let h = Harness::new();

        let job = h.job(&[&src], serde_json::json!({ "targetFormat": "bmp" }));
        h.run(&job).await.unwrap();
        assert!(h.out_dir.join("ünïcode tëst 🎉.bmp").exists());
    }

    #[tokio::test]
    async fn the_source_file_is_never_modified() {
        let f = Fixtures::new();
        let src = f.gradient("original.png", 32, 32, ImgFormat::Png);
        let before = std::fs::read(&src).unwrap();
        let h = Harness::new();

        let job = h.job(&[&src], serde_json::json!({ "targetFormat": "jpeg" }));
        h.run(&job).await.unwrap();

        assert_eq!(std::fs::read(&src).unwrap(), before);
    }

    #[tokio::test]
    async fn an_oversized_image_is_refused_before_it_is_decoded() {
        // A PNG header claiming 60000×60000 — 14 GB of RGBA if we believed it.
        let f = Fixtures::new();
        let path = f.path("bomb.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(b"\x00\x00\x00\rIHDR");
        bytes.extend_from_slice(&60_000u32.to_be_bytes());
        bytes.extend_from_slice(&60_000u32.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        std::fs::write(&path, &bytes).unwrap();

        let h = Harness::new();
        let job = h.job(&[&path], serde_json::json!({ "targetFormat": "jpeg" }));
        let err = h.run(&job).await.unwrap_err();
        assert_eq!(err.message_key, "error.image.tooLarge");
    }

    #[test]
    fn batch_preflight_reports_bad_files_individually() {
        let f = Fixtures::new();
        let good = f.gradient("good.png", 20, 10, ImgFormat::Png);
        let heic = f.path("phone.heic");
        std::fs::write(&heic, b"\x00\x00\x00\x18ftypheic\x00\x00\x00\x00heicmif1").unwrap();

        let options = ImageOptions::parse(&serde_json::json!({ "targetFormat": "png" })).unwrap();
        let inputs = vec![descriptor(&good), descriptor(&heic)];
        let report = preflight_batch(&inputs, &options);

        assert_eq!(report.convertible_count, 1);
        assert_eq!(report.unsupported_count, 1);
        assert_eq!(report.files[0].error_message_key, None);
        assert_eq!((report.files[0].width, report.files[0].height), (20, 10));
        assert_eq!(
            report.files[1].error_message_key.as_deref(),
            Some("error.image.heicUnsupported")
        );
    }

    #[test]
    fn batch_preflight_asks_for_a_background_only_when_alpha_is_at_risk() {
        let f = Fixtures::new();
        let transparent = f.with_alpha("logo.png", 16, 16);
        let opaque = f.gradient("photo.png", 16, 16, ImgFormat::Png);
        let inputs = vec![descriptor(&transparent), descriptor(&opaque)];

        let to_jpeg = ImageOptions::parse(&serde_json::json!({ "targetFormat": "jpeg" })).unwrap();
        assert!(preflight_batch(&inputs, &to_jpeg).background_required);

        let to_png = ImageOptions::parse(&serde_json::json!({ "targetFormat": "png" })).unwrap();
        assert!(
            !preflight_batch(&inputs, &to_png).background_required,
            "PNG keeps alpha, so no background is needed"
        );

        let opaque_only = vec![descriptor(&opaque)];
        assert!(!preflight_batch(&opaque_only, &to_jpeg).background_required);
    }

    #[test]
    fn batch_warnings_are_stated_once_not_once_per_file() {
        let f = Fixtures::new();
        let a = f.with_alpha("a.png", 8, 8);
        let b = f.with_alpha("b.png", 8, 8);
        let c = f.with_alpha("c.png", 8, 8);
        let inputs = vec![descriptor(&a), descriptor(&b), descriptor(&c)];

        let options = ImageOptions::parse(&serde_json::json!({ "targetFormat": "jpeg" })).unwrap();
        let report = preflight_batch(&inputs, &options);

        let flattened = report
            .warnings
            .iter()
            .filter(|w| w.message_key == "warning.image.transparencyFlattened")
            .count();
        assert_eq!(flattened, 1, "got {:?}", report.warnings);
    }

    #[tokio::test]
    async fn a_lossless_target_does_not_warn_about_growing() {
        // JPEG -> PNG is always much larger: that is what lossless costs, and
        // warning about it made a correct conversion look like a failure.
        let f = Fixtures::new();
        let src = f.gradient("photo.jpg", 120, 90, ImgFormat::Jpeg);
        let h = Harness::new();

        let result = h
            .run(&h.job(&[&src], serde_json::json!({ "targetFormat": "png" })))
            .await
            .unwrap();

        assert!(result.output_grew(), "sanity: PNG really is bigger here");
        assert!(
            !result
                .warnings
                .iter()
                .any(|w| w.message_key == "warning.output.largerThanInput"),
            "expected growth must not be reported as a problem: {:?}",
            result.warnings
        );
    }

    #[tokio::test]
    async fn a_lossy_target_still_warns_when_it_grows() {
        // Re-encoding a tiny JPEG as JPEG can grow it; that IS worth saying,
        // because compression was the point.
        let f = Fixtures::new();
        let src = f.gradient("small.jpg", 24, 24, ImgFormat::Jpeg);
        let h = Harness::new();

        let result = h
            .run(&h.job(
                &[&src],
                serde_json::json!({ "targetFormat": "jpeg", "quality": 100 }),
            ))
            .await
            .unwrap();

        if result.output_grew() {
            assert!(
                result
                    .warnings
                    .iter()
                    .any(|w| w.message_key == "warning.output.largerThanInput"),
                "a lossy re-encode that grew should say so"
            );
        }
    }

    #[test]
    fn output_names_replace_the_extension_rather_than_appending() {
        assert_eq!(
            output_file_name("holiday.heic", ImageOutputFormat::Jpeg),
            "holiday.jpg"
        );
        assert_eq!(
            output_file_name("archive.tar.png", ImageOutputFormat::Bmp),
            "archive.tar.bmp"
        );
        assert_eq!(
            output_file_name("noextension", ImageOutputFormat::Png),
            "noextension.png"
        );
    }

    #[test]
    fn fit_dimensions_matches_the_resizer() {
        assert_eq!(fit_dimensions(200, 100, 50, 50), (50, 25));
        assert_eq!(fit_dimensions(100, 200, 50, 50), (25, 50));
        assert_eq!(fit_dimensions(1000, 1, 100, 100), (100, 1));
    }

    #[test]
    fn orientation_transforms_swap_axes_for_rotated_photos() {
        let wide = DynamicImage::ImageRgb8(image::RgbImage::new(20, 10));
        assert_eq!(apply_orientation(wide.clone(), 6).dimensions(), (10, 20));
        assert_eq!(apply_orientation(wide.clone(), 8).dimensions(), (10, 20));
        assert_eq!(apply_orientation(wide.clone(), 3).dimensions(), (20, 10));
        assert_eq!(apply_orientation(wide, 1).dimensions(), (20, 10));
    }
}
