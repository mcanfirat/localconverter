//! PDF structural operations and image→PDF.
//!
//! Pure Rust via `lopdf`. What this build does and does not do is a deliberate
//! line drawn at "needs a rasteriser":
//!
//! * **Structural** operations — merge, extract/reorder/rotate pages, split,
//!   strip metadata, images→PDF — manipulate the object graph and need no
//!   renderer, so they are here and stable.
//! * **Rendering** operations — PDF→images, and image recompression /
//!   downsampling inside a PDF — need a rasteriser (PDFium/MuPDF). Bundling one
//!   is a packaging decision, so those are declined by name, exactly like HEIC,
//!   rather than half-done.
//!
//! Two honesty notes carried into the docs:
//! * Validation reopens the output with `lopdf` and checks the page count and
//!   that it parses. That is structural, not visual — render-based validation
//!   lands with the rasteriser.
//! * Modifying a digitally signed PDF invalidates its signature. Likely
//!   signatures are detected and the user is warned before any modifying op.

use std::path::Path;

use lopdf::{dictionary, Document, Object, ObjectId, Stream};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{ConversionError, ConversionErrorCode, Result};
use crate::file::FileFormat;
use crate::job::{ConversionJob, JobProgress, JobWarning, ProgressStage};
use crate::operation::{
    PDF_FROM_IMAGES_OPERATION_ID, PDF_MERGE_OPERATION_ID, PDF_PAGES_OPERATION_ID,
    PDF_REMOVE_METADATA_OPERATION_ID, PDF_SPLIT_OPERATION_ID,
};
use crate::runner::{ExecutionOutput, JobContext, StagedOutput};
use crate::validation::{OutputMetadata, ValidationCheck, ValidationReport};

/// Guard against a pathological page count in a range spec.
const MAX_PAGES: u32 = 50_000;

// ---------------------------------------------------------------------------
// page-range parser (spec §11.2)
// ---------------------------------------------------------------------------

/// Parses a 1-based page-range spec like `"1-3,7,10-12"` into an ordered list of
/// page numbers. The order is preserved (so `"3,1"` really means page 3 then
/// page 1 — that is how reordering is expressed), and every page is validated
/// against `page_count`.
///
/// Rejected: empty spec, zero, out-of-range, reversed ranges (`5-3`), and
/// anything non-numeric. Duplicates are allowed within reason (a page may be
/// repeated when a caller genuinely wants it twice) but the total is capped.
pub fn parse_page_ranges(spec: &str, page_count: u32) -> Result<Vec<u32>> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(range_error("page range is empty"));
    }
    let mut pages = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(range_error("empty segment in page range"));
        }
        match part.split_once('-') {
            Some((start, end)) => {
                let start = parse_page_number(start, page_count)?;
                let end = parse_page_number(end, page_count)?;
                if start > end {
                    return Err(range_error(&format!("reversed range {start}-{end}")));
                }
                for page in start..=end {
                    pages.push(page);
                    if pages.len() as u32 > MAX_PAGES {
                        return Err(range_error("page range expands too far"));
                    }
                }
            }
            None => pages.push(parse_page_number(part, page_count)?),
        }
    }
    Ok(pages)
}

fn parse_page_number(text: &str, page_count: u32) -> Result<u32> {
    let n: u32 = text
        .trim()
        .parse()
        .map_err(|_| range_error(&format!("'{text}' is not a page number")))?;
    if n == 0 || n > page_count {
        return Err(range_error(&format!(
            "page {n} is out of range (1..={page_count})"
        )));
    }
    Ok(n)
}

fn range_error(detail: &str) -> ConversionError {
    ConversionError::new(ConversionErrorCode::InvalidInput, detail.to_owned())
        .with_message_key("error.pdf.badPageRange")
}

