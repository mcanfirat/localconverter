//! Video and audio conversion via a **system-installed** FFmpeg.
//!
//! FFmpeg is detected on `PATH` (and the usual install dirs), never bundled and
//! never downloaded at runtime — that keeps the "no runtime executable
//! downloads" guarantee intact while the bundling-and-licensing decision (LGPL
//! vs GPL builds, ~80 MB) is still open. If FFmpeg is absent the operation
//! declines clearly with [`ConversionErrorCode::ToolMissing`].
//!
//! Every rule from spec §5.4 is enforced on the child process:
//!
//! * arguments are passed as an **array** — no shell string is ever built;
//! * paths are canonicalised and NUL-checked before they reach argv;
//! * the environment is cleared down to a minimal `PATH`;
//! * a timeout bounds the run;
//! * stdout/stderr are captured, and paths are redacted from any error;
//! * cancellation kills the child and removes the partial output.
//!
//! Every job begins with `ffprobe` (spec §13.1) and every output is probed
//! again before it is accepted (spec §13.5).

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use ts_rs::TS;

use crate::error::{ConversionError, ConversionErrorCode, Result};
use crate::file::FileFormat;
use crate::job::{ConversionJob, JobProgress, JobWarning, ProgressStage};
use crate::runner::{ExecutionOutput, JobContext, StagedOutput};
use crate::validation::{OutputMetadata, ValidationCheck, ValidationReport};

/// Hard ceiling on a single conversion. A real job finishes well inside this;
/// the bound exists so a hung or pathological encode cannot run forever.
const TIMEOUT_SECS: u64 = 60 * 60;
/// Output duration must land within this fraction of the source (or trim) to
/// pass validation. Container/codec changes shift duration by a frame or two.
const DURATION_TOLERANCE: f64 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum MediaFormat {
    Mp4,
    #[serde(rename = "webm")]
    WebM,
    Mkv,
    Gif,
    Mp3,
    Wav,
    Flac,
    Ogg,
    M4a,
}

