//! Save by patching in place: the original package with only the changed
//! `<c>` elements rewritten. Every other part — charts, validation, theme,
//! conditional formats, macros — is copied byte for byte, so nothing the
//! engine does not understand is lost. calcChain goes (its cell list may no
//! longer match) and the workbook asks Excel to recalculate on open.

use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use std::ops::Range;

use roxmltree::Node;
use zip::write::SimpleFileOptions;

use crate::parts::{self, CONTENT_TYPES, WORKBOOK, WORKBOOK_RELS, element};
use crate::{Error, Value, cell_name};

/// The `<f>` a patched cell carries.
pub(crate) enum Formula {
    /// The cell's own `<f>` as the file has it (shared and array formulas stay intact).
    Keep,
    /// A formula typed into the cell, in file form.
    Set(String),
    /// A constant typed into the cell.
    Clear,
}

pub(crate) struct CellPatch {
    pub row: i32,
    pub col: i32,
    pub value: Value,
    pub formula: Formula,
}

/// `<calcPr>` sits after these in `<workbook>` (CT_Workbook's sequence).
const AFTER_CALC_PR: [&str; 9] = [
    "oleSize",
    "customWorkbookViews",
    "pivotCaches",
    "smartTagPr",
    "smartTagTypes",
    "webPublishing",
    "fileRecoveryPr",
    "webPublishObjects",
    "extLst",
];

/// The package with `patches` (by worksheet path) written into it.
pub(crate) fn write(
    xlsx: &[u8],
    patches: &BTreeMap<String, Vec<CellPatch>>,
) -> Result<Vec<u8>, Error> {
    let mut zip = parts::open(xlsx)?;
    let calc_chain = parts::rels(&mut zip, WORKBOOK)?
        .into_iter()
        .find(|(_, kind, _)| kind.ends_with("/calcChain"))
        .map(|(_, _, target)| target);

    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let deflated =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for i in 0..zip.len() {
        let name = zip
            .by_index_raw(i)
            .map_err(|e| Error::Unreadable(e.to_string()))?
            .name()
            .to_string();
        if Some(&name) == calc_chain.as_ref() {
            continue;
        }
        let rewritten = if let Some(cells) = patches.get(&name) {
            Some(patch_sheet(&name, &read(&mut zip, &name)?, cells)?)
        } else if name == WORKBOOK {
            Some(full_calc_on_load(&read(&mut zip, &name)?)?)
        } else if name == WORKBOOK_RELS && calc_chain.is_some() {
            let xml = read(&mut zip, &name)?;
            Some(remove(&name, &xml, |n| {
                n.attribute("Type")
                    .is_some_and(|t| t.ends_with("/calcChain"))
            })?)
        } else if let (CONTENT_TYPES, Some(chain)) = (name.as_str(), &calc_chain) {
            let part = format!("/{chain}");
            let xml = read(&mut zip, &name)?;
            Some(remove(&name, &xml, |n| {
                n.attribute("PartName") == Some(part.as_str())
            })?)
        } else {
            None
        };
        let io = |e: std::io::Error| Error::Unreadable(format!("{name}: {e}"));
        let zip_err = |e: zip::result::ZipError| Error::Unreadable(format!("{name}: {e}"));
        match rewritten {
            Some(xml) => {
                out.start_file(name.as_str(), deflated).map_err(zip_err)?;
                out.write_all(xml.as_bytes()).map_err(io)?;
            }
            None => {
                let file = zip.by_index_raw(i).map_err(zip_err)?;
                out.raw_copy_file(file).map_err(zip_err)?;
            }
        }
    }
    out.finish()
        .map(Cursor::into_inner)
        .map_err(|e| Error::Unreadable(e.to_string()))
}

fn read(zip: &mut parts::Zip, name: &str) -> Result<String, Error> {
    parts::text(zip, name)?.ok_or_else(|| Error::Unreadable(format!("{name} is missing")))
}

/// Apply non-overlapping replacements (an empty range inserts). Insertions at
/// one position keep the order they were given in.
fn splice(xml: &str, mut edits: Vec<(Range<usize>, String)>) -> String {
    edits.sort_by_key(|(r, _)| r.start);
    let mut out =
        String::with_capacity(xml.len() + edits.iter().map(|(_, s)| s.len()).sum::<usize>());
    let mut at = 0;
    for (range, text) in edits {
        out.push_str(&xml[at..range.start]);
        out.push_str(&text);
        at = range.end;
    }
    out.push_str(&xml[at..]);
    out
}

