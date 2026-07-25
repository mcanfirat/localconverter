//! Spreadsheet and structured-data conversion.
//!
//! Pure Rust — `csv`, `calamine` (read XLSX), `rust_xlsxwriter` (write XLSX),
//! `encoding_rs`, `serde_json`. One operation, `spreadsheet.convert`, moving
//! between CSV, TSV, XLSX and JSON.
//!
//! The reason this module is careful rather than a thin wrapper: the classic way
//! a "convert my CSV" tool destroys data is by helpfully turning `007` into `7`,
//! `1234567890123456` into `1.23457E+15`, or `03/04` into a date. Spec §12.2
//! forbids all of that, so the governing rule here is:
//!
//! > **Every value is preserved exactly as text unless a column is explicitly
//! > typed otherwise.** The safe default never coerces.
//!
//! Coercion happens only when the user sets a column to Number/Boolean/Date, and
//! even then a value that cannot be represented without loss is kept as text and
//! the user is warned by column — never silently mangled.

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{ConversionError, ConversionErrorCode, Result};
use crate::file::FileFormat;
use crate::job::{ConversionJob, JobProgress, JobWarning, ProgressStage};
use crate::runner::{ExecutionOutput, JobContext, StagedOutput};
use crate::validation::{OutputMetadata, ValidationCheck, ValidationReport};

/// Guard against loading an enormous sheet entirely into memory. Excel's own
/// ceiling is ~1M rows × 16k cols; this caps the product well below OOM.
const MAX_CELLS: usize = 5_000_000;
/// Beyond this many digits a bare integer is almost certainly an identifier
/// (account/card/phone), so Automatic typing keeps it as text.
const MAX_AUTONUMBER_DIGITS: usize = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TabularFormat {
    Csv,
    Tsv,
    Xlsx,
    Json,
}

impl TabularFormat {
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Tsv => "tsv",
            Self::Xlsx => "xlsx",
            Self::Json => "json",
        }
    }

    #[must_use]
    fn from_format(format: FileFormat) -> Option<Self> {
        Some(match format {
            FileFormat::Csv => Self::Csv,
            FileFormat::Tsv => Self::Tsv,
            FileFormat::Xlsx => Self::Xlsx,
            FileFormat::Json => Self::Json,
            _ => return None,
        })
    }

    fn is_delimited(self) -> bool {
        matches!(self, Self::Csv | Self::Tsv)
    }
}

/// How a column's text should be written to a typed output (XLSX/JSON).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ColumnType {
    /// Exact preservation. The default, and the only safe one for identifiers.
    #[default]
    Text,
    Number,
    Boolean,
    /// ISO `YYYY-MM-DD` only; anything else stays text with a warning.
    Date,
    /// Number when it round-trips without loss and is not identifier-shaped,
    /// boolean for true/false, otherwise text.
    Automatic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpreadsheetOptions {
    pub target_format: TabularFormat,
    /// Delimiter override for reading a CSV/TSV, as a single character. `None`
    /// means detect it.
    #[serde(default)]
    pub delimiter: Option<char>,
    /// `true` (default) treats the first row as column names.
    #[serde(default = "default_true")]
    pub has_header: bool,
    /// Which sheet to read from an XLSX. `None` reads the first.
    #[serde(default)]
    pub sheet: Option<String>,
    /// Per-column type by header name. Anything unlisted defaults to `Text`.
    #[serde(default)]
    pub column_types: std::collections::HashMap<String, ColumnType>,
    /// Write a UTF-8 BOM into CSV/TSV output (helps legacy Excel on Windows).
    #[serde(default)]
    pub write_bom: bool,
}

fn default_true() -> bool {
    true
}

impl SpreadsheetOptions {
    fn parse(options: &serde_json::Value) -> Result<Self> {
        serde_json::from_value(options.clone()).map_err(|err| {
            ConversionError::new(
                ConversionErrorCode::InvalidInput,
                format!("invalid spreadsheet options: {err}"),
            )
            .with_message_key("error.options.invalid")
        })
    }

    fn column_type(&self, header: &str) -> ColumnType {
        self.column_types.get(header).copied().unwrap_or_default()
    }
}

/// The format-neutral middle. Every cell is a string, so nothing is lost on the
/// way in; typing is applied only on the way out.
#[derive(Debug, Clone, PartialEq)]
struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    /// `true` when the header names were invented (`Column 1`, …) because the
    /// source had no header row. Writers must not emit them as a data row, and
    /// JSON says so in a warning, because they are not the user's data.
    synthetic_headers: bool,
}

impl Table {
    fn column_count(&self) -> usize {
        self.headers.len()
    }

    fn cell(&self, row: usize, col: usize) -> &str {
        self.rows
            .get(row)
            .and_then(|r| r.get(col))
            .map_or("", |s| s.as_str())
    }
}

// ---------------------------------------------------------------------------
// dispatch
// ---------------------------------------------------------------------------

