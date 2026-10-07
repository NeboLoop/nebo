//! The fixture `model.xlsx` is `model.json` built by nebo-office
//! (`nebo-office xlsx create model.json`) with a calcChain part added, as
//! Excel writes one: three sheets, unlocked inputs on a protected sheet,
//! formulas across sheets through defined names, a chart, data validation.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use nebo_sheet::{Book, Edit};
use serde_json::{Value, json};

const FIXTURE: &[u8] = include_bytes!("fixtures/model.xlsx");

fn book() -> Book {
    Book::open(FIXTURE.to_vec()).expect("fixture opens")
}

fn view(book: &Book) -> Value {
    serde_json::to_value(book.view(None, None)).expect("view serializes")
}

fn cell<'a>(view: &'a Value, sheet: &str, r: i64, c: i64) -> &'a Value {
    view["sheets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == sheet)
        .unwrap_or_else(|| panic!("no sheet {sheet}"))["cells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|cell| cell["r"] == r && cell["c"] == c)
        .unwrap_or_else(|| panic!("no cell {sheet} r{r} c{c}"))
}

fn edit(sheet: &str, cell: &str, input: &str) -> Edit {
    Edit {
        sheet: sheet.into(),
        cell: cell.into(),
        input: input.into(),
    }
}

fn parts(xlsx: &[u8]) -> HashMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(xlsx)).unwrap();
    (0..zip.len())
        .map(|i| {
            let mut file = zip.by_index(i).unwrap();
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).unwrap();
            (file.name().to_string(), bytes)
        })
        .collect()
}

#[test]
fn view_model_matches_the_contract() {
    let view = view(&book());
    let sheets = view["sheets"].as_array().unwrap();
    assert_eq!(
        sheets
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["Inputs", "Lookups", "Model"]
    );

    let inputs = &sheets[0];
    assert_eq!(inputs["tabColor"], "#4472C4");
    assert_eq!(inputs["hidden"], false);
    assert_eq!(inputs["protected"], true);
    assert_eq!(inputs["freeze"], json!({"rows": 1, "cols": 1}));
    assert_eq!(inputs["dims"], json!({"rows": 5, "cols": 2}));
    assert_eq!(inputs["colWidths"]["A"], 168.0);
    assert_eq!(inputs["rowHeights"]["1"], 32.0);
    assert_eq!(sheets[1]["protected"], false);

    let price = cell(&view, "Inputs", 2, 2);
    assert_eq!(price["display"], "$30");
    assert_eq!(price["value"], 30.0);
    assert_eq!(price["formula"], Value::Null);
    assert_eq!(
        view["styles"][price["style"].as_u64().unwrap() as usize]["numFmt"],
        "$#,##0"
    );

    let header = cell(&view, "Inputs", 1, 1);
    let header_style = &view["styles"][header["style"].as_u64().unwrap() as usize];
    assert_eq!(header_style["bold"], true);
    assert_eq!(header_style["fill"], "#EDD8A0");

    let revenue = cell(&view, "Model", 3, 3);
    assert_eq!(revenue["display"], "$2,700.00");
    assert_eq!(revenue["value"], 2700.0);
    assert_eq!(
        revenue["formula"],
        "=B3*price*(1-INDEX(Lookups!B2:B4,MATCH(Inputs!B5,Lookups!A2:A4,0)))"
    );
    assert_eq!(
        cell(&view, "Model", 4, 2)["formula"],
        "=ROUNDUP(B3*(1+growth),0)"
    );
    assert_eq!(cell(&view, "Model", 3, 4)["value"], "small");

    let model = &sheets[2];
    assert_eq!(model["merges"], json!(["A1:D1"]));
    let chart = &model["charts"][0];
    assert_eq!(chart["type"], "line");
    assert_eq!(chart["title"], "Revenue");
    assert_eq!(chart["anchor"], "F2:M16");
    assert_eq!(chart["series"][0]["name"], "Revenue");
    assert_eq!(chart["series"][0]["ref"], "'Model'!$C$3:$C$5");
    assert_eq!(chart["series"][0]["categories"], "'Model'!$A$3:$A$5");

    assert_eq!(view["names"]["price"], "Inputs!$B$2");
    assert_eq!(view["names"]["growth"], "Inputs!$B$3");
}