/// Byte index of the `>` closing an element's start tag.
fn start_tag_end(xml: &str, node: Node) -> usize {
    let start = node.range().start;
    let mut quote = None;
    for (i, ch) in xml[start..].char_indices() {
        match (quote, ch) {
            (None, '"' | '\'') => quote = Some(ch),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return start + i,
            _ => {}
        }
    }
    xml.len()
}

fn self_closing(xml: &str, node: Node) -> bool {
    xml[..=start_tag_end(xml, node)].ends_with("/>")
}

/// Byte index where an element's end tag starts.
fn end_tag_start(xml: &str, node: Node) -> usize {
    let range = node.range();
    range.start + xml[range.clone()].rfind("</").unwrap_or(range.len())
}

/// The namespace prefix an element is written with (`x:` in `<x:row>`), so
/// what we add matches what is there.
fn prefix(xml: &str, node: Node) -> String {
    let qname: String = xml[node.range().start + 1..]
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '>' && *c != '/')
        .collect();
    qname
        .strip_suffix(node.tag_name().name())
        .unwrap_or_default()
        .to_string()
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn remove(name: &str, xml: &str, matches: impl Fn(Node) -> bool) -> Result<String, Error> {
    let doc = parts::parse(name, xml)?;
    let edits = doc
        .root_element()
        .children()
        .filter(|n| n.is_element() && matches(*n))
        .map(|n| (n.range(), String::new()))
        .collect();
    Ok(splice(xml, edits))
}

/// `<calcPr fullCalcOnLoad="1">`: the cached values we wrote are IronCalc's,
/// and Excel should compute its own on open.
fn full_calc_on_load(xml: &str) -> Result<String, Error> {
    let doc = parts::parse(WORKBOOK, xml)?;
    let root = doc.root_element();
    let attr = r#"fullCalcOnLoad="1""#;
    let edit = match element(root, "calcPr") {
        Some(calc) => match calc.attributes().find(|a| a.name() == "fullCalcOnLoad") {
            Some(a) => (a.range(), attr.to_string()),
            None => {
                let at = calc.range().start + 1 + prefix(xml, calc).len() + "calcPr".len();
                (at..at, format!(" {attr}"))
            }
        },
        None => {
            let at = root
                .children()
                .find(|n| n.is_element() && AFTER_CALC_PR.contains(&n.tag_name().name()))
                .map(|n| n.range().start)
                .unwrap_or_else(|| end_tag_start(xml, root));
            (at..at, format!("<{}calcPr {attr}/>", prefix(xml, root)))
        }
    };
    Ok(splice(xml, vec![edit]))
}

/// A1 coordinates of a `<c r="B7">` / `<row r="7">`, or the next position
/// when the file leaves `r` out (allowed; cells are then consecutive).
fn position(r: Option<&str>, next: i32, is_cell: bool) -> i32 {
    let Some(r) = r else { return next };
    if !is_cell {
        return r.parse().unwrap_or(next);
    }
    r.chars()
        .take_while(char::is_ascii_alphabetic)
        .fold(0, |acc, c| {
            acc * 26 + (c.to_ascii_uppercase() as i32 - 'A' as i32 + 1)
        })
}