// ---------------------------------------------------------------------------
// options
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PagesOptions {
    /// Which pages to keep, in order. `None` keeps them all (useful with rotate).
    #[serde(default)]
    pub pages: Option<String>,
    /// Rotation applied to the kept pages, in degrees: 0, 90, 180 or 270.
    #[serde(default)]
    pub rotate: Option<i64>,
    /// Base name for the output PDF.
    #[serde(default)]
    pub output_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MergeOptions {
    #[serde(default)]
    pub output_name: Option<String>,
}

// ---------------------------------------------------------------------------
// dispatch
// ---------------------------------------------------------------------------

pub async fn execute(job: &ConversionJob, ctx: &JobContext) -> Result<ExecutionOutput> {
    match job.operation_id.as_str() {
        PDF_MERGE_OPERATION_ID => merge(job, ctx),
        PDF_PAGES_OPERATION_ID => pages(job, ctx),
        PDF_SPLIT_OPERATION_ID => split(job, ctx),
        PDF_REMOVE_METADATA_OPERATION_ID => remove_metadata(job, ctx),
        PDF_FROM_IMAGES_OPERATION_ID => images_to_pdf(job, ctx),
        other => Err(ConversionError::internal(format!("pdf engine got {other}"))),
    }
}

pub async fn validate(
    _job: &ConversionJob,
    ctx: &JobContext,
    execution: &ExecutionOutput,
) -> Result<Vec<ValidationReport>> {
    ctx.report(JobProgress::indeterminate(
        ProgressStage::Validating,
        "progress.validating",
    ));

    let mut reports = Vec::with_capacity(execution.outputs.len());
    for staged in &execution.outputs {
        let mut checks = vec![magic_check(&staged.staged_path)];
        // Reopen with lopdf — a structural re-parse and page count. Not a render.
        match Document::load(&staged.staged_path) {
            Ok(doc) => {
                checks.push(ValidationCheck::passed("pdf.reopens"));
                let count = doc.get_pages().len();
                checks.push(if count > 0 {
                    ValidationCheck::passed("pdf.hasPages")
                } else {
                    ValidationCheck::failed("pdf.hasPages", "output has no pages")
                });
            }
            Err(err) => checks.push(ValidationCheck::failed("pdf.reopens", format!("{err}"))),
        }
        reports.push(ValidationReport::from_checks(
            "pdf",
            checks,
            OutputMetadata {
                size_bytes: staged.size_bytes,
                properties: serde_json::json!({}),
            },
        ));
    }
    Ok(reports)
}

// ---------------------------------------------------------------------------
// operations
// ---------------------------------------------------------------------------

fn merge(job: &ConversionJob, ctx: &JobContext) -> Result<ExecutionOutput> {
    if job.input_files.len() < 2 {
        return Err(ConversionError::new(
            ConversionErrorCode::InvalidInput,
            "merge needs at least two PDFs",
        )
        .with_message_key("error.pdf.mergeNeedsTwo"));
    }

    let mut warnings = Vec::new();
    let mut target = Document::with_version("1.5");
    let mut max_id: u32 = 1;
    let mut merged_page_ids: Vec<ObjectId> = Vec::new();
    let mut input_total_bytes = 0u64;

    for input in &job.input_files {
        ctx.check_cancelled()?;
        require_pdf(input)?;
        let mut doc = load_pdf(&input.path)?;
        if is_signed(&doc) {
            warnings.push(JobWarning::new("warning.pdf.signatureInvalidated"));
        }
        input_total_bytes = input_total_bytes.saturating_add(input.size_bytes);

        // Shift this document's object ids past everything already merged, then
        // fold its objects and page ids into the target.
        doc.renumber_objects_with(max_id);
        max_id = doc.max_id + 1;

        // Re-parenting severs each page from the /Pages node it was inheriting
        // /MediaBox, /Resources, /CropBox and /Rotate from (ISO 32000-1
        // §7.7.3.4 — macOS's own PDF writer relies on this). Copy those down
        // onto the pages first, or the merged pages lose their size and fonts
        // and render blank.
        flatten_inherited_attributes(&mut doc);

        for page_id in doc.get_pages().into_values() {
            merged_page_ids.push(page_id);
        }
        target.objects.extend(doc.objects);
    }

    // `Document::with_version` leaves max_id at 0, so without this the id
    // allocator in `finalize_pages_tree` would hand out 1, 2, … — ids already
    // occupied by merged content, silently overwriting real pages.
    target.max_id = target.objects.keys().map(|(id, _)| *id).max().unwrap_or(0);

    finalize_pages_tree(&mut target, &merged_page_ids)?;

    let file_name = output_file_name(merge_options(job)?.output_name.as_deref(), "merged");
    let staged = write_pdf(ctx, &mut target, &file_name)?;
    // Structural op, not compression: no size baseline (see single_output).
    let _ = input_total_bytes;
    Ok(single_output(staged, warnings, 0))
}

fn pages(job: &ConversionJob, ctx: &JobContext) -> Result<ExecutionOutput> {
    let input = single_input(job)?;
    require_pdf(input)?;
    let options: PagesOptions = parse_options(&job.options)?;
    let rotation = validate_rotation(options.rotate)?;

    let mut doc = load_pdf(&input.path)?;
    let mut warnings = Vec::new();
    if is_signed(&doc) {
        warnings.push(JobWarning::new("warning.pdf.signatureInvalidated"));
    }

    let page_map = doc.get_pages();
    let page_count = page_map.len() as u32;

    // Which 1-based page numbers to keep, in order.
    let keep: Vec<u32> = match options.pages.as_deref() {
        Some(spec) => parse_page_ranges(spec, page_count)?,
        None => (1..=page_count).collect(),
    };
    if keep.is_empty() {
        return Err(
            ConversionError::new(ConversionErrorCode::InvalidInput, "no pages selected")
                .with_message_key("error.pdf.noPages"),
        );
    }

    // Delete everything not kept. Pages not present in `keep` are removed;
    // reordering within `keep` is handled by rebuilding the Kids array in order.
    let keep_set: std::collections::HashSet<u32> = keep.iter().copied().collect();
    let to_delete: Vec<u32> = (1..=page_count).filter(|p| !keep_set.contains(p)).collect();
    doc.delete_pages(&to_delete);

    // delete_pages renumbers the survivors 1..n in original order, so map the
    // caller's requested ordering onto the survivors by position.
    let surviving = doc.get_pages();
    let ordered_ids = remap_to_kept_order(&keep, &surviving);

    if let Some(degrees) = rotation {
        for &page_id in &ordered_ids {
            if let Ok(dict) = doc.get_dictionary_mut(page_id) {
                dict.set("Rotate", Object::Integer(degrees));
            }
        }
    }

    set_kids_order(&mut doc, &ordered_ids)?;
    // delete_pages only unlinks; the discarded pages' content streams, fonts and
    // images stay in the file. Drop anything the surviving tree no longer
    // references, or "extract page 1 of 40" ships all 40 pages' data.
    doc.prune_objects();

    let file_name = output_file_name(options.output_name.as_deref(), "pages");
    let staged = write_pdf(ctx, &mut doc, &file_name)?;
    Ok(single_output(staged, warnings, 0))
}

fn split(job: &ConversionJob, ctx: &JobContext) -> Result<ExecutionOutput> {
    let input = single_input(job)?;
    require_pdf(input)?;
    let base = load_pdf(&input.path)?;
    let mut warnings = Vec::new();
    if is_signed(&base) {
        warnings.push(JobWarning::new("warning.pdf.signatureInvalidated"));
    }

    let page_count = base.get_pages().len() as u32;
    if page_count == 0 {
        return Err(
            ConversionError::new(ConversionErrorCode::CorruptedInput, "PDF has no pages")
                .with_message_key("error.corruptedInput"),
        );
    }

    let stem = strip_pdf_ext(&input.display_name);
    let mut outputs = Vec::with_capacity(page_count as usize);
    for page in 1..=page_count {
        ctx.check_cancelled()?;
        ctx.report(JobProgress::counted(
            ProgressStage::Running,
            u64::from(page),
            u64::from(page_count),
            "progress.splitting",
        ));

        // Clone the whole doc and delete every page but this one.
        let mut single = load_pdf(&input.path)?;
        let to_delete: Vec<u32> = (1..=page_count).filter(|p| *p != page).collect();
        single.delete_pages(&to_delete);
        // Same as `pages`: without pruning, every split page carries the whole
        // document's content.
        single.prune_objects();

        let file_name = format!("{stem}/page-{page:03}.pdf");
        let staged = write_pdf(ctx, &mut single, &file_name)?;
        outputs.push(staged);
    }

    Ok(ExecutionOutput {
        outputs,
        warnings,
        input_total_bytes: 0,
        size_growth_expected: false,
    })
}

fn remove_metadata(job: &ConversionJob, ctx: &JobContext) -> Result<ExecutionOutput> {
    let input = single_input(job)?;
    require_pdf(input)?;
    let mut doc = load_pdf(&input.path)?;
    let mut warnings = Vec::new();
    if is_signed(&doc) {
        warnings.push(JobWarning::new("warning.pdf.signatureInvalidated"));
    }

    // The document Info dictionary (author, title, producer, dates…).
    doc.trailer.remove(b"Info");
    // The XMP metadata stream hanging off the catalog.
    if let Ok(catalog) = doc.catalog_mut() {
        catalog.remove(b"Metadata");
    }
    doc.prune_objects();

    let stem = strip_pdf_ext(&input.display_name);
    let file_name = format!("{stem}.pdf");
    let staged = write_pdf(ctx, &mut doc, &file_name)?;
    Ok(single_output(staged, warnings, 0))
}

fn images_to_pdf(job: &ConversionJob, ctx: &JobContext) -> Result<ExecutionOutput> {
    if job.input_files.is_empty() {
        return Err(
            ConversionError::new(ConversionErrorCode::InvalidInput, "no images")
                .with_message_key("error.input.missing"),
        );
    }

    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut page_ids = Vec::new();
    let mut input_total_bytes = 0u64;
    let total = job.input_files.len() as u64;

    for (index, input) in job.input_files.iter().enumerate() {
        ctx.check_cancelled()?;
        ctx.report(JobProgress::counted(
            ProgressStage::Running,
            index as u64,
            total,
            "progress.buildingPdf",
        ));

        if !crate::image_engine::can_decode(input.detected_format) {
            return Err(ConversionError::new(
                ConversionErrorCode::UnsupportedFormat,
                format!(
                    "{} is not an image that can be placed in a PDF",
                    input.display_name
                ),
            )
            .with_message_key("error.pdf.notAnImage"));
        }
        input_total_bytes = input_total_bytes.saturating_add(input.size_bytes);

        // Read the dimensions cheaply for the page box.
        let (width, height) = image_dimensions(&input.path)?;

        let image_stream = lopdf::xobject::image(&input.path).map_err(|err| {
            ConversionError::new(
                ConversionErrorCode::CorruptedInput,
                format!("cannot embed image: {err}"),
            )
            .with_message_key("error.corruptedInput")
        })?;
        let img_id = doc.add_object(image_stream);
        let img_name = format!("X{}", img_id.0);

        let content = lopdf::content::Content {
            operations: vec![
                lopdf::content::Operation::new(
                    "cm",
                    vec![
                        width.into(),
                        0.into(),
                        0.into(),
                        height.into(),
                        0.into(),
                        0.into(),
                    ],
                ),
                lopdf::content::Operation::new(
                    "Do",
                    vec![Object::Name(img_name.as_bytes().to_vec())],
                ),
            ],
        };
        let content_id = doc.add_object(Stream::new(
            dictionary! {},
            content.encode().map_err(pdf_error)?,
        ));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "MediaBox" => vec![0.into(), 0.into(), width.into(), height.into()],
        });
        doc.add_xobject(page_id, img_name.as_bytes(), img_id)
            .map_err(pdf_error)?;
        page_ids.push(page_id);
    }

    // Build the Pages tree using the id reserved up front.
    let kids: Vec<Object> = page_ids.iter().map(|id| Object::Reference(*id)).collect();
    let count = page_ids.len() as i64;
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => count,
        }),
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);

    let file_name = output_file_name(
        merge_options(job)
            .ok()
            .and_then(|o| o.output_name)
            .as_deref(),
        "images",
    );
    let staged = write_pdf(ctx, &mut doc, &file_name)?;
    let _ = input_total_bytes;
    Ok(single_output(staged, Vec::new(), 0))
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Copies the four inheritable page attributes down from a page's ancestors
/// onto the page itself.
///
/// PDF lets /MediaBox, /CropBox, /Resources and /Rotate live on any ancestor of
/// a page. Merging re-parents pages under a brand-new /Pages node, which cuts
/// them off from whatever they were inheriting — so they must carry their own
/// copy across first.
fn flatten_inherited_attributes(doc: &mut Document) {
    const INHERITABLE: [&[u8]; 4] = [b"MediaBox", b"CropBox", b"Resources", b"Rotate"];

    let page_ids: Vec<ObjectId> = doc.get_pages().into_values().collect();
    let mut pending: Vec<(ObjectId, Vec<u8>, Object)> = Vec::new();

    for page_id in page_ids {
        for key in INHERITABLE {
            let already_set = doc
                .get_dictionary(page_id)
                .map(|dict| dict.has(key))
                .unwrap_or(false);
            if already_set {
                continue;
            }
            if let Some(value) = inherited_value(doc, page_id, key) {
                pending.push((page_id, key.to_vec(), value));
            }
        }
    }

    for (page_id, key, value) in pending {
        if let Ok(dict) = doc.get_dictionary_mut(page_id) {
            dict.set(key, value);
        }
    }
}