#[test]
fn only_unlocked_inputs_are_editable() {
    let view = view(&book());
    for row in 2..=5 {
        assert_eq!(
            cell(&view, "Inputs", row, 2)["editable"],
            true,
            "Inputs row {row}"
        );
        assert_eq!(
            cell(&view, "Inputs", row, 1)["editable"],
            false,
            "label row {row}"
        );
    }
    assert_eq!(cell(&view, "Model", 3, 3)["editable"], false);
    assert_eq!(cell(&view, "Lookups", 2, 2)["editable"], false);
}

#[test]
fn rows_page_the_cells() {
    let view = serde_json::to_value(book().view(Some("Model"), Some(3..=4))).unwrap();
    assert_eq!(view["sheets"][0]["cells"], json!([]));
    assert_eq!(view["sheets"][0]["dims"], json!({"rows": 5, "cols": 2}));
    let rows: Vec<i64> = view["sheets"][2]["cells"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["r"].as_i64().unwrap())
        .collect();
    assert!(
        !rows.is_empty() && rows.iter().all(|r| (3..=4).contains(r)),
        "{rows:?}"
    );
    assert_eq!(view["sheets"][2]["dims"], json!({"rows": 8, "cols": 4}));
}

#[test]
fn an_edit_recalculates_dependents_across_sheets() {
    let mut book = book();
    let result = serde_json::to_value(book.edit(&[edit("Inputs", "B2", "40")])).unwrap();
    assert_eq!(result["errors"], json!([]));
    let changed: HashMap<String, Value> = result["changed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                format!(
                    "{}!{}",
                    c["sheet"].as_str().unwrap(),
                    c["cell"].as_str().unwrap()
                ),
                c.clone(),
            )
        })
        .collect();
    assert_eq!(changed["Inputs!B2"]["display"], "$40");
    // 100 customers × $40 × (1 − 10% Pro discount)
    assert_eq!(changed["Model!C3"]["value"], 3600.0);
    assert_eq!(changed["Model!C3"]["display"], "$3,600.00");
    assert_eq!(changed["Model!C6"]["value"], 12400.0);
    assert!(
        !changed.contains_key("Model!B3"),
        "customers do not depend on price"
    );

    // Text input flows through lookups: the tier picks a different discount.
    let result = serde_json::to_value(book.edit(&[edit("Inputs", "B5", "Max")])).unwrap();
    let c3 = result["changed"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["sheet"] == "Model" && c["cell"] == "C3")
        .unwrap();
    assert_eq!(c3["value"], 3200.0);
}

