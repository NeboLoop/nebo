//! CSV → XLSX via the pure-Rust `rust_xlsxwriter`.
//!
//! Numbers are written as numbers (so spreadsheet apps can compute on them),
//! everything else as strings; the header row is bold with autofit columns.

use rust_xlsxwriter::{Format, Workbook};

use crate::RenderError;

/// Convert CSV text to a .xlsx file (bytes). RFC 4180 parsing — quoted fields
/// may contain commas, `""` escapes, and newlines. Malformed input where
/// records were glued onto one line (a row carrying a multiple of the header's
/// column count) is rejected with a corrective message rather than rendered
/// as one giant row.
pub fn csv_to_xlsx(csv: &str) -> Result<Vec<u8>, RenderError> {
    let records = parse_csv(csv);
    if let Some(header) = records.first() {
        let cols = header.len();
        if cols > 1 {
            if let Some(bad) = records.iter().skip(1).find(|r| r.len() >= cols * 2) {
                return Err(RenderError::Input(format!(
                    "CSV is malformed: a row has {} fields but the header defines {cols} columns — \
                     multiple records appear to be joined on one line. Rewrite the .csv with exactly \
                     one record per line (rows separated by newlines), then convert again",
                    bad.len(),
                )));
            }
        }
    }

    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();
    let bold = Format::new().set_bold();

    for (r, record) in records.iter().enumerate() {
        for (c, cell) in record.iter().enumerate() {
            let row = r as u32;
            let col = c as u16;
            let write = if r == 0 {
                sheet.write_string_with_format(row, col, cell, &bold).map(|_| ())
            } else if let Ok(n) = cell.parse::<f64>() {
                sheet.write_number(row, col, n).map(|_| ())
            } else {
                sheet.write_string(row, col, cell).map(|_| ())
            };
            write.map_err(|e| RenderError::Export(format!("xlsx write: {e}")))?;
        }
    }
    sheet.autofit();

    workbook
        .save_to_buffer()
        .map_err(|e| RenderError::Export(format!("xlsx save: {e}")))
}

/// Parse CSV text into records, honoring double-quoted fields (`""` escapes,
/// embedded commas and newlines). Blank lines are skipped.
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut records: Vec<Vec<String>> = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => record.push(std::mem::take(&mut field).trim().to_string()),
            '\r' if !in_quotes => {} // CRLF: the '\n' that follows ends the record
            '\n' if !in_quotes => {
                record.push(std::mem::take(&mut field).trim().to_string());
                if record.iter().any(|f| !f.is_empty()) {
                    records.push(std::mem::take(&mut record));
                } else {
                    record.clear();
                }
            }
            _ => field.push(ch),
        }
    }
    if !field.trim().is_empty() || !record.is_empty() {
        record.push(field.trim().to_string());
        records.push(record);
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_renders_to_xlsx() {
        let csv = "Name,Qty,Price\nWidget,3,9.99\n\"Gizmo, large\",12,149.5\n";
        let bytes = csv_to_xlsx(csv).expect("xlsx");
        assert_eq!(&bytes[..2], b"PK"); // xlsx is a ZIP container
        assert!(bytes.len() > 500);
    }

    #[test]
    fn quoted_fields_parse_correctly() {
        let rows = parse_csv("\"a, b\",c,\"d \"\"e\"\"\"\nx,y,z\n");
        assert_eq!(rows[0], vec!["a, b", "c", "d \"e\""]);
        assert_eq!(rows[1], vec!["x", "y", "z"]);
    }

    #[test]
    fn quoted_newlines_stay_in_one_record() {
        let rows = parse_csv("Name,Notes\nWidget,\"line one\nline two\"\n");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1][1], "line one\nline two");
    }

    #[test]
    fn glued_records_are_rejected_with_correction() {
        // Five 5-field records joined onto a single line (real failure mode).
        let csv = "Name,Size,Difficulty,Rules,Description\n\
                   A,\"9x9\",\"Easy\",\"r1\",\"d1\",\"B\",\"9x9\",\"Hard\",\"r2\",\"d2\"\n";
        let err = csv_to_xlsx(csv).expect_err("must reject glued records");
        let msg = err.to_string();
        assert!(msg.contains("one record per line"), "corrective message: {msg}");
    }
}
