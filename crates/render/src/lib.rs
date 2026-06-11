//! Document rendering — the canonical PDF engine.
//!
//! Markdown (the agent's document source of truth) compiles to PDF through the
//! embedded Typst typesetter: pure Rust, embedded fonts, identical output on
//! macOS/Windows/Linux. Document conversion must NEVER depend on host binaries
//! (wkhtmltopdf is abandoned upstream) or on the bundled Obscura browser (a
//! DOM+JS automation browser with no layout engine — it cannot print).

use std::sync::OnceLock;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use typst::diag::{FileError, FileResult};
use typst::{Library, LibraryExt as _};
use typst::foundations::{Bytes, Datetime};
use typst::layout::PagedDocument;
use typst::syntax::{FileId, Source, VirtualPath};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst_kit::fonts::{FontSearcher, Fonts};

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("typst compile failed: {0}")]
    Compile(String),
    #[error("pdf export failed: {0}")]
    Export(String),
}

/// Embedded fonts + font book, loaded once per process (search is not free).
fn fonts() -> &'static Fonts {
    static FONTS: OnceLock<Fonts> = OnceLock::new();
    FONTS.get_or_init(|| FontSearcher::new().include_system_fonts(false).search())
}

/// Minimal single-file Typst world: one in-memory source, embedded fonts,
/// no package or filesystem access.
struct DocWorld {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    source: Source,
}

impl DocWorld {
    fn new(typst_source: String) -> Self {
        Self {
            library: LazyHash::new(Library::default()),
            book: LazyHash::new(fonts().book.clone()),
            source: Source::new(
                FileId::new(None, VirtualPath::new("/document.typ")),
                typst_source,
            ),
        }
    }
}

impl typst::World for DocWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.source.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.source.id() {
            Ok(self.source.clone())
        } else {
            Err(FileError::NotFound(id.vpath().as_rootless_path().into()))
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        Err(FileError::NotFound(id.vpath().as_rootless_path().into()))
    }

    fn font(&self, index: usize) -> Option<Font> {
        fonts().fonts.get(index)?.get()
    }

    fn today(&self, _offset: Option<i64>) -> Option<Datetime> {
        None
    }
}

/// Compile Typst markup to PDF bytes.
pub fn typst_to_pdf(typst_source: &str) -> Result<Vec<u8>, RenderError> {
    let world = DocWorld::new(typst_source.to_string());
    let document: PagedDocument = typst::compile(&world)
        .output
        .map_err(|errors| {
            let msgs: Vec<String> = errors.iter().map(|e| e.message.to_string()).collect();
            RenderError::Compile(msgs.join("; "))
        })?;
    typst_pdf::pdf(&document, &typst_pdf::PdfOptions::default()).map_err(|errors| {
        let msgs: Vec<String> = errors.iter().map(|e| e.message.to_string()).collect();
        RenderError::Export(msgs.join("; "))
    })
}

/// Render Markdown to PDF: translate to Typst markup, then compile.
pub fn markdown_to_pdf(markdown: &str) -> Result<Vec<u8>, RenderError> {
    typst_to_pdf(&markdown_to_typst(markdown))
}

