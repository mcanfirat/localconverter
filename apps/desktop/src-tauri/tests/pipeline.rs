//! End-to-end pipeline tests against a real Tauri app handle.
//!
//! These exercise the layer the unit tests cannot reach: the registry, the
//! scheduler, event emission and the state machine driven by the same code the
//! window drives. Phase 0's acceptance criterion — "a job runs and reports
//! progress" — is asserted here rather than claimed.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::{Duration, Instant};

use localconvert_core::{ConversionJob, JobStatus, OverwritePolicy, SELFTEST_OPERATION_ID};
use localconvert_desktop_lib::state::{AppState, JOB_UPDATED_EVENT};
use tauri::test::{mock_app, MockRuntime};
use tauri::{App, Listener};
use uuid::Uuid;

struct Harness {
    app: App<MockRuntime>,
    state: AppState,
    _temp: tempfile::TempDir,
    out_dir: std::path::PathBuf,
}

fn harness() -> Harness {
    let temp = tempfile::tempdir().unwrap();
    let out_dir = temp.path().join("out");
    std::fs::create_dir_all(&out_dir).unwrap();

    Harness {
        app: mock_app(),
        state: AppState::new(temp.path().join("apptemp")),
        _temp: temp,
        out_dir,
    }
}

impl Harness {
    fn job(&self, size_bytes: u64, policy: OverwritePolicy) -> ConversionJob {
        ConversionJob::new(
            SELFTEST_OPERATION_ID,
            Vec::new(),
            self.out_dir.to_string_lossy(),
            policy,
            serde_json::json!({ "sizeBytes": size_bytes }),
        )
    }

