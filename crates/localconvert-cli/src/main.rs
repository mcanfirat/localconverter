//! `localconvert` — the command-line interface.
//!
//! Shares `localconvert-core` with the desktop app, so every guarantee (source
//! never modified, output validated before it is kept, honest failures) holds
//! here too. Exit codes are stable and documented in `docs/CLI.md`.

mod exit;
mod run;
mod text;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use localconvert_core::{
    ConversionError, ConversionErrorCode, ImageOutputFormat, MediaFormat, TabularFormat,
};
use run::{parse_policy, resolve_output, RunOptions};

#[derive(Parser)]
#[command(
    name = "localconvert",
    version,
    about = "Private, local file conversion. Your files never leave your computer."
)]
struct Cli {
    /// Emit a machine-readable JSON result instead of human text.
    #[arg(long, global = true)]
    json: bool,
    /// Suppress progress and non-essential output.
    #[arg(long, global = true)]
    quiet: bool,
    /// What to do if an output file already exists: fail|rename|skip|overwrite.
    #[arg(long, global = true, default_value = "rename")]
    overwrite: String,
    /// Directory (or filename) to write the result to. Defaults to the input's folder.
    #[arg(long, short, global = true)]
    output: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Convert an image or media file to another format.
    Convert {
        input: PathBuf,
        /// Target format, e.g. jpg, png, webp, mp4, mp3.
        #[arg(long)]
        to: String,
        /// JPEG quality 1–100 (images only).
        #[arg(long)]
        quality: Option<u8>,
        /// Background for transparency when converting to a format without alpha
        /// (e.g. JPEG): white|black|#rrggbb.
        #[arg(long)]
        background: Option<String>,
        /// Media quality preset: high|balanced|small.
        #[arg(long)]
        preset: Option<String>,
    },
    /// Archive operations.
    Archive {
        #[command(subcommand)]
        action: ArchiveAction,
    },
    /// PDF operations.
    Pdf {
        #[command(subcommand)]
        action: PdfAction,
    },
    /// Spreadsheet / structured-data conversion.
    Spreadsheet {
        input: PathBuf,
        /// Target format: csv|tsv|xlsx|json.
        #[arg(long)]
        to: String,
        /// Column typing, repeatable: --column id:text --column age:number.
        #[arg(long = "column")]
        columns: Vec<String>,
    },
}

#[derive(Subcommand)]
enum ArchiveAction {
    /// Bundle files into an archive.
    Create {
        files: Vec<PathBuf>,
        /// Archive format: zip|tar|targz.
        #[arg(long, default_value = "zip")]
        format: String,
    },
    /// Extract an archive.
    Extract { archive: PathBuf },
}

#[derive(Subcommand)]
enum PdfAction {
    /// Merge PDFs into one.
    Merge { files: Vec<PathBuf> },
    /// Split a PDF into one file per page.
    Split { file: PathBuf },
    /// Keep/reorder pages and optionally rotate.
    Pages {
        file: PathBuf,
        /// Pages to keep, e.g. "1-3,7,10-12". Blank = all.
        #[arg(long)]
        pages: Option<String>,
        /// Rotation: 0|90|180|270.
        #[arg(long)]
        rotate: Option<i64>,
    },
    /// Strip document metadata.
    RemoveMetadata { file: PathBuf },
    /// Combine images into a PDF.
    FromImages { images: Vec<PathBuf> },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("failed to start runtime: {err}");
            return ExitCode::from(exit::USAGE as u8);
        }
    };
    let outcome = runtime.block_on(dispatch(&cli));
    report(&cli, outcome)
}

/// Turns the parsed command into a job, runs it, and returns the result.
async fn dispatch(cli: &Cli) -> localconvert_core::Result<localconvert_core::JobResult> {
    let overwrite = parse_policy(&cli.overwrite)
        .map_err(|msg| ConversionError::new(ConversionErrorCode::InvalidInput, msg))?;

    match &cli.command {
        Command::Convert {
            input,
            to,
            quality,
            background,
            preset,
        } => {
            convert(
                cli,
                input,
                to,
                *quality,
                background.as_deref(),
                preset.as_deref(),
                overwrite,
            )
            .await
        }
        Command::Archive { action } => archive(cli, action, overwrite).await,
        Command::Pdf { action } => pdf(cli, action, overwrite).await,
        Command::Spreadsheet { input, to, columns } => {
            spreadsheet(cli, input, to, columns, overwrite).await
        }
    }
}

