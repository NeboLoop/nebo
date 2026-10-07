//! The parts of the package IronCalc does not model, read straight from the
//! xlsx: where each worksheet lives, cell and sheet protection, and charts.

use std::io::{Cursor, Read};

use roxmltree::{Document, Node};

use crate::Error;

pub(crate) const WORKBOOK: &str = "xl/workbook.xml";
pub(crate) const WORKBOOK_RELS: &str = "xl/_rels/workbook.xml.rels";
pub(crate) const CONTENT_TYPES: &str = "[Content_Types].xml";
const STYLES: &str = "xl/styles.xml";
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

pub(crate) type Zip<'a> = zip::ZipArchive<Cursor<&'a [u8]>>;

/// What the viewer needs from the package beyond IronCalc's model.
pub(crate) struct Parts {
    /// Zip path of each worksheet, in workbook order (IronCalc's sheet index).
    pub sheet_paths: Vec<String>,
    /// Per cellXfs index: the style carries `<protection locked="0"/>`.
    /// Excel locks every cell whose style does not say otherwise.
    pub unlocked: Vec<bool>,
    /// Per sheet: `<sheetProtection sheet="1">` is on.
    pub protected: Vec<bool>,
    pub charts: Vec<Vec<ChartPart>>,
}

impl Parts {
    pub fn unlocked(&self, style: i32) -> bool {
        usize::try_from(style)
            .ok()
            .and_then(|i| self.unlocked.get(i))
            .copied()
            .unwrap_or(false)
    }
}

/// Where a chart sits: two cells, or a top-left cell plus a size in EMU
/// (`oneCellAnchor`), which the view turns into cells with the sheet's sizes.
pub(crate) enum Anchor {
    Cells { from: (i32, i32), to: (i32, i32) },
    Extent { from: (i32, i32), cx: i64, cy: i64 },
}

pub(crate) struct ChartPart {
    pub kind: String,
    pub title: Option<String>,
    pub anchor: Anchor,
    pub series: Vec<SeriesPart>,
}

pub(crate) struct SeriesPart {
    pub name: Option<String>,
    pub values: String,
    pub categories: Option<String>,
}

pub(crate) fn open(xlsx: &[u8]) -> Result<Zip<'_>, Error> {
    zip::ZipArchive::new(Cursor::new(xlsx)).map_err(|e| Error::Unreadable(e.to_string()))
}

/// A part's text, or None when the package has no such part.
pub(crate) fn text(zip: &mut Zip, name: &str) -> Result<Option<String>, Error> {
    let mut file = match zip.by_name(name) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(Error::Unreadable(format!("{name}: {e}"))),
    };
    let mut s = String::new();
    file.read_to_string(&mut s)
        .map_err(|e| Error::Unreadable(format!("{name}: {e}")))?;
    Ok(Some(s))
}

pub(crate) fn parse<'a>(name: &str, xml: &'a str) -> Result<Document<'a>, Error> {
    Document::parse(xml).map_err(|e| Error::Unreadable(format!("{name}: {e}")))
}

pub(crate) fn element<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children()
        .find(|n| n.is_element() && n.tag_name().name() == name)
}

fn truthy(v: Option<&str>) -> bool {
    matches!(v, Some("1" | "true"))
}

/// The relationships of `part` as (type, resolved target) pairs, by id.
pub(crate) fn rels(zip: &mut Zip, part: &str) -> Result<Vec<(String, String, String)>, Error> {
    let (dir, file) = part.rsplit_once('/').unwrap_or(("", part));
    let rels_name = format!("{dir}/_rels/{file}.rels");
    let Some(xml) = text(zip, &rels_name)? else {
        return Ok(Vec::new());
    };
    let doc = parse(&rels_name, &xml)?;
    Ok(doc
        .root_element()
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "Relationship")
        .filter(|n| n.attribute("TargetMode") != Some("External"))
        .filter_map(|n| {
            Some((
                n.attribute("Id")?.to_string(),
                n.attribute("Type")?.to_string(),
                resolve(dir, n.attribute("Target")?),
            ))
        })
        .collect())
}

/// A relationship target relative to the source part's folder, as a zip path.
pub(crate) fn resolve(dir: &str, target: &str) -> String {
    if let Some(absolute) = target.strip_prefix('/') {
        return absolute.to_string();
    }
    let mut parts: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    for seg in target.split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}