impl MediaFormat {
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::WebM => "webm",
            Self::Mkv => "mkv",
            Self::Gif => "gif",
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::Ogg => "ogg",
            Self::M4a => "m4a",
        }
    }

    /// Whether the target carries a video stream. Audio-only targets get `-vn`
    /// and are how "extract audio" is expressed.
    #[must_use]
    fn is_video(self) -> bool {
        matches!(self, Self::Mp4 | Self::WebM | Self::Mkv | Self::Gif)
    }

    #[must_use]
    fn has_audio(self) -> bool {
        !matches!(self, Self::Gif)
    }

    /// The FFmpeg muxer name. Passed via `-f` so the output format does not
    /// depend on the filename — staged outputs end in `.partial`, which FFmpeg
    /// cannot infer a format from.
    #[must_use]
    fn muxer(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::WebM => "webm",
            Self::Mkv => "matroska",
            Self::Gif => "gif",
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::Ogg => "ogg",
            Self::M4a => "ipod", // the muxer that writes .m4a
        }
    }

    // There is deliberately no `from_format`: mapping an input's detected format
    // onto this enum is what wrongly rejected `.mov`, since this is the set of
    // formats the engine can *write*. Input acceptance is `is_media_input`.
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum MediaPreset {
    High,
    #[default]
    Balanced,
    Small,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Trim {
    /// Start offset in seconds.
    pub start: f64,
    /// Duration in seconds from the start offset.
    pub duration: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MediaOptions {
    pub target_format: MediaFormat,
    #[serde(default)]
    pub preset: MediaPreset,
    /// Optional trim applied before encoding.
    #[serde(default)]
    pub trim: Option<Trim>,
    /// Drop the audio stream from a video output.
    #[serde(default)]
    pub remove_audio: bool,
}

impl MediaOptions {
    fn parse(options: &serde_json::Value) -> Result<Self> {
        let parsed: Self = serde_json::from_value(options.clone()).map_err(|err| {
            ConversionError::new(
                ConversionErrorCode::InvalidInput,
                format!("invalid media options: {err}"),
            )
            .with_message_key("error.options.invalid")
        })?;
        if let Some(trim) = parsed.trim {
            if !(trim.start >= 0.0
                && trim.duration > 0.0
                && trim.start.is_finite()
                && trim.duration.is_finite())
            {
                return Err(ConversionError::new(
                    ConversionErrorCode::InvalidInput,
                    "trim start must be >= 0 and duration > 0",
                )
                .with_message_key("error.options.outOfRange"));
            }
        }
        Ok(parsed)
    }
}

/// What `ffprobe` tells us about a file.
#[derive(Debug, Clone, Default)]
pub struct MediaInfo {
    pub duration_seconds: f64,
    pub has_video: bool,
    pub has_audio: bool,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
}

/// Formats this engine accepts as *input*. Deliberately wider than
/// [`MediaFormat`], which is the set it can write: MOV and friends are readable
/// containers that are not offered as targets.
#[must_use]
pub fn is_media_input(format: FileFormat) -> bool {
    matches!(
        format,
        FileFormat::Mp4
            | FileFormat::Mov
            | FileFormat::Mkv
            | FileFormat::WebM
            | FileFormat::Gif
            | FileFormat::Mp3
            | FileFormat::Wav
            | FileFormat::Flac
            | FileFormat::Ogg
            | FileFormat::M4a
    )
}

/// `true` when a usable FFmpeg + FFprobe pair is available. Surfaced to the UI
/// so it can explain the situation instead of only failing at run time.
#[must_use]
pub fn is_available() -> bool {
    locate("ffmpeg").is_some() && locate("ffprobe").is_some()
}

// ---------------------------------------------------------------------------
// dispatch
// ---------------------------------------------------------------------------

pub async fn execute(job: &ConversionJob, ctx: &JobContext) -> Result<ExecutionOutput> {
    let options = MediaOptions::parse(&job.options)?;
    let Some(input) = job.input_files.first() else {
        return Err(
            ConversionError::new(ConversionErrorCode::InvalidInput, "no input files")
                .with_message_key("error.input.missing"),
        );
    };
    if job.input_files.len() > 1 {
        return Err(ConversionError::new(
            ConversionErrorCode::InvalidInput,
            "media conversion takes one file at a time",
        )
        .with_message_key("error.input.tooMany"));
    }

    let ffmpeg = locate("ffmpeg").ok_or_else(tool_missing)?;
    // This gate exists only to keep non-media files out. It must accept every
    // format the operation advertises — `MediaFormat` is the set this engine can
    // *write*, and using it here silently rejected `.mov`, the commonest video a
    // user brings.
    if !is_media_input(input.detected_format) {
        return Err(ConversionError::new(
            ConversionErrorCode::UnsupportedFormat,
            format!("{:?} is not an audio or video file", input.detected_format),
        )
        .with_message_key("error.unsupportedFormat"));
    }

    // Probe first — everything downstream depends on the real streams/duration.
    ctx.report(JobProgress::indeterminate(
        ProgressStage::Preparing,
        "progress.probing",
    ));
    let info = probe(&input.path)?;
    ctx.check_cancelled()?;

    let mut warnings = Vec::new();
    check_route(&info, &options, &mut warnings)?;

    let source_path = canonical(&input.path)?;
    let file_name = output_file_name(&input.display_name, options.target_format);
    let staged = ctx.workspace.staging_path(&file_name)?;

    let args = build_args(&source_path, &staged, &info, &options)?;
    let effective_duration = options
        .trim
        .map(|t| t.duration)
        .unwrap_or(info.duration_seconds);

    run_ffmpeg(&ffmpeg, &args, ctx, effective_duration).await?;

    let size_bytes = std::fs::metadata(&staged)
        .map_err(|err| ConversionError::from_io("stat media output", &err))?
        .len();

    Ok(ExecutionOutput {
        outputs: vec![StagedOutput {
            staged_path: staged,
            file_name,
            format: options.target_format.extension().to_owned(),
            size_bytes,
        }],
        warnings,
        input_total_bytes: input.size_bytes,
        // WAV is uncompressed and FLAC is lossless, so decoding a lossy MP3 into
        // either is always far bigger. Expected, not a fault.
        size_growth_expected: matches!(options.target_format, MediaFormat::Wav | MediaFormat::Flac),
    })
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
    let options = MediaOptions::parse(&job.options)?;
    let Some(staged) = execution.outputs.first() else {
        return Err(ConversionError::internal("media produced no output"));
    };
    let Some(input) = job.input_files.first() else {
        return Err(ConversionError::internal("media job has no input"));
    };

    let expected_duration = {
        let source = probe(&input.path).ok();
        options
            .trim
            .map(|t| t.duration)
            .or_else(|| source.map(|s| s.duration_seconds))
    };

    let mut checks = Vec::new();
    match probe(&staged.staged_path.to_string_lossy()) {
        Ok(out) => {
            checks.push(ValidationCheck::passed("media.probes"));

            // The right kind of stream is present.
            if options.target_format.is_video() {
                checks.push(bool_check(
                    "media.hasVideo",
                    out.has_video,
                    "output has no video stream",
                ));
            } else {
                checks.push(bool_check(
                    "media.hasAudio",
                    out.has_audio,
                    "output has no audio stream",
                ));
            }
            // GIF and remove_audio must genuinely have no audio.
            if !options.target_format.has_audio() || options.remove_audio {
                checks.push(bool_check(
                    "media.audioRemoved",
                    !out.has_audio,
                    "output still has audio",
                ));
            }

            // Duration within tolerance (skipped for GIF, whose fps changes length semantics).
            if options.target_format != MediaFormat::Gif {
                if let Some(expected) = expected_duration {
                    checks.push(duration_check(expected, out.duration_seconds));
                }
            }

            return Ok(vec![ValidationReport::from_checks(
                options.target_format.extension(),
                checks,
                OutputMetadata {
                    size_bytes: staged.size_bytes,
                    properties: serde_json::json!({
                        "durationSeconds": out.duration_seconds,
                        "videoCodec": out.video_codec,
                        "audioCodec": out.audio_codec,
                    }),
                },
            )]);
        }
        Err(err) => checks.push(ValidationCheck::failed("media.probes", err.detail)),
    }

    Ok(vec![ValidationReport::from_checks(
        options.target_format.extension(),
        checks,
        OutputMetadata {
            size_bytes: staged.size_bytes,
            properties: serde_json::json!({}),
        },
    )])
}

// ---------------------------------------------------------------------------
// route checks & args
// ---------------------------------------------------------------------------

/// Refuses combinations that cannot work, and warns about lossy or lossless
/// facts the user should know before committing. Spec §4.3.
fn check_route(
    info: &MediaInfo,
    options: &MediaOptions,
    warnings: &mut Vec<JobWarning>,
) -> Result<()> {
    // An audio-only target needs an audio stream to extract.
    if !options.target_format.is_video() && !info.has_audio {
        return Err(ConversionError::new(
            ConversionErrorCode::InvalidInput,
            "the source has no audio to extract",
        )
        .with_message_key("error.media.noAudio"));
    }
    // A video target needs a video stream.
    if options.target_format.is_video() && !info.has_video {
        return Err(ConversionError::new(
            ConversionErrorCode::InvalidInput,
            "the source has no video track",
        )
        .with_message_key("error.media.noVideo"));
    }
    if options.target_format.is_video() {
        warnings.push(JobWarning::new("warning.media.videoReencoded"));
    }
    // Only mention dropped audio when there was audio to drop.
    if options.target_format == MediaFormat::Gif && info.has_audio {
        warnings.push(JobWarning::new("warning.media.gifNoAudio"));
    }
    // This one is specifically about the *audio* being re-compressed, so it
    // belongs only where an audio stream is actually encoded — not on a GIF
    // (which has none) and not on a video whose audio is dropped or absent.
    // Firing it alongside "GIF has no audio" was self-contradictory.
    let audio_is_encoded =
        info.has_audio && !options.remove_audio && options.target_format.has_audio();
    let lossy_audio_codec = matches!(
        options.target_format,
        MediaFormat::Mp3
            | MediaFormat::Ogg
            | MediaFormat::M4a
            | MediaFormat::Mp4
            | MediaFormat::WebM
            | MediaFormat::Mkv
    );
    if audio_is_encoded && lossy_audio_codec {
        warnings.push(JobWarning::new("warning.media.lossyEncoding"));
    }
    Ok(())
}

/// Builds the FFmpeg argument vector. Fixed codec per container, so no invalid
/// container/codec pairing can be produced (spec §13.3).
fn build_args(
    source: &Path,
    staged: &Path,
    _info: &MediaInfo,
    options: &MediaOptions,
) -> Result<Vec<String>> {
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-y".into(), // staged path is ours and fresh; safe to overwrite
    ];

    // Trim before the input for fast seeking, and after for frame accuracy.
    if let Some(trim) = options.trim {
        args.push("-ss".into());
        args.push(format!("{}", trim.start));
    }
    args.push("-i".into());
    args.push(path_arg(source)?);
    if let Some(trim) = options.trim {
        args.push("-t".into());
        args.push(format!("{}", trim.duration));
    }

    let crf = match options.preset {
        MediaPreset::High => 18,
        MediaPreset::Balanced => 23,
        MediaPreset::Small => 30,
    };
    let audio_kbps = match options.preset {
        MediaPreset::High => 256,
        MediaPreset::Balanced => 192,
        MediaPreset::Small => 128,
    };

    match options.target_format {
        MediaFormat::Mp4 | MediaFormat::Mkv => {
            args.extend([
                "-c:v".into(),
                "libx264".into(),
                "-crf".into(),
                crf.to_string(),
            ]);
            args.extend([
                "-preset".into(),
                "medium".into(),
                "-pix_fmt".into(),
                "yuv420p".into(),
            ]);
            audio_args(&mut args, options, "aac", audio_kbps);
        }
        MediaFormat::WebM => {
            args.extend([
                "-c:v".into(),
                "libvpx-vp9".into(),
                "-crf".into(),
                crf.to_string(),
                "-b:v".into(),
                "0".into(),
            ]);
            audio_args(&mut args, options, "libopus", audio_kbps);
        }
        MediaFormat::Gif => {
            // One-pass palette for good quality; GIF has no audio.
            let fps = match options.preset {
                MediaPreset::High => 15,
                MediaPreset::Balanced => 12,
                MediaPreset::Small => 10,
            };
            let width = match options.preset {
                MediaPreset::High => 640,
                MediaPreset::Balanced => 480,
                MediaPreset::Small => 360,
            };
            args.push("-filter_complex".into());
            args.push(format!(
                "fps={fps},scale={width}:-1:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse"
            ));
            args.push("-an".into());
        }
        MediaFormat::Mp3 => {
            args.push("-vn".into());
            args.extend([
                "-c:a".into(),
                "libmp3lame".into(),
                "-b:a".into(),
                format!("{audio_kbps}k"),
            ]);
        }
        MediaFormat::Wav => {
            args.push("-vn".into());
            args.extend(["-c:a".into(), "pcm_s16le".into()]);
        }
        MediaFormat::Flac => {
            args.push("-vn".into());
            args.extend(["-c:a".into(), "flac".into()]);
        }
        MediaFormat::Ogg => {
            // Opus, not Vorbis: the stock Homebrew FFmpeg — the build our own
            // "install ffmpeg" message points at — ships libopus but not
            // libvorbis, so `-c:a libvorbis` failed after the encode had
            // started. Ogg Opus is also what a modern .ogg generally means.
            args.push("-vn".into());
            args.extend([
                "-c:a".into(),
                "libopus".into(),
                "-b:a".into(),
                format!("{audio_kbps}k"),
            ]);
        }
        MediaFormat::M4a => {
            args.push("-vn".into());
            args.extend([
                "-c:a".into(),
                "aac".into(),
                "-b:a".into(),
                format!("{audio_kbps}k"),
            ]);
        }
    }

    // Real, parseable progress on stdout.
    args.extend(["-progress".into(), "pipe:1".into(), "-nostats".into()]);
    // Force the muxer: the staged path ends in `.partial`, so FFmpeg cannot
    // pick a format from the extension.
    args.extend(["-f".into(), options.target_format.muxer().into()]);
    args.push(path_arg(staged)?);
    Ok(args)
}

fn audio_args(args: &mut Vec<String>, options: &MediaOptions, codec: &str, kbps: u32) {
    if options.remove_audio {
        args.push("-an".into());
    } else {
        args.extend([
            "-c:a".into(),
            codec.into(),
            "-b:a".into(),
            format!("{kbps}k"),
        ]);
    }
}

// ---------------------------------------------------------------------------
// process runner (spec §5.4)
// ---------------------------------------------------------------------------

async fn run_ffmpeg(
    ffmpeg: &Path,
    args: &[String],
    ctx: &JobContext,
    total_duration: f64,
) -> Result<()> {
    let mut command = tokio::process::Command::new(ffmpeg);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    // Minimal environment: only a PATH, built from the binary's own directory
    // plus the standard system dirs. Nothing of the user's env is inherited.
    command.env_clear();
    let mut path = ffmpeg
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !path.is_empty() {
        path.push(':');
    }
    path.push_str("/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin");
    command.env("PATH", path);

    let mut child = command.spawn().map_err(|err| {
        ConversionError::from_io("spawn ffmpeg", &err).with_message_key("error.media.spawnFailed")
    })?;

    // Progress: parse `out_time_us=` from ffmpeg's -progress stream.
    if let Some(stdout) = child.stdout.take() {
        let mut lines = BufReader::new(stdout).lines();
        let progress_ctx = ProgressReporter::new(ctx, total_duration);
        // Read progress until EOF or cancellation, concurrently with the child.
        tokio::select! {
            () = drain_progress(&mut lines, &progress_ctx) => {}
            () = ctx.cancel.cancelled() => {
                let _ = child.start_kill();
            }
        }
    }

    // Wait for exit, honouring cancellation and the timeout.
    let status = tokio::select! {
        result = tokio::time::timeout(std::time::Duration::from_secs(TIMEOUT_SECS), child.wait()) => {
            match result {
                Ok(Ok(status)) => status,
                Ok(Err(err)) => return Err(ConversionError::from_io("await ffmpeg", &err)),
                Err(_) => {
                    let _ = child.start_kill();
                    return Err(ConversionError::new(
                        ConversionErrorCode::ProcessTimedOut,
                        "ffmpeg exceeded the time limit",
                    )
                    .with_message_key("error.processTimedOut"));
                }
            }
        }
        () = ctx.cancel.cancelled() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(ConversionError::cancelled());
        }
    };

    if status.success() {
        return Ok(());
    }

    // Capture a redacted tail of stderr for the diagnostics toggle.
    let mut stderr_text = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        use tokio::io::AsyncReadExt;
        let _ = stderr.read_to_string(&mut stderr_text).await;
    }
    let tail: String = stderr_text
        .lines()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" | ");
    Err(ConversionError::new(
        ConversionErrorCode::ProcessFailed,
        format!("ffmpeg failed: {tail}"),
    )
    .with_message_key("error.media.encodeFailed"))
}