async fn convert(
    cli: &Cli,
    input: &std::path::Path,
    to: &str,
    quality: Option<u8>,
    background: Option<&str>,
    preset: Option<&str>,
    overwrite: localconvert_core::OverwritePolicy,
) -> localconvert_core::Result<localconvert_core::JobResult> {
    // Probe the input up front so a missing/unreadable file fails clearly here
    // rather than deep in the engine.
    let _ = run::detect_format(input)?;
    let default_dir = parent_or_cwd(input);
    let (output_dir, _name) = resolve_output(cli.output.as_deref(), &default_dir);

    // Route by what the target extension is: an image target uses the image
    // engine, a media target uses the media engine.
    if let Some(target) = image_target(to) {
        let options = serde_json::json!({
            "targetFormat": target,
            "quality": quality,
            "background": background.map(parse_color).transpose()?,
        });
        run::run(RunOptions {
            operation_id: "image.convert".into(),
            input_paths: vec![input.to_path_buf()],
            output_dir,
            overwrite,
            options,
            quiet: cli.quiet || cli.json,
        })
        .await
    } else if let Some(target) = media_target(to) {
        let options = serde_json::json!({
            "targetFormat": target,
            "preset": preset.unwrap_or("balanced"),
        });
        run::run(RunOptions {
            operation_id: "media.convert".into(),
            input_paths: vec![input.to_path_buf()],
            output_dir,
            overwrite,
            options,
            quiet: cli.quiet || cli.json,
        })
        .await
    } else {
        Err(ConversionError::new(
            ConversionErrorCode::UnsupportedFormat,
            format!("cannot convert to '{to}'"),
        )
        .with_message_key("error.unsupportedFormat"))
    }
}

async fn archive(
    cli: &Cli,
    action: &ArchiveAction,
    overwrite: localconvert_core::OverwritePolicy,
) -> localconvert_core::Result<localconvert_core::JobResult> {
    match action {
        ArchiveAction::Create { files, format } => {
            let (output_dir, name) = resolve_output(cli.output.as_deref(), &cwd());
            let options = serde_json::json!({
                "format": archive_format(format)?,
                "archiveName": name,
            });
            run::run(RunOptions {
                operation_id: "archive.create".into(),
                input_paths: files.clone(),
                output_dir,
                overwrite,
                options,
                quiet: cli.quiet || cli.json,
            })
            .await
        }
        ArchiveAction::Extract { archive } => {
            let (output_dir, _) = resolve_output(cli.output.as_deref(), &parent_or_cwd(archive));
            run::run(RunOptions {
                operation_id: "archive.extract".into(),
                input_paths: vec![archive.clone()],
                output_dir,
                overwrite,
                options: serde_json::Value::Null,
                quiet: cli.quiet || cli.json,
            })
            .await
        }
    }
}

async fn pdf(
    cli: &Cli,
    action: &PdfAction,
    overwrite: localconvert_core::OverwritePolicy,
) -> localconvert_core::Result<localconvert_core::JobResult> {
    let (output_dir, name) = resolve_output(cli.output.as_deref(), &cwd());
    let quiet = cli.quiet || cli.json;
    match action {
        PdfAction::Merge { files } => {
            run::run(RunOptions {
                operation_id: "pdf.merge".into(),
                input_paths: files.clone(),
                output_dir,
                overwrite,
                options: serde_json::json!({ "outputName": name }),
                quiet,
            })
            .await
        }
        PdfAction::Split { file } => {
            run::run(RunOptions {
                operation_id: "pdf.split".into(),
                input_paths: vec![file.clone()],
                output_dir: cli.output.clone().unwrap_or_else(|| parent_or_cwd(file)),
                overwrite,
                options: serde_json::Value::Null,
                quiet,
            })
            .await
        }
        PdfAction::Pages {
            file,
            pages,
            rotate,
        } => run::run(RunOptions {
            operation_id: "pdf.pages".into(),
            input_paths: vec![file.clone()],
            output_dir,
            overwrite,
            options: serde_json::json!({ "pages": pages, "rotate": rotate, "outputName": name }),
            quiet,
        })
        .await,
        PdfAction::RemoveMetadata { file } => {
            run::run(RunOptions {
                operation_id: "pdf.removeMetadata".into(),
                input_paths: vec![file.clone()],
                output_dir,
                overwrite,
                options: serde_json::Value::Null,
                quiet,
            })
            .await
        }
        PdfAction::FromImages { images } => {
            run::run(RunOptions {
                operation_id: "pdf.fromImages".into(),
                input_paths: images.clone(),
                output_dir,
                overwrite,
                options: serde_json::json!({ "outputName": name }),
                quiet,
            })
            .await
        }
    }
}

async fn spreadsheet(
    cli: &Cli,
    input: &std::path::Path,
    to: &str,
    columns: &[String],
    overwrite: localconvert_core::OverwritePolicy,
) -> localconvert_core::Result<localconvert_core::JobResult> {
    let target = tabular_format(to)?;
    let (output_dir, _) = resolve_output(cli.output.as_deref(), &parent_or_cwd(input));

    // --column id:text --column age:number
    let mut column_types = serde_json::Map::new();
    for spec in columns {
        let Some((name, kind)) = spec.split_once(':') else {
            return Err(ConversionError::new(
                ConversionErrorCode::InvalidInput,
                format!("column spec '{spec}' must be name:type"),
            ));
        };
        column_types.insert(
            name.to_owned(),
            serde_json::Value::String(kind.to_ascii_lowercase()),
        );
    }

    run::run(RunOptions {
        operation_id: "spreadsheet.convert".into(),
        input_paths: vec![input.to_path_buf()],
        output_dir,
        overwrite,
        options: serde_json::json!({
            "targetFormat": target,
            "columnTypes": column_types,
        }),
        quiet: cli.quiet || cli.json,
    })
    .await
}

