# Roadmap

Phases ship in order. Each one is a separate work item with its own acceptance
criteria; nothing is marked done until its tests pass and
[CONVERSION_MATRIX.md](CONVERSION_MATRIX.md) reflects reality.

The ordering is deliberate: foundation, then safe file handling, then one
conversion engine done properly, then breadth. Not images, PDF, spreadsheets and
FFmpeg simultaneously.

---

## ✅ Phase 0 — Foundation

**Delivered.**

- Cargo + pnpm workspaces, Tauri 2 shell, React UI, typed IPC contracts
- Job state machine, safe path handling, temp workspace manager with startup
  recovery, output-validation framework
- Structured local logging, CI matrix for four platforms
- Pipeline self-test proving the machine end to end

**Acceptance:** application builds; a job runs and reports real progress; no
generic shell or filesystem access is exposed. Verified — see
[TESTING.md](TESTING.md).

---

## 🟡 Phase 1 — Image engine (mostly delivered)

**Delivered:** magic-byte format detection (B3); JPG/PNG/WebP/TIFF/BMP/GIF
decode and JPG/PNG/WebP/TIFF/BMP encode; resize; JPEG quality; EXIF orientation
baked into pixels; explicit transparency handling; animation and metadata
warnings; batch conversion; independent output validation; a decode-bomb guard.

**Remaining:**

| # | Work | Blocked on |
|---|---|---|
| C4/C5 | HEIC, AVIF | Bundling libheif and an AV1 decoder — a packaging and licensing decision |
| — | Lossy WebP output | Bundling libwebp |
| C7 | Crop, rotate, flip | — |
| C8 | Metadata *preservation* (removal already works and is reported) | — |
| C9 | ICC profile handling | — |
| C10 | PNG lossless re-optimisation | — |
| C12 | Target file-size mode | — |
| C15 | Perceptual regression thresholds | — |
| B7 | Disk-space preflight | — |
| — | Playwright E2E; network-isolation assertion | — |

Original issue list, for reference:

| # | Work |
|---|---|
| B3 | Magic-byte format detection (container inspection → parser probe → MIME hint → extension as weak fallback) |
| B4 | File metadata reader |
| B7 | Disk-space preflight |
| C1–C2 | Image plugin manifest; JPG and PNG decode/encode |
| C3–C6 | WebP; HEIC/HEIF; AVIF; TIFF and BMP |
| C7 | Resize |
| C8 | Metadata policy (strip / preserve / selective) |
| C9 | ICC profile handling, EXIF orientation baked into pixels |
| C10–C11 | Lossless PNG optimisation; JPG/WebP/AVIF compression |
| C12 | Target-size mode (bounded binary search, hard iteration cap) |
| C13 | Batch queue |
| C14–C15 | Independent output validation; perceptual regression tests |

Also lands here: the fixture corpus (valid/large/corrupt/wrong-extension/
metadata-heavy/Unicode-name/spaces-in-name), the Playwright E2E suite, and the
network-isolation assertion.

**Acceptance:** every stable route passes on all four CI platforms ⏳ (macOS ARM
only so far); lossless routes prove pixel equality ✅; a `.jpg` containing PNG
magic bytes is detected as PNG and the user is warned ✅; lossy routes meet
per-route perceptual thresholds ⏳; ICC fixtures pass ⏳.

**v0.1.0 ships when the CI matrix is green on all four platforms.**

---

## ✅ Phase 2 — Archives (delivered)

ZIP, TAR, TAR.GZ create and extract, pure Rust. Traversal, symlink, absolute-path
and decompression-bomb (entry-count / total-size / ratio) protection; created
archives are reopened and enumerated; extraction stages the whole tree and
validates containment before anything reaches the destination.

**Acceptance:** ✅ traversal, absolute-path and symlink fixtures cannot escape;
✅ created archives reopen and match; ✅ round-trip preserves bytes and Unicode
names. Cross-platform CI ⏳ (macOS ARM only so far).

Not done: password-protected ZIP (needs a vetted crypto path). **Ships in v0.2.0
with the CI matrix.**

