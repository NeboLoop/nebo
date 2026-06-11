//! Markdown → DOCX via the pure-Rust `docx-rs` OOXML writer.
//!
//! Same doctrine as the PDF path: documents generate in-process, identically
//! on every platform, never via host binaries. Covers the report subset —
//! headings, paragraphs, bold/italic/code runs, nested lists, tables, quotes —
//! with a styled treatment (colored headings, shaded table headers, banded
//! rows, modern line spacing) so the output presents like a designed document,
//! not typewriter text.

use docx_rs::{
    AlignmentType, BorderType, Docx, IndentLevel, LineSpacing, LineSpacingType, NumberingId,
    Paragraph, Run, RunFonts, Shading, ShdType, Table, TableBorder, TableBorderPosition,
    TableBorders, TableCell, TableRow, WidthType,
};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::RenderError;

/// Heading font sizes in half-points (docx unit), h1..h6.
const HEADING_SIZES: [usize; 6] = [48, 38, 32, 28, 26, 24];
const BODY_SIZE: usize = 22;
/// Brand accent for headings and table headers (Nebo teal).
const ACCENT: &str = "0077A8";
/// Banded table rows + subtle borders.
const BAND_FILL: &str = "EFF5F9";
const BORDER_GRAY: &str = "D7DDE3";

#[derive(Default, Clone, Copy)]
struct RunStyle {
    bold: bool,
    italic: bool,
    code: bool,
    color: Option<&'static str>,
}

/// Convert Markdown to a .docx file (bytes).
pub fn markdown_to_docx(markdown: &str) -> Result<Vec<u8>, RenderError> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);

    let mut docx = Docx::new();

    // Current paragraph under construction + its runs.
    let mut para = Paragraph::new();
    let mut para_has_content = false;
    let mut style = RunStyle::default();
    // Heading level for the paragraph being built (None = body).
    let mut heading: Option<usize> = None;
    // List nesting: depth + ordered flag (rendered as indented bullet/number text).
    let mut lists: Vec<bool> = Vec::new();
    // Table assembly.
    let mut in_table = false;
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut in_block_quote = false;

    fn styled_run(text: &str, style: RunStyle, size: usize) -> Run {
        let mut run = Run::new().add_text(text).size(size);
        if style.bold {
            run = run.bold();
        }
        if style.italic {
            run = run.italic();
        }
        if style.code {
            run = run.fonts(RunFonts::new().ascii("Courier New"));
        }
        if let Some(color) = style.color {
            run = run.color(color);
        }
        run
    }

    // Headings get air above and a little below; body gets 1.15 line spacing
    // with space after — the modern document rhythm instead of typewriter walls.
    macro_rules! flush_para {
        () => {
            if para_has_content {
                let spacing = if heading.is_some() {
                    LineSpacing::new().before(280).after(120)
                } else {
                    LineSpacing::new()
                        .after(140)
                        .line(276)
                        .line_rule(LineSpacingType::Auto)
                };
                docx = docx.add_paragraph(para.line_spacing(spacing));
                para = Paragraph::new();
                para_has_content = false;
            } else {
                para = Paragraph::new();
            }
        };
    }

    for event in Parser::new_ext(markdown, opts) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    flush_para!();
                    heading = Some(match level {
                        HeadingLevel::H1 => 0,
                        HeadingLevel::H2 => 1,
                        HeadingLevel::H3 => 2,
                        HeadingLevel::H4 => 3,
                        HeadingLevel::H5 => 4,
                        HeadingLevel::H6 => 5,
                    });
                    style.bold = true;
                    style.color = Some(ACCENT);
                }
                Tag::Paragraph => {
                    if !in_table {
                        flush_para!();
                    }
                }
                Tag::Strong => style.bold = true,
                Tag::Emphasis => style.italic = true,
                Tag::CodeBlock(_) => {
                    flush_para!();
                    style.code = true;
                }
                Tag::List(ordered) => lists.push(ordered.is_some()),
                Tag::Item => {
                    flush_para!();
                    let depth = lists.len().saturating_sub(1);
                    // Word's default numbering definition id 1 = bullets; keep
                    // ordered lists as plain numbered text for fidelity without
                    // a numbering.xml definition.
                    if lists.last() == Some(&false) {
                        para = para
                            .numbering(NumberingId::new(1), IndentLevel::new(depth));
                    } else {
                        para = para.indent(Some(720 * (depth as i32 + 1)), None, None, None);
                    }
                }
                Tag::BlockQuote(_) => {
                    flush_para!();
                    in_block_quote = true;
                    style.italic = true;
                }
                Tag::Table(_) => {
                    flush_para!();
                    in_table = true;
                    table_rows.clear();
                }
                Tag::TableRow | Tag::TableHead => current_row = Vec::new(),
                Tag::TableCell => {}
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Heading(_) => {
                    flush_para!();
                    heading = None;
                    style.bold = false;
                    style.color = None;
                }
                TagEnd::Paragraph => {
                    if !in_table {
                        flush_para!();
                    }
                }
                TagEnd::Strong => style.bold = false,
                TagEnd::Emphasis => style.italic = false,
                TagEnd::CodeBlock => {
                    flush_para!();
                    style.code = false;
                }
                TagEnd::List(_) => {
                    lists.pop();
                }
                TagEnd::Item => flush_para!(),
                TagEnd::BlockQuote(_) => {
                    flush_para!();
                    in_block_quote = false;
                    style.italic = false;
                }
                TagEnd::TableHead | TagEnd::TableRow => {
                    if !current_row.is_empty() {
                        table_rows.push(std::mem::take(&mut current_row));
                    }
                }
                TagEnd::Table => {
                    in_table = false;
                    if !table_rows.is_empty() {
                        docx = docx.add_table(build_table(std::mem::take(&mut table_rows)));
                        // Breathing room below the table.
                        docx = docx.add_paragraph(
                            Paragraph::new()
                                .line_spacing(LineSpacing::new().after(120))
                                .add_run(Run::new().add_text("").size(2)),
                        );
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                if in_table {
                    current_row.push(t.to_string());
                } else {
                    let size = heading.map(|h| HEADING_SIZES[h]).unwrap_or(BODY_SIZE);
                    let _ = in_block_quote;
                    para = para.add_run(styled_run(&t, style, size));
                    para_has_content = true;
                }
            }
            Event::Code(c) => {
                if in_table {
                    current_row.push(c.to_string());
                } else {
                    let size = heading.map(|h| HEADING_SIZES[h]).unwrap_or(BODY_SIZE);
                    let s = RunStyle { code: true, ..style };
                    para = para.add_run(styled_run(&c, s, size));
                    para_has_content = true;
                }
            }
            Event::SoftBreak => {
                para = para.add_run(styled_run(" ", style, BODY_SIZE));
            }
            Event::HardBreak => {
                flush_para!();
            }
            Event::Rule => {
                flush_para!();
                docx = docx.add_paragraph(
                    Paragraph::new()
                        .align(AlignmentType::Center)
                        .add_run(Run::new().add_text("— · —").size(BODY_SIZE).color(BORDER_GRAY)),
                );
            }
            _ => {}
        }
    }
    if para_has_content {
        docx = docx.add_paragraph(para);
    }

    let mut buf = std::io::Cursor::new(Vec::new());
    docx.build()
        .pack(&mut buf)
        .map_err(|e| RenderError::Export(format!("docx pack: {e}")))?;
    Ok(buf.into_inner())
}

