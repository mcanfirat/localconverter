//! End-to-end CLI tests: run the built binary against real files and assert on
//! its exit code and output, the way a user or a script would.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// Path to the compiled `localconvert` binary for this test run.
fn binary() -> PathBuf {
    // cargo sets CARGO_BIN_EXE_<name> for integration tests.
    PathBuf::from(env!("CARGO_BIN_EXE_localconvert"))
}

fn workdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// Writes a small real (opaque) PNG for image tests.
fn write_png(path: &Path) {
    image::RgbImage::from_pixel(4, 4, image::Rgb([120, 160, 200]))
        .save_with_format(path, image::ImageFormat::Png)
        .unwrap();
}

#[test]
fn convert_png_to_png_succeeds_and_prints_the_output_path() {
    let dir = workdir();
    let src = dir.path().join("pic.png");
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    write_png(&src);

    let output = Command::new(binary())
        .args(["convert"])
        .arg(&src)
        .args(["--to", "png", "-o"])
        .arg(&out)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("pic.png"), "stdout was: {stdout}");
    assert!(out.join("pic.png").exists());
}

#[test]
fn a_missing_input_exits_with_the_input_code() {
    let dir = workdir();
    let status = Command::new(binary())
        .args(["--quiet", "convert"])
        .arg(dir.path().join("nope.png"))
        .args(["--to", "png"])
        .status()
        .unwrap();
    // exit::INPUT
    assert_eq!(status.code(), Some(2));
}

#[test]
fn json_output_is_machine_readable() {
    let dir = workdir();
    let csv = dir.path().join("data.csv");
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(&csv, "id,name\n007,Ada\n").unwrap();

    let output = Command::new(binary())
        .args(["--json", "spreadsheet"])
        .arg(&csv)
        .args(["--to", "json", "-o"])
        .arg(&out)
        .output()
        .unwrap();

    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], true);
    assert!(value["outputs"][0]["path"]
        .as_str()
        .unwrap()
        .ends_with("data.json"));

    // The headline guarantee, end to end through the CLI: 007 survived.
    let written = std::fs::read_to_string(out.join("data.json")).unwrap();
    assert!(written.contains("007"), "leading zero lost: {written}");
}

#[test]
fn an_unknown_target_format_is_rejected() {
    let dir = workdir();
    let src = dir.path().join("pic.png");
    write_png(&src);
    let output = Command::new(binary())
        .args(["--json", "convert"])
        .arg(&src)
        .args(["--to", "xyz"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], false);
}

#[test]
fn the_help_text_lists_the_subcommands() {
    let output = Command::new(binary()).arg("--help").output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    for sub in ["convert", "archive", "pdf", "spreadsheet"] {
        assert!(help.contains(sub), "help missing {sub}");
    }
}
