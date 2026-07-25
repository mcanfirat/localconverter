# CLI

`localconvert` is the command-line face of the same engine the desktop app uses.
Every guarantee holds: your source is never modified, output is verified before
it is kept, and failures are explicit.

```bash
cargo run -p localconvert-cli -- <command> …
# or, from a release build:
localconvert <command> …
```

## Commands

```bash
# Images (and media — routed by the target format)
localconvert convert photo.png --to jpg --background white
localconvert convert shot.heic --to png            # HEIC is declined clearly
localconvert convert clip.mov  --to mp4 --preset balanced
localconvert convert video.mp4 --to mp3            # extract audio

# Archives
localconvert archive create a.txt b.txt --format zip -o bundle.zip
localconvert archive extract bundle.zip -o ./unpacked

# PDF (structural — no rasteriser needed)
localconvert pdf merge a.pdf b.pdf -o merged.pdf
localconvert pdf split document.pdf -o ./pages
localconvert pdf pages document.pdf --pages "1-3,7,10-12" --rotate 90
localconvert pdf remove-metadata report.pdf
localconvert pdf from-images scan1.png scan2.png -o scans.pdf

# Spreadsheets & structured data
localconvert spreadsheet data.csv --to xlsx
localconvert spreadsheet data.csv --to json --column id:text --column age:number
```

## Global options

| Option | Meaning |
|---|---|
| `--output`, `-o` | Destination directory, or a filename (its stem names the output). Defaults to the input's folder. |
| `--overwrite` | `fail` \| `rename` (default) \| `skip` \| `overwrite`. |
| `--json` | Emit a machine-readable JSON result on stdout instead of human text. |
| `--quiet` | Suppress progress and non-essential output. |

`--json` output shape:

```json
{ "ok": true, "outputs": [{ "path": "…", "sizeBytes": 1234, "format": "jpg" }],
  "warnings": ["warning.image.metadataRemoved"],
  "inputTotalBytes": 4096, "outputTotalBytes": 1234, "sizeChangePercent": -69.8 }
```

On failure: `{ "ok": false, "code": "UnsupportedFormat", "messageKey": "…",
"detail": "…", "sourceSafe": true, "partialOutputRemoved": false }`.

## Exit codes

Stable — scripts may rely on them.

| Code | Meaning |
|---:|---|
| 0 | Success. |
| 1 | Usage error (bad arguments). |
| 2 | Input problem: unsupported format, corrupt, or not found. |
| 3 | Conversion or output verification failed. |
| 4 | A required external tool (FFmpeg) is missing. |
| 130 | Interrupted. |

## Notes

- Media commands require a system-installed FFmpeg (`brew install ffmpeg`,
  `apt install ffmpeg`, …). Nothing is bundled or downloaded; if FFmpeg is
  absent the command exits 4 with a clear message.
- `convert` routes by the `--to` format: an image target uses the image engine,
  a media target uses FFmpeg.
- Shell completion is not generated yet.