/// Walks a page's /Parent chain looking for `key`. Bounded, so a malformed file
/// with a parent cycle cannot hang the job.
fn inherited_value(doc: &Document, page_id: ObjectId, key: &[u8]) -> Option<Object> {
    let mut current = doc
        .get_dictionary(page_id)
        .ok()?
        .get(b"Parent")
        .ok()?
        .as_reference()
        .ok()?;

    for _ in 0..32 {
        let dict = doc.get_dictionary(current).ok()?;
        if let Ok(value) = dict.get(key) {
            return Some(value.clone());
        }
        current = dict.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

/// Rebuilds the catalog + Pages tree so `page_ids` are the document's pages, in
/// order. Used by merge.
fn finalize_pages_tree(doc: &mut Document, page_ids: &[ObjectId]) -> Result<()> {
    if page_ids.is_empty() {
        return Err(
            ConversionError::new(ConversionErrorCode::CorruptedInput, "no pages to merge")
                .with_message_key("error.corruptedInput"),
        );
    }
    let pages_id = doc.new_object_id();
    // Point each page at the new Pages node as its parent.
    for &page_id in page_ids {
        if let Ok(dict) = doc.get_dictionary_mut(page_id) {
            dict.set("Parent", pages_id);
        }
    }
    let kids: Vec<Object> = page_ids.iter().map(|id| Object::Reference(*id)).collect();
    let count = page_ids.len() as i64;
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => count,
        }),
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc.max_id = doc.objects.keys().map(|(id, _)| *id).max().unwrap_or(0);
    Ok(())
}

