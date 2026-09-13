//! `pack` — the ONE way a seat creates, adds, lists, and removes the
//! industry, franchise, and company packs this Nebo works by.
//!
//! A pack is a directory under `packs/<slug>/` with exactly one marker
//! (`INDUSTRY.md`, `FRANCHISE.md`, or `COMPANY.md`) and typed folders. The
//! pack watcher sees the write and raises `layers_changed`, so every seat
//! reads the new pack and writes its own context section. This tool never
//! touches a seat; it only puts the pack where the loader looks.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

pub struct PackTool;

impl PackTool {
    fn dir() -> Result<PathBuf, String> {
        config::packs_dir().map_err(|e| e.to_string())
    }
}

fn slug_ok(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 64
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !slug.starts_with('-')
}

fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Write one pack from its parts into a staging dir, validate it with the
/// loader, then move it into place. A bad pack never reaches `packs/`.
fn create(input: &Value) -> Result<String, String> {
    let slug = input.get("slug").and_then(|v| v.as_str()).unwrap_or("").trim();
    if !slug_ok(slug) {
        return Err("`slug` is required: lowercase letters, digits, hyphens (e.g. `insurance-restoration-roofing`)".into());
    }
    let staging = PackTool::dir()?.join(format!(".staging-{slug}"));
    let out = create_in(input, slug, &staging);
    if out.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    out
}