    /// Blocks until the job reaches a terminal status. The scheduler runs on
    /// Tauri's own async runtime, so this polls rather than awaiting.
    fn wait_for_terminal(&self, id: Uuid) -> ConversionJob {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(job) = self.state.job(id) {
                if job.status.is_terminal() {
                    return job;
                }
            }
            assert!(Instant::now() < deadline, "job {id} never finished");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn a_job_runs_end_to_end_and_reports_progress() {
    let h = harness();
    let handle = h.app.handle();

    let updates = std::sync::Arc::new(std::sync::Mutex::new(Vec::<ConversionJob>::new()));
    let sink = std::sync::Arc::clone(&updates);
    handle.listen(JOB_UPDATED_EVENT, move |event| {
        if let Ok(job) = serde_json::from_str::<ConversionJob>(event.payload()) {
            if let Ok(mut guard) = sink.lock() {
                guard.push(job);
            }
        }
    });

    let queued = h.state.spawn(handle, h.job(400_000, OverwritePolicy::Fail));
    assert_eq!(queued.status, JobStatus::Queued);

    let finished = h.wait_for_terminal(queued.id);
    assert_eq!(
        finished.status,
        JobStatus::Completed,
        "error: {:?}",
        finished.error
    );

    // The output landed, is the right size, and passed every check.
    let output = h.out_dir.join("localconvert-selftest.bin");
    assert_eq!(std::fs::metadata(&output).unwrap().len(), 400_000);

    let result = finished.result.expect("a completed job carries a result");
    assert_eq!(result.outputs.len(), 1);
    assert_eq!(result.output_total_bytes, 400_000);
    assert!(result.validation_reports.iter().all(|report| report.valid));

    // Progress was reported, was real, and never went backwards.
    let observed = updates.lock().unwrap();
    let statuses: Vec<JobStatus> = observed.iter().map(|job| job.status).collect();
    assert!(statuses.contains(&JobStatus::Preparing));
    assert!(statuses.contains(&JobStatus::Running));
    assert!(statuses.contains(&JobStatus::Validating));
    assert_eq!(statuses.last(), Some(&JobStatus::Completed));

    let percents: Vec<f32> = observed
        .iter()
        .filter_map(|job| job.progress.percent)
        .collect();
    assert!(
        percents.len() >= 5,
        "expected chunked progress, got {percents:?}"
    );
    assert!(percents.windows(2).all(|w| w[1] >= w[0]));
}

#[test]
fn the_temp_workspace_is_gone_once_the_job_finishes() {
    let h = harness();
    let queued = h
        .state
        .spawn(h.app.handle(), h.job(64_000, OverwritePolicy::Fail));
    h.wait_for_terminal(queued.id);

    let jobs_root = h.state.temp_root().join("jobs");
    let leftovers: Vec<_> = std::fs::read_dir(&jobs_root)
        .map(|entries| entries.filter_map(Result::ok).collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "temp workspace leaked: {leftovers:?}");
}

#[test]
fn cancelling_a_running_job_leaves_no_output_behind() {
    let h = harness();
    // Large enough that the cancel lands mid-write rather than after the fact.
    let queued = h.state.spawn(
        h.app.handle(),
        h.job(48 * 1024 * 1024, OverwritePolicy::Fail),
    );

    // Wait until it is actually writing, then cancel.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let job = h.state.job(queued.id).unwrap();
        if job.status == JobStatus::Running || job.status.is_terminal() {
            break;
        }
        assert!(Instant::now() < deadline, "job never started running");
        std::thread::sleep(Duration::from_millis(5));
    }
    h.state.cancel(queued.id).unwrap();

    let finished = h.wait_for_terminal(queued.id);
    assert_eq!(finished.status, JobStatus::Cancelled);
    assert!(!h.out_dir.join("localconvert-selftest.bin").exists());
    assert!(finished.error.map(|e| e.source_safe).unwrap_or(false));
}

#[test]
fn an_unknown_operation_fails_the_job_without_touching_the_destination() {
    let h = harness();
    let mut job = h.job(1024, OverwritePolicy::Fail);
    job.operation_id = "nonexistent.operation".to_owned();

    let queued = h.state.spawn(h.app.handle(), job);
    let finished = h.wait_for_terminal(queued.id);

    assert_eq!(finished.status, JobStatus::Failed);
    assert_eq!(
        finished.error.map(|e| e.message_key).unwrap_or_default(),
        "error.operation.unknown"
    );
    assert_eq!(std::fs::read_dir(&h.out_dir).unwrap().count(), 0);
}

#[test]
fn queued_jobs_run_one_at_a_time_and_all_complete() {
    let h = harness();
    let handle = h.app.handle();

    let ids: Vec<Uuid> = (0..3)
        .map(|_| {
            h.state
                .spawn(handle, h.job(32_000, OverwritePolicy::Rename))
                .id
        })
        .collect();

    for id in &ids {
        let finished = h.wait_for_terminal(*id);
        assert_eq!(
            finished.status,
            JobStatus::Completed,
            "{:?}",
            finished.error
        );
    }

    // Rename policy means three distinct files, none clobbered.
    let written: Vec<String> = std::fs::read_dir(&h.out_dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(written.len(), 3, "got {written:?}");
    for name in &written {
        assert_eq!(
            std::fs::metadata(h.out_dir.join(name)).unwrap().len(),
            32_000
        );
    }
}

/// The whole point of the product, driven through the same registry and
/// scheduler the window uses: real images in, real images out, verified.
#[test]
fn images_convert_end_to_end_through_the_job_layer() {
    let h = harness();
    let sources = h._temp.path().join("sources");
    std::fs::create_dir_all(&sources).unwrap();

    // Three real PNGs, one of them deliberately mislabelled as .jpg.
    let mut names = Vec::new();
    let mut originals: Vec<Vec<u8>> = Vec::new();
    for (name, w, h_px) in [
        ("alpine.png", 80u32, 60u32),
        ("ünïcode tëst 🎉.png", 40, 40),
        ("mislabelled.jpg", 32, 24),
    ] {
        let mut img = image::RgbImage::new(w, h_px);
        for (x, y, pixel) in img.enumerate_pixels_mut() {
            *pixel = image::Rgb([(x * 3) as u8, (y * 5) as u8, 90]);
        }
        let path = sources.join(name);
        img.save_with_format(&path, image::ImageFormat::Png)
            .unwrap();
        originals.push(std::fs::read(&path).unwrap());
        names.push(path);
    }

    let job = ConversionJob::new(
        "image.convert",
        names
            .iter()
            .map(|p| localconvert_core::FileDescriptor::probe(p).unwrap())
            .collect(),
        h.out_dir.to_string_lossy(),
        OverwritePolicy::Fail,
        serde_json::json!({ "targetFormat": "jpeg", "quality": 80 }),
    );

    let queued = h.state.spawn(h.app.handle(), job);
    let finished = h.wait_for_terminal(queued.id);

    assert!(
        matches!(
            finished.status,
            JobStatus::Completed | JobStatus::CompletedWithWarnings
        ),
        "job failed: {:?}",
        finished.error
    );

    let result = finished.result.expect("a finished job carries a result");
    assert_eq!(result.outputs.len(), 3);
    assert!(result.validation_reports.iter().all(|report| report.valid));

    // Each output is a genuine JPEG of the right size, per an independent parser.
    for (name, expected) in [
        ("alpine.jpg", (80usize, 60usize)),
        ("ünïcode tëst 🎉.jpg", (40, 40)),
        ("mislabelled.jpg", (32, 24)),
    ] {
        let out = h.out_dir.join(name);
        assert!(out.exists(), "{name} was not written");
        let size = imagesize::size(&out).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!((size.width, size.height), expected, "{name}");
        assert_eq!(
            localconvert_core::detect(&out).unwrap().format,
            localconvert_core::FileFormat::Jpeg,
            "{name} is not really a JPEG"
        );
    }

    // The mislabelled file was handled by its real format, and the user is told.
    assert!(result
        .warnings
        .iter()
        .any(|w| w.message_key == "warning.image.extensionMismatch"));

    // Every source is byte-for-byte untouched. Compared as raw bytes rather
    // than decoded: `image::open` trusts the extension, which is exactly the
    // assumption this project refuses to make.
    for (path, before) in names.iter().zip(originals.iter()) {
        assert_eq!(
            &std::fs::read(path).unwrap(),
            before,
            "{path:?} was modified"
        );
    }

    assert!(result.output_total_bytes > 0);
    assert!(result.input_total_bytes > 0);
}

/// Archives run through the same registry/scheduler as everything else: create
/// a ZIP, then extract it, and confirm the bytes survive the round trip.
#[test]
fn archives_create_and_extract_through_the_job_layer() {
    let h = harness();
    let sources = h._temp.path().join("arc-src");
    std::fs::create_dir_all(&sources).unwrap();
    let a = sources.join("notes.txt");
    let b = sources.join("data.bin");
    std::fs::write(&a, b"the quick brown fox").unwrap();
    std::fs::write(&b, vec![7u8; 5000]).unwrap();

    let create = ConversionJob::new(
        "archive.create",
        vec![
            localconvert_core::FileDescriptor::probe(&a).unwrap(),
            localconvert_core::FileDescriptor::probe(&b).unwrap(),
        ],
        h.out_dir.to_string_lossy(),
        OverwritePolicy::Overwrite,
        serde_json::json!({ "format": "zip", "archiveName": "bundle" }),
    );
    let created = h.wait_for_terminal(h.state.spawn(h.app.handle(), create).id);
    assert!(
        matches!(
            created.status,
            JobStatus::Completed | JobStatus::CompletedWithWarnings
        ),
        "create failed: {:?}",
        created.error
    );
    let archive = h.out_dir.join("bundle.zip");
    assert!(archive.exists());

    // Extract into a separate destination.
    let h2 = harness();
    let extract = ConversionJob::new(
        "archive.extract",
        vec![localconvert_core::FileDescriptor::probe(&archive).unwrap()],
        h2.out_dir.to_string_lossy(),
        OverwritePolicy::Fail,
        serde_json::Value::Null,
    );
    let extracted = h2.wait_for_terminal(h2.state.spawn(h2.app.handle(), extract).id);
    assert_eq!(
        extracted.status,
        JobStatus::Completed,
        "{:?}",
        extracted.error
    );

    assert_eq!(
        std::fs::read(h2.out_dir.join("bundle/notes.txt")).unwrap(),
        b"the quick brown fox"
    );
    assert_eq!(
        std::fs::read(h2.out_dir.join("bundle/data.bin")).unwrap(),
        vec![7u8; 5000]
    );
}

/// A spreadsheet conversion through the job layer, checking the headline
/// value-preservation guarantee end to end: 007 survives CSV -> XLSX -> CSV.
#[test]
fn spreadsheets_preserve_values_through_the_job_layer() {
    let h = harness();
    let sources = h._temp.path().join("sheet-src");
    std::fs::create_dir_all(&sources).unwrap();
    let csv = sources.join("accounts.csv");
    std::fs::write(&csv, "id,name\n007,Ada\n0042,Bo\n").unwrap();

    let to_xlsx = ConversionJob::new(
        "spreadsheet.convert",
        vec![localconvert_core::FileDescriptor::probe(&csv).unwrap()],
        h.out_dir.to_string_lossy(),
        OverwritePolicy::Overwrite,
        serde_json::json!({ "targetFormat": "xlsx" }),
    );
    let made = h.wait_for_terminal(h.state.spawn(h.app.handle(), to_xlsx).id);
    assert!(
        matches!(
            made.status,
            JobStatus::Completed | JobStatus::CompletedWithWarnings
        ),
        "{:?}",
        made.error
    );
    let xlsx = h.out_dir.join("accounts.xlsx");
    assert!(xlsx.exists());
    assert_eq!(
        localconvert_core::detect(&xlsx).unwrap().format,
        localconvert_core::FileFormat::Xlsx
    );

    let h2 = harness();
    let back = ConversionJob::new(
        "spreadsheet.convert",
        vec![localconvert_core::FileDescriptor::probe(&xlsx).unwrap()],
        h2.out_dir.to_string_lossy(),
        OverwritePolicy::Fail,
        serde_json::json!({ "targetFormat": "csv" }),
    );
    let round = h2.wait_for_terminal(h2.state.spawn(h2.app.handle(), back).id);
    assert!(
        matches!(
            round.status,
            JobStatus::Completed | JobStatus::CompletedWithWarnings
        ),
        "{:?}",
        round.error
    );
    let text = std::fs::read_to_string(h2.out_dir.join("accounts.csv")).unwrap();
    assert!(
        text.contains("007"),
        "leading zero lost across the round trip: {text}"
    );
    assert!(text.contains("0042"), "leading zeros lost: {text}");
}

/// Images -> PDF -> split, through the job layer: proves the PDF engine builds
/// a real multi-page document and can take one apart again.
#[test]
fn pdf_build_and_split_through_the_job_layer() {
    let h = harness();
    let src = h._temp.path().join("pdf-src");
    std::fs::create_dir_all(&src).unwrap();

    // Two real images.
    let mut made = Vec::new();
    for (name, w, hgt) in [("a.png", 120u32, 90u32), ("b.png", 80, 110)] {
        let img = image::RgbImage::from_pixel(w, hgt, image::Rgb([90, 120, 150]));
        let path = src.join(name);
        img.save_with_format(&path, image::ImageFormat::Png)
            .unwrap();
        made.push(path);
    }

    let build = ConversionJob::new(
        "pdf.fromImages",
        made.iter()
            .map(|p| localconvert_core::FileDescriptor::probe(p).unwrap())
            .collect(),
        h.out_dir.to_string_lossy(),
        OverwritePolicy::Overwrite,
        serde_json::json!({ "outputName": "album" }),
    );
    let built = h.wait_for_terminal(h.state.spawn(h.app.handle(), build).id);
    assert_eq!(built.status, JobStatus::Completed, "{:?}", built.error);
    let pdf = h.out_dir.join("album.pdf");
    assert!(pdf.exists());
    assert_eq!(
        localconvert_core::detect(&pdf).unwrap().format,
        localconvert_core::FileFormat::Pdf
    );

    // Split it back into pages.
    let h2 = harness();
    let split = ConversionJob::new(
        "pdf.split",
        vec![localconvert_core::FileDescriptor::probe(&pdf).unwrap()],
        h2.out_dir.to_string_lossy(),
        OverwritePolicy::Fail,
        serde_json::Value::Null,
    );
    let done = h2.wait_for_terminal(h2.state.spawn(h2.app.handle(), split).id);
    assert_eq!(done.status, JobStatus::Completed, "{:?}", done.error);
    assert_eq!(
        done.result.unwrap().outputs.len(),
        2,
        "expected one PDF per page"
    );
    assert!(h2.out_dir.join("album/page-001.pdf").exists());
    assert!(h2.out_dir.join("album/page-002.pdf").exists());
}

/// Audio extraction through the job layer with a real (system) FFmpeg. Skips
/// cleanly when FFmpeg is not installed, so CI without it still passes.
#[test]
fn media_converts_through_the_job_layer_when_ffmpeg_is_present() {
    if !localconvert_core::media::is_available() {
        eprintln!("skipping media pipeline test: ffmpeg not installed");
        return;
    }
    let h = harness();
    let src = h._temp.path().join("media-src");
    std::fs::create_dir_all(&src).unwrap();
    let wav = src.join("tone.wav");

    // Synthesize a 1s tone with the system ffmpeg.
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
        ])
        .arg(&wav)
        .status()
        .unwrap();
    assert!(status.success());

