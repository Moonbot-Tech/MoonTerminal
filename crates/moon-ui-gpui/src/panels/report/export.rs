//! Report file export as semicolon-delimited CSV with a UTF-8 BOM or as a real Excel workbook
//! produced by `rust_xlsxwriter`. Export runs its own SQLite query using the panel's CURRENT filter
//! (period preset or manual From/To dates, cores, coin, side, and emulator) and current sort. It does
//! not use the table's `MAX_REPORT_ROWS` cap: the panel shows the top 100 rows, while export writes
//! every matching row up to [`EXPORT_MAX_ROWS`]. Cell values retain their RAW database
//! representation (`isshort` and `emulator` remain 0/1, and numbers receive no presentation
//! formatting or localized value labels), making this a machine-readable extract rather than a
//! snapshot of the table. Date columns are formatted as `YYYY-MM-DD HH:MM` UTC text, matching the
//! table. In XLSX, large identifiers in `strategyid`, `exorderid`, `taskid`, and `newrecid` are also
//! written as text so Excel preserves every digit.

use std::path::{Path, PathBuf};

use rusqlite::types::Value;

use super::columns;
use moon_core::db::{self, ReportFilter};

/// Caps an otherwise complete filtered export to protect against enormous result sets; reaching the
/// cap is logged because the database may accumulate hundreds of thousands of rows over the years.
const EXPORT_MAX_ROWS: usize = 200_000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Format {
    Csv,
    Xlsx,
}

impl Format {
    pub(super) fn ext(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Xlsx => "xlsx",
        }
    }
}

/// Builds the suggested filename from the filter period, such as `report_2026-07-13.csv` or
/// `report_2026-07-01_2026-07-10.xlsx`; an undated export becomes `report_all.csv`.
///
/// Exporting the full runtime report schema adds the `-All` suffix, for example
/// `report_2026-07-13-All.csv`, distinguishing it from a visible-column export on disk.
pub(super) fn suggested_name(filter: &ReportFilter, fmt: Format, all_cols: bool) -> String {
    let day = |secs: i64| {
        let full = db::fmt_unix(secs);
        full.get(..10).unwrap_or(&full).to_string()
    };
    let range = match (filter.date_from, filter.date_to) {
        (Some(f), Some(t)) => format!("{}_{}", day(f), day(t)),
        (Some(f), None) => day(f),
        (None, Some(t)) => format!("_{}", day(t)),
        (None, None) => "all".to_string(),
    };
    let all = if all_cols { "-All" } else { "" };
    format!("report_{range}{all}.{}", fmt.ext())
}

/// Returns the user's profile or home directory for the save dialog, falling back to the current
/// directory.
pub(super) fn default_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Run a blocking export and return the number of data rows written.
///
/// `cols` preserves table order. The full query completes before a CSV/XLSX
/// writer opens; both `NotReady` and `Failed` abort without writing export bytes
/// because a partial file would look complete. Call from a background executor.
pub(super) fn run(
    path: &Path,
    fmt: Format,
    cols: &[String],
    filter: &ReportFilter,
    sort_key: &str,
    sort_desc: bool,
) -> anyhow::Result<usize> {
    let conn = db::open_reader().map_err(|e| anyhow::anyhow!("БД отчётов недоступна: {e}"))?;
    let table = db::query_reports(&conn, filter, sort_key, sort_desc, EXPORT_MAX_ROWS)
        .map_err(|e| anyhow::anyhow!("выборка отчёта не прочитана: {e}"))?;
    if table.rows.len() == EXPORT_MAX_ROWS {
        log::warn!("отчёт: экспорт упёрся в кап {EXPORT_MAX_ROWS} строк — выборка усечена");
    }
    // Map requested columns to query-row indices while preserving request order.
    let idx: Vec<(usize, &str)> = cols
        .iter()
        .filter_map(|name| {
            table
                .cols
                .iter()
                .position(|c| c == name)
                .map(|i| (i, name.as_str()))
        })
        .collect();
    if idx.is_empty() {
        anyhow::bail!("нет колонок для экспорта");
    }
    match fmt {
        Format::Csv => write_csv(path, &idx, &table.rows)?,
        Format::Xlsx => write_xlsx(path, &idx, &table.rows)?,
    }
    Ok(table.rows.len())
}