fn create_in(input: &Value, slug: &str, staging: &Path) -> Result<String, String> {
    let layer = input.get("layer").and_then(|v| v.as_str()).unwrap_or("industry");
    let marker = match layer {
        "industry" => "INDUSTRY.md",
        "franchise" => "FRANCHISE.md",
        "company" => "COMPANY.md",
        other => return Err(format!("`layer` must be industry, franchise, or company, not `{other}`")),
    };
    let body = input.get("body").and_then(|v| v.as_str()).unwrap_or("").trim();
    if body.is_empty() {
        return Err("`body` is required: the marker's markdown, what a new hire must know about how this trade (or this company) runs".into());
    }
    let version = input.get("version").and_then(|v| v.as_str()).unwrap_or("0.1.0");
    let name = input.get("name").and_then(|v| v.as_str()).unwrap_or(slug);
    let capabilities: Vec<String> = input
        .get("capabilities")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let extra_fm = input.get("frontmatter").and_then(|v| v.as_object()).cloned().unwrap_or_default();

    let root = PackTool::dir()?;
    let dest = root.join(slug);
    if staging.exists() {
        std::fs::remove_dir_all(staging).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(staging).map_err(|e| e.to_string())?;

    let mut fm = serde_json::Map::new();
    fm.insert("type".into(), json!("pack"));
    fm.insert("scope".into(), json!(layer));
    fm.insert(layer.into(), json!(slug));
    fm.insert("name".into(), json!(name));
    fm.insert("version".into(), json!(version));
    fm.insert("capabilities".into(), json!(capabilities));
    for (k, v) in extra_fm {
        fm.insert(k, v);
    }
    let fm_yaml = fm
        .iter()
        .map(|(k, v)| format!("{k}: {}", serde_json::to_string(v).unwrap_or_default()))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(staging.join(marker), format!("---\n{fm_yaml}\n---\n\n{body}\n"))
        .map_err(|e| e.to_string())?;

    // Folders: {"rules": [{"name": "deductibles", "frontmatter": {...}, "body": "..."}], ...}
    let mut written = 0usize;
    if let Some(folders) = input.get("folders").and_then(|v| v.as_object()) {
        for (folder, entries) in folders {
            if !napp::pack::FOLDERS.contains(&folder.as_str()) {
                return Err(format!(
                    "unknown folder `{folder}`; a pack has only {}",
                    napp::pack::FOLDERS.join(", ")
                ));
            }
            let Some(entries) = entries.as_array() else { continue };
            let fdir = staging.join(folder);
            std::fs::create_dir_all(&fdir).map_err(|e| e.to_string())?;
            for e in entries {
                let ename = e.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
                if !name_ok(ename) {
                    return Err(format!("entry in `{folder}` needs a `name` (letters, digits, hyphens, underscores)"));
                }
                let ebody = e.get("body").and_then(|v| v.as_str()).unwrap_or("").trim();
                let efm = e.get("frontmatter").and_then(|v| v.as_object()).cloned().unwrap_or_default();
                let efm_yaml = efm
                    .iter()
                    .map(|(k, v)| format!("{k}: {}", serde_json::to_string(v).unwrap_or_default()))
                    .collect::<Vec<_>>()
                    .join("\n");
                let text = if efm_yaml.is_empty() {
                    format!("{ebody}\n")
                } else {
                    format!("---\n{efm_yaml}\n---\n\n{ebody}\n")
                };
                std::fs::write(fdir.join(format!("{ename}.md")), text).map_err(|e| e.to_string())?;
                written += 1;
            }
        }
    }

    // The loader is the validator. Nothing bad reaches packs/.
    let pack = napp::pack::load_pack(staging).map_err(|e| format!("the pack did not validate: {e}"))?;
    let replaced = dest.exists();
    if replaced {
        std::fs::remove_dir_all(&dest).map_err(|e| e.to_string())?;
    }
    std::fs::rename(staging, &dest).map_err(|e| e.to_string())?;
    Ok(format!(
        "{} pack `{}` {} at {} ({} entries: {} rules, {} laws, {} standards, {} questions). Every employee will read it now and write what matters to its job into its own context.",
        pack.layer.as_str(),
        pack.slug,
        if replaced { "updated" } else { "created" },
        dest.display(),
        written,
        pack.rules.len(),
        pack.laws.len(),
        pack.standards.len(),
        pack.questions.len(),
    ))
}

fn add_from_path(input: &Value) -> Result<String, String> {
    let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("").trim();
    if path.is_empty() {
        return Err("`path` is required: a folder holding one marker file and its typed folders".into());
    }
    let src = PathBuf::from(path);
    let pack = napp::pack::load_pack(&src).map_err(|e| format!("not a valid pack: {e}"))?;
    let dest = PackTool::dir()?.join(&pack.slug);
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(|e| e.to_string())?;
    }
    copy_dir(&src, &dest).map_err(|e| e.to_string())?;
    Ok(format!("{} pack `{}` added from {}. Every employee reads it now.", pack.layer.as_str(), pack.slug, src.display()))
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn list() -> Result<String, String> {
    let packs = napp::pack::scan_packs(&PackTool::dir()?);
    if packs.is_empty() {
        return Ok("No packs installed. Create one with `pack create` or add a folder with `pack add`.".into());
    }
    let rows: Vec<Value> = packs
        .iter()
        .map(|p| {
            json!({
                "layer": p.layer.as_str(),
                "slug": p.slug,
                "name": p.name,
                "version": p.version,
                "capabilities": p.capabilities,
                "rules": p.rules.len(),
                "laws": p.laws.len(),
                "standards": p.standards.len(),
                "questions": p.questions.len(),
                "vocabulary": p.vocabulary.len(),
                "parties": p.parties.len(),
                "workflows": p.workflows.len(),
                "reference": p.reference.len(),
            })
        })
        .collect();
    Ok(json!({ "packs": rows }).to_string())
}

fn show(input: &Value) -> Result<String, String> {
    let slug = input.get("slug").and_then(|v| v.as_str()).unwrap_or("").trim();
    if !slug_ok(slug) {
        return Err("`slug` is required".into());
    }
    let pack = napp::pack::load_pack(&PackTool::dir()?.join(slug)).map_err(|e| e.to_string())?;
    Ok(pack.prompt_text())
}

fn remove(input: &Value) -> Result<String, String> {
    let slug = input.get("slug").and_then(|v| v.as_str()).unwrap_or("").trim();
    if !slug_ok(slug) {
        return Err("`slug` is required".into());
    }
    let dest = PackTool::dir()?.join(slug);
    if !dest.is_dir() {
        return Err(format!("no pack `{slug}`"));
    }
    std::fs::remove_dir_all(&dest).map_err(|e| e.to_string())?;
    Ok(format!("pack `{slug}` removed. Every employee will drop what came only from it."))
}

impl DynTool for PackTool {
    fn name(&self) -> &str {
        "pack"
    }

    fn description(&self) -> String {
        "The industry, franchise, and company packs this company works by. \
         `create` writes a pack from parts (marker body + typed folders: vocabulary, parties, rules, laws, standards, workflows, reference); \
         `add` installs a pack folder from a path; `list`, `show`, `remove`. \
         A pack is knowledge, never a skill. Every employee reads a new or changed pack once and writes what matters to its own job into its context. \
         Laws carry `ceiling: [\"<capability.resource.action>\"]` in their frontmatter and are the only Blocked operations. \
         Rules may carry `always: true`. Standards carry `id` (semantic, dotted) and `value`; questions are standards with `question: true`, `scope: company|seat`, and `money: true` when a value is money (never a default)."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["create", "add", "list", "show", "remove"] },
                "slug": { "type": "string", "description": "Pack id: lowercase, digits, hyphens. Required for create, show, remove." },
                "layer": { "type": "string", "enum": ["industry", "franchise", "company"], "description": "create: which layer this pack is (default industry)." },
                "name": { "type": "string", "description": "create: display name." },
                "version": { "type": "string", "description": "create: semver, default 0.1.0." },
                "capabilities": { "type": "array", "items": { "type": "string" }, "description": "create: the capabilities the trade uses (mail, calendar, crm, ledger, billing, support, ...)." },
                "body": { "type": "string", "description": "create: the marker's markdown. How this trade (or company) runs, written for a new hire." },
                "frontmatter": { "type": "object", "description": "create: extra marker frontmatter (parent for a franchise pack, license, status...)." },
                "folders": {
                    "type": "object",
                    "description": "create: {folder: [{name, frontmatter?, body}]} for vocabulary, parties, rules, laws, standards, workflows, reference.",
                    "additionalProperties": { "type": "array", "items": { "type": "object", "properties": { "name": {"type":"string"}, "frontmatter": {"type":"object"}, "body": {"type":"string"} }, "required": ["name", "body"] } }
                },
                "path": { "type": "string", "description": "add: absolute path of a pack folder." }
            },
            "required": ["action"]
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("list");
            let out = match action {
                "create" => create(&input),
                "add" => add_from_path(&input),
                "list" => list(),
                "show" => show(&input),
                "remove" => remove(&input),
                other => Err(format!("unknown action `{other}`")),
            };
            match out {
                Ok(s) => ToolResult::ok(s),
                Err(e) => ToolResult::error(format!("pack {action}: {e}")),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pack_created_from_parts_loads_and_a_bad_one_never_lands() {
        let root = std::env::temp_dir().join(format!("nebo-packtool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        // SAFETY: test-only override of the data dir for this process.
        unsafe { std::env::set_var("NEBO_HOME", &root) };
        let ok = create(&json!({
            "action": "create", "slug": "sample-trade", "layer": "industry", "name": "Sample trade",
            "capabilities": ["mail"], "body": "# Sample trade\n\nHow it runs.",
            "folders": {
                "rules": [{"name": "deposits", "frontmatter": {"always": true}, "body": "Deposits are due before work starts."}],
                "laws": [{"name": "no-waived-deductibles", "frontmatter": {"ceiling": ["billing.invoice.discount"]}, "body": "A deductible is never waived."}],
                "standards": [{"name": "net-terms", "frontmatter": {"id": "finance.ar.net_terms", "value": 30}, "body": "Net terms."}]
            }
        }))
        .unwrap();
        assert!(ok.contains("created"), "{ok}");
        let packs = napp::pack::scan_packs(&config::packs_dir().unwrap());
        assert_eq!(packs.len(), 1);
        assert_eq!(packs[0].laws.len(), 1);
        assert_eq!(packs[0].standards.len(), 1);

        let bad = create(&json!({"action": "create", "slug": "bad", "body": "x", "folders": {"skills": [{"name": "s", "body": "x"}]}}));
        assert!(bad.is_err());
        assert!(!config::packs_dir().unwrap().join("bad").exists());
        assert!(!config::packs_dir().unwrap().join(".staging-bad").exists());

        assert!(remove(&json!({"slug": "sample-trade"})).is_ok());
        assert!(napp::pack::scan_packs(&config::packs_dir().unwrap()).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
