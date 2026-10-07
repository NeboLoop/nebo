//! nebo-sheet — the spreadsheet engine behind the Work panel's sheet view.
//!
//! IronCalc loads and calculates the workbook. This crate adds what IronCalc
//! does not model — the editable mask (a cell is editable when its style has
//! `locked="0"`, Excel's own marker), sheet protection and charts — by reading
//! those parts of the package itself, and saves by patching the changed cells
//! into the original file so every part it does not understand survives.

mod parts;
mod patch;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::RangeInclusive;

use ironcalc::base::Model;
use ironcalc::base::expressions::parser::static_analysis::remove_redundant_implicit_intersection;
use ironcalc::base::expressions::parser::stringify::{to_english_string, to_excel_string};
use ironcalc::base::expressions::types::CellReferenceRC;
use ironcalc::base::types::{
    BorderItem, Cell, Color, FormulaValue, HorizontalAlignment, SheetState, SpillValue,
    VerticalAlignment, Worksheet,
};
use serde::{Deserialize, Serialize};

use patch::{CellPatch, Formula};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a workbook the sheet viewer can read: {0}")]
    Unreadable(String),
}

/// (sheet index, row, column), 1-based row and column as in A1.
type Key = (u32, i32, i32);

/// A cell's value, as the view shows it and as a save writes it.
#[derive(Clone, Debug, PartialEq)]
enum Value {
    Empty,
    Number(f64),
    Text(String),
    Bool(bool),
    Error(String),
}

impl Serialize for Value {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Value::Empty => s.serialize_none(),
            Value::Number(n) => s.serialize_f64(*n),
            Value::Text(t) | Value::Error(t) => s.serialize_str(t),
            Value::Bool(b) => s.serialize_bool(*b),
        }
    }
}

/// The view model the grid renders (the sheet viewer contract).
#[derive(Serialize)]
pub struct View {
    pub sheets: Vec<SheetView>,
    pub styles: Vec<Style>,
    pub names: BTreeMap<String, String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SheetView {
    pub name: String,
    pub tab_color: Option<String>,
    pub hidden: bool,
    pub protected: bool,
    pub dims: Dims,
    pub freeze: Dims,
    /// Pixels, by column letter.
    pub col_widths: BTreeMap<String, f64>,
    /// Pixels, by row number.
    pub row_heights: BTreeMap<String, f64>,
    pub merges: Vec<String>,
    pub cells: Vec<CellView>,
    pub charts: Vec<Chart>,
}

#[derive(Serialize)]
pub struct Dims {
    pub rows: i32,
    pub cols: i32,
}

#[derive(Serialize)]
pub struct CellView {
    pub r: i32,
    pub c: i32,
    pub display: String,
    value: Value,
    pub formula: Option<String>,
    pub style: usize,
    pub editable: bool,
}

#[derive(Serialize, Clone, Default, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct Style {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub underline: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub wrap: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub align: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valign: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_fmt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub border: Option<Borders>,
}

#[derive(Serialize, Clone, Default, PartialEq, Eq, Hash)]
pub struct Borders {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top: Option<BorderSide>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right: Option<BorderSide>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bottom: Option<BorderSide>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<BorderSide>,
}

#[derive(Serialize, Clone, PartialEq, Eq, Hash)]
pub struct BorderSide {
    pub style: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Serialize)]
pub struct Chart {
    #[serde(rename = "type")]
    pub kind: String,
    pub title: Option<String>,
    pub anchor: String,
    pub series: Vec<Series>,
}

#[derive(Serialize)]
pub struct Series {
    pub name: Option<String>,
    #[serde(rename = "ref")]
    pub values: String,
    pub categories: Option<String>,
}

/// One cell typed into: `input` as the person typed it (`12`, `10%`, `=A1*2`).
#[derive(Deserialize)]
pub struct Edit {
    pub sheet: String,
    pub cell: String,
    pub input: String,
}

#[derive(Serialize)]
pub struct Edited {
    pub changed: Vec<Changed>,
    pub errors: Vec<EditError>,
}

#[derive(Serialize)]
pub struct Changed {
    pub sheet: String,
    pub cell: String,
    pub display: String,
    value: Value,
}

#[derive(Serialize)]
pub struct EditError {
    pub sheet: String,
    pub cell: String,
    pub error: String,
}