pub async fn execute(job: &ConversionJob, ctx: &JobContext) -> Result<ExecutionOutput> {
    let options = SpreadsheetOptions::parse(&job.options)?;
    let Some(input) = job.input_files.first() else {
        return Err(
            ConversionError::new(ConversionErrorCode::InvalidInput, "no input files")
                .with_message_key("error.input.missing"),
        );
    };
    if job.input_files.len() > 1 {
        return Err(ConversionError::new(
            ConversionErrorCode::InvalidInput,
            "spreadsheet conversion takes one file at a time",
        )
        .with_message_key("error.input.tooMany"));
    }

    let source_format = TabularFormat::from_format(input.detected_format).ok_or_else(|| {
        ConversionError::new(
            ConversionErrorCode::UnsupportedFormat,
            format!("{:?} is not a spreadsheet format", input.detected_format),
        )
        .with_message_key("error.unsupportedFormat")
    })?;

    ctx.report(JobProgress::indeterminate(
        ProgressStage::Running,
        "progress.reading",
    ));
    ctx.check_cancelled()?;

    let path = Path::new(&input.path);
    let (table, mut warnings) = read_table(path, source_format, &options)?;

    guard_cell_count(&table)?;
    add_lossy_route_warnings(source_format, options.target_format, &mut warnings);

    ctx.report(JobProgress::indeterminate(
        ProgressStage::Running,
        "progress.writing",
    ));
    ctx.check_cancelled()?;

    let file_name = output_file_name(&input.display_name, options.target_format);
    let staged = ctx.workspace.staging_path(&file_name)?;
    let write_warnings = write_table(&table, &staged, &options)?;
    warnings.extend(write_warnings);

    let size_bytes = std::fs::metadata(&staged)
        .map_err(|err| ConversionError::from_io("stat spreadsheet output", &err))?
        .len();

    Ok(ExecutionOutput {
        outputs: vec![StagedOutput {
            staged_path: staged,
            file_name,
            format: options.target_format.extension().to_owned(),
            size_bytes,
        }],
        warnings: dedupe(warnings),
        input_total_bytes: input.size_bytes,
        // JSON repeats every column name on every row, and XLSX is a zipped
        // XML bundle with fixed overhead — both are routinely larger than a
        // terse CSV. That is the format's shape, not a problem worth flagging.
        size_growth_expected: matches!(
            options.target_format,
            TabularFormat::Json | TabularFormat::Xlsx
        ),
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
    let options = SpreadsheetOptions::parse(&job.options)?;
    let Some(staged) = execution.outputs.first() else {
        return Err(ConversionError::internal("spreadsheet produced no output"));
    };
    let Some(input) = job.input_files.first() else {
        return Err(ConversionError::internal("spreadsheet job has no input"));
    };

    // Re-read the source and the output independently and compare shape and
    // values — the spec's "parse output again, confirm counts, compare cells".
    let source_format = TabularFormat::from_format(input.detected_format)
        .ok_or_else(|| ConversionError::internal("input format vanished"))?;
    let (source_table, _) = read_table(Path::new(&input.path), source_format, &options)?;
    let (output_table, _) = read_table(&staged.staged_path, options.target_format, &options)?;

    let mut checks = Vec::new();

    checks.push(row_count_check(&source_table, &output_table));
    checks.push(column_count_check(&source_table, &output_table));
    checks.push(unicode_check(&source_table, &output_table));
    checks.push(value_preservation_check(
        &source_table,
        &output_table,
        &options,
    ));

    Ok(vec![ValidationReport::from_checks(
        options.target_format.extension(),
        checks,
        OutputMetadata {
            size_bytes: staged.size_bytes,
            properties: serde_json::json!({
                "rows": output_table.rows.len(),
                "columns": output_table.column_count(),
            }),
        },
    )])
}

// ---------------------------------------------------------------------------
// reading
// ---------------------------------------------------------------------------

fn read_table(
    path: &Path,
    format: TabularFormat,
    options: &SpreadsheetOptions,
) -> Result<(Table, Vec<JobWarning>)> {
    match format {
        TabularFormat::Csv | TabularFormat::Tsv => read_delimited(path, format, options),
        TabularFormat::Xlsx => read_xlsx(path, options),
        TabularFormat::Json => read_json(path),
    }
}

fn read_delimited(
    path: &Path,
    format: TabularFormat,
    options: &SpreadsheetOptions,
) -> Result<(Table, Vec<JobWarning>)> {
    let raw =
        std::fs::read(path).map_err(|err| ConversionError::from_io("read spreadsheet", &err))?;
    let (text, warnings) = decode_text(&raw);

    let delimiter = match options.delimiter {
        Some(c) => u8::try_from(c as u32).map_err(|_| {
            ConversionError::new(
                ConversionErrorCode::InvalidInput,
                "delimiter must be a single-byte character",
            )
            .with_message_key("error.options.invalid")
        })?,
        None if format == TabularFormat::Tsv => b'\t',
        None => detect_delimiter(&text),
    };

    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());

    let mut records: Vec<Vec<String>> = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|err| {
            ConversionError::new(
                ConversionErrorCode::CorruptedInput,
                format!("csv parse error: {err}"),
            )
            .with_message_key("error.corruptedInput")
        })?;
        records.push(record.iter().map(str::to_owned).collect());
    }

    // Exported CSVs very often end with a blank line, which the csv crate reads
    // as a final all-empty record. The XLSX reader already drops those, so this
    // one must agree or a round trip looks like it lost a row.
    while records
        .last()
        .is_some_and(|row| row.iter().all(|cell| cell.is_empty()))
    {
        records.pop();
    }

    let (headers, rows, synthetic_headers) = split_header(records, options.has_header);
    let width = headers.len();
    Ok((
        Table {
            headers,
            rows: pad_rows(rows, width),
            synthetic_headers,
        },
        warnings,
    ))
}