/// Escape text content for Typst markup mode.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' | '`' | '$' | '#' | '*' | '_' | '[' | ']' | '<' | '>' | '@' | '~' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// Translate the common Markdown document subset to Typst markup: headings,
/// paragraphs, emphasis, code (inline + fenced), lists (nested, ordered and
/// not), links, blockquotes, tables, rules. Unknown constructs degrade to
/// their plain text.
pub fn markdown_to_typst(markdown: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);

    let mut out = String::from("#set page(margin: 0.85in)\n#set text(size: 11pt)\n\n");
    // Stack of list markers ("-" bullet / "+" numbered) for nesting depth.
    let mut lists: Vec<&str> = Vec::new();
    // Table assembly state: cells collected per row, column count from header.
    let mut table_cells: Vec<String> = Vec::new();
    let mut table_cols = 0usize;
    let mut in_table_head = false;
    let mut in_table = false;
    // Inside a code block, text passes through raw (no escaping).
    let mut in_code_block = false;

    for event in Parser::new_ext(markdown, opts) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    let n = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    };
                    out.push('\n');
                    out.push_str(&"=".repeat(n));
                    out.push(' ');
                }
                Tag::Paragraph => {
                    if !in_table {
                        out.push('\n');
                    }
                }
                Tag::Strong => out.push('*'),
                Tag::Emphasis => out.push('_'),
                Tag::Strikethrough => out.push_str("#strike["),
                Tag::Link { dest_url, .. } => {
                    out.push_str("#link(\"");
                    out.push_str(&dest_url.replace('"', "\\\""));
                    out.push_str("\")[");
                }
                Tag::CodeBlock(kind) => {
                    in_code_block = true;
                    out.push_str("\n```");
                    if let CodeBlockKind::Fenced(lang) = kind {
                        if let Some(first) = lang.split_whitespace().next() {
                            out.push_str(first);
                        }
                    }
                    out.push('\n');
                }
                Tag::List(start) => {
                    lists.push(if start.is_some() { "+" } else { "-" });
                    out.push('\n');
                }
                Tag::Item => {
                    let depth = lists.len().saturating_sub(1);
                    out.push_str(&"  ".repeat(depth));
                    out.push_str(lists.last().unwrap_or(&"-"));
                    out.push(' ');
                }
                Tag::BlockQuote(_) => out.push_str("\n#quote(block: true)["),
                Tag::Table(alignments) => {
                    in_table = true;
                    table_cols = alignments.len();
                    table_cells.clear();
                }
                Tag::TableHead => in_table_head = true,
                Tag::TableRow | Tag::TableCell => {}
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Heading(_) => out.push('\n'),
                TagEnd::Paragraph => {
                    if !in_table {
                        out.push('\n');
                    }
                }
                TagEnd::Strong => out.push('*'),
                TagEnd::Emphasis => out.push('_'),
                TagEnd::Strikethrough => out.push(']'),
                TagEnd::Link => out.push(']'),
                TagEnd::CodeBlock => {
                    in_code_block = false;
                    out.push_str("```\n");
                }
                TagEnd::List(_) => {
                    lists.pop();
                    if lists.is_empty() {
                        out.push('\n');
                    }
                }
                TagEnd::Item => out.push('\n'),
                TagEnd::BlockQuote(_) => out.push_str("]\n"),
                TagEnd::TableCell => {
                    // Cell content was accumulated into `out` since the cell
                    // started — but cells need wrapping, so we instead collect
                    // them via the marker inserted at TableCell start. To keep
                    // this translator single-pass and simple, cells capture is
                    // handled below in the Text arm when in_table is set.
                }
                TagEnd::Table => {
                    in_table = false;
                    out.push_str("\n#table(\n  columns: ");
                    out.push_str(&table_cols.to_string());
                    out.push_str(",\n");
                    for cell in &table_cells {
                        out.push_str("  [");
                        out.push_str(cell);
                        out.push_str("],\n");
                    }
                    out.push_str(")\n");
                    table_cells.clear();
                }
                TagEnd::TableHead => in_table_head = false,
                _ => {}
            },
            Event::Text(t) => {
                if in_table {
                    // Each Text event inside a table is one cell's content
                    // (formatting inside table cells degrades to plain text).
                    let _ = in_table_head;
                    table_cells.push(escape(&t));
                } else if in_code_block {
                    out.push_str(&t);
                } else {
                    out.push_str(&escape(&t));
                }
            }
            Event::Code(c) => {
                if in_table {
                    table_cells.push(escape(&c));
                } else {
                    // Inline raw; drop interior backticks (vanishingly rare).
                    out.push('`');
                    out.push_str(&c.replace('`', "'"));
                    out.push('`');
                }
            }
            Event::SoftBreak => out.push(' '),
            Event::HardBreak => out.push_str(" \\\n"),
            Event::Rule => out.push_str("\n#line(length: 100%)\n"),
            Event::TaskListMarker(done) => {
                out.push_str(if done { "[x] " } else { "[ ] " });
            }
            Event::Html(_) | Event::InlineHtml(_) => {}
            Event::FootnoteReference(_) | Event::InlineMath(_) | Event::DisplayMath(_) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_renders_to_pdf() {
        let md = "# Report Title\n\nSome **bold** and _italic_ text with `code`.\n\n\
                  ## Section\n\n- item one\n- item two\n  1. nested\n\n\
                  | Name | Value |\n|------|-------|\n| a | 1 |\n| b | 2 |\n\n\
                  ```rust\nfn main() {}\n```\n\n> a quote\n";
        let pdf = markdown_to_pdf(md).expect("render");
        assert!(pdf.starts_with(b"%PDF-"), "output is a PDF");
        assert!(pdf.len() > 1000, "non-trivial document");
    }

    #[test]
    fn special_chars_escape_cleanly() {
        let md = "Cost is $5 * 3 #tags [brackets] <angle> @mention _under_";
        let pdf = markdown_to_pdf(md).expect("render with special chars");
        assert!(pdf.starts_with(b"%PDF-"));
    }
}