// --- output / reporting -----------------------------------------------------

fn report(cli: &Cli, outcome: localconvert_core::Result<localconvert_core::JobResult>) -> ExitCode {
    match outcome {
        Ok(result) => {
            if cli.json {
                println!("{}", run::result_json(&result));
            } else if !cli.quiet {
                if result.outputs.is_empty() {
                    eprintln!("nothing was written (a file with that name already exists)");
                }
                for output in &result.outputs {
                    println!("{}", output.path);
                }
                for warning in &result.warnings {
                    eprintln!("note: {}", text::warning(&warning.message_key));
                }
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            if cli.json {
                println!("{}", run::error_json(&err));
            } else {
                // Lead with the authored English detail; the machine-readable
                // code and key follow, for scripting and bug reports. Leading
                // with the key printed a raw identifier at the user.
                if err.detail.is_empty() {
                    eprintln!("error: {:?}", err.code);
                } else {
                    eprintln!("error: {}", err.detail);
                    eprintln!("  ({:?} / {})", err.code, err.message_key);
                }
                eprintln!(
                    "  your originals were {}.",
                    if err.source_safe {
                        "not changed"
                    } else {
                        "possibly affected — check them"
                    }
                );
            }
            ExitCode::from(exit::code_for(err.code) as u8)
        }
    }
}

// --- format mapping ---------------------------------------------------------

/// Parses white|black|#rrggbb into the engine's background object.
fn parse_color(value: &str) -> localconvert_core::Result<serde_json::Value> {
    let (r, g, b) = match value.to_ascii_lowercase().as_str() {
        "white" => (255u8, 255u8, 255u8),
        "black" => (0, 0, 0),
        hex if hex.starts_with('#') && hex.len() == 7 => {
            let parse =
                |i: usize| -> Option<u8> { u8::from_str_radix(hex.get(i..i + 2)?, 16).ok() };
            match (parse(1), parse(3), parse(5)) {
                (Some(r), Some(g), Some(b)) => (r, g, b),
                _ => {
                    return Err(ConversionError::new(
                        ConversionErrorCode::InvalidInput,
                        format!("invalid colour '{value}'"),
                    ))
                }
            }
        }
        other => {
            return Err(ConversionError::new(
                ConversionErrorCode::InvalidInput,
                format!("invalid colour '{other}' (white|black|#rrggbb)"),
            ))
        }
    };
    Ok(serde_json::json!({ "r": r, "g": g, "b": b }))
}

fn image_target(to: &str) -> Option<ImageOutputFormat> {
    Some(match to.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => ImageOutputFormat::Jpeg,
        "png" => ImageOutputFormat::Png,
        "webp" => ImageOutputFormat::WebP,
        "tiff" | "tif" => ImageOutputFormat::Tiff,
        "bmp" => ImageOutputFormat::Bmp,
        _ => return None,
    })
}

fn media_target(to: &str) -> Option<MediaFormat> {
    Some(match to.to_ascii_lowercase().as_str() {
        "mp4" => MediaFormat::Mp4,
        "webm" => MediaFormat::WebM,
        "mkv" => MediaFormat::Mkv,
        "gif" => MediaFormat::Gif,
        "mp3" => MediaFormat::Mp3,
        "wav" => MediaFormat::Wav,
        "flac" => MediaFormat::Flac,
        "ogg" => MediaFormat::Ogg,
        "m4a" => MediaFormat::M4a,
        _ => return None,
    })
}

fn tabular_format(to: &str) -> localconvert_core::Result<TabularFormat> {
    Ok(match to.to_ascii_lowercase().as_str() {
        "csv" => TabularFormat::Csv,
        "tsv" => TabularFormat::Tsv,
        "xlsx" => TabularFormat::Xlsx,
        "json" => TabularFormat::Json,
        other => {
            return Err(ConversionError::new(
                ConversionErrorCode::InvalidInput,
                format!("unknown spreadsheet format '{other}'"),
            ))
        }
    })
}

fn archive_format(format: &str) -> localconvert_core::Result<&'static str> {
    Ok(match format.to_ascii_lowercase().as_str() {
        "zip" => "zip",
        "tar" => "tar",
        "targz" | "tar.gz" | "tgz" => "tarGz",
        other => {
            return Err(ConversionError::new(
                ConversionErrorCode::InvalidInput,
                format!("unknown archive format '{other}'"),
            ))
        }
    })
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn parent_or_cwd(path: &std::path::Path) -> PathBuf {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(cwd, std::path::Path::to_path_buf)
}