fn patch_sheet(name: &str, xml: &str, cells: &[CellPatch]) -> Result<String, Error> {
    let doc = parts::parse(name, xml)?;
    let sheet_data = element(doc.root_element(), "sheetData")
        .ok_or_else(|| Error::Unreadable(format!("{name} has no sheetData")))?;
    let p = prefix(xml, sheet_data);

    let mut rows: BTreeMap<i32, Node> = BTreeMap::new();
    let mut next = 1;
    for row in sheet_data
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "row")
    {
        let r = position(row.attribute("r"), next, false);
        rows.insert(r, row);
        next = r + 1;
    }
    let mut by_row: BTreeMap<i32, Vec<&CellPatch>> = BTreeMap::new();
    for cell in cells {
        by_row.entry(cell.row).or_default().push(cell);
    }

    let mut edits = Vec::new();
    let mut new_rows = String::new();
    for (r, mut patches) in by_row {
        patches.sort_by_key(|c| c.col);
        let Some(row) = rows.get(&r).copied() else {
            let body: String = patches.iter().map(|c| cell_xml(xml, None, c, &p)).collect();
            let row_xml = format!(r#"<{p}row r="{r}">{body}</{p}row>"#);
            match rows.range(r + 1..).next() {
                Some((_, after)) => edits.push((after.range().start..after.range().start, row_xml)),
                None => new_rows.push_str(&row_xml),
            }
            continue;
        };
        if self_closing(xml, row) {
            let tag_end = start_tag_end(xml, row);
            let open = xml[row.range().start..tag_end].trim_end_matches('/');
            let body: String = patches.iter().map(|c| cell_xml(xml, None, c, &p)).collect();
            edits.push((row.range(), format!("{open}>{body}</{p}row>")));
            continue;
        }
        let mut existing: Vec<(i32, Node)> = Vec::new();
        let mut next = 1;
        for c in row
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "c")
        {
            let col = position(c.attribute("r"), next, true);
            existing.push((col, c));
            next = col + 1;
        }
        for patch in patches {
            match existing.iter().find(|(col, _)| *col == patch.col) {
                Some((_, node)) => {
                    edits.push((node.range(), cell_xml(xml, Some(*node), patch, &p)))
                }
                None => {
                    let at = existing
                        .iter()
                        .find(|(col, _)| *col > patch.col)
                        .map(|(_, n)| n.range().start)
                        .unwrap_or_else(|| end_tag_start(xml, row));
                    edits.push((at..at, cell_xml(xml, None, patch, &p)));
                }
            }
        }
    }
    if !new_rows.is_empty() {
        if self_closing(xml, sheet_data) {
            edits.push((
                sheet_data.range(),
                format!("<{p}sheetData>{new_rows}</{p}sheetData>"),
            ));
        } else {
            let at = end_tag_start(xml, sheet_data);
            edits.push((at..at, new_rows));
        }
    }
    Ok(splice(xml, edits))
}

