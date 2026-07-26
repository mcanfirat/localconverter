# Testing

## Run everything

```bash
pnpm check
```

That is exactly what CI runs: TypeScript typecheck, ESLint, Vitest, `cargo fmt
--check`, `cargo clippy -D warnings`, `cargo test --workspace`.

## What exists today

| Suite | Count | Where |
|---|---:|---|
| Rust unit | 213 | `crates/localconvert-core/src/**` |
| Rust IPC/state unit | 11 | `apps/desktop/src-tauri/src/**` |
| Rust pipeline integration | 11 | `apps/desktop/src-tauri/tests/pipeline.rs` |
| Frontend unit + component | 26 | `apps/desktop/src/**/*.test.ts(x)` |
| CLI unit + end-to-end | 13 | `crates/localconvert-cli/{src,tests}` |

## The tests that matter most

These are the ones guarding a stated guarantee. If you change the code they
cover, the test failing is the point.

**State machine** (`job.rs`) — the exhaustive transition table. Two tests worth
reading before touching it:

- `completion_requires_passing_through_validation` iterates every status pair and
  asserts `Completed` is reachable only from `Validating`.
- `terminal_states_accept_nothing` iterates every pair from a terminal status.

**Path safety** (`paths.rs`) — NUL rejection, `..` escape, symlink escape,
absolute-path rejection, Unicode and spaces preserved, the `photo (1).jpg`
naming rule, and `commit_output` refusing to clobber.

**Temp cleanup** (`workspace.rs`) — `cleanup_never_follows_a_symlink_out_of_the_temp_root`
plants a symlink named as a valid UUID pointing at a directory of "user data" and
asserts cleanup leaves it alone. `remove_fenced_refuses_targets_outside_the_fence`
covers the direct case.

**Pipeline** (`tests/pipeline.rs`) — drives real jobs through the real registry
and scheduler against a mock Tauri runtime: end-to-end completion with monotonic
progress, cancellation mid-write leaving nothing behind, unknown operation
failing without touching the destination, three queued jobs completing one at a
time, and startup recovery of a crashed job's workspace.

**Format detection** (`detect.rs`) — every signature, plus the cases that
actually bite: RIFF sub-type separating WebP from WAV, ISO-BMFF compatible
brands separating AVIF from HEIC, tar's marker at offset 257, and truncated
headers of every length from 0 to 24 not panicking. The headline case,
`a_png_named_jpg_is_detected_as_png_and_flagged`, is the reason the module exists.