    let job = ConversionJob::new(
        "media.convert",
        vec![localconvert_core::FileDescriptor::probe(&wav).unwrap()],
        h.out_dir.to_string_lossy(),
        OverwritePolicy::Overwrite,
        serde_json::json!({ "targetFormat": "mp3", "preset": "balanced" }),
    );
    let done = h.wait_for_terminal(h.state.spawn(h.app.handle(), job).id);
    assert!(
        matches!(
            done.status,
            JobStatus::Completed | JobStatus::CompletedWithWarnings
        ),
        "{:?}",
        done.error
    );
    let out = h.out_dir.join("tone.mp3");
    assert!(out.exists());
    assert_eq!(
        localconvert_core::detect(&out).unwrap().format,
        localconvert_core::FileFormat::Mp3
    );
}

#[test]
fn startup_cleanup_removes_a_crashed_jobs_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let temp_root = temp.path().join("apptemp");
    let jobs_root = temp_root.join("jobs");

    // Simulate what a crash leaves behind.
    let orphan = Uuid::new_v4();
    std::fs::create_dir_all(jobs_root.join(orphan.to_string())).unwrap();
    std::fs::write(
        jobs_root.join(orphan.to_string()).join("out.bin.partial"),
        b"half a conversion",
    )
    .unwrap();

    let state = AppState::new(temp_root.clone());
    let removed = localconvert_core::cleanup_stale(&temp_root, &state.active_job_ids()).unwrap();

    assert_eq!(removed, 1);
    assert!(!jobs_root.join(orphan.to_string()).exists());
}