/// A cell rewritten with its new value. Attributes (`r`, `s`, `cm`, …) stay as
/// written except `t`, which follows the value; the `<f>` follows `patch.formula`.
fn cell_xml(xml: &str, original: Option<Node>, patch: &CellPatch, p: &str) -> String {
    let attrs = match original {
        Some(node) => {
            let tag_start = node.range().start;
            let tag_end = start_tag_end(xml, node);
            let attrs_start = tag_start + 1 + prefix(xml, node).len() + 1;
            let edits = node
                .attributes()
                .filter(|a| a.name() == "t" && a.namespace().is_none())
                .map(|a| {
                    let span = a.range();
                    let ws = xml[..span.start].len() - xml[..span.start].trim_end().len();
                    (
                        span.start - ws - attrs_start..span.end - attrs_start,
                        String::new(),
                    )
                })
                .collect();
            splice(&xml[attrs_start..tag_end], edits)
                .trim_end_matches('/')
                .to_string()
        }
        None => format!(r#" r="{}""#, cell_name(patch.row, patch.col)),
    };
    let formula = match (&patch.formula, original) {
        (Formula::Keep, Some(node)) => element(node, "f")
            .map(|f| xml[f.range()].to_string())
            .unwrap_or_default(),
        (Formula::Set(f), _) => format!("<{p}f>{}</{p}f>", escape(f)),
        _ => String::new(),
    };
    let v = |t: &str| format!("<{p}v>{}</{p}v>", escape(t));
    let (t, value) = match &patch.value {
        Value::Empty => ("", String::new()),
        Value::Number(n) => ("", v(&n.to_string())),
        Value::Bool(b) => (r#" t="b""#, v(if *b { "1" } else { "0" })),
        Value::Error(e) => (r#" t="e""#, v(e)),
        Value::Text(s) if !formula.is_empty() => (r#" t="str""#, v(s)),
        Value::Text(s) => {
            let space = if s.trim() != s {
                r#" xml:space="preserve""#
            } else {
                ""
            };
            (
                r#" t="inlineStr""#,
                format!("<{p}is><{p}t{space}>{}</{p}t></{p}is>", escape(s)),
            )
        }
    };
    if formula.is_empty() && value.is_empty() {
        format!("<{p}c{attrs}{t}/>")
    } else {
        format!("<{p}c{attrs}{t}>{formula}{value}</{p}c>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(row: i32, col: i32, n: f64) -> CellPatch {
        CellPatch {
            row,
            col,
            value: Value::Number(n),
            formula: Formula::Clear,
        }
    }

    #[test]
    fn cells_and_rows_are_inserted_in_order() {
        let xml = r#"<worksheet><sheetData><row r="2"><c r="A2"><v>1</v></c><c r="C2" t="s"><v>0</v></c></row><row r="4" spans="1:2"/><row r="9"><c r="A9"/></row></sheetData></worksheet>"#;
        let patched = patch_sheet(
            "sheet",
            xml,
            &[
                number(2, 2, 5.0),
                number(2, 3, 6.0),
                number(4, 1, 7.0),
                number(6, 1, 8.0),
                number(12, 2, 9.0),
            ],
        )
        .unwrap();
        assert_eq!(
            patched,
            r#"<worksheet><sheetData><row r="2"><c r="A2"><v>1</v></c><c r="B2"><v>5</v></c><c r="C2"><v>6</v></c></row><row r="4" spans="1:2"><c r="A4"><v>7</v></c></row><row r="6"><c r="A6"><v>8</v></c></row><row r="9"><c r="A9"/></row><row r="12"><c r="B12"><v>9</v></c></row></sheetData></worksheet>"#
        );
    }

    #[test]
    fn an_empty_sheet_and_a_prefixed_namespace() {
        let xml = r#"<x:worksheet xmlns:x="urn:x"><x:sheetData/></x:worksheet>"#;
        let patched = patch_sheet("sheet", xml, &[number(1, 1, 1.0)]).unwrap();
        assert_eq!(
            patched,
            r#"<x:worksheet xmlns:x="urn:x"><x:sheetData><x:row r="1"><x:c r="A1"><x:v>1</x:v></x:c></x:row></x:sheetData></x:worksheet>"#
        );
    }

    #[test]
    fn values_take_the_type_they_hold() {
        let xml = r#"<worksheet><sheetData><row r="1"><c r="A1" t="s" s="2"><v>3</v></c><c r="B1"><f t="shared" ref="B1:B9" si="0">A1*2</f><v>2</v></c><c r="C1" t="str"><f>A1&amp;"x"</f><v>ax</v></c></row></sheetData></worksheet>"#;
        let patched = patch_sheet(
            "sheet",
            xml,
            &[
                CellPatch {
                    row: 1,
                    col: 1,
                    value: Value::Text(" a & b".into()),
                    formula: Formula::Clear,
                },
                CellPatch {
                    row: 1,
                    col: 2,
                    value: Value::Error("#VALUE!".into()),
                    formula: Formula::Keep,
                },
                CellPatch {
                    row: 1,
                    col: 3,
                    value: Value::Bool(true),
                    formula: Formula::Keep,
                },
            ],
        )
        .unwrap();
        assert_eq!(
            patched,
            r#"<worksheet><sheetData><row r="1"><c r="A1" s="2" t="inlineStr"><is><t xml:space="preserve"> a &amp; b</t></is></c><c r="B1" t="e"><f t="shared" ref="B1:B9" si="0">A1*2</f><v>#VALUE!</v></c><c r="C1" t="b"><f>A1&amp;"x"</f><v>1</v></c></row></sheetData></worksheet>"#
        );
    }

    #[test]
    fn full_calc_on_load_is_set_once() {
        let add =
            full_calc_on_load(r#"<workbook><sheets/><calcPr calcId="1"/></workbook>"#).unwrap();
        assert_eq!(
            add,
            r#"<workbook><sheets/><calcPr fullCalcOnLoad="1" calcId="1"/></workbook>"#
        );
        let replace =
            full_calc_on_load(r#"<workbook><calcPr fullCalcOnLoad="0"/></workbook>"#).unwrap();
        assert_eq!(
            replace,
            r#"<workbook><calcPr fullCalcOnLoad="1"/></workbook>"#
        );
        let insert = full_calc_on_load(r#"<workbook><sheets/><extLst/></workbook>"#).unwrap();
        assert_eq!(
            insert,
            r#"<workbook><sheets/><calcPr fullCalcOnLoad="1"/><extLst/></workbook>"#
        );
        let at_end = full_calc_on_load(r#"<workbook><sheets/></workbook>"#).unwrap();
        assert_eq!(
            at_end,
            r#"<workbook><sheets/><calcPr fullCalcOnLoad="1"/></workbook>"#
        );
    }
}