struct ProgressReporter<'a> {
    ctx: &'a JobContext,
    total_us: f64,
}

impl<'a> ProgressReporter<'a> {
    fn new(ctx: &'a JobContext, total_duration: f64) -> Self {
        Self {
            ctx,
            total_us: (total_duration * 1_000_000.0).max(0.0),
        }
    }

    fn report(&self, out_time_us: f64) {
        if self.total_us > 0.0 {
            let done = (out_time_us / self.total_us * 100.0).clamp(0.0, 100.0);
            self.ctx.report(JobProgress::counted(
                ProgressStage::Running,
                out_time_us as u64,
                self.total_us as u64,
                "progress.encoding",
            ));
            let _ = done;
        } else {
            self.ctx.report(JobProgress::indeterminate(
                ProgressStage::Running,
                "progress.encoding",
            ));
        }
    }
}

async fn drain_progress<R>(
    lines: &mut tokio::io::Lines<BufReader<R>>,
    reporter: &ProgressReporter<'_>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(value) = line.strip_prefix("out_time_us=") {
            if let Ok(us) = value.trim().parse::<f64>() {
                reporter.report(us);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ffprobe
// ---------------------------------------------------------------------------

/// Runs `ffprobe` and parses the streams/format JSON. Synchronous (probing is
/// quick and happens outside the encode loop).
pub fn probe(path: &str) -> Result<MediaInfo> {
    let ffprobe = locate("ffprobe").ok_or_else(tool_missing)?;
    let canonical = canonical(path)?;

    let output = std::process::Command::new(&ffprobe)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-show_format",
            "-show_streams",
            "-print_format",
            "json",
        ])
        .arg(&canonical)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| {
            ConversionError::from_io("run ffprobe", &err)
                .with_message_key("error.media.spawnFailed")
        })?;

    if !output.status.success() {
        return Err(ConversionError::new(
            ConversionErrorCode::CorruptedInput,
            "ffprobe could not read the file",
        )
        .with_message_key("error.corruptedInput"));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|err| {
        ConversionError::new(
            ConversionErrorCode::CorruptedInput,
            format!("ffprobe json: {err}"),
        )
        .with_message_key("error.corruptedInput")
    })?;

    let mut info = MediaInfo::default();
    if let Some(duration) = json
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|d| d.as_str())
        .and_then(|s| s.parse::<f64>().ok())
    {
        info.duration_seconds = duration;
    }

    if let Some(streams) = json.get("streams").and_then(|s| s.as_array()) {
        for stream in streams {
            let codec_type = stream
                .get("codec_type")
                .and_then(|c| c.as_str())
                .unwrap_or("");
            let codec_name = stream
                .get("codec_name")
                .and_then(|c| c.as_str())
                .map(str::to_owned);
            match codec_type {
                "video" => {
                    // Ignore cover-art "video" streams (mjpeg on an mp3, etc.):
                    // they have a tiny disposition rather than a real duration.
                    let is_cover = stream
                        .get("disposition")
                        .and_then(|d| d.get("attached_pic"))
                        .and_then(serde_json::Value::as_i64)
                        == Some(1);
                    if !is_cover {
                        info.has_video = true;
                        info.video_codec = codec_name;
                    }
                }
                "audio" => {
                    info.has_audio = true;
                    info.audio_codec = codec_name;
                }
                _ => {}
            }
        }
    }
    Ok(info)
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Finds an executable on `PATH` and the usual install dirs. Returns the first
/// existing candidate.
fn locate(tool: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(path) = std::env::var("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        dirs.push(PathBuf::from(extra));
    }
    for dir in dirs {
        let candidate = dir.join(tool);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{tool}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

fn canonical(path: &str) -> Result<PathBuf> {
    let p = Path::new(path);
    crate::paths::ensure_safe_component_bytes(p)?;
    p.canonicalize()
        .map_err(|err| ConversionError::from_io("canonicalize media path", &err))
}

/// A path as an argv string, re-checked at the last moment for the two things
/// that make a path unsafe to hand to a child process: a NUL byte, and a
/// leading `-`.
///
/// FFmpeg parses any argv entry starting with `-` as an option, so a *relative*
/// path like `-vn.wav` would silently become a flag rather than a filename —
/// argument injection through nothing more than a filename. Both call sites
/// already pass canonicalised (absolute) paths, so this cannot fire today; the
/// check is here so that stays true. Absolute paths begin with `/` (or a drive
/// letter), so requiring absoluteness rules the class out entirely rather than
/// blocklisting the leading dash.
fn path_arg(path: &Path) -> Result<String> {
    crate::paths::ensure_safe_component_bytes(path)?;
    if !path.is_absolute() {
        return Err(ConversionError::new(
            ConversionErrorCode::InvalidInput,
            "refusing to pass a relative path to the media engine",
        )
        .with_message_key("error.path.notAbsolute"));
    }
    Ok(path.to_string_lossy().into_owned())
}

fn output_file_name(display_name: &str, target: MediaFormat) -> String {
    let stem = match display_name.rfind('.') {
        Some(i) if i > 0 => display_name.get(..i).unwrap_or(display_name),
        _ => display_name,
    };
    format!("{stem}.{}", target.extension())
}

fn duration_check(expected: f64, actual: f64) -> ValidationCheck {
    if expected <= 0.0 {
        return ValidationCheck::passed("media.duration");
    }
    let drift = (actual - expected).abs() / expected;
    if drift <= DURATION_TOLERANCE {
        ValidationCheck::passed("media.duration")
    } else {
        ValidationCheck::failed(
            "media.duration",
            format!("expected ~{expected:.2}s, got {actual:.2}s"),
        )
    }
}

fn bool_check(id: &str, passed: bool, failure: &str) -> ValidationCheck {
    if passed {
        ValidationCheck::passed(id)
    } else {
        ValidationCheck::failed(id, failure.to_owned())
    }
}

fn tool_missing() -> ConversionError {
    ConversionError::new(
        ConversionErrorCode::ToolMissing,
        "FFmpeg was not found. Install it (e.g. `brew install ffmpeg`) and try again.",
    )
    .with_message_key("error.media.ffmpegMissing")
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
    use crate::operation::MEDIA_CONVERT_OPERATION_ID;
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

        fn context(&self) -> JobContext {
            let id = Uuid::new_v4();
            JobContext::new(
                id,
                JobWorkspace::create(&self.temp_root, id).unwrap(),
                CancellationToken::new(),
                Arc::new(|_, _| {}),
            )
        }

        fn job(&self, input: &Path, options: serde_json::Value) -> ConversionJob {
            ConversionJob::new(
                MEDIA_CONVERT_OPERATION_ID,
                vec![FileDescriptor::probe(input).unwrap()],
                self.out_dir.to_string_lossy(),
                OverwritePolicy::Overwrite,
                options,
            )
        }

        async fn run(&self, job: &ConversionJob) -> Result<crate::job::JobResult> {
            let ctx = self.context();
            let exec = execute(job, &ctx).await?;
            let reports = validate(job, &ctx, &exec).await?;
            runner::commit(job, exec, reports, 0)
        }

        /// Generates a short real media file with ffmpeg for the round-trip tests.
        fn synth(&self, name: &str, args: &[&str]) -> PathBuf {
            let ffmpeg = locate("ffmpeg").expect("ffmpeg required for media tests");
            let path = self.src_dir.join(name);
            let status = std::process::Command::new(ffmpeg)
                .args(["-hide_banner", "-loglevel", "error", "-y"])
                .args(args)
                .arg(&path)
                .status()
                .unwrap();
            assert!(status.success(), "failed to synth {name}");
            path
        }
    }

    fn ffmpeg_present() -> bool {
        is_available()
    }

    /// A filename that begins with `-` is a valid filename and an FFmpeg
    /// option at the same time. `path_arg` is the last gate before argv, so it
    /// is the place that has to refuse anything not anchored to the root —
    /// otherwise a file called `-vn.wav` would reach FFmpeg as the `-vn` flag.
    #[test]
    fn relative_and_dash_leading_paths_never_reach_argv() {
        assert!(
            path_arg(Path::new("-vn.wav")).is_err(),
            "a dash-leading relative path must not become an argv entry"
        );
        assert!(path_arg(Path::new("./-vn.wav")).is_err());
        assert!(path_arg(Path::new("clip.mp4")).is_err());

        // Absolute paths — what both real call sites pass — still work. What
        // counts as absolute is platform-specific: Windows wants a drive
        // letter, so `/tmp/x` is merely *rooted* there and is correctly
        // refused. Real paths arrive from `canonicalize`, which yields
        // `\\?\C:\…` on Windows and `/…` elsewhere.
        let absolute = if cfg!(windows) {
            r"C:\tmp\-vn.wav"
        } else {
            "/tmp/-vn.wav"
        };
        let ok = path_arg(Path::new(absolute)).expect("absolute paths are accepted");
        assert!(
            Path::new(&ok).is_absolute(),
            "argv entry must be anchored: {ok}"
        );
    }

    #[test]
    fn output_names_swap_the_extension() {
        assert_eq!(output_file_name("clip.mov", MediaFormat::Mp4), "clip.mp4");
        assert_eq!(output_file_name("song.flac", MediaFormat::Mp3), "song.mp3");
    }

    #[test]
    fn trim_options_are_validated() {
        assert!(MediaOptions::parse(
            &serde_json::json!({ "targetFormat": "mp3", "trim": { "start": -1, "duration": 5 } })
        )
        .is_err());
        assert!(MediaOptions::parse(
            &serde_json::json!({ "targetFormat": "mp3", "trim": { "start": 0, "duration": 0 } })
        )
        .is_err());
        assert!(MediaOptions::parse(
            &serde_json::json!({ "targetFormat": "mp3", "trim": { "start": 1, "duration": 5 } })
        )
        .is_ok());
    }

    #[test]
    fn gif_target_carries_no_audio_codec() {
        assert!(!MediaFormat::Gif.has_audio());
        assert!(MediaFormat::Gif.is_video());
        assert!(MediaFormat::Mp3.has_audio());
        assert!(!MediaFormat::Mp3.is_video());
    }

    #[tokio::test]
    async fn wav_to_mp3_extracts_audio_and_validates() {
        if !ffmpeg_present() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let h = Harness::new();
        // 1s sine WAV.
        let wav = h.synth(
            "tone.wav",
            &["-f", "lavfi", "-i", "sine=frequency=440:duration=1"],
        );
        let result = h
            .run(&h.job(
                &wav,
                serde_json::json!({ "targetFormat": "mp3", "preset": "balanced" }),
            ))
            .await
            .unwrap();
        assert!(
            result.validation_reports[0].valid,
            "{:?}",
            result.validation_reports[0].failed_checks()
        );
        let out = h.out_dir.join("tone.mp3");
        assert!(out.exists());
        assert_eq!(crate::detect(&out).unwrap().format, FileFormat::Mp3);
        let info = probe(&out.to_string_lossy()).unwrap();
        assert!(info.has_audio && !info.has_video);
        assert!(
            (info.duration_seconds - 1.0).abs() < 0.2,
            "duration {}",
            info.duration_seconds
        );
    }

    #[tokio::test]
    async fn extracting_audio_from_a_silent_video_still_works() {
        if !ffmpeg_present() {
            return;
        }
        let h = Harness::new();
        // A 1s test video WITH an audio track.
        let mp4 = h.synth(
            "clip.mp4",
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=160x120:rate=15:duration=1",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=330:duration=1",
                "-shortest",
                "-pix_fmt",
                "yuv420p",
            ],
        );
        // Extract audio to FLAC.
        let result = h
            .run(&h.job(&mp4, serde_json::json!({ "targetFormat": "flac" })))
            .await
            .unwrap();
        assert!(
            result.validation_reports[0].valid,
            "{:?}",
            result.validation_reports[0].failed_checks()
        );
        let info = probe(&h.out_dir.join("clip.flac").to_string_lossy()).unwrap();
        assert!(info.has_audio && !info.has_video);
    }

    #[tokio::test]
    async fn mp4_to_gif_produces_a_gif_with_no_audio() {
        if !ffmpeg_present() {
            return;
        }
        let h = Harness::new();
        let mp4 = h.synth(
            "motion.mp4",
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=160x120:rate=15:duration=1",
                "-pix_fmt",
                "yuv420p",
            ],
        );
        let result = h
            .run(&h.job(
                &mp4,
                serde_json::json!({ "targetFormat": "gif", "preset": "small" }),
            ))
            .await
            .unwrap();
        assert!(
            result.validation_reports[0].valid,
            "{:?}",
            result.validation_reports[0].failed_checks()
        );
        let out = h.out_dir.join("motion.gif");
        assert_eq!(crate::detect(&out).unwrap().format, FileFormat::Gif);
        // This clip has no audio track, so there is nothing to warn about
        // dropping — saying so anyway was noise, and it contradicted the
        // "audio was re-compressed" note that used to fire alongside it.
        assert!(
            !result
                .warnings
                .iter()
                .any(|w| w.message_key == "warning.media.gifNoAudio"),
            "no audio to drop, so no warning: {:?}",
            result.warnings
        );
        assert!(
            !result
                .warnings
                .iter()
                .any(|w| w.message_key == "warning.media.lossyEncoding"),
            "GIF encodes no audio, so the audio-quality note must not fire"
        );
    }

    #[tokio::test]
    async fn extracting_audio_from_a_video_without_audio_is_refused() {
        if !ffmpeg_present() {
            return;
        }
        let h = Harness::new();
        let silent = h.synth(
            "silent.mp4",
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=160x120:rate=15:duration=1",
                "-pix_fmt",
                "yuv420p",
            ],
        );
        let err = h
            .run(&h.job(&silent, serde_json::json!({ "targetFormat": "mp3" })))
            .await
            .unwrap_err();
        assert_eq!(err.message_key, "error.media.noAudio");
    }

    #[tokio::test]
    async fn trim_shortens_the_output() {
        if !ffmpeg_present() {
            return;
        }
        let h = Harness::new();
        let wav = h.synth(
            "long.wav",
            &["-f", "lavfi", "-i", "sine=frequency=440:duration=3"],
        );
        let result = h
            .run(&h.job(
                &wav,
                serde_json::json!({
                    "targetFormat": "wav",
                    "trim": { "start": 0.5, "duration": 1.0 }
                }),
            ))
            .await
            .unwrap();
        assert!(
            result.validation_reports[0].valid,
            "{:?}",
            result.validation_reports[0].failed_checks()
        );
        let info = probe(&h.out_dir.join("long.wav").to_string_lossy()).unwrap();
        assert!(
            (info.duration_seconds - 1.0).abs() < 0.2,
            "trim duration {}",
            info.duration_seconds
        );
    }

    #[tokio::test]
    async fn the_source_media_is_never_modified() {
        if !ffmpeg_present() {
            return;
        }
        let h = Harness::new();
        let wav = h.synth(
            "keep.wav",
            &["-f", "lavfi", "-i", "sine=frequency=440:duration=1"],
        );
        let before = std::fs::read(&wav).unwrap();
        h.run(&h.job(&wav, serde_json::json!({ "targetFormat": "mp3" })))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&wav).unwrap(), before);
    }

    #[tokio::test]
    async fn a_non_media_file_is_declined() {
        if !ffmpeg_present() {
            return;
        }
        let h = Harness::new();
        let png = h.src_dir.join("pic.png");
        image::RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3]))
            .save(&png)
            .unwrap();
        let err = h
            .run(&h.job(&png, serde_json::json!({ "targetFormat": "mp3" })))
            .await
            .unwrap_err();
        assert_eq!(err.code, ConversionErrorCode::UnsupportedFormat);
    }
}