/// Writes semicolon-delimited CSV with a UTF-8 BOM and CRLF line endings for Excel compatibility.
///
/// A Russian-locale Excel installation accepts the delimiter without its import wizard, while the
/// BOM preserves Cyrillic text. Fields containing semicolons, quotes, or line breaks are quoted.
fn write_csv(path: &Path, idx: &[(usize, &str)], rows: &[Vec<Value>]) -> anyhow::Result<()> {
    let mut out = String::with_capacity(64 * (rows.len() + 1));
    out.push('\u{FEFF}');
    let header: Vec<String> = idx
        .iter()
        .map(|(_, name)| csv_field(&columns::header_for(name)))
        .collect();
    out.push_str(&header.join(";"));
    out.push_str("\r\n");
    for row in rows {
        let mut first = true;
        for (i, name) in idx {
            if !first {
                out.push(';');
            }
            first = false;
            let val = row.get(*i).unwrap_or(&Value::Null);
            out.push_str(&csv_field(&field_text(name, val)));
        }
        out.push_str("\r\n");
    }
    std::fs::write(path, out)?;
    Ok(())
}

/// Returns whether a database column stores Unix seconds that export formats as UTC date text.
fn date_col(col: &str) -> bool {
    matches!(
        col,
        "buydate" | "closedate" | "sellsetdate" | "last_update_at"
    )
}

/// Returns whether an identifier column must be written to XLSX as text.
///
/// Excel displays large `i64` numbers in scientific notation and loses precision. These values are
/// identifiers rather than amounts, so text preserves every digit.
fn id_text_col(col: &str) -> bool {
    matches!(col, "strategyid" | "exorderid" | "taskid" | "newrecid")
}

/// Converts a database value to CSV text, formatting dates while leaving other values raw.
///
/// `isshort` and `emulator` remain 0/1, numbers use their `Display` form, and NULL becomes an empty
/// field. Report queries do not contain blobs.
fn field_text(col: &str, v: &Value) -> String {
    if date_col(col) {
        return match v {
            Value::Integer(i) => db::fmt_unix(*i),
            Value::Real(r) => db::fmt_unix(*r as i64),
            _ => String::new(),
        };
    }
    match v {
        Value::Null => String::new(),
        Value::Integer(i) => i.to_string(),
        Value::Real(r) => r.to_string(),
        Value::Text(t) => t.clone(),
        Value::Blob(_) => String::new(),
    }
}

fn csv_field(s: &str) -> String {
    if s.contains([';', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Writes XLSX with bold headers and automatically fitted columns.
///
/// Database Integer and Real values become numbers, Text remains text, and NULL leaves an empty
/// cell. Dates become formatted strings, and large identifier columns remain text to preserve them.
fn write_xlsx(path: &Path, idx: &[(usize, &str)], rows: &[Vec<Value>]) -> anyhow::Result<()> {
    use rust_xlsxwriter::{Format as XFormat, Workbook};

    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    let bold = XFormat::new().set_bold();
    for (c, (_, name)) in idx.iter().enumerate() {
        ws.write_string_with_format(0, c as u16, columns::header_for(name), &bold)?;
    }
    for (r, row) in rows.iter().enumerate() {
        let r = (r + 1) as u32;
        for (c, (i, name)) in idx.iter().enumerate() {
            let c = c as u16;
            let val = row.get(*i).unwrap_or(&Value::Null);
            if date_col(name) {
                let text = field_text(name, val);
                if !text.is_empty() {
                    ws.write_string(r, c, text)?;
                }
                continue;
            }
            // Write identifier columns as text so Excel cannot convert large i64 values to imprecise
            // scientific notation.
            if id_text_col(name) {
                if let Value::Integer(n) = val {
                    ws.write_string(r, c, n.to_string())?;
                    continue;
                }
            }
            match val {
                Value::Integer(n) => {
                    ws.write_number(r, c, *n as f64)?;
                }
                Value::Real(n) => {
                    ws.write_number(r, c, *n)?;
                }
                Value::Text(t) => {
                    ws.write_string(r, c, t)?;
                }
                Value::Null | Value::Blob(_) => {}
            }
        }
    }
    ws.autofit();
    wb.save(path)?;
    Ok(())
}
