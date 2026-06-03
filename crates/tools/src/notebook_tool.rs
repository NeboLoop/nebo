//! NotebookTool — edit and inspect Jupyter notebook (.ipynb) cells.
//!
//! A notebook is JSON with a top-level `cells` array. `os(file, edit)` would corrupt
//! that structure (string replace across JSON), so cell-level edits go through here.
//! Mirrors Claude Code's NotebookEdit: replace / insert / delete a cell, located by its
//! `id` (nbformat ≥4.5) or by zero-based index.

use crate::errors;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};
use serde_json::{json, Value};

pub struct NotebookTool;

impl NotebookTool {
    pub fn new() -> Self {
        Self
    }

    /// Render a cell's `source` (string or array of lines) as a single string.
    fn source_to_string(source: &Value) -> String {
        match source {
            Value::String(s) => s.clone(),
            Value::Array(arr) => arr
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(""),
            _ => String::new(),
        }
    }

    /// Locate a cell index by `cell_id` — matches a cell's `id` field, else a numeric index.
    fn find_cell(cells: &[Value], cell_id: &str) -> Option<usize> {
        if let Some(idx) = cells
            .iter()
            .position(|c| c.get("id").and_then(|v| v.as_str()) == Some(cell_id))
        {
            return Some(idx);
        }
        cell_id.parse::<usize>().ok().filter(|&i| i < cells.len())
    }
}

impl DynTool for NotebookTool {
    fn name(&self) -> &str {
        "notebook"
    }