/// After `delete_pages`, maps the caller's ordered page numbers onto the
/// surviving page object ids. `surviving` is keyed by the *new* 1..n numbering,
/// which follows the original order of the kept pages.
fn remap_to_kept_order(
    keep: &[u32],
    surviving: &std::collections::BTreeMap<u32, ObjectId>,
) -> Vec<ObjectId> {
    // The survivors, in original order, are new pages 1..=k. Sort the caller's
    // request into original order to learn each kept page's new index, then emit
    // ids in the caller's requested order.
    let mut sorted = keep.to_vec();
    sorted.sort_unstable();
    sorted.dedup();

    let mut original_to_new = std::collections::HashMap::new();
    for (new_index, original) in sorted.iter().enumerate() {
        original_to_new.insert(*original, (new_index + 1) as u32);
    }
    keep.iter()
        .filter_map(|original| {
            let new_number = original_to_new.get(original)?;
            surviving.get(new_number).copied()
        })
        .collect()
}

fn set_kids_order(doc: &mut Document, ordered_ids: &[ObjectId]) -> Result<()> {
    // Find the Pages node from the catalog and rewrite its Kids in the given
    // order (delete_pages keeps original order; this applies any reordering).
    let pages_id = doc
        .catalog()
        .ok()
        .and_then(|cat| cat.get(b"Pages").ok())
        .and_then(|obj| obj.as_reference().ok())
        .ok_or_else(|| ConversionError::internal("PDF has no Pages node"))?;

    for &page_id in ordered_ids {
        if let Ok(dict) = doc.get_dictionary_mut(page_id) {
            dict.set("Parent", pages_id);
        }
    }
    let kids: Vec<Object> = ordered_ids
        .iter()
        .map(|id| Object::Reference(*id))
        .collect();
    let count = ordered_ids.len() as i64;
    if let Ok(pages) = doc.get_dictionary_mut(pages_id) {
        pages.set("Kids", kids);
        pages.set("Count", count);
    }
    Ok(())
}

