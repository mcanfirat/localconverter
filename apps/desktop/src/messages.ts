/**
 * Every user-facing string the Rust side can refer to.
 *
 * The backend never sends prose — it sends a `messageKey`, and this is where
 * that key becomes a sentence. `messages.test.ts` scans the Rust sources and
 * fails if a key exists there but not here, so an error can never surface as a
 * raw identifier.
 *
 * ponytail: a plain lookup, not i18next. There is one locale; add the library
 * when there is a second, at which point this map becomes `en.json` unchanged.
 */
export const MESSAGES: Record<string, string> = {
  // --- failure codes -------------------------------------------------------
  "error.unsupportedFormat": "That file format is not supported.",
  "error.unsupportedCodec": "That codec is not supported.",
  "error.invalidInput": "The input could not be used.",
  "error.corruptedInput": "The input file appears to be damaged.",
  "error.passwordRequired": "This file is password protected.",
  "error.invalidPassword": "That password did not work.",
  "error.insufficientDiskSpace": "There is not enough free disk space.",
  "error.insufficientMemory": "There is not enough memory for this file.",
  "error.permissionDenied": "LocalConvert is not allowed to write there.",
  "error.destinationUnavailable": "The destination folder is not available.",
  "error.toolMissing": "A required conversion engine is missing.",
  "error.toolChecksumMismatch":
    "A bundled engine failed its checksum check and was not run.",
  "error.processFailed": "The conversion engine stopped with an error.",
  "error.processTimedOut": "The conversion took too long and was stopped.",
  "error.cancelled": "Cancelled.",
  "error.outputValidationFailed":
    "The result failed verification, so it was not kept.",
  "error.outputLargerThanInput": "The result was larger than the original.",
  "error.archiveUnsafe": "That archive looks unsafe to extract.",
  "error.internalError": "Something went wrong inside LocalConvert.",

  // --- paths ---------------------------------------------------------------
  "error.path.nulByte": "That file name contains characters that are not safe.",
  "error.path.traversal": "That path tries to escape its folder.",
  "error.path.absolute": "A relative path was expected.",

  // --- destination ---------------------------------------------------------
  "error.destination.exists": "A file with that name already exists.",
  "error.destination.renameExhausted":
    "Too many files with that name already exist.",
  "error.destination.unavailable": "That folder could not be opened.",
  "error.destination.notADirectory": "That destination is not a folder.",
  "error.destination.wouldOverwriteSource":
    "That would write the result on top of your original file. Choose a different folder, or let LocalConvert rename the result.",

  // --- input ---------------------------------------------------------------
  "error.input.notAFile": "That is not a regular file.",
  "error.input.tooMany": "This operation takes one file at a time.",
  "error.input.missing": "No files were selected.",
  "error.input.notFound": "That file could not be found.",

  // --- images --------------------------------------------------------------
  "error.image.heicUnsupported":
    "HEIC needs a codec this build does not include. On a Mac you can export the photo as JPEG from Preview, then convert it here.",
  "error.image.avifUnsupported":
    "AVIF needs a codec this build does not include.",
  "error.image.backgroundRequired":
    "This image has transparent areas and the chosen format cannot keep them. Pick a background colour first.",
  "error.image.qualityNotApplicable":
    "That format is written losslessly, so a quality setting would do nothing.",
  "error.image.tooLarge": "That image is too large to open safely.",
  "error.image.encodeFailed": "The image could not be written in that format.",

  // --- archives ------------------------------------------------------------
  "error.archive.duplicateName":
    "Two of the selected files have the same name, which an archive cannot hold. Rename one first.",
  "error.archive.unsafe":
    "This archive contains an entry that tries to escape the destination folder. It was not extracted.",
  "error.archive.tooManyEntries":
    "This archive contains an unusually large number of files and was not extracted.",
  "error.archive.tooLarge":
    "This archive would expand to an unsafe size and was not extracted.",
  "error.archive.bomb":
    "This archive looks like a decompression bomb and was not extracted.",
  "error.archive.empty": "This archive contained no files to extract.",
  "error.archive.notATar":
    "This is a single compressed file, not a .tar.gz archive, so there is nothing to unpack from it.",

  // --- spreadsheets --------------------------------------------------------
  "error.spreadsheet.jsonNotTabular":
    "This JSON is not an array of rows, so it cannot become a table.",
  "error.spreadsheet.tooLarge":
    "This sheet is too large to convert in one pass.",

  // --- pdf -----------------------------------------------------------------
  "error.pdf.badPageRange":
    "That page range is not valid. Use something like 1-3,7,10-12.",
  "error.pdf.badRotation": "Rotation must be 0, 90, 180 or 270 degrees.",
  "error.pdf.mergeNeedsTwo": "Merging needs at least two PDFs.",
  "error.pdf.noPages": "No pages were selected.",
  "error.pdf.notAPdf": "That file is not a PDF.",
  "error.pdf.notAnImage": "That file is not an image that can be placed in a PDF.",

  // --- media ---------------------------------------------------------------
  "error.media.ffmpegMissing":
    "FFmpeg is required for audio and video and was not found. Install it (on a Mac: brew install ffmpeg) and reopen LocalConvert.",
  "error.media.noAudio": "This file has no audio to extract.",
  "error.media.noVideo": "This file has no video track.",
  "error.media.spawnFailed": "The media engine could not be started.",
  "error.media.encodeFailed": "FFmpeg could not complete the conversion.",

  // --- jobs and options ----------------------------------------------------
  "error.operation.unknown": "That operation is not available in this build.",
  "error.options.invalid": "Those settings are not valid.",
  "error.options.outOfRange": "That value is outside the allowed range.",
  "error.job.notRunning": "That job is not running.",
  "error.job.unknown": "That job is not in the queue.",

  // --- warnings ------------------------------------------------------------
  "warning.destination.skipped": "Skipped: a file with that name already exists.",
  "warning.destination.overwritten": "An existing file was replaced.",
  "warning.output.largerThanInput":
    "The result came out larger than the original.",
  "warning.image.extensionMismatch":
    "A file's contents do not match its extension. LocalConvert used the real format.",
  "warning.image.transparencyFlattened":
    "Transparent areas were filled with the background colour — JPEG cannot store transparency.",
  "warning.image.animationFlattened":
    "Only the first frame was exported. The chosen format cannot store animation.",
  "warning.image.metadataRemoved":
    "Metadata such as camera details and GPS location was removed.",
  "warning.image.lossyEncoding":
    "JPEG is a lossy format, so the image was re-compressed.",
  "warning.image.webpLossless":
    "WebP is written losslessly here, which preserves every pixel but produces larger files than lossy WebP.",
  "warning.spreadsheet.oneSheetOnly":
    "The workbook has several sheets; only the selected one was converted.",
  "warning.spreadsheet.xlsxFeaturesLost":
    "Formulas, styles, merged cells, charts and extra sheets are not carried into a flat format.",
  "warning.spreadsheet.jsonFlattened":
    "Nested JSON values were flattened to text; a table cannot hold nested structure.",
  "warning.spreadsheet.coercionKeptAsText":
    "Some values in a column typed as Number/Date could not be converted without loss, so they were kept as text.",
  "warning.spreadsheet.generatedKeys":
    "The source had no header row, so JSON keys were generated (Column 1, Column 2, …).",
  "warning.spreadsheet.encodingReplaced":
    "Some characters could not be decoded and were replaced.",
  "warning.pdf.signatureInvalidated":
    "This PDF appears to be digitally signed. Editing it invalidates the signature.",
  "warning.media.videoReencoded":
    "The video is re-encoded, so some quality is lost.",
  "warning.media.gifNoAudio": "GIF has no audio, so the audio track is dropped.",
  "warning.media.lossyEncoding": "This format re-compresses audio, so some quality is lost.",

  // --- progress ------------------------------------------------------------
  "progress.queued": "Waiting in the queue",
  "progress.preparing": "Preparing",
  "progress.running": "Working",
  "progress.writing": "Writing output",
  "progress.converting": "Converting",
  "progress.archiving": "Adding to archive",
  "progress.extracting": "Extracting",
  "progress.reading": "Reading",
  "progress.splitting": "Splitting pages",
  "progress.buildingPdf": "Building PDF",
  "progress.probing": "Reading media",
  "progress.encoding": "Encoding",
  "progress.validating": "Verifying the result",
  "progress.finalizing": "Saving to your folder",
  "progress.done": "Done",

  // --- operations ----------------------------------------------------------
  "operation.convert.label": "Convert images",
  "operation.convert.description":
    "Convert between JPG, PNG, WebP, TIFF and BMP. Resize, re-compress, and strip metadata. Every result is decoded again and checked before it is saved.",
  "operation.archiveCreate.label": "Create archive",
  "operation.archiveCreate.description":
    "Bundle files into a ZIP, TAR or TAR.GZ archive. The archive is reopened and its contents checked before it is saved.",
  "operation.archiveExtract.label": "Extract archive",
  "operation.archiveExtract.description":
    "Unpack a ZIP, TAR or TAR.GZ archive. Entries that try to escape the destination, symlinks, and decompression bombs are all refused.",
  "operation.spreadsheet.label": "Convert spreadsheets & data",
  "operation.spreadsheet.description":
    "Convert between CSV, TSV, XLSX and JSON. Values are preserved exactly as text by default — leading zeros and long numbers are never mangled — and each column can be typed as Number, Date or Boolean when you want coercion.",
  "operation.pdfMerge.label": "Merge PDFs",
  "operation.pdfMerge.description":
    "Combine several PDFs into one, in the order you choose. Page count is checked after merging.",
  "operation.pdfPages.label": "Extract, reorder & rotate pages",
  "operation.pdfPages.description":
    "Keep or reorder pages with a range like 1-3,7,10-12, and optionally rotate them. The result is verified before it is saved.",
  "operation.pdfSplit.label": "Split PDF",
  "operation.pdfSplit.description":
    "Split a PDF into one file per page.",
  "operation.pdfRemoveMetadata.label": "Remove PDF metadata",
  "operation.pdfRemoveMetadata.description":
    "Strip author, title and other document metadata from a PDF.",
  "operation.pdfFromImages.label": "Images to PDF",
  "operation.pdfFromImages.description":
    "Combine images into a single PDF, one image per page.",
  "operation.media.label": "Convert audio & video",
  "operation.media.description":
    "Convert between MP4, WebM, MKV, GIF and audio formats, and extract audio — using FFmpeg on your machine. Every output is probed and its duration checked before it is saved.",
  "operation.selftest.label": "Pipeline self-test",
  "operation.selftest.description":
    "Writes a verifiable test file to a folder you choose, then reads it back and checks every byte. Use it to confirm that jobs, progress, cancellation and verification all work on this machine.",
};

/** Resolves a backend message key. Unknown keys fall back to the key itself. */
export function t(key: string): string {
  return MESSAGES[key] ?? key;
}

/** `t` with an explicit fallback, for keys derived at runtime. */
export function tOr(key: string, fallback: string): string {
  return MESSAGES[key] ?? fallback;
}

/** `diagnostics.selftest` → `operation.selftest.label` */
export function operationLabelKey(operationId: string): string {
  return `operation.${operationId.split(".").pop() ?? operationId}.label`;
}