#[test]
fn locked_and_formula_cells_are_refused() {
    let mut book = book();
    let result = serde_json::to_value(book.edit(&[
        edit("Inputs", "A2", "Cost"),
        edit("Model", "C3", "5"),
        edit("Nope", "A1", "1"),
        edit("Inputs", "B3", "20%"),
    ]))
    .unwrap();
    let errors = result["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 3, "{errors:?}");
    assert!(errors[0]["error"].as_str().unwrap().contains("locked"));
    assert!(errors[1]["error"].as_str().unwrap().contains("formula"));
    assert!(errors[2]["error"].as_str().unwrap().contains("no sheet"));
    // The valid edit in the same batch still applied, in the file's own format.
    let b3 = result["changed"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["sheet"] == "Inputs" && c["cell"] == "B3")
        .unwrap();
    assert_eq!(b3["value"], 0.2);
    assert_eq!(b3["display"], "20.0%");
}

#[test]
fn nothing_to_save_without_a_change() {
    let mut book = book();
    assert!(book.save().unwrap().is_none());
    book.edit(&[edit("Inputs", "B2", "30")]);
    // Typed, even to the same value: the cell is written.
    assert!(book.save().unwrap().is_some());
}

#[test]
fn save_patches_cells_in_place_and_keeps_every_other_part() {
    let mut book = book();
    book.edit(&[edit("Inputs", "B2", "40"), edit("Inputs", "B5", "Max")]);
    let saved = book.save().unwrap().expect("there are changes");

    let before = parts(FIXTURE);
    let after = parts(&saved);
    assert!(!after.contains_key("xl/calcChain.xml"));
    for (name, bytes) in &before {
        let rewritten = [
            "xl/calcChain.xml",
            "xl/worksheets/sheet1.xml",
            "xl/worksheets/sheet3.xml",
            "xl/workbook.xml",
            "xl/_rels/workbook.xml.rels",
            "[Content_Types].xml",
        ];
        if !rewritten.contains(&name.as_str()) {
            assert_eq!(
                after.get(name),
                Some(bytes),
                "{name} must be byte-identical"
            );
        }
    }
    for kept in [
        "xl/charts/chart1.xml",
        "xl/drawings/drawing1.xml",
        "xl/theme/theme1.xml",
        "xl/styles.xml",
        "xl/sharedStrings.xml",
    ] {
        assert!(after.contains_key(kept), "{kept}");
    }

    let text = |name: &str| String::from_utf8(after[name].clone()).unwrap();
    assert!(!text("xl/_rels/workbook.xml.rels").contains("calcChain"));
    assert!(!text("[Content_Types].xml").contains("calcChain"));
    assert!(text("xl/workbook.xml").contains(r#"<calcPr calcId="191029" fullCalcOnLoad="1"/>"#));

    let inputs = text("xl/worksheets/sheet1.xml");
    assert!(
        inputs.contains(r#"<c r="B2" s="3"><v>40</v></c>"#),
        "{inputs}"
    );
    assert!(
        inputs.contains(r#"<c r="B5" s="5" t="inlineStr"><is><t>Max</t></is></c>"#),
        "{inputs}"
    );
    // Protection and validation around the cells are untouched.
    assert!(inputs.contains("<sheetProtection"));
    assert!(inputs.contains("<dataValidations"));
    let model = text("xl/worksheets/sheet3.xml");
    assert!(
        model.contains("<c r=\"C3\" s=\"6\"><f>B3*price*(1-INDEX(Lookups!B2:B4,MATCH(Inputs!B5,Lookups!A2:A4,0)))</f><v>3200</v></c>"),
        "{model}"
    );
    assert!(model.contains(r#"<c r="D5" t="str"><f>CHOOSE("#), "{model}");
    // Cells that did not change keep their exact bytes.
    assert!(model.contains(r#"<c r="B3"><f>Inputs!B4</f><v>100</v></c>"#));

    // The stored results read back as written, without recalculating.
    let workbook = ironcalc::import::load_from_xlsx_bytes(&saved, "saved", "en", "UTC").unwrap();
    let stored = ironcalc::base::Model::from_workbook(workbook, "en").unwrap();
    let number = |s, r, c| match stored.get_cell_value_by_index(s, r, c).unwrap() {
        ironcalc::base::cell::CellValue::Number(n) => n,
        other => panic!("not a number: {other:?}"),
    };
    assert_eq!(number(0, 2, 2), 40.0);
    assert_eq!(number(2, 3, 3), 3200.0);
    assert_eq!(number(2, 6, 3), 3200.0 + 3520.0 + 4840.0);

    // And the saved file opens as a book with the same numbers.
    let reopened = Book::open(saved.clone()).unwrap();
    let view = serde_json::to_value(reopened.view(None, None)).unwrap();
    assert_eq!(cell(&view, "Inputs", 5, 2)["value"], "Max");
    assert_eq!(cell(&view, "Model", 6, 3)["value"], 11560.0);
    assert_eq!(cell(&view, "Inputs", 2, 2)["editable"], true);

    // After a commit the book edits from the saved file: nothing left to save.
    book.commit(saved);
    assert!(book.save().unwrap().is_none());
}

#[test]
fn clearing_an_input_keeps_its_style() {
    let mut book = book();
    // B6 has no cell and no unlocked style: refused like any locked cell.
    let result = serde_json::to_value(book.edit(&[edit("Inputs", "B6", "1")])).unwrap();
    assert_eq!(result["errors"].as_array().unwrap().len(), 1);
    // Clearing an input empties the cell and keeps its style.
    book.edit(&[edit("Inputs", "B4", "")]);
    let saved = book.save().unwrap().unwrap();
    let inputs = String::from_utf8(parts(&saved)["xl/worksheets/sheet1.xml"].clone()).unwrap();
    assert!(inputs.contains(r#"<c r="B4" s="5"/>"#), "{inputs}");
}