fn read_xlsx(path: &Path, options: &SpreadsheetOptions) -> Result<(Table, Vec<JobWarning>)> {
    use calamine::{open_workbook_auto, Reader};

    let mut workbook = open_workbook_auto(path).map_err(|err| {
        ConversionError::new(
            ConversionErrorCode::CorruptedInput,
            format!("cannot open workbook: {err}"),
        )
        .with_message_key("error.corruptedInput")
    })?;

    let sheet_names = workbook.sheet_names().to_vec();
    let mut warnings = Vec::new();
    if sheet_names.len() > 1 {
        // XLSX → CSV/TSV/JSON keeps one sheet; the user is told which.
        warnings.push(JobWarning::new("warning.spreadsheet.oneSheetOnly"));
    }
    let sheet = options
        .sheet
        .clone()
        .filter(|s| sheet_names.contains(s))
        .or_else(|| sheet_names.first().cloned())
        .ok_or_else(|| {
            ConversionError::new(
                ConversionErrorCode::CorruptedInput,
                "workbook has no sheets",
            )
            .with_message_key("error.corruptedInput")
        })?;

    let range = workbook.worksheet_range(&sheet).map_err(|err| {
        ConversionError::new(
            ConversionErrorCode::CorruptedInput,
            format!("cannot read sheet: {err}"),
        )
        .with_message_key("error.corruptedInput")
    })?;

    let mut records: Vec<Vec<String>> = Vec::new();
    for row in range.rows() {
        records.push(row.iter().map(stringify_cell).collect());
    }

    // Trim trailing fully-empty rows calamine sometimes reports.
    while records
        .last()
        .is_some_and(|r| r.iter().all(|c| c.is_empty()))
    {
        records.pop();
    }

    let (headers, rows, synthetic_headers) = split_header(records, options.has_header);
    let width = headers_len(&headers);
    Ok((
        Table {
            headers,
            rows: pad_rows(rows, width),
            synthetic_headers,
        },
        warnings,
    ))
}

/// XLSX cell → exact string. Integers stay integers (no `.0`), floats use the
/// shortest round-tripping form, dates become ISO. Excel stores everything as
/// f64, so precision beyond 2^53 was already lost in the *source* — we surface
/// that in validation rather than pretending otherwise.
fn stringify_cell(cell: &calamine::Data) -> String {
    use calamine::Data;
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => {
            if f.fract() == 0.0 && f.abs() < 9.007_199_254_740_992e15 {
                format!("{}", *f as i64)
            } else {
                format!("{f}")
            }
        }
        Data::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
        Data::DateTime(dt) => dt
            .as_datetime()
            .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| dt.as_f64().to_string()),
        Data::DateTimeIso(s) => s.clone(),
        Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("#ERROR:{e:?}"),
    }
}