/// A likely digital signature. Modifying such a PDF invalidates it, so the user
/// is warned. Heuristic: an AcroForm with SigFlags, or a signature field.
fn is_signed(doc: &Document) -> bool {
    if let Ok(catalog) = doc.catalog() {
        if let Ok(acroform_obj) = catalog.get(b"AcroForm") {
            // AcroForm is usually an indirect reference, so resolve it first.
            if let Ok(acroform) = doc.dereference(acroform_obj).and_then(|(_, o)| o.as_dict()) {
                if acroform.has(b"SigFlags") {
                    return true;
                }
            }
        }
    }
    // Any object that is a signature dictionary (a /Sig with a /ByteRange).
    doc.objects.values().any(|obj| {
        obj.as_dict()
            .map(|d| d.has_type(b"Sig") || d.has(b"ByteRange"))
            .unwrap_or(false)
    })
}

fn image_dimensions(path: &str) -> Result<(u32, u32)> {
    let size = imagesize::size(path).map_err(|err| {
        ConversionError::new(
            ConversionErrorCode::CorruptedInput,
            format!("cannot read image size: {err}"),
        )
        .with_message_key("error.corruptedInput")
    })?;
    Ok((
        u32::try_from(size.width).unwrap_or(u32::MAX),
        u32::try_from(size.height).unwrap_or(u32::MAX),
    ))
}

fn validate_rotation(rotate: Option<i64>) -> Result<Option<i64>> {
    match rotate {
        None | Some(0) => Ok(rotate.filter(|r| *r != 0)),
        Some(r @ (90 | 180 | 270)) => Ok(Some(r)),
        Some(other) => Err(ConversionError::new(
            ConversionErrorCode::InvalidInput,
            format!("rotation must be 0, 90, 180 or 270, got {other}"),
        )
        .with_message_key("error.pdf.badRotation")),
    }
}

fn require_pdf(input: &crate::file::FileDescriptor) -> Result<()> {
    if input.detected_format == FileFormat::Pdf {
        Ok(())
    } else {
        Err(ConversionError::new(
            ConversionErrorCode::UnsupportedFormat,
            format!("{} is not a PDF", input.display_name),
        )
        .with_message_key("error.pdf.notAPdf"))
    }
}

fn load_pdf(path: &str) -> Result<Document> {
    Document::load(path).map_err(|err| {
        ConversionError::new(
            ConversionErrorCode::CorruptedInput,
            format!("cannot open PDF: {err}"),
        )
        .with_message_key("error.corruptedInput")
    })
}

fn write_pdf(ctx: &JobContext, doc: &mut Document, file_name: &str) -> Result<StagedOutput> {
    let staged = ctx.workspace.staging_path(file_name)?;
    if let Some(parent) = staged.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| ConversionError::from_io("create pdf workspace dir", &err))?;
    }
    doc.save(&staged).map_err(|err| {
        ConversionError::new(
            ConversionErrorCode::ProcessFailed,
            format!("cannot write PDF: {err}"),
        )
        .with_message_key("error.processFailed")
    })?;
    let size_bytes = std::fs::metadata(&staged)
        .map_err(|err| ConversionError::from_io("stat pdf output", &err))?
        .len();
    Ok(StagedOutput {
        staged_path: staged,
        file_name: file_name.to_owned(),
        format: "pdf".to_owned(),
        size_bytes,
    })
}

/// PDF structural ops report a 0 input baseline: they are not compression, so
/// a "larger than input" comparison would be a meaningless warning.
fn single_output(
    staged: StagedOutput,
    warnings: Vec<JobWarning>,
    input_total_bytes: u64,
) -> ExecutionOutput {
    ExecutionOutput {
        outputs: vec![staged],
        warnings,
        input_total_bytes,
        size_growth_expected: false,
    }
}

fn single_input(job: &ConversionJob) -> Result<&crate::file::FileDescriptor> {
    if job.input_files.len() > 1 {
        return Err(ConversionError::new(
            ConversionErrorCode::InvalidInput,
            "this operation takes one PDF at a time",
        )
        .with_message_key("error.input.tooMany"));
    }
    job.input_files.first().ok_or_else(|| {
        ConversionError::new(ConversionErrorCode::InvalidInput, "no input files")
            .with_message_key("error.input.missing")
    })
}