/// An open workbook: IronCalc's model over the original xlsx bytes.
pub struct Book {
    model: Model<'static>,
    xlsx: Vec<u8>,
    parts: parts::Parts,
    /// Every cell's value as of the bytes in `xlsx`; a save writes the cells
    /// that differ from it.
    baseline: HashMap<Key, Value>,
    /// Cells that held a formula in the file. They are calculated, not inputs.
    formulas: HashSet<Key>,
    /// Cells typed into since `xlsx`.
    edited: HashSet<Key>,
}

/// Column letters for a 1-based column (`28` → `AB`).
pub fn column_name(mut col: i32) -> String {
    let mut name = String::new();
    while col > 0 {
        let rem = (col - 1) % 26;
        name.insert(0, char::from(b'A' + rem as u8));
        col = (col - 1) / 26;
    }
    name
}

fn cell_name(row: i32, col: i32) -> String {
    format!("{}{row}", column_name(col))
}

/// `B7` → (7, 2). Absolute markers (`$B$7`) are accepted.
fn parse_cell(name: &str) -> Option<(i32, i32)> {
    let name = name.replace('$', "");
    let split = name.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = name.split_at(split);
    if letters.is_empty() || !letters.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let col = letters.chars().fold(0i32, |acc, c| {
        acc * 26 + (c.to_ascii_uppercase() as i32 - 'A' as i32 + 1)
    });
    let row = digits.parse().ok().filter(|r| *r > 0)?;
    Some((row, col))
}

// Excel stores column widths in characters of the default font and row
// heights in points; the grid draws pixels (7 px per character, 96 dpi).
// Unset sizes are Excel's defaults: 64 px columns, 20 px rows.
fn col_px(ws: &Worksheet, col: i32) -> f64 {
    match ws.cols.iter().find(|c| (c.min..=c.max).contains(&col)) {
        Some(c) if c.hidden => 0.0,
        Some(c) => (c.width * 7.0).round(),
        None => 64.0,
    }
}

fn row_px(ws: &Worksheet, row: i32) -> f64 {
    match ws.rows.iter().find(|r| r.r == row) {
        Some(r) if r.hidden => 0.0,
        Some(r) if r.custom_height => (r.height * 4.0 / 3.0).round(),
        _ => 20.0,
    }
}

fn rgb(color: &Color, theme: &ironcalc::base::types::Theme) -> Option<String> {
    Some(color.to_rgb(theme)).filter(|c| !c.is_empty())
}

impl Book {
    /// Load an xlsx and calculate it.
    pub fn open(xlsx: Vec<u8>) -> Result<Book, Error> {
        let workbook = ironcalc::import::load_from_xlsx_bytes(&xlsx, "workbook", "en", "UTC")
            .map_err(|e| Error::Unreadable(e.to_string()))?;
        let mut model = Model::from_workbook(workbook, "en").map_err(Error::Unreadable)?;
        model.evaluate();
        let parts = parts::read(&xlsx)?;
        if parts.sheet_paths.len() != model.workbook.worksheets.len() {
            return Err(Error::Unreadable(
                "the workbook's sheet list does not match its parts".into(),
            ));
        }
        let formulas = Self::cells(&model)
            .filter(|(_, cell)| cell.get_formula().is_some())
            .map(|(key, _)| key)
            .collect();
        let mut book = Book {
            model,
            xlsx,
            parts,
            baseline: HashMap::new(),
            formulas,
            edited: HashSet::new(),
        };
        book.baseline = book.values();
        Ok(book)
    }