**CLI** (`crates/localconvert-cli`) — unit tests for exit-code mapping and
`--output` resolution, plus `tests/cli.rs` which runs the *built binary* against
real files: successful convert, missing-input exit code, `--json` shape, unknown
target rejection, and `--help`. Finding a real concurrency bug (per-invocation
`cleanup_stale` sweeping a concurrent run's workspace) is why these exist.

**Media engine** (`media.rs`) — 14 tests that run real FFmpeg conversions
(and skip cleanly when it is absent): WAV→MP3, extract-audio-from-video (FLAC),
MP4→GIF, trim-shortens-output, `extracting_audio_from_a_video_without_audio_is_refused`,
`the_source_media_is_never_modified`, plus option validation and format checks.

**Regressions from the full audit.** A 56-agent audit across all five engines
found 50 confirmed defects; the tests below lock the serious ones shut:
`merge_does_not_clobber_objects_that_came_from_the_inputs` (merge used to
allocate object ids over live merged content, destroying pages while reporting
success), `merge_keeps_page_attributes_that_were_inherited`,
`extracting_one_page_does_not_carry_the_whole_document`,
`cells_past_the_header_width_are_not_dropped`,
`a_headerless_source_converts_without_inventing_a_data_row`,
`a_lossless_target_does_not_warn_about_growing`, and — in the CLI —
`every_warning_key_has_prose`, which scans the engines so no raw
`warning.foo.bar` identifier can reach a terminal.

**PDF engine** (`pdf.rs`) — 13 tests: the page-range parser (`1-3,7,10-12`,
reordering via `3,1,2`, and rejection of `0`/`5-3`/`a-b`), merge page-count
concatenation, extract, split-into-pages, rotate, images→PDF, metadata stripping,
`a_signed_pdf_warns_before_modification`, and `the_source_pdf_is_never_modified`.

**Spreadsheet engine** (`spreadsheet.rs`) — 17 tests centred on value
preservation: `csv_to_xlsx_preserves_leading_zeros_by_default`,
`a_long_identifier_is_not_turned_into_scientific_notation`,
`a_number_column_with_an_identifier_keeps_it_as_text_and_warns`,
`safe_number_protects_the_dangerous_shapes`, plus encoding (`utf16_with_a_bom_is_decoded`),
delimiter detection, and round trips across all four formats.

**Archive engine** (`archive.rs`) — 14 tests, most of them adversarial:
`a_zip_with_a_traversal_entry_cannot_escape`, `a_tar_with_an_absolute_entry_is_refused`,
`a_tar_with_a_symlink_entry_is_refused`, `an_entry_count_bomb_is_refused`,
`a_high_ratio_entry_is_flagged_as_a_bomb`, plus ZIP/TAR/TAR.GZ round trips,
Unicode entry names, and `the_source_archive_is_never_modified`.

**Image engine** (`image_engine.rs`) — 32 tests. The ones worth reading:
`converting_transparency_to_jpeg_without_a_background_is_refused` (and that
nothing is written when it is), `a_fully_opaque_png_does_not_demand_a_background`
(no false alarms), `png_to_bmp_is_pixel_exact`, `lower_quality_produces_a_smaller_jpeg`,
`an_oversized_image_is_refused_before_it_is_decoded` (a 60000×60000 header claim),
`heic_and_avif_are_declined_explicitly`, and `the_source_file_is_never_modified`.

Fixtures are generated in-process rather than committed as binaries, so they are
readable, diffable and cannot silently rot.

**Contract drift** — `cargo test` regenerates `apps/desktop/src/bindings/`; CI
fails if that leaves the tree dirty. `messages.test.ts` scans every `.rs` file
for message keys and fails if one has no string in `messages.ts`.

**IPC surface** — `the_window_capability_grants_no_dangerous_permissions` reads
`capabilities/default.json` and fails on any `fs:`, `shell:`, `process:`,
`http:` or `opener:` grant. `the_ipc_surface_exposes_no_generic_filesystem_or_shell_command`
reads `lib.rs` and fails if a passthrough command is registered.

## Conventions

- Test names are sentences describing the guarantee, not the function.
- `clippy::unwrap_used` and friends are denied in production code and allowed
  inside `mod tests` — a panic in a test is a failure report.
- Anything touching the filesystem uses `tempfile::tempdir()`. No test writes
  outside its own temp directory.
- Every test that produces output asserts on the *input* too, so "source files
  are never modified" is checked rather than assumed.

## What is not here yet, and when it arrives

| Missing | Arrives with | Why not now |
|---|---|---|
| Perceptual / visual-fidelity comparison (SSIM thresholds) | Phase 1 remainder | Present coverage asserts dimensions, format, pixel-exactness on lossless routes, and that lower quality is smaller. A perceptual metric is what would catch a subtly wrong colour transform. |
| ICC profile fixtures (Display P3, CMYK) | Phase 1 remainder | Colour management is not implemented yet, so a fixture would only assert current behaviour. |
| Property-based tests (page ranges, CSV escaping, archive paths) | Phases 2–4 | The parsers they target do not exist. |
| Fuzzing (detectors, CSV, page ranges, archive paths) | Phases 2–4 | Same. |
| Playwright E2E (drag-drop, retry, offline, keyboard) | Phase 1 | Driving a UI whose only action is a diagnostic proves little; the pipeline integration tests cover the same paths headlessly and faster. |
| Network-isolation assertion | Phase 1 | Meaningful once an engine subprocess exists that *could* dial out. Today no HTTP client is in the dependency tree at all. |

Large-fixture runs move to a nightly workflow once fixtures exist.

## Known platform gap: the pipeline tests do not run on Windows

`apps/desktop/src-tauri/tests/pipeline.rs` is `#![cfg(not(windows))]`. On
Windows the test binary exits with `STATUS_ENTRYPOINT_NOT_FOUND` (0xc0000139)
before `main` runs — a DLL it imports resolves, but one of the exports it needs
is missing. Because the crash precedes `main`, `#[ignore]` cannot skip it; the
target itself has to be excluded.

It is the only target that enables Tauri's `test` feature, and both the crate's
unit tests and the `main.rs` harness start normally on Windows, which points at
something linked by `MockRuntime` rather than by the app. Two hypotheses have
been tested and disproved: the `cdylib`/`staticlib` crate types (removed — the
crash survived) and a misplaced `WebView2Loader.dll` (copied next to the binary
— the crash survived).

To pick this up, the import table is the place to start, and it has to be read
with the `cfg` temporarily removed — with the target excluded the binary links
nothing unusual, which is exactly why a permanent CI diagnostic was dropped
rather than left in place reporting an empty answer:

```powershell
# after deleting the #![cfg(not(windows))] line
cargo build -p localconvert-desktop --tests
dumpbin /DEPENDENTS target\debug\deps\pipeline-*.exe
```

The DLL named there, minus the ones the passing binaries in the same directory
also import, is the one to chase.

**What is still covered on Windows:** every `localconvert-core` test — all the
engines, validation, path fencing and archive safety — plus the desktop crate's
own unit tests over the registry and state machine. **What is not:** the job
layer end-to-end through a real Tauri app handle. macOS and Linux run it on
every push.