/// Full-width table: accent-filled header row with white bold text, banded
/// data rows, hairline gray borders.
fn build_table(rows: Vec<Vec<String>>) -> Table {
    let table_rows: Vec<TableRow> = rows
        .into_iter()
        .enumerate()
        .map(|(i, cells)| {
            TableRow::new(
                cells
                    .into_iter()
                    .map(|c| {
                        let mut run = Run::new().add_text(c).size(BODY_SIZE);
                        let mut cell = TableCell::new();
                        if i == 0 {
                            run = run.bold().color("FFFFFF");
                            cell = cell.shading(Shading::new().shd_type(ShdType::Clear).fill(ACCENT));
                        } else if i % 2 == 0 {
                            cell = cell.shading(Shading::new().shd_type(ShdType::Clear).fill(BAND_FILL));
                        }
                        cell.add_paragraph(
                            Paragraph::new()
                                .line_spacing(LineSpacing::new().before(60).after(60))
                                .add_run(run),
                        )
                    })
                    .collect(),
            )
        })
        .collect();

    let mut borders = TableBorders::new();
    for pos in [
        TableBorderPosition::Top,
        TableBorderPosition::Bottom,
        TableBorderPosition::Left,
        TableBorderPosition::Right,
        TableBorderPosition::InsideH,
        TableBorderPosition::InsideV,
    ] {
        borders = borders.set(
            TableBorder::new(pos)
                .border_type(BorderType::Single)
                .size(4)
                .color(BORDER_GRAY),
        );
    }

    Table::new(table_rows)
        .width(5000, WidthType::Pct)
        .set_borders(borders)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_renders_to_docx() {
        let md = "# Title\n\nSome **bold** and _italic_ with `code`.\n\n\
                  - bullet one\n- bullet two\n\n\
                  | H1 | H2 |\n|----|----|\n| a | b |\n| c | d |\n";
        let bytes = markdown_to_docx(md).expect("docx");
        // DOCX is a ZIP container: PK magic.
        assert_eq!(&bytes[..2], b"PK");
        assert!(bytes.len() > 1000);
    }
}
