# Conversion matrix

This table reflects **what has actually been tested**, not what is planned. A
route appears here only once it passes its fixture suite; until then it lives in
[ROADMAP.md](ROADMAP.md).

Status meanings:

| Status | Means |
|---|---|
| **Stable** | Passes its full fixture suite on every supported platform in CI. Shown in the main tool sections of the UI. |
| **Beta** | Passes on at least one platform; edge cases known to be incomplete. Shown, but labelled. |
| **Experimental** | Behind a setting. Not shown by default. |
| **Planned** | Not implemented. Not shown at all. |

`list_operations()` returns exactly the Stable and Beta rows below — the UI
cannot advertise something this table does not claim.

## Diagnostics

| Operation | Windows | macOS ARM | macOS Intel | Linux | Validation | Status |
|---|:---:|:---:|:---:|:---:|---|---|
| Pipeline self-test | ⏳ | ✅ | ⏳ | ⏳ | Byte-exact read-back + size + destination checks | Stable |

## Images

Readable: JPG, PNG, WebP, TIFF, BMP, GIF. Writable: JPG, PNG, WebP, TIFF, BMP.
Every route is validated by magic bytes, an independent header parser
(`imagesize`), a full re-decode, and a dimension check.

| Operation | Windows | macOS ARM | macOS Intel | Linux | Validation | Status |
|---|:---:|:---:|:---:|:---:|---|---|
| JPG → PNG / WebP / TIFF / BMP | ⏳ | ✅ | ⏳ | ⏳ | Magic + independent parse + decode + dimensions | Stable |
| PNG → JPG / WebP / TIFF / BMP | ⏳ | ✅ | ⏳ | ⏳ | As above; PNG→BMP proven pixel-exact | Stable |
| WebP → JPG / PNG / TIFF / BMP | ⏳ | ✅ | ⏳ | ⏳ | As above | Stable |
| TIFF → JPG / PNG / WebP / BMP | ⏳ | ✅ | ⏳ | ⏳ | As above | Stable |
| BMP → JPG / PNG / WebP / TIFF | ⏳ | ✅ | ⏳ | ⏳ | As above | Stable |
| GIF → JPG / PNG / WebP / TIFF / BMP | ⏳ | ✅ | ⏳ | ⏳ | As above; first frame only, warned | Stable |
| Resize (fit / exact) | ⏳ | ✅ | ⏳ | ⏳ | Output dimensions asserted | Stable |
| JPEG quality control | ⏳ | ✅ | ⏳ | ⏳ | Lower quality proven smaller | Stable |
| Transparency flattening | ⏳ | ✅ | ⏳ | ⏳ | Refused without an explicit background | Stable |
| EXIF orientation baked into pixels | ⏳ | ✅ | ⏳ | ⏳ | Axis swap asserted | Stable |
| Batch conversion | ⏳ | ✅ | ⏳ | ⏳ | One verified output per input | Stable |
| HEIC/HEIF → anything | — | — | — | — | Declined explicitly | Blocked — needs libheif |
| AVIF → anything | — | — | — | — | Declined explicitly | Blocked — needs a native AV1 decoder |
| Lossy WebP output | — | — | — | — | Not available | Blocked — needs libwebp |
| Crop / rotate / flip | — | — | — | — | Not implemented | Planned |
| PNG lossless re-optimise | — | — | — | — | Not implemented | Planned |
| Target-size compression | — | — | — | — | Not implemented | Planned |
| ICC colour-profile handling | — | — | — | — | Not implemented | Planned |
| Metadata preservation | — | — | — | — | Not implemented — metadata is stripped and the user is told | Planned |

### Why some rows say "Blocked"

HEIC, AVIF and lossy WebP each need a native C codec (libheif, an AV1 decoder,
libwebp). Bundling those is a packaging and licensing decision, not a coding
one — see [PACKAGING.md](PACKAGING.md). Until that decision is made the engine
**declines these formats by name** rather than producing something else. A
`.heic` file dropped into the app produces a clear message, never a broken JPEG.

## Archives

Pure Rust (`zip`, `tar`, `flate2`) — no bundled binaries.

| Operation | Windows | macOS ARM | macOS Intel | Linux | Validation | Status |
|---|:---:|:---:|:---:|:---:|---|---|
| Create ZIP | ⏳ | ✅ | ⏳ | ⏳ | Reopen + enumerate entries | Stable |
| Create TAR | ⏳ | ✅ | ⏳ | ⏳ | Reopen + enumerate entries | Stable |
| Create TAR.GZ | ⏳ | ✅ | ⏳ | ⏳ | Reopen + enumerate entries | Stable |
| Extract ZIP / TAR / TAR.GZ | ⏳ | ✅ | ⏳ | ⏳ | Containment + per-file existence | Stable |
| Traversal / symlink / absolute-path protection | ⏳ | ✅ | ⏳ | ⏳ | Malicious-entry fixtures rejected | Stable |
| Decompression-bomb protection | ⏳ | ✅ | ⏳ | ⏳ | Entry-count, total-size, ratio guards | Stable |
| Password-protected ZIP | — | — | — | — | Not implemented | Planned |

## PDF

Structural operations in pure Rust (`lopdf`). Anything needing a rasteriser is
declined by name rather than half-done.