fn parse_options<T: for<'de> Deserialize<'de>>(options: &serde_json::Value) -> Result<T> {
    let value = if options.is_null() {
        serde_json::json!({})
    } else {
        options.clone()
    };
    serde_json::from_value(value).map_err(|err| {
        ConversionError::new(
            ConversionErrorCode::InvalidInput,
            format!("invalid PDF options: {err}"),
        )
        .with_message_key("error.options.invalid")
    })
}

fn merge_options(job: &ConversionJob) -> Result<MergeOptions> {
    parse_options(&job.options)
}

fn magic_check(path: &Path) -> ValidationCheck {
    match std::fs::read(path) {
        Ok(bytes) if bytes.starts_with(b"%PDF-") => ValidationCheck::passed("pdf.magicBytes"),
        Ok(_) => ValidationCheck::failed("pdf.magicBytes", "output does not start with %PDF-"),
        Err(err) => ValidationCheck::failed("pdf.magicBytes", format!("{err}")),
    }
}

fn strip_pdf_ext(display_name: &str) -> String {
    display_name
        .strip_suffix(".pdf")
        .or_else(|| display_name.strip_suffix(".PDF"))
        .unwrap_or(display_name)
        .to_owned()
}

fn output_file_name(requested: Option<&str>, fallback: &str) -> String {
    let stem = requested
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback);
    format!("{stem}.pdf")
}