pub(crate) fn read(xlsx: &[u8]) -> Result<Parts, Error> {
    let mut zip = open(xlsx)?;
    let workbook_rels = rels(&mut zip, WORKBOOK)?;
    let xml = text(&mut zip, WORKBOOK)?
        .ok_or_else(|| Error::Unreadable(format!("{WORKBOOK} is missing")))?;
    let doc = parse(WORKBOOK, &xml)?;
    let sheet_paths = doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "sheet")
        .map(|n| {
            let id = n.attribute((REL_NS, "id")).unwrap_or_default();
            workbook_rels
                .iter()
                .find(|(rid, _, _)| rid == id)
                .map(|(_, _, target)| target.clone())
                .ok_or_else(|| Error::Unreadable(format!("sheet relationship {id} is missing")))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let unlocked = match text(&mut zip, STYLES)? {
        Some(xml) => {
            let doc = parse(STYLES, &xml)?;
            element(doc.root_element(), "cellXfs")
                .map(|xfs| {
                    xfs.children()
                        .filter(|n| n.is_element())
                        .map(|xf| {
                            element(xf, "protection")
                                .and_then(|p| p.attribute("locked"))
                                .is_some_and(|v| matches!(v, "0" | "false"))
                        })
                        .collect()
                })
                .unwrap_or_default()
        }
        None => Vec::new(),
    };

    let mut protected = Vec::with_capacity(sheet_paths.len());
    let mut charts = Vec::with_capacity(sheet_paths.len());
    for path in &sheet_paths {
        let xml =
            text(&mut zip, path)?.ok_or_else(|| Error::Unreadable(format!("{path} is missing")))?;
        let doc = parse(path, &xml)?;
        protected.push(
            element(doc.root_element(), "sheetProtection")
                .is_some_and(|p| truthy(p.attribute("sheet"))),
        );
        charts.push(sheet_charts(&mut zip, path)?);
    }

    Ok(Parts {
        sheet_paths,
        unlocked,
        protected,
        charts,
    })
}

/// Every chart drawn on a sheet: the sheet's drawing, each anchor in it, and
/// the chart part an anchor's graphic frame points at.
fn sheet_charts(zip: &mut Zip, sheet: &str) -> Result<Vec<ChartPart>, Error> {
    let mut out = Vec::new();
    for (_, kind, drawing) in rels(zip, sheet)? {
        if !kind.ends_with("/drawing") {
            continue;
        }
        let Some(xml) = text(zip, &drawing)? else {
            continue;
        };
        let drawing_rels = rels(zip, &drawing)?;
        let doc = parse(&drawing, &xml)?;
        for anchor in doc.root_element().children().filter(|n| n.is_element()) {
            let Some(chart_id) = anchor
                .descendants()
                .find(|n| n.is_element() && n.tag_name().name() == "chart")
                .and_then(|n| n.attribute((REL_NS, "id")))
            else {
                continue;
            };
            let Some(place) = anchor_of(anchor) else {
                continue;
            };
            let Some((_, _, chart_path)) = drawing_rels.iter().find(|(id, _, _)| id == chart_id)
            else {
                continue;
            };
            let Some(chart_xml) = text(zip, chart_path)? else {
                continue;
            };
            if let Some(chart) = chart(&parse(chart_path, &chart_xml)?, place) {
                out.push(chart);
            }
        }
    }
    Ok(out)
}

fn marker(node: Node) -> Option<(i32, i32)> {
    let num = |name: &str| element(node, name)?.text()?.trim().parse::<i32>().ok();
    Some((num("row")?, num("col")?))
}

fn anchor_of(anchor: Node) -> Option<Anchor> {
    let from = marker(element(anchor, "from")?)?;
    match anchor.tag_name().name() {
        "twoCellAnchor" => Some(Anchor::Cells {
            from,
            to: marker(element(anchor, "to")?)?,
        }),
        "oneCellAnchor" => {
            let ext = element(anchor, "ext")?;
            Some(Anchor::Extent {
                from,
                cx: ext.attribute("cx")?.parse().ok()?,
                cy: ext.attribute("cy")?.parse().ok()?,
            })
        }
        _ => None,
    }
}

fn chart(doc: &Document, anchor: Anchor) -> Option<ChartPart> {
    let chart = element(doc.root_element(), "chart")?;
    let title = element(chart, "title").map(|t| {
        t.descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "t")
            .filter_map(|n| n.text())
            .collect::<String>()
    });
    let plots: Vec<Node> = element(chart, "plotArea")?
        .children()
        .filter(|n| n.is_element() && n.tag_name().name().ends_with("Chart"))
        .collect();
    let first = plots.first()?;
    let mut kind = first
        .tag_name()
        .name()
        .trim_end_matches("Chart")
        .trim_end_matches("3D")
        .to_string();
    if kind == "bar" && element(*first, "barDir").and_then(|n| n.attribute("val")) == Some("col") {
        kind = "column".to_string();
    }
    let formula = |node: Option<Node>| {
        node?
            .descendants()
            .find(|n| n.is_element() && n.tag_name().name() == "f")?
            .text()
            .map(str::to_string)
    };
    let series = plots
        .iter()
        .flat_map(|p| {
            p.children()
                .filter(|n| n.is_element() && n.tag_name().name() == "ser")
        })
        .filter_map(|ser| {
            let name = element(ser, "tx").and_then(|tx| {
                tx.descendants()
                    .find(|n| n.is_element() && n.tag_name().name() == "v")
                    .and_then(|v| v.text())
                    .map(str::to_string)
            });
            Some(SeriesPart {
                name,
                values: formula(element(ser, "val").or_else(|| element(ser, "yVal")))?,
                categories: formula(element(ser, "cat").or_else(|| element(ser, "xVal"))),
            })
        })
        .collect();
    Some(ChartPart {
        kind,
        title: title.filter(|t| !t.is_empty()),
        anchor,
        series,
    })
}