| Operation | Windows | macOS ARM | macOS Intel | Linux | Validation | Status |
|---|:---:|:---:|:---:|:---:|---|---|
| Merge | ⏳ | ✅ | ⏳ | ⏳ | Reopen + page count | Stable |
| Extract / reorder pages (range parser) | ⏳ | ✅ | ⏳ | ⏳ | Reopen + page count | Stable |
| Rotate pages | ⏳ | ✅ | ⏳ | ⏳ | Reopen + /Rotate set | Stable |
| Split into pages | ⏳ | ✅ | ⏳ | ⏳ | One valid PDF per page | Stable |
| Remove metadata | ⏳ | ✅ | ⏳ | ⏳ | Info dictionary gone | Stable |
| Images → PDF | ⏳ | ✅ | ⏳ | ⏳ | Reopen + page count (Quick Look confirmed render) | Stable |
| Signed-document warning | ⏳ | ✅ | ⏳ | ⏳ | AcroForm/Sig detection | Stable |
| PDF → images | — | — | — | — | Declined — needs a rasteriser | Blocked |
| Image recompression / downsample | — | — | — | — | Declined — needs a rasteriser | Blocked |
| Structural optimize / password ops | — | — | — | — | Not implemented | Planned |

Validation for PDF ops is **structural** — the output is reopened with `lopdf`
and its page count checked. Visual/render validation lands with the rasteriser
that also unblocks PDF → images.

## Spreadsheets

Pure Rust (`csv`, `calamine`, `rust_xlsxwriter`, `encoding_rs`). Values are kept
exactly as text by default; coercion is per-column and opt-in.

| Operation | Windows | macOS ARM | macOS Intel | Linux | Validation | Status |
|---|:---:|:---:|:---:|:---:|---|---|
| CSV / TSV ↔ XLSX | ⏳ | ✅ | ⏳ | ⏳ | Re-parse + row/col counts + value & Unicode preservation | Stable |
| CSV / TSV ↔ JSON | ⏳ | ✅ | ⏳ | ⏳ | As above | Stable |
| XLSX ↔ JSON | ⏳ | ✅ | ⏳ | ⏳ | As above | Stable |
| Leading-zero / long-number preservation | ⏳ | ✅ | ⏳ | ⏳ | `007` and 16-digit ids stay text | Stable |
| Encoding detection (UTF-8/BOM, UTF-16 LE/BE) | ⏳ | ✅ | ⏳ | ⏳ | BOM round-trip fixture | Stable |
| Delimiter detection + manual override | ⏳ | ✅ | ⏳ | ⏳ | Semicolon/pipe fixtures | Stable |
| Per-column typing (Number/Boolean/Date/Automatic) | ⏳ | ✅ | ⏳ | ⏳ | Coercion refused when lossy, kept as text | Stable |
| Multi-sheet XLSX (select one) | ⏳ | ✅ | ⏳ | ⏳ | Warned; first/selected sheet | Beta |
| Formula / style / chart preservation | — | — | — | — | Not implemented (warned on loss) | Planned |

## Media

Uses a **system-installed** FFmpeg (detected on PATH, never bundled or
downloaded). Beta: correctness depends on the user's FFmpeg build, and only
macOS ARM has been exercised. If FFmpeg is absent the tab explains how to
install it and jobs decline with a clear message.

| Operation | Windows | macOS ARM | macOS Intel | Linux | Validation | Status |
|---|:---:|:---:|:---:|:---:|---|---|
| Video convert (MP4/WebM/MKV, presets) | ⏳ | ✅ | ⏳ | ⏳ | ffprobe: streams + duration | Beta |
| MP4 → GIF | ⏳ | ✅ | ⏳ | ⏳ | ffprobe: video, no audio | Beta |
| Audio convert (MP3/WAV/FLAC/OGG/M4A) | ⏳ | ✅ | ⏳ | ⏳ | ffprobe: audio + duration | Beta |
| Extract audio from video | ⏳ | ✅ | ⏳ | ⏳ | ffprobe: audio, no video | Beta |
| Trim | ⏳ | ✅ | ⏳ | ⏳ | Output duration within tolerance | Beta |
| Remove audio from video | ⏳ | ✅ | ⏳ | ⏳ | ffprobe: no audio stream | Beta |
| Real progress (from ffmpeg -progress) | ⏳ | ✅ | ⏳ | ⏳ | — | Beta |
| Cancellation (kills the child) | ⏳ | ✅ | ⏳ | ⏳ | — | Beta |
| Bundled FFmpeg | — | — | — | — | Not implemented (uses system FFmpeg) | Planned |
| Loudness normalise, frame-rate/bitrate UI | — | — | — | — | Not implemented | Planned |

## Explicitly not in scope for 1.0

DOCX → PDF, PPTX → PDF, complex office rendering, OCR, AI processing, DRM
removal, password cracking, repair of severely corrupted files, proprietary CAD,
cloud sync. See §3 of the specification.

---

Legend: ✅ verified in CI · ⏳ not yet verified on this platform · — not
implemented.

⏳ is honest rather than cautious: the suite runs in CI on all four targets, but
only macOS ARM has been executed on real hardware so far. Each mark flips to ✅
the first time that platform's job goes green.