    fn cells<'m>(model: &'m Model) -> impl Iterator<Item = (Key, &'m Cell)> + 'm {
        model
            .workbook
            .worksheets
            .iter()
            .enumerate()
            .flat_map(|(s, ws)| {
                ws.sheet_data
                    .cells()
                    .map(move |(r, c, cell)| ((s as u32, r, c), cell))
            })
    }

    fn value(&self, cell: &Cell) -> Value {
        let text = |t: &str| Value::Text(t.to_string());
        match cell {
            Cell::EmptyCell { .. } => Value::Empty,
            Cell::BooleanCell { v, .. } => Value::Bool(*v),
            Cell::NumberCell { v, .. } => Value::Number(*v),
            Cell::ErrorCell { ei, .. } => Value::Error(ei.to_string()),
            Cell::SharedString { si, .. } => usize::try_from(*si)
                .ok()
                .and_then(|i| self.model.workbook.shared_strings.get(i))
                .map_or(Value::Empty, |s| text(s)),
            Cell::CellFormula { v, .. } | Cell::ArrayFormula { v, .. } => match v {
                FormulaValue::Unevaluated => Value::Empty,
                FormulaValue::Boolean(b) => Value::Bool(*b),
                FormulaValue::Number(n) => Value::Number(*n),
                FormulaValue::Text(t) => text(t),
                FormulaValue::Error { ei, .. } => Value::Error(ei.to_string()),
            },
            Cell::SpillCell { v, .. } => match v {
                SpillValue::Boolean(b) => Value::Bool(*b),
                SpillValue::Number(n) => Value::Number(*n),
                SpillValue::Text(t) => text(t),
                SpillValue::Error(e) => Value::Error(e.to_string()),
            },
        }
    }

    fn values(&self) -> HashMap<Key, Value> {
        Self::cells(&self.model)
            .map(|(key, cell)| (key, self.value(cell)))
            .collect()
    }

    fn display(&self, (s, r, c): Key) -> String {
        self.model
            .get_formatted_cell_value(s, r, c)
            .unwrap_or_default()
    }

    /// The cell's formula as the formula bar shows it (`=B3*price`), without
    /// the implicit-intersection `@` IronCalc adds to formulas read from files.
    fn formula(&self, (s, r, c): Key, cell: &Cell, excel: bool) -> Option<String> {
        let index = usize::try_from(cell.get_formula()?).ok()?;
        let (node, _) = self.model.parsed_formulas.get(s as usize)?.get(index)?;
        let context = CellReferenceRC {
            sheet: self.model.workbook.worksheets.get(s as usize)?.name.clone(),
            row: r,
            column: c,
        };
        if excel {
            return Some(to_excel_string(node, &context));
        }
        let mut node = node.as_ref().clone();
        remove_redundant_implicit_intersection(&mut node, true);
        Some(format!("={}", to_english_string(&node, &context)))
    }

    fn editable(&self, (s, r, c): Key) -> bool {
        self.model
            .get_cell_style_index(s, r, c)
            .is_ok_and(|style| self.parts.unlocked(style))
    }

    /// The view model. `sheet` limits cells to that sheet (the others come
    /// without cells) and `rows` to those rows (1-based, inclusive); every
    /// sheet's dims, sizes, merges and charts are always there.
    pub fn view(&self, sheet: Option<&str>, rows: Option<RangeInclusive<i32>>) -> View {
        let theme = &self.model.workbook.theme;
        let mut styles: Vec<Style> = Vec::new();
        let mut style_ids: HashMap<Style, usize> = HashMap::new();
        let mut by_index: HashMap<i32, usize> = HashMap::new();
        let mut style_of = |key: Key, index: i32| -> usize {
            *by_index.entry(index).or_insert_with(|| {
                let style = self.style(key);
                *style_ids.entry(style.clone()).or_insert_with(|| {
                    styles.push(style);
                    styles.len() - 1
                })
            })
        };

        let mut sheets = Vec::new();
        for (s, ws) in self.model.workbook.worksheets.iter().enumerate() {
            let mut cells: Vec<CellView> = Vec::new();
            let (mut max_r, mut max_c) = (0, 0);
            for (r, c, cell) in ws.sheet_data.cells() {
                let key = (s as u32, r, c);
                let value = self.value(cell);
                let editable = self.editable(key) && !self.formulas.contains(&key);
                if value == Value::Empty && cell.get_style() == 0 && !editable {
                    continue;
                }
                max_r = max_r.max(r);
                max_c = max_c.max(c);
                if sheet.is_some_and(|name| name != ws.name)
                    || rows.as_ref().is_some_and(|range| !range.contains(&r))
                {
                    continue;
                }
                cells.push(CellView {
                    r,
                    c,
                    display: self.display(key),
                    value,
                    formula: self.formula(key, cell, false),
                    style: style_of(key, cell.get_style()),
                    editable,
                });
            }

            let mut col_widths = BTreeMap::new();
            for col in &ws.cols {
                for c in col.min..=col.max.min(max_c.max(col.min)) {
                    col_widths.insert(column_name(c), col_px(ws, c));
                }
            }
            let row_heights = ws
                .rows
                .iter()
                .filter(|row| row.custom_height || row.hidden)
                .filter(|row| rows.as_ref().is_none_or(|range| range.contains(&row.r)))
                .map(|row| (row.r.to_string(), row_px(ws, row.r)))
                .collect();

            let charts = self.parts.charts[s]
                .iter()
                .map(|chart| Chart {
                    kind: chart.kind.clone(),
                    title: chart.title.clone(),
                    anchor: self.anchor(s, &chart.anchor),
                    series: chart
                        .series
                        .iter()
                        .map(|series| Series {
                            name: series.name.clone(),
                            values: series.values.clone(),
                            categories: series.categories.clone(),
                        })
                        .collect(),
                })
                .collect();

            sheets.push(SheetView {
                name: ws.name.clone(),
                tab_color: rgb(&ws.color, theme),
                hidden: !matches!(ws.state, SheetState::Visible),
                protected: self.parts.protected[s],
                dims: Dims {
                    rows: max_r,
                    cols: max_c,
                },
                freeze: Dims {
                    rows: ws.frozen_rows,
                    cols: ws.frozen_columns,
                },
                col_widths,
                row_heights,
                merges: ws
                    .merged_cells
                    .iter()
                    .map(|m| {
                        format!(
                            "{}:{}",
                            cell_name(m.row, m.column),
                            cell_name(m.row + m.height - 1, m.column + m.width - 1)
                        )
                    })
                    .collect(),
                cells,
                charts,
            });
        }

        let names = self
            .model
            .get_defined_name_list()
            .into_iter()
            .map(|(name, scope, formula)| {
                let key = match scope.and_then(|s| self.model.workbook.worksheets.get(s as usize)) {
                    Some(ws) => format!("{}!{name}", ws.name),
                    None => name,
                };
                (key, formula)
            })
            .collect();

        View {
            sheets,
            styles,
            names,
        }
    }

    fn style(&self, (s, r, c): Key) -> Style {
        let Ok(style) = self.model.get_style_for_cell(s, r, c) else {
            return Style::default();
        };
        let theme = &self.model.workbook.theme;
        let side = |item: &Option<BorderItem>| {
            item.as_ref().map(|b| BorderSide {
                style: b.style.to_string(),
                color: rgb(&b.color, theme),
            })
        };
        let border = Borders {
            top: side(&style.border.top),
            right: side(&style.border.right),
            bottom: side(&style.border.bottom),
            left: side(&style.border.left),
        };
        let alignment = style.alignment.unwrap_or_default();
        Style {
            bold: style.font.b,
            italic: style.font.i,
            underline: style.font.u,
            wrap: alignment.wrap_text,
            fill: rgb(&style.fill.color, theme),
            color: rgb(&style.font.color, theme),
            size: Some(style.font.sz).filter(|sz| *sz != 11),
            align: (alignment.horizontal != HorizontalAlignment::General)
                .then(|| alignment.horizontal.to_string()),
            valign: (alignment.vertical != VerticalAlignment::Bottom)
                .then(|| alignment.vertical.to_string()),
            num_fmt: Some(style.num_fmt).filter(|f| f != "general"),
            border: (border != Borders::default()).then_some(border),
        }
    }

    /// A chart's anchor as a cell range. A one-cell anchor's size is walked
    /// across the sheet's column widths and row heights.
    fn anchor(&self, sheet: usize, anchor: &parts::Anchor) -> String {
        let ((r1, c1), (r2, c2)) = match *anchor {
            parts::Anchor::Cells { from, to } => (from, to),
            parts::Anchor::Extent { from, cx, cy } => {
                // EMU: 9525 per pixel.
                let ws = &self.model.workbook.worksheets[sheet];
                let (mut c, mut w) = (from.1, cx as f64 / 9525.0);
                while w > 0.0 && c < from.1 + 1000 {
                    w -= col_px(ws, c + 1).max(1.0);
                    c += 1;
                }
                let (mut r, mut h) = (from.0, cy as f64 / 9525.0);
                while h > 0.0 && r < from.0 + 10_000 {
                    h -= row_px(ws, r + 1).max(1.0);
                    r += 1;
                }
                (from, ((r - 1).max(from.0), (c - 1).max(from.1)))
            }
        };
        format!(
            "{}:{}",
            cell_name(r1 + 1, c1 + 1),
            cell_name(r2 + 1, c2 + 1)
        )
    }

    /// Type into cells, recalculate, and answer every cell whose value or
    /// display changed, on any sheet. Only editable (unlocked) input cells
    /// take input; anything else is refused with the reason.
    pub fn edit(&mut self, edits: &[Edit]) -> Edited {
        let before = self.values();
        let mut errors = Vec::new();
        let mut typed = Vec::new();
        for edit in edits {
            match self.apply(edit) {
                Ok(key) => typed.push(key),
                Err(error) => errors.push(EditError {
                    sheet: edit.sheet.clone(),
                    cell: edit.cell.clone(),
                    error,
                }),
            }
        }
        if !typed.is_empty() {
            self.model.evaluate();
        }
        let after = self.values();
        let mut keys: Vec<Key> = after
            .iter()
            .filter(|(key, value)| before.get(*key) != Some(*value))
            .map(|(key, _)| *key)
            .chain(
                before
                    .keys()
                    .filter(|key| !after.contains_key(*key))
                    .copied(),
            )
            .chain(typed)
            .collect();
        keys.sort_unstable();
        keys.dedup();
        let changed = keys
            .into_iter()
            .map(|key @ (s, r, c)| Changed {
                sheet: self.model.workbook.worksheets[s as usize].name.clone(),
                cell: cell_name(r, c),
                display: self.display(key),
                value: after.get(&key).cloned().unwrap_or(Value::Empty),
            })
            .collect();
        Edited { changed, errors }
    }

    fn apply(&mut self, edit: &Edit) -> Result<Key, String> {
        let at = format!("{}!{}", edit.sheet, edit.cell);
        let s = self
            .model
            .workbook
            .worksheets
            .iter()
            .position(|ws| ws.name == edit.sheet)
            .ok_or_else(|| format!("There is no sheet named \"{}\".", edit.sheet))?
            as u32;
        let (r, c) =
            parse_cell(&edit.cell).ok_or_else(|| format!("\"{}\" is not a cell.", edit.cell))?;
        let key = (s, r, c);
        if self.formulas.contains(&key) {
            return Err(format!(
                "{at} is calculated by a formula; only input cells can be edited."
            ));
        }
        let style = self.model.get_cell_style_index(s, r, c)?;
        if !self.parts.unlocked(style) {
            return Err(format!(
                "{at} is locked; only cells marked editable can be changed."
            ));
        }
        self.model.set_user_input(s, r, c, edit.input.clone())?;
        // Typing `10%` or `$5` makes IronCalc pick a matching number format;
        // the file's own format stays, as it would in a protected sheet.
        self.model
            .workbook
            .worksheet_mut(s)?
            .set_cell_style(r, c, style)?;
        self.edited.insert(key);
        Ok(key)
    }

    /// The xlsx with every changed cell patched in, or None when nothing
    /// differs from the file. The book keeps its edits until [`Book::commit`].
    pub fn save(&self) -> Result<Option<Vec<u8>>, Error> {
        let now = self.values();
        let mut keys: Vec<Key> = now
            .iter()
            .filter(|(key, value)| self.baseline.get(*key) != Some(*value))
            .map(|(key, _)| *key)
            .chain(
                self.baseline
                    .keys()
                    .filter(|key| !now.contains_key(*key))
                    .copied(),
            )
            .chain(self.edited.iter().copied())
            .collect();
        if keys.is_empty() {
            return Ok(None);
        }
        keys.sort_unstable();
        keys.dedup();
        let mut patches: BTreeMap<String, Vec<CellPatch>> = BTreeMap::new();
        for key @ (s, r, c) in keys {
            let cell = self.model.workbook.worksheets[s as usize].cell(r, c);
            let formula = if self.edited.contains(&key) {
                cell.and_then(|cell| self.formula(key, cell, true))
                    .map_or(Formula::Clear, Formula::Set)
            } else {
                Formula::Keep
            };
            patches
                .entry(self.parts.sheet_paths[s as usize].clone())
                .or_default()
                .push(CellPatch {
                    row: r,
                    col: c,
                    value: now.get(&key).cloned().unwrap_or(Value::Empty),
                    formula,
                });
        }
        patch::write(&self.xlsx, &patches).map(Some)
    }

    /// Make `xlsx` — what [`Book::save`] returned, now stored — the file this
    /// book edits from.
    pub fn commit(&mut self, xlsx: Vec<u8>) {
        self.xlsx = xlsx;
        self.baseline = self.values();
        self.edited.clear();
    }
}