fn read_json(path: &Path) -> Result<(Table, Vec<JobWarning>)> {
    let raw = std::fs::read(path).map_err(|err| ConversionError::from_io("read json", &err))?;
    let value: serde_json::Value = serde_json::from_slice(&raw).map_err(|err| {
        ConversionError::new(
            ConversionErrorCode::CorruptedInput,
            format!("invalid JSON: {err}"),
        )
        .with_message_key("error.corruptedInput")
    })?;

    let serde_json::Value::Array(items) = value else {
        return Err(ConversionError::new(
            ConversionErrorCode::InvalidInput,
            "JSON must be an array of objects (or of arrays) to become a table",
        )
        .with_message_key("error.spreadsheet.jsonNotTabular"));
    };

    // Array of objects: headers are the union of keys, ordered by first sighting.
    let mut headers: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut all_objects = true;
    for item in &items {
        match item {
            serde_json::Value::Object(map) => {
                for key in map.keys() {
                    if seen.insert(key.clone()) {
                        headers.push(key.clone());
                    }
                }
            }
            _ => all_objects = false,
        }
    }

    let rows = if all_objects {
        items
            .iter()
            .map(|item| {
                let map = item.as_object();
                headers
                    .iter()
                    .map(|h| {
                        map.and_then(|m| m.get(h))
                            .map(json_scalar)
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .collect()
    } else {
        // Array of arrays: first row is the header.
        let rows_of: Vec<Vec<String>> = items
            .iter()
            .map(|item| match item {
                serde_json::Value::Array(cells) => cells.iter().map(json_scalar).collect(),
                other => vec![json_scalar(other)],
            })
            .collect();
        let (h, r, synthetic_headers) = split_header(rows_of, true);
        headers = h;
        return Ok((
            Table {
                rows: pad_rows(r, headers.len()),
                headers,
                synthetic_headers,
            },
            Vec::new(),
        ));
    };

    Ok((
        Table {
            rows: pad_rows(rows, headers.len()),
            headers,
            synthetic_headers: false,
        },
        Vec::new(),
    ))
}

/// A JSON scalar as its exact textual form. Objects/arrays are serialised so no
/// data is dropped, though nested structure in a cell is inherently lossy.
fn json_scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// writing
// ---------------------------------------------------------------------------

fn write_table(
    table: &Table,
    staged: &Path,
    options: &SpreadsheetOptions,
) -> Result<Vec<JobWarning>> {
    match options.target_format {
        TabularFormat::Csv => write_delimited(table, staged, b',', options),
        TabularFormat::Tsv => write_delimited(table, staged, b'\t', options),
        TabularFormat::Xlsx => write_xlsx(table, staged, options),
        TabularFormat::Json => write_json(table, staged, options),
    }
}

fn write_delimited(
    table: &Table,
    staged: &Path,
    delimiter: u8,
    options: &SpreadsheetOptions,
) -> Result<Vec<JobWarning>> {
    let mut buffer: Vec<u8> = Vec::new();
    if options.write_bom {
        buffer.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    {
        let mut writer = csv::WriterBuilder::new()
            .delimiter(delimiter)
            .from_writer(&mut buffer);
        // Invented `Column N` names are not the user's data — emitting them
        // would add a row the source never had, which the row-count check would
        // then (rightly) reject.
        if !table.synthetic_headers {
            writer
                .write_record(&table.headers)
                .map_err(csv_write_error)?;
        }
        for row in &table.rows {
            writer.write_record(row).map_err(csv_write_error)?;
        }
        writer
            .flush()
            .map_err(|err| ConversionError::from_io("flush csv", &err))?;
    }
    std::fs::write(staged, &buffer).map_err(|err| ConversionError::from_io("write csv", &err))?;
    Ok(Vec::new())
}

fn write_xlsx(
    table: &Table,
    staged: &Path,
    options: &SpreadsheetOptions,
) -> Result<Vec<JobWarning>> {
    use rust_xlsxwriter::Workbook;

    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();
    let mut warnings = Vec::new();
    let mut coercion_failed_columns = std::collections::HashSet::new();

    // Only write a header row when the source actually had one; otherwise the
    // output would gain a `Column 1, Column 2, …` row the user never supplied.
    let first_data_row = if table.synthetic_headers {
        0
    } else {
        for (col, header) in table.headers.iter().enumerate() {
            let column = u16::try_from(col).map_err(|_| too_wide())?;
            sheet.write_string(0, column, header).map_err(xlsx_error)?;
        }
        1
    };

    for (row_index, row) in table.rows.iter().enumerate() {
        let excel_row = u32::try_from(row_index + first_data_row).map_err(|_| too_tall())?;
        for (col, value) in row.iter().enumerate() {
            let column = u16::try_from(col).map_err(|_| too_wide())?;
            let header = table.headers.get(col).map(String::as_str).unwrap_or("");
            let column_type = options.column_type(header);

            match typed_cell(value, column_type) {
                TypedCell::Text(s) => sheet
                    .write_string(excel_row, column, s)
                    .map_err(xlsx_error)?,
                TypedCell::Number(n) => sheet
                    .write_number(excel_row, column, n)
                    .map_err(xlsx_error)?,
                TypedCell::Bool(b) => sheet
                    .write_boolean(excel_row, column, b)
                    .map_err(xlsx_error)?,
                TypedCell::CoercionRefused(s) => {
                    // The user asked for a type this value cannot take without
                    // loss. Keep the exact text and warn — never mangle it.
                    coercion_failed_columns.insert(header.to_owned());
                    sheet
                        .write_string(excel_row, column, s)
                        .map_err(xlsx_error)?
                }
            };
        }
    }

    workbook.save(staged).map_err(xlsx_error)?;

    for column in coercion_failed_columns {
        warnings.push(JobWarning {
            message_key: "warning.spreadsheet.coercionKeptAsText".to_owned(),
            detail: Some(column),
        });
    }
    Ok(warnings)
}

fn write_json(
    table: &Table,
    staged: &Path,
    options: &SpreadsheetOptions,
) -> Result<Vec<JobWarning>> {
    // JSON objects need key names, so unlike CSV/XLSX we cannot simply omit
    // invented ones — but the user should know they were generated.
    let mut warnings = Vec::new();
    if table.synthetic_headers {
        warnings.push(JobWarning::new("warning.spreadsheet.generatedKeys"));
    }
    let mut array = Vec::with_capacity(table.rows.len());
    for row in &table.rows {
        let mut object = serde_json::Map::new();
        for (col, header) in table.headers.iter().enumerate() {
            let value = row.get(col).map(String::as_str).unwrap_or("");
            let json = match typed_cell(value, options.column_type(header)) {
                // A whole number becomes a JSON integer (`36`, not `36.0`).
                TypedCell::Number(n) if n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 => {
                    serde_json::Value::from(n as i64)
                }
                TypedCell::Number(n) => serde_json::Value::from(n),
                TypedCell::Bool(b) => serde_json::Value::Bool(b),
                TypedCell::Text(s) | TypedCell::CoercionRefused(s) => {
                    serde_json::Value::String(s.to_owned())
                }
            };
            object.insert(header.clone(), json);
        }
        array.push(serde_json::Value::Object(object));
    }

    let bytes = serde_json::to_vec_pretty(&serde_json::Value::Array(array))
        .map_err(|err| ConversionError::internal(format!("serialise json: {err}")))?;
    let mut file = std::fs::File::create(staged)
        .map_err(|err| ConversionError::from_io("create json", &err))?;
    file.write_all(&bytes)
        .map_err(|err| ConversionError::from_io("write json", &err))?;
    Ok(warnings)
}

enum TypedCell<'a> {
    Text(&'a str),
    Number(f64),
    Bool(bool),
    /// The column asked for a type this value cannot safely take.
    CoercionRefused(&'a str),
}

/// The one place coercion decisions are made, so text/XLSX/JSON output agree.
fn typed_cell(value: &str, column_type: ColumnType) -> TypedCell<'_> {
    match column_type {
        ColumnType::Text => TypedCell::Text(value),
        ColumnType::Number => match safe_number(value) {
            Some(n) => TypedCell::Number(n),
            None if value.is_empty() => TypedCell::Text(value),
            None => TypedCell::CoercionRefused(value),
        },
        ColumnType::Boolean => match parse_bool(value) {
            Some(b) => TypedCell::Bool(b),
            None if value.is_empty() => TypedCell::Text(value),
            None => TypedCell::CoercionRefused(value),
        },
        ColumnType::Date => TypedCell::Text(value), // ISO dates already sort/round-trip as text
        ColumnType::Automatic => {
            if let Some(b) = parse_bool_strict(value) {
                TypedCell::Bool(b)
            } else if let Some(n) = safe_number(value) {
                TypedCell::Number(n)
            } else {
                TypedCell::Text(value)
            }
        }
    }
}

/// A number *only* when converting to f64 and back reproduces the exact input.
/// This is what protects `007`, `1.10`, `1234567890123456789` and `+1 555` —
/// each fails the round trip and stays text.
fn safe_number(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Leading zero (but not "0" or "0.5") means a preserved identifier.
    let digits: String = trimmed.trim_start_matches(['-', '+']).to_owned();
    if digits.len() > 1 && digits.starts_with('0') && !digits.starts_with("0.") {
        return None;
    }
    // Identifier-length integers stay text even though f64 might hold them.
    if digits.chars().all(|c| c.is_ascii_digit()) && digits.len() > MAX_AUTONUMBER_DIGITS {
        return None;
    }
    let parsed: f64 = trimmed.parse().ok()?;
    if !parsed.is_finite() {
        return None;
    }
    // The decisive test: does the parsed number print back to the same string?
    // Rust's float Display is the shortest round-tripping form.
    let integer_form = if parsed.fract() == 0.0 && parsed.abs() < 9.007_199_254_740_992e15 {
        Some(format!("{}", parsed as i64))
    } else {
        None
    };
    if integer_form.as_deref() == Some(trimmed) || format!("{parsed}") == trimmed {
        Some(parsed)
    } else {
        None
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" => Some(true),
        "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

/// Automatic mode only auto-detects the unambiguous words, never `1`/`0` (those
/// are far more likely to be numbers or flags the user wants preserved).
fn parse_bool_strict(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// validation checks
// ---------------------------------------------------------------------------

fn row_count_check(source: &Table, output: &Table) -> ValidationCheck {
    if source.rows.len() == output.rows.len() {
        ValidationCheck::passed("spreadsheet.rowCount")
    } else {
        ValidationCheck::failed(
            "spreadsheet.rowCount",
            format!(
                "source has {} rows, output {}",
                source.rows.len(),
                output.rows.len()
            ),
        )
    }
}

fn column_count_check(source: &Table, output: &Table) -> ValidationCheck {
    // A table with headers but no rows is a normal export (a filtered report
    // with no matches). Written as JSON it becomes `[]`, which genuinely carries
    // no column information — the format's nature, not a lost column.
    if source.rows.is_empty() && output.rows.is_empty() {
        return ValidationCheck::passed("spreadsheet.columnCount");
    }
    if source.column_count() == output.column_count() {
        ValidationCheck::passed("spreadsheet.columnCount")
    } else {
        ValidationCheck::failed(
            "spreadsheet.columnCount",
            format!(
                "source has {} columns, output {}",
                source.column_count(),
                output.column_count()
            ),
        )
    }
}

/// Confirms a representative non-ASCII value survived, when the source has one.
fn unicode_check(source: &Table, output: &Table) -> ValidationCheck {
    let Some((r, c)) = find_non_ascii(source) else {
        return ValidationCheck::passed("spreadsheet.unicodePreserved");
    };
    if source.cell(r, c) == output.cell(r, c) {
        ValidationCheck::passed("spreadsheet.unicodePreserved")
    } else {
        ValidationCheck::failed(
            "spreadsheet.unicodePreserved",
            format!(
                "cell ({r},{c}) changed: {:?} -> {:?}",
                source.cell(r, c),
                output.cell(r, c)
            ),
        )
    }
}

/// The important one. For every text-typed column, the output value must equal
/// the source value byte-for-byte; for a numeric column, they must be
/// numerically equal. A leading zero silently dropped fails here.
fn value_preservation_check(
    source: &Table,
    output: &Table,
    options: &SpreadsheetOptions,
) -> ValidationCheck {
    let rows = source.rows.len().min(output.rows.len());
    let cols = source.column_count().min(output.column_count());

    for r in 0..rows {
        for c in 0..cols {
            let header = source.headers.get(c).map(String::as_str).unwrap_or("");
            let before = source.cell(r, c);
            let after = output.cell(r, c);
            let matches = match options.column_type(header) {
                ColumnType::Text | ColumnType::Date => before == after,
                _ => values_equal_typed(before, after),
            };
            if !matches {
                return ValidationCheck::failed(
                    "spreadsheet.valuesPreserved",
                    format!("cell ({r},{c}) changed: {before:?} -> {after:?}"),
                );
            }
        }
    }
    ValidationCheck::passed("spreadsheet.valuesPreserved")
}

/// Equal as text, or equal as numbers (so `7` == `7.0` after a Number round trip).
fn values_equal_typed(before: &str, after: &str) -> bool {
    if before == after {
        return true;
    }
    match (before.trim().parse::<f64>(), after.trim().parse::<f64>()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn find_non_ascii(table: &Table) -> Option<(usize, usize)> {
    for (r, row) in table.rows.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            if !cell.is_ascii() {
                return Some((r, c));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Detects encoding from a BOM, falling back to UTF-8. Returns the decoded text
/// and a warning if any byte could not be decoded and was replaced.
fn decode_text(raw: &[u8]) -> (String, Vec<JobWarning>) {
    let replaced = vec![JobWarning::new("warning.spreadsheet.encodingReplaced")];
    if let Some(rest) = raw.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        let (text, _, had_errors) = encoding_rs::UTF_8.decode(rest);
        return (
            text.into_owned(),
            if had_errors { replaced } else { Vec::new() },
        );
    }
    if let Some(rest) = raw.strip_prefix(&[0xFF, 0xFE]) {
        let (text, _, had_errors) = encoding_rs::UTF_16LE.decode(rest);
        return (
            text.into_owned(),
            if had_errors { replaced } else { Vec::new() },
        );
    }
    if let Some(rest) = raw.strip_prefix(&[0xFE, 0xFF]) {
        let (text, _, had_errors) = encoding_rs::UTF_16BE.decode(rest);
        return (
            text.into_owned(),
            if had_errors { replaced } else { Vec::new() },
        );
    }
    let (text, _, had_errors) = encoding_rs::UTF_8.decode(raw);
    (
        text.into_owned(),
        if had_errors { replaced } else { Vec::new() },
    )
}

/// Picks the delimiter whose per-line count is highest and most consistent
/// across the first few lines. Manual override always wins over this.
fn detect_delimiter(text: &str) -> u8 {
    const CANDIDATES: &[u8] = b",;\t|";
    let sample: Vec<&str> = text.lines().take(20).filter(|l| !l.is_empty()).collect();
    if sample.is_empty() {
        return b',';
    }
    let mut best = b',';
    let mut best_score = -1i64;
    for &candidate in CANDIDATES {
        let counts: Vec<usize> = sample
            .iter()
            .map(|line| line.bytes().filter(|b| *b == candidate).count())
            .collect();
        let min = counts.iter().copied().min().unwrap_or(0);
        // Reward columns present on every line; a delimiter that appears on one
        // line only is noise.
        let score = if min == 0 {
            0
        } else {
            (min as i64) * (counts.len() as i64)
        };
        if score > best_score {
            best_score = score;
            best = candidate;
        }
    }
    best
}

/// Splits the header row off, and widens the header to cover the widest row.
///
/// That widening matters: writers iterate the *headers*, so any cell sitting
/// past the last header would be dropped on the floor — silently deleting data
/// from a CSV that merely has a short first line (a title or preamble row is a
/// very common export shape).
fn split_header(
    mut records: Vec<Vec<String>>,
    has_header: bool,
) -> (Vec<String>, Vec<Vec<String>>, bool) {
    let widest = records.iter().map(Vec::len).max().unwrap_or(0);

    if has_header && !records.is_empty() {
        let mut headers = records.remove(0);
        // Name the surplus columns rather than truncate the rows to fit.
        for index in headers.len()..widest {
            headers.push(format!("Column {}", index + 1));
        }
        (headers, records, false)
    } else {
        let headers = (1..=widest).map(|i| format!("Column {i}")).collect();
        (headers, records, true)
    }
}

fn headers_len(headers: &[String]) -> usize {
    headers.len()
}

/// Rows shorter than the header get empty cells; rows longer keep their extra
/// cells (a ragged source is preserved, not silently truncated).
fn pad_rows(rows: Vec<Vec<String>>, width: usize) -> Vec<Vec<String>> {
    rows.into_iter()
        .map(|mut row| {
            if row.len() < width {
                row.resize(width, String::new());
            }
            row
        })
        .collect()
}

fn guard_cell_count(table: &Table) -> Result<()> {
    let cells = table.rows.len().saturating_mul(table.column_count());
    if cells > MAX_CELLS {
        return Err(ConversionError::new(
            ConversionErrorCode::InsufficientMemory,
            format!("sheet has {cells} cells, above the {MAX_CELLS} limit"),
        )
        .with_message_key("error.spreadsheet.tooLarge"));
    }
    Ok(())
}

fn add_lossy_route_warnings(
    source: TabularFormat,
    target: TabularFormat,
    warnings: &mut Vec<JobWarning>,
) {
    // XLSX carries things a flat format cannot.
    if source == TabularFormat::Xlsx && target != TabularFormat::Xlsx {
        warnings.push(JobWarning::new("warning.spreadsheet.xlsxFeaturesLost"));
    }
    // A delimited format cannot express nested JSON.
    if source == TabularFormat::Json && target.is_delimited() {
        warnings.push(JobWarning::new("warning.spreadsheet.jsonFlattened"));
    }
}

fn output_file_name(display_name: &str, target: TabularFormat) -> String {
    let stem = match display_name.rfind('.') {
        Some(i) if i > 0 => display_name.get(..i).unwrap_or(display_name),
        _ => display_name,
    };
    format!("{stem}.{}", target.extension())
}

fn dedupe(warnings: Vec<JobWarning>) -> Vec<JobWarning> {
    let mut seen = std::collections::HashSet::new();
    warnings
        .into_iter()
        .filter(|w| seen.insert((w.message_key.clone(), w.detail.clone())))
        .collect()
}

fn csv_write_error(err: csv::Error) -> ConversionError {
    ConversionError::new(
        ConversionErrorCode::ProcessFailed,
        format!("csv write error: {err}"),
    )
    .with_message_key("error.processFailed")
}

fn xlsx_error(err: rust_xlsxwriter::XlsxError) -> ConversionError {
    ConversionError::new(
        ConversionErrorCode::ProcessFailed,
        format!("xlsx error: {err}"),
    )
    .with_message_key("error.processFailed")
}

fn too_wide() -> ConversionError {
    ConversionError::new(
        ConversionErrorCode::InvalidInput,
        "too many columns for XLSX",
    )
    .with_message_key("error.spreadsheet.tooLarge")
}

fn too_tall() -> ConversionError {
    ConversionError::new(ConversionErrorCode::InvalidInput, "too many rows for XLSX")
        .with_message_key("error.spreadsheet.tooLarge")
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

        fn file(&self, name: &str, contents: &[u8]) -> std::path::PathBuf {
            let path = self.src_dir.join(name);
            std::fs::write(&path, contents).unwrap();
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

        fn job(&self, input: &Path, options: serde_json::Value) -> ConversionJob {
            ConversionJob::new(
                crate::operation::SPREADSHEET_CONVERT_OPERATION_ID,
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
    }

    #[tokio::test]
    async fn csv_to_xlsx_preserves_leading_zeros_by_default() {
        let h = Harness::new();
        // The canonical data-corruption case: 007 must not become 7.
        let src = h.file("ids.csv", b"id,name\n007,Ada\n0042,Bo\n");
        let job = h.job(&src, serde_json::json!({ "targetFormat": "xlsx" }));
        let result = h.run(&job).await.unwrap();
        assert!(
            result.validation_reports[0].valid,
            "{:?}",
            result.validation_reports[0].failed_checks()
        );

        // Re-read the produced XLSX independently: still "007".
        let out = h.out_dir.join("ids.xlsx");
        let (table, _) = read_xlsx(
            &out,
            &SpreadsheetOptions::parse(&serde_json::json!({ "targetFormat": "xlsx" })).unwrap(),
        )
        .unwrap();
        assert_eq!(table.cell(0, 0), "007");
        assert_eq!(table.cell(1, 0), "0042");
    }

    #[tokio::test]
    async fn a_long_identifier_is_not_turned_into_scientific_notation() {
        let h = Harness::new();
        let src = h.file("cards.csv", b"card\n1234567890123456\n");
        // Even with Automatic typing, a 16-digit integer stays text.
        let job = h.job(
            &src,
            serde_json::json!({
                "targetFormat": "xlsx",
                "columnTypes": { "card": "automatic" }
            }),
        );
        let result = h.run(&job).await.unwrap();
        assert!(result.validation_reports[0].valid);
        let out = h.out_dir.join("cards.xlsx");
        let (table, _) = read_xlsx(
            &out,
            &SpreadsheetOptions::parse(&serde_json::json!({ "targetFormat": "xlsx" })).unwrap(),
        )
        .unwrap();
        assert_eq!(table.cell(0, 0), "1234567890123456");
    }

    #[tokio::test]
    async fn an_explicit_number_column_becomes_real_numbers() {
        let h = Harness::new();
        let src = h.file("n.csv", b"qty,price\n3,9.5\n10,100\n");
        let job = h.job(
            &src,
            serde_json::json!({
                "targetFormat": "xlsx",
                "columnTypes": { "qty": "number", "price": "number" }
            }),
        );
        let result = h.run(&job).await.unwrap();
        assert!(result.validation_reports[0].valid);

        use calamine::{open_workbook_auto, Data, Reader};
        let mut wb = open_workbook_auto(h.out_dir.join("n.xlsx")).unwrap();
        let sheet = wb.sheet_names()[0].clone();
        let range = wb.worksheet_range(&sheet).unwrap();
        // Row 1 (after header), col 0 should be a numeric 3, not the string "3".
        assert!(matches!(
            range.get((1, 0)),
            Some(Data::Int(3)) | Some(Data::Float(_))
        ));
    }

    #[tokio::test]
    async fn a_number_column_with_an_identifier_keeps_it_as_text_and_warns() {
        let h = Harness::new();
        let src = h.file("mix.csv", b"code\n007\n");
        let job = h.job(
            &src,
            serde_json::json!({
                "targetFormat": "xlsx",
                "columnTypes": { "code": "number" }
            }),
        );
        let result = h.run(&job).await.unwrap();
        // 007 cannot become a number without losing the zero, so it stays text.
        let out = h.out_dir.join("mix.xlsx");
        let (table, _) = read_xlsx(
            &out,
            &SpreadsheetOptions::parse(&serde_json::json!({ "targetFormat": "xlsx" })).unwrap(),
        )
        .unwrap();
        assert_eq!(table.cell(0, 0), "007");
        assert!(result
            .warnings
            .iter()
            .any(|w| w.message_key == "warning.spreadsheet.coercionKeptAsText"));
    }

    #[tokio::test]
    async fn xlsx_to_csv_round_trips_values() {
        let h = Harness::new();
        // Build an XLSX from CSV first, then convert it back.
        let csv = h.file("start.csv", b"id,label\n007,\xC3\xA9lan\n");
        h.run(&h.job(&csv, serde_json::json!({ "targetFormat": "xlsx" })))
            .await
            .unwrap();
        let xlsx = h.out_dir.join("start.xlsx");

        let h2 = Harness::new();
        let result = h2
            .run(&h2.job(&xlsx, serde_json::json!({ "targetFormat": "csv" })))
            .await
            .unwrap();
        assert!(result.validation_reports[0].valid);
        let text = std::fs::read_to_string(h2.out_dir.join("start.csv")).unwrap();
        assert!(text.contains("007"), "leading zero lost: {text}");
        assert!(text.contains("élan"), "unicode lost: {text}");
    }

    #[tokio::test]
    async fn csv_to_json_produces_array_of_objects() {
        let h = Harness::new();
        let src = h.file("p.csv", b"name,age\nAda,36\nBo,29\n");
        let job = h.job(
            &src,
            serde_json::json!({
                "targetFormat": "json",
                "columnTypes": { "age": "number" }
            }),
        );
        h.run(&job).await.unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(h.out_dir.join("p.json")).unwrap()).unwrap();
        assert_eq!(value[0]["name"], "Ada");
        assert_eq!(value[0]["age"], 36);
        assert_eq!(value[1]["name"], "Bo");
    }

    #[tokio::test]
    async fn json_to_csv_flattens_and_warns() {
        let h = Harness::new();
        let src = h.file(
            "d.json",
            r#"[{"id":"007","city":"Zürich"},{"id":"008","city":"Paris"}]"#.as_bytes(),
        );
        let result = h
            .run(&h.job(&src, serde_json::json!({ "targetFormat": "csv" })))
            .await
            .unwrap();
        assert!(result.validation_reports[0].valid);
        let text = std::fs::read_to_string(h.out_dir.join("d.csv")).unwrap();
        assert!(text.contains("007") && text.contains("Zürich"));
        assert!(result
            .warnings
            .iter()
            .any(|w| w.message_key == "warning.spreadsheet.jsonFlattened"));
    }

    #[tokio::test]
    async fn semicolon_delimiter_is_detected() {
        let h = Harness::new();
        let src = h.file("euro.csv", b"a;b;c\n1;2;3\n4;5;6\n");
        h.run(&h.job(&src, serde_json::json!({ "targetFormat": "json" })))
            .await
            .unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(h.out_dir.join("euro.json")).unwrap()).unwrap();
        // Three columns detected, not one wide column.
        assert_eq!(value[0].as_object().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn a_manual_delimiter_overrides_detection() {
        let h = Harness::new();
        let src = h.file("pipe.csv", b"a|b\nx|y\n");
        h.run(&h.job(
            &src,
            serde_json::json!({ "targetFormat": "json", "delimiter": "|" }),
        ))
        .await
        .unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(h.out_dir.join("pipe.json")).unwrap()).unwrap();
        assert_eq!(value[0]["a"], "x");
    }

    #[tokio::test]
    async fn utf16_with_a_bom_is_decoded() {
        let h = Harness::new();
        // "id,name\n1,café\n" as UTF-16LE with BOM.
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "id,name\n1,café\n".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let src = h.file("wide.csv", &bytes);
        h.run(&h.job(&src, serde_json::json!({ "targetFormat": "json" })))
            .await
            .unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(h.out_dir.join("wide.json")).unwrap()).unwrap();
        assert_eq!(value[0]["name"], "café");
    }

    #[tokio::test]
    async fn the_source_file_is_never_modified() {
        let h = Harness::new();
        let src = h.file("keep.csv", b"a,b\n1,2\n");
        let before = std::fs::read(&src).unwrap();
        h.run(&h.job(&src, serde_json::json!({ "targetFormat": "xlsx" })))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&src).unwrap(), before);
    }

    #[tokio::test]
    async fn a_non_tabular_json_is_declined() {
        let h = Harness::new();
        let src = h.file("obj.json", br#"{"not":"an array"}"#);
        let err = h
            .run(&h.job(&src, serde_json::json!({ "targetFormat": "csv" })))
            .await
            .unwrap_err();
        assert_eq!(err.message_key, "error.spreadsheet.jsonNotTabular");
    }

    #[test]
    fn safe_number_protects_the_dangerous_shapes() {
        // These must all stay text.
        for identifier in [
            "007",
            "0042",
            "1234567890123456",
            "+15551234567",
            "1.10",
            "00",
        ] {
            assert!(
                safe_number(identifier).is_none(),
                "{identifier} should not be a number"
            );
        }
        // These are genuinely numbers.
        for number in ["0", "7", "9.5", "-3", "0.5", "100"] {
            assert!(safe_number(number).is_some(), "{number} should be a number");
        }
    }

    #[tokio::test]
    async fn cells_past_the_header_width_are_not_dropped() {
        // A short first line (a title/preamble row) used to make the table one
        // column wide, silently deleting every other cell on every row.
        let h = Harness::new();
        let src = h.file("ragged.csv", b"title\na,b,c\nd,e,f\n");
        h.run(&h.job(&src, serde_json::json!({ "targetFormat": "json" })))
            .await
            .unwrap();

        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(h.out_dir.join("ragged.json")).unwrap()).unwrap();
        let first = value[0].as_object().unwrap();
        assert_eq!(first.len(), 3, "all three cells must survive: {first:?}");
        let kept: Vec<&str> = first.values().filter_map(|v| v.as_str()).collect();
        for cell in ["a", "b", "c"] {
            assert!(kept.contains(&cell), "cell {cell} was dropped: {kept:?}");
        }
    }

    #[tokio::test]
    async fn a_headerless_source_converts_without_inventing_a_data_row() {
        // With hasHeader=false the writers used to emit the synthesised
        // `Column 1,…` names as a real row, so the re-read saw N+1 rows and the
        // job was rejected — making three of four targets unreachable.
        let h = Harness::new();
        let src = h.file("nohead.csv", b"a,1\nb,2\n");

        for target in ["csv", "tsv", "xlsx", "json"] {
            let out = Harness::new();
            let job = out.job(
                &src,
                serde_json::json!({ "targetFormat": target, "hasHeader": false }),
            );
            let result = out
                .run(&job)
                .await
                .unwrap_or_else(|e| panic!("{target} failed: {e:?}"));
            assert!(result.validation_reports[0].valid, "{target}");
        }

        // And the delimited output must not have gained a header line.
        let out = Harness::new();
        out.run(&out.job(
            &src,
            serde_json::json!({ "targetFormat": "csv", "hasHeader": false }),
        ))
        .await
        .unwrap();
        let text = std::fs::read_to_string(out.out_dir.join("nohead.csv")).unwrap();
        assert!(
            !text.contains("Column 1"),
            "invented header leaked into the data: {text}"
        );
    }

    #[test]
    fn output_names_swap_the_extension() {
        assert_eq!(
            output_file_name("data.csv", TabularFormat::Xlsx),
            "data.xlsx"
        );
        assert_eq!(
            output_file_name("report.xlsx", TabularFormat::Json),
            "report.json"
        );
    }
}