    fn description(&self) -> String {
        "Edit or inspect Jupyter notebook (.ipynb) cells. Notebooks are JSON — use this, \
         not os(file, edit), for cell changes.\n\
         - notebook(action: \"read\", notebook_path: \"/path/nb.ipynb\") — list cells (index, id, type, preview)\n\
         - notebook(action: \"edit\", notebook_path, cell_id, new_source) — replace a cell's source\n\
         - notebook(action: \"edit\", notebook_path, cell_id, new_source, cell_type, edit_mode: \"insert\") — insert a new cell after cell_id (cell_type required)\n\
         - notebook(action: \"edit\", notebook_path, cell_id, edit_mode: \"delete\") — delete a cell\n\
         cell_id matches a cell's id, or a zero-based index. For insert, an empty cell_id inserts at the top."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["read", "edit"], "description": "read = list cells; edit = modify a cell" },
                "notebook_path": { "type": "string", "description": "Absolute path to the .ipynb file" },
                "cell_id": { "type": "string", "description": "Cell id, or zero-based index. Empty with edit_mode=insert inserts at the top." },
                "new_source": { "type": "string", "description": "New cell source (for replace/insert)" },
                "cell_type": { "type": "string", "enum": ["code", "markdown"], "description": "Cell type (required for insert)" },
                "edit_mode": { "type": "string", "enum": ["replace", "insert", "delete"], "description": "Defaults to replace" }
            },
            "required": ["action", "notebook_path"],
            "additionalProperties": false
        })
    }

    fn requires_approval(&self) -> bool {
        true
    }

    /// Reading cells is read-only; only edits need approval.
    fn requires_approval_for(&self, input: &Value) -> bool {
        input.get("action").and_then(|v| v.as_str()) != Some("read")
    }

    fn is_concurrent_safe(&self, input: &Value) -> bool {
        input.get("action").and_then(|v| v.as_str()) == Some("read")
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
            let path = match input.get("notebook_path").and_then(|v| v.as_str()) {
                Some(p) if !p.is_empty() => crate::file_tool::expand_path(p),
                _ => {
                    return ToolResult::error(errors::missing_param(
                        action,
                        "notebook_path",
                        "notebook(action: \"read\", notebook_path: \"/path/nb.ipynb\")",
                    ))
                }
            };

            let raw = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return ToolResult::error(errors::file_not_found(&path))
                }
                Err(e) => return ToolResult::error(format!("Error reading notebook: {}", e)),
            };
            let mut nb: Value = match serde_json::from_str(&raw) {
                Ok(v) => v,
                Err(e) => return ToolResult::error(format!("Not a valid .ipynb (JSON parse failed): {}", e)),
            };

            let cells = match nb.get_mut("cells").and_then(|c| c.as_array_mut()) {
                Some(c) => c,
                None => return ToolResult::error("Notebook has no `cells` array — not a valid .ipynb."),
            };

            if action == "read" {
                let mut lines = vec![format!("{} cells in {}", cells.len(), path)];
                for (i, cell) in cells.iter().enumerate() {
                    let id = cell.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                    let ct = cell.get("cell_type").and_then(|v| v.as_str()).unwrap_or("?");
                    let src = Self::source_to_string(cell.get("source").unwrap_or(&Value::Null));
                    let preview = src.lines().next().unwrap_or("").chars().take(80).collect::<String>();
                    lines.push(format!("[{}] id={} {} | {}", i, id, ct, preview));
                }
                return ToolResult::ok(lines.join("\n"));
            }

            if action != "edit" {
                return ToolResult::error(format!("Unknown action: {} (valid: read, edit)", action));
            }

            let cell_id = input.get("cell_id").and_then(|v| v.as_str()).unwrap_or("");
            let new_source = input.get("new_source").and_then(|v| v.as_str()).unwrap_or("");
            let cell_type = input.get("cell_type").and_then(|v| v.as_str());
            let edit_mode = input.get("edit_mode").and_then(|v| v.as_str()).unwrap_or("replace");

            let summary = match edit_mode {
                "delete" => {
                    let idx = match Self::find_cell(cells, cell_id) {
                        Some(i) => i,
                        None => return ToolResult::error(format!("Cell not found: {}", cell_id)),
                    };
                    cells.remove(idx);
                    format!("Deleted cell {} from {}", cell_id, path)
                }
                "insert" => {
                    let ct = match cell_type {
                        Some(c) => c,
                        None => return ToolResult::error("cell_type is required for insert (code or markdown)"),
                    };
                    let mut new_cell = json!({
                        "cell_type": ct,
                        "metadata": {},
                        "source": new_source,
                    });
                    if ct == "code" {
                        new_cell["outputs"] = json!([]);
                        new_cell["execution_count"] = Value::Null;
                    }
                    // Insert AFTER cell_id, or at the top when cell_id is empty.
                    let at = if cell_id.is_empty() {
                        0
                    } else {
                        match Self::find_cell(cells, cell_id) {
                            Some(i) => i + 1,
                            None => return ToolResult::error(format!("Cell not found: {}", cell_id)),
                        }
                    };
                    cells.insert(at, new_cell);
                    format!("Inserted {} cell at position {} in {}", ct, at, path)
                }
                "replace" => {
                    let idx = match Self::find_cell(cells, cell_id) {
                        Some(i) => i,
                        None => return ToolResult::error(format!("Cell not found: {}", cell_id)),
                    };
                    cells[idx]["source"] = json!(new_source);
                    if let Some(ct) = cell_type {
                        cells[idx]["cell_type"] = json!(ct);
                    }
                    // Editing a code cell invalidates its prior outputs.
                    if cells[idx].get("cell_type").and_then(|v| v.as_str()) == Some("code") {
                        cells[idx]["outputs"] = json!([]);
                        cells[idx]["execution_count"] = Value::Null;
                    }
                    format!("Replaced cell {} in {}", cell_id, path)
                }
                other => return ToolResult::error(format!(
                    "Unknown edit_mode: {} (valid: replace, insert, delete)",
                    other
                )),
            };

            let serialized = match serde_json::to_string_pretty(&nb) {
                Ok(s) => s,
                Err(e) => return ToolResult::error(format!("Error serializing notebook: {}", e)),
            };
            if let Err(e) = std::fs::write(&path, serialized) {
                return ToolResult::error(format!("Error writing notebook: {}", e));
            }
            ToolResult::ok(summary)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::origin::{Origin, ToolContext};

    fn ctx() -> ToolContext {
        ToolContext::new(Origin::User)
    }

    fn sample_nb() -> String {
        json!({
            "cells": [
                {"cell_type": "markdown", "id": "intro", "metadata": {}, "source": "# Title"},
                {"cell_type": "code", "id": "c1", "metadata": {}, "source": "print(1)", "outputs": [], "execution_count": 1}
            ],
            "metadata": {},
            "nbformat": 4,
            "nbformat_minor": 5
        })
        .to_string()
    }

    fn write_nb(dir: &std::path::Path) -> std::path::PathBuf {
        let p = dir.join("n.ipynb");
        std::fs::write(&p, sample_nb()).unwrap();
        p
    }

    #[tokio::test]
    async fn read_lists_cells() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_nb(dir.path());
        let tool = NotebookTool::new();
        let r = tool
            .execute_dyn(&ctx(), json!({"action": "read", "notebook_path": p.to_str().unwrap()}))
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("id=intro"));
        assert!(r.content.contains("id=c1"));
    }

    #[tokio::test]
    async fn replace_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_nb(dir.path());
        let tool = NotebookTool::new();
        let r = tool
            .execute_dyn(&ctx(), json!({"action":"edit","notebook_path": p.to_str().unwrap(),"cell_id":"c1","new_source":"print(2)"}))
            .await;
        assert!(!r.is_error, "{}", r.content);
        let nb: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(nb["cells"][1]["source"], "print(2)");
        // Code cell outputs are reset on edit.
        assert_eq!(nb["cells"][1]["outputs"], json!([]));
    }

    #[tokio::test]
    async fn insert_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_nb(dir.path());
        let tool = NotebookTool::new();
        let pth = p.to_str().unwrap();
        // Insert a markdown cell after the first cell (index 0 / id "intro").
        let r = tool
            .execute_dyn(&ctx(), json!({"action":"edit","notebook_path": pth,"cell_id":"intro","new_source":"## Section","cell_type":"markdown","edit_mode":"insert"}))
            .await;
        assert!(!r.is_error, "{}", r.content);
        let nb: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(nb["cells"].as_array().unwrap().len(), 3);
        assert_eq!(nb["cells"][1]["source"], "## Section");

        // Delete it by index.
        let r = tool
            .execute_dyn(&ctx(), json!({"action":"edit","notebook_path": pth,"cell_id":"1","edit_mode":"delete"}))
            .await;
        assert!(!r.is_error, "{}", r.content);
        let nb: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(nb["cells"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn insert_requires_cell_type() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_nb(dir.path());
        let tool = NotebookTool::new();
        let r = tool
            .execute_dyn(&ctx(), json!({"action":"edit","notebook_path": p.to_str().unwrap(),"cell_id":"intro","new_source":"x","edit_mode":"insert"}))
            .await;
        assert!(r.is_error);
        assert!(r.content.contains("cell_type is required"));
    }
}