fn pdf_error(err: lopdf::Error) -> ConversionError {
    ConversionError::new(
        ConversionErrorCode::ProcessFailed,
        format!("PDF error: {err}"),
    )
    .with_message_key("error.processFailed")
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

    use lopdf::{dictionary, Document, Object, Stream};
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    use super::*;
    use crate::file::FileDescriptor;
    use crate::paths::OverwritePolicy;
    use crate::runner;
    use crate::workspace::JobWorkspace;

    struct Harness {
        _temp: tempfile::TempDir,
        src_dir: std::path::PathBuf,
        out_dir: std::path::PathBuf,
        temp_root: std::path::PathBuf,
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

        /// Builds a minimal valid multi-page PDF with `pages` blank pages.
        fn pdf(&self, name: &str, pages: usize) -> std::path::PathBuf {
            let mut doc = Document::with_version("1.5");
            let pages_id = doc.new_object_id();
            let mut kids = Vec::new();
            for _ in 0..pages {
                let content_id = doc.add_object(Stream::new(dictionary! {}, b"".to_vec()));
                let page_id = doc.add_object(dictionary! {
                    "Type" => "Page",
                    "Parent" => pages_id,
                    "Contents" => content_id,
                    "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                });
                kids.push(Object::Reference(page_id));
            }
            let count = kids.len() as i64;
            doc.objects.insert(
                pages_id,
                Object::Dictionary(dictionary! {
                    "Type" => "Pages", "Kids" => kids, "Count" => count,
                }),
            );
            let catalog_id =
                doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
            doc.trailer.set("Root", catalog_id);
            let path = self.src_dir.join(name);
            doc.save(&path).unwrap();
            path
        }

        /// A PDF whose pages inherit /MediaBox and /Resources from the /Pages
        /// node instead of carrying their own — legal per ISO 32000-1 §7.7.3.4
        /// and what macOS's own writer produces.
        fn inheriting_pdf(&self, name: &str, pages: usize) -> std::path::PathBuf {
            let mut doc = Document::with_version("1.5");
            let pages_id = doc.new_object_id();
            let font_id = doc.add_object(dictionary! {
                "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
            });
            let mut kids = Vec::new();
            for _ in 0..pages {
                let content_id = doc.add_object(Stream::new(dictionary! {}, b"".to_vec()));
                // Deliberately no MediaBox/Resources on the page itself.
                let page_id = doc.add_object(dictionary! {
                    "Type" => "Page",
                    "Parent" => pages_id,
                    "Contents" => content_id,
                });
                kids.push(Object::Reference(page_id));
            }
            let count = kids.len() as i64;
            doc.objects.insert(
                pages_id,
                Object::Dictionary(dictionary! {
                    "Type" => "Pages",
                    "Kids" => kids,
                    "Count" => count,
                    "MediaBox" => vec![0.into(), 0.into(), 400.into(), 500.into()],
                    "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
                }),
            );
            let catalog_id =
                doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
            doc.trailer.set("Root", catalog_id);
            let path = self.src_dir.join(name);
            doc.save(&path).unwrap();
            path
        }

        fn png(&self, name: &str, w: u32, h: u32) -> std::path::PathBuf {
            let img = image::RgbImage::from_pixel(w, h, image::Rgb([120, 160, 200]));
            let path = self.src_dir.join(name);
            img.save_with_format(&path, image::ImageFormat::Png)
                .unwrap();
            path
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

        fn job(&self, op: &str, inputs: &[&Path], options: serde_json::Value) -> ConversionJob {
            ConversionJob::new(
                op,
                inputs
                    .iter()
                    .map(|p| FileDescriptor::probe(p).unwrap())
                    .collect(),
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
    }

    fn page_count(path: &Path) -> usize {
        Document::load(path).unwrap().get_pages().len()
    }

    #[test]
    fn page_range_parser_handles_the_spec_example() {
        assert_eq!(
            parse_page_ranges("1-3,7,10-12", 12).unwrap(),
            vec![1, 2, 3, 7, 10, 11, 12]
        );
    }

    #[test]
    fn page_range_parser_preserves_order_for_reordering() {
        assert_eq!(parse_page_ranges("3,1,2", 3).unwrap(), vec![3, 1, 2]);
    }

    #[test]
    fn page_range_parser_rejects_bad_input() {
        assert!(parse_page_ranges("", 5).is_err());
        assert!(parse_page_ranges("0", 5).is_err());
        assert!(parse_page_ranges("6", 5).is_err());
        assert!(parse_page_ranges("5-3", 5).is_err());
        assert!(parse_page_ranges("1,,2", 5).is_err());
        assert!(parse_page_ranges("a-b", 5).is_err());
    }

    #[tokio::test]
    async fn merge_concatenates_page_counts() {
        let h = Harness::new();
        let a = h.pdf("a.pdf", 2);
        let b = h.pdf("b.pdf", 3);
        let result = h
            .run(&h.job(
                PDF_MERGE_OPERATION_ID,
                &[&a, &b],
                serde_json::json!({ "outputName": "out" }),
            ))
            .await
            .unwrap();
        assert!(
            result.validation_reports[0].valid,
            "{:?}",
            result.validation_reports[0].failed_checks()
        );
        assert_eq!(page_count(&h.out_dir.join("out.pdf")), 5);
    }

    #[tokio::test]
    async fn extract_keeps_only_selected_pages() {
        let h = Harness::new();
        let src = h.pdf("doc.pdf", 10);
        h.run(&h.job(
            PDF_PAGES_OPERATION_ID,
            &[&src],
            serde_json::json!({ "pages": "1-3,7", "outputName": "ex" }),
        ))
        .await
        .unwrap();
        assert_eq!(page_count(&h.out_dir.join("ex.pdf")), 4);
    }

    #[tokio::test]
    async fn split_produces_one_pdf_per_page() {
        let h = Harness::new();
        let src = h.pdf("multi.pdf", 4);
        let result = h
            .run(&h.job(PDF_SPLIT_OPERATION_ID, &[&src], serde_json::Value::Null))
            .await
            .unwrap();
        assert_eq!(result.outputs.len(), 4);
        for page in 1..=4 {
            let out = h.out_dir.join(format!("multi/page-{page:03}.pdf"));
            assert!(out.exists(), "{out:?} missing");
            assert_eq!(page_count(&out), 1);
        }
    }

    #[tokio::test]
    async fn rotate_sets_the_rotate_key() {
        let h = Harness::new();
        let src = h.pdf("r.pdf", 1);
        h.run(&h.job(
            PDF_PAGES_OPERATION_ID,
            &[&src],
            serde_json::json!({ "rotate": 90, "outputName": "rot" }),
        ))
        .await
        .unwrap();
        let doc = Document::load(h.out_dir.join("rot.pdf")).unwrap();
        let page_id = *doc.get_pages().get(&1).unwrap();
        let dict = doc.get_dictionary(page_id).unwrap();
        assert_eq!(dict.get(b"Rotate").unwrap().as_i64().unwrap(), 90);
    }

    #[tokio::test]
    async fn an_invalid_rotation_is_refused() {
        let h = Harness::new();
        let src = h.pdf("r.pdf", 1);
        let err = h
            .run(&h.job(
                PDF_PAGES_OPERATION_ID,
                &[&src],
                serde_json::json!({ "rotate": 45 }),
            ))
            .await
            .unwrap_err();
        assert_eq!(err.message_key, "error.pdf.badRotation");
    }

    #[tokio::test]
    async fn images_become_a_pdf_with_one_page_each() {
        let h = Harness::new();
        let a = h.png("one.png", 100, 80);
        let b = h.png("two.png", 60, 90);
        let result = h
            .run(&h.job(
                PDF_FROM_IMAGES_OPERATION_ID,
                &[&a, &b],
                serde_json::json!({ "outputName": "album" }),
            ))
            .await
            .unwrap();
        assert!(
            result.validation_reports[0].valid,
            "{:?}",
            result.validation_reports[0].failed_checks()
        );
        assert_eq!(page_count(&h.out_dir.join("album.pdf")), 2);
    }

    #[tokio::test]
    async fn remove_metadata_strips_the_info_dictionary() {
        let h = Harness::new();
        // A PDF with an Info dictionary.
        let mut doc = Document::load(h.pdf("meta.pdf", 1)).unwrap();
        let info_id =
            doc.add_object(dictionary! { "Author" => Object::string_literal("Secret Person") });
        doc.trailer.set("Info", info_id);
        let path = h.src_dir.join("withmeta.pdf");
        doc.save(&path).unwrap();

        h.run(&h.job(
            PDF_REMOVE_METADATA_OPERATION_ID,
            &[&path],
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
        let cleaned = Document::load(h.out_dir.join("withmeta.pdf")).unwrap();
        assert!(
            cleaned.trailer.get(b"Info").is_err(),
            "Info dictionary survived"
        );
    }

    #[tokio::test]
    async fn a_signed_pdf_warns_before_modification() {
        let h = Harness::new();
        let mut doc = Document::load(h.pdf("signed.pdf", 2)).unwrap();
        // Fake an AcroForm with SigFlags to look signed.
        let acro = doc.add_object(dictionary! { "SigFlags" => 3 });
        if let Ok(cat) = doc.catalog_mut() {
            cat.set("AcroForm", acro);
        }
        let path = h.src_dir.join("signed2.pdf");
        doc.save(&path).unwrap();

        let result = h
            .run(&h.job(
                PDF_PAGES_OPERATION_ID,
                &[&path],
                serde_json::json!({ "pages": "1" }),
            ))
            .await
            .unwrap();
        assert!(result
            .warnings
            .iter()
            .any(|w| w.message_key == "warning.pdf.signatureInvalidated"));
    }

    #[tokio::test]
    async fn a_non_pdf_is_declined() {
        let h = Harness::new();
        let png = h.png("not.png", 10, 10);
        let err = h
            .run(&h.job(
                PDF_PAGES_OPERATION_ID,
                &[&png],
                serde_json::json!({ "pages": "1" }),
            ))
            .await
            .unwrap_err();
        assert_eq!(err.code, ConversionErrorCode::UnsupportedFormat);
    }

    #[tokio::test]
    async fn merge_does_not_clobber_objects_that_came_from_the_inputs() {
        // The new /Pages and /Catalog used to be allocated as ids 1 and 2, which
        // are live objects in most real PDFs (macOS writes the first page as
        // object 1), silently destroying that page.
        let h = Harness::new();
        let a = h.pdf("first.pdf", 2);
        let b = h.pdf("second.pdf", 3);

        let result = h
            .run(&h.job(
                PDF_MERGE_OPERATION_ID,
                &[&a, &b],
                serde_json::json!({ "outputName": "m" }),
            ))
            .await
            .unwrap();
        assert!(result.validation_reports[0].valid);

        let merged = Document::load(h.out_dir.join("m.pdf")).unwrap();
        assert_eq!(merged.get_pages().len(), 5);
        // Every page must still resolve to a real page dictionary — a clobbered
        // object shows up here as a missing or wrong-typed entry.
        for (_, page_id) in merged.get_pages() {
            let dict = merged
                .get_dictionary(page_id)
                .unwrap_or_else(|e| panic!("page object {page_id:?} is gone: {e}"));
            assert!(dict.has_type(b"Page"), "object {page_id:?} is not a page");
        }
    }

    #[tokio::test]
    async fn merge_keeps_page_attributes_that_were_inherited() {
        // /MediaBox and /Resources may live on an ancestor /Pages node. Merging
        // re-parents pages, so those must be copied down or the page loses its
        // size and its fonts and renders blank.
        let h = Harness::new();
        let a = h.inheriting_pdf("inherit.pdf", 2);
        let b = h.pdf("plain.pdf", 1);

        h.run(&h.job(
            PDF_MERGE_OPERATION_ID,
            &[&a, &b],
            serde_json::json!({ "outputName": "inh" }),
        ))
        .await
        .unwrap();

        let merged = Document::load(h.out_dir.join("inh.pdf")).unwrap();
        let pages = merged.get_pages();
        assert_eq!(pages.len(), 3);
        for page_number in [1u32, 2] {
            let page_id = *pages.get(&page_number).unwrap();
            let dict = merged.get_dictionary(page_id).unwrap();
            assert!(
                dict.has(b"MediaBox"),
                "page {page_number} lost its inherited MediaBox"
            );
            assert!(
                dict.has(b"Resources"),
                "page {page_number} lost its inherited Resources"
            );
        }
    }

    #[tokio::test]
    async fn extracting_one_page_does_not_carry_the_whole_document() {
        // delete_pages only unlinks; without pruning, "page 1 of 12" shipped all
        // twelve pages' content streams.
        let h = Harness::new();
        let src = h.pdf("big.pdf", 12);

        h.run(&h.job(
            PDF_PAGES_OPERATION_ID,
            &[&src],
            serde_json::json!({ "pages": "1", "outputName": "one" }),
        ))
        .await
        .unwrap();

        let whole = std::fs::metadata(&src).unwrap().len();
        let single = std::fs::metadata(h.out_dir.join("one.pdf")).unwrap().len();
        assert!(
            single < whole,
            "one extracted page ({single} bytes) should be smaller than all 12 ({whole} bytes)"
        );
    }

    #[tokio::test]
    async fn the_source_pdf_is_never_modified() {
        let h = Harness::new();
        let src = h.pdf("keep.pdf", 3);
        let before = std::fs::read(&src).unwrap();
        h.run(&h.job(PDF_SPLIT_OPERATION_ID, &[&src], serde_json::Value::Null))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&src).unwrap(), before);
    }
}
