//! CSV → XLSX via the pure-Rust `rust_xlsxwriter`.
//!
//! Numbers are written as numbers (so spreadsheet apps can compute on them),
//! everything else as strings; the header row is bold with autofit columns.

use rust_xlsxwriter::{Format, Workbook};

use crate::RenderError;

/// Convert CSV text to a .xlsx file (bytes). Simple comma-splitting with
/// double-quote awareness — agent-produced CSV, not arbitrary dialects.
pub fn csv_to_xlsx(csv: &str) -> Result<Vec<u8>, RenderError> {
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();
    let bold = Format::new().set_bold();

    for (r, line) in csv.lines().filter(|l| !l.trim().is_empty()).enumerate() {
        for (c, cell) in split_csv_line(line).iter().enumerate() {
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
    sheet
        .autofit();

    workbook
        .save_to_buffer()
        .map_err(|e| RenderError::Export(format!("xlsx save: {e}")))
}

/// Split one CSV line honoring double-quoted fields (with "" escapes).
fn split_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => fields.push(std::mem::take(&mut field).trim().to_string()),
            _ => field.push(ch),
        }
    }
    fields.push(field.trim().to_string());
    fields
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
    fn quoted_fields_split_correctly() {
        let fields = split_csv_line("\"a, b\",c,\"d \"\"e\"\"\"");
        assert_eq!(fields, vec!["a, b", "c", "d \"e\""]);
    }
}