---

## 🟡 Phase 3 — PDF (structural half delivered)

Delivered, pure Rust (`lopdf`): merge; extract/reorder/rotate pages via a
validated page-range parser (`1-3,7,10-12`); split into per-page PDFs; strip
metadata; images→PDF; and a signed-document warning before any modifying op.

**Blocked on a rasteriser** (PDFium/MuPDF — a bundling decision): PDF→images,
and image recompression/downsampling inside a PDF. These are declined by name,
like HEIC. Render-based validation arrives with the same rasteriser; today
validation is structural (reopen + page count).

**Acceptance:** ✅ page counts match after every op; ✅ signed-document warnings
fire; ✅ images→PDF confirmed to render in an independent viewer (macOS Quick
Look). Cross-platform CI ⏳. Not done: password ops, structural optimize.
**Ships in v0.3.0.**

---

## ✅ Phase 4 — Spreadsheets (delivered)

CSV/TSV/XLSX/JSON in every direction, pure Rust. Encoding detection (UTF-8/BOM,
UTF-16 LE/BE), delimiter detection with manual override, header-row control,
per-column typing (Number/Boolean/Date/Automatic), and — the whole point —
values preserved exactly as text by default so `007` and 16-digit identifiers
are never mangled. Validation re-parses the output and compares row/column
counts, cell values and a Unicode sample against the source.

**Acceptance:** ✅ leading-zero and long-number fixtures round-trip unchanged;
✅ Unicode preserved; ✅ coercion is refused (kept as text + warning) when it
would lose data. Cross-platform CI ⏳.

Not done: a desktop per-column type editor (typing works via options/CLI today);
formula/style/chart preservation (warned as lost). **Ships in v0.4.0.**

---

## 🟡 Phase 5 — Media (delivered against system FFmpeg)

Video and audio conversion, compression presets (High/Balanced/Small), trim,
audio extraction, remove-audio, and MP4→GIF — driven by a **system-installed**
FFmpeg, detected on PATH. Every job probes with ffprobe first and validates the
output's streams and duration after. Real progress is parsed from ffmpeg's
`-progress` stream; cancellation kills the child and removes partial output. The
child-process security rules from spec §5.4 (arg arrays, minimal env, timeout,
captured+redacted stderr) are all enforced.

**Still open — the bundling decision:** LocalConvert does not yet ship FFmpeg.
That is the vendor-manifest + checksum-verification + LGPL-vs-GPL work, and it is
the difference between "works if the user has FFmpeg" (today, Beta) and "works
out of the box" (Stable). See [PACKAGING.md](PACKAGING.md).

**Acceptance:** ✅ outputs probe; ✅ duration and stream checks pass; ✅ child
terminates on cancel; ✅ progress is real. Bundling + cross-platform CI ⏳.
**v0.5.0 ships when FFmpeg is bundled and verified.**

---

## ✅ Phase 6 — CLI (delivered)

`localconvert` binary in `crates/localconvert-cli`, sharing the core crate and
running the identical execute→validate→commit pipeline. Subcommands for convert
(image + media), archive, pdf and spreadsheet; stable documented exit codes;
`--json` machine-readable output; `--overwrite`/`--quiet`/`--output`. See
[CLI.md](CLI.md).

**Acceptance:** ✅ exit codes stable and tested; ✅ `--json` round-trips; ✅
end-to-end tests drive the built binary against real files. Not done: shell
completion. **Ships in v0.6.0.**

---

## Phase 7 — Packaging and release

Signed Windows packages, signed and notarized macOS packages, AppImage and
`.deb`, SBOM, checksums, release notes.

---

## Phase 8 — Plugin SDK

Only after the core formats are stable. Public plugin trait, template,
compatibility checker, test kit. Third-party binary plugins are **not**
dynamically loadable until a signing and permission model exists.

---

## v1.0.0

Released only after: a stable conversion matrix, green cross-platform CI, a
security review, fixture coverage, successful package smoke tests, no known
data-corruption defects, and complete documentation.
