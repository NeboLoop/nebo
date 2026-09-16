//! `pack` — the ONE way a seat creates, adds, lists, and removes the
//! industry, franchise, and company packs this Nebo works by.
//!
//! A pack is a directory under `packs/<slug>/` with exactly one marker
//! (`INDUSTRY.md`, `FRANCHISE.md`, or `COMPANY.md`) and typed folders. Every
//! write here lands through `napp::commit_change`, the ONE gate a pack change
//! passes on any path (CODE_AUDITOR 8.1) — the owner's layers screen, an org
//! install and this tool all stage the change and let the real loader refuse it,
//! so there is one place the loader has to be the gate and no path can drift past
//! it. This tool never touches a seat; it only puts the pack where the loader
//! looks. The pack watcher then PARKS the change for the owner: a layer edit
//! reaches a seat when the owner applies it, never on a write.

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

/// Write one pack from its parts. `napp::commit_change` stages it and lets the
/// loader refuse it, so a bad pack never reaches `packs/`.
fn create(input: &Value) -> Result<String, String> {
    let slug = input.get("slug").and_then(|v| v.as_str()).unwrap_or("").trim();
    if !slug_ok(slug) {
        return Err("`slug` is required: lowercase letters, digits, hyphens (e.g. `insurance-restoration-roofing`)".into());
    }
    let dest = PackTool::dir()?.join(slug);
    let replaced = dest.exists();
    let mut written = 0usize;
    let pack = napp::commit_change(&dest, |staging| {
        written = build_pack(input, slug, staging)?;
        Ok(())
    })
    .map_err(|e| format!("the pack did not validate: {e}"))?;
    Ok(format!(
        "{} pack `{}` {} at {} ({} entries: {} rules, {} laws, {} standards, {} questions). It is parked for the owner: every employee reads it when the owner applies the layer change.",
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

/// Write the pack's files into `staging`, returning how many typed entries it
/// wrote. The loader reads what this leaves behind.
fn build_pack(input: &Value, slug: &str, staging: &Path) -> Result<usize, napp::PackError> {
    let refuse = |m: String| napp::PackError::File(slug.to_string(), m);
    let layer = input.get("layer").and_then(|v| v.as_str()).unwrap_or("industry");
    let marker = match layer {
        "industry" => "INDUSTRY.md",
        "franchise" => "FRANCHISE.md",
        // Writing the company layer is a gated operation
        // (`layers.company.write`, critical): the owner grants a seat that
        // authority on the employee's Approvals screen, and the runner's
        // per-operation gate has already decided by the time the call arrives
        // here. Authority is the employee's, not the channel's — so this
        // function only has to write a valid pack.
        "company" => "COMPANY.md",
        other => {
            return Err(refuse(format!(
                "`layer` must be industry, franchise, or company, not `{other}`"
            )))
        }
    };
    let body = input.get("body").and_then(|v| v.as_str()).unwrap_or("").trim();
    if body.is_empty() {
        return Err(refuse("`body` is required: the marker's markdown, what a new hire must know about how this trade (or this company) runs".into()));
    }
    let version = input.get("version").and_then(|v| v.as_str()).unwrap_or("0.1.0");
    let name = input.get("name").and_then(|v| v.as_str()).unwrap_or(slug);
    let capabilities: Vec<String> = input
        .get("capabilities")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let extra_fm = input.get("frontmatter").and_then(|v| v.as_object()).cloned().unwrap_or_default();

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
    std::fs::write(staging.join(marker), format!("---\n{fm_yaml}\n---\n\n{body}\n"))?;

    // Folders: {"rules": [{"name": "deductibles", "frontmatter": {...}, "body": "..."}], ...}
    let mut written = 0usize;
    if let Some(folders) = input.get("folders").and_then(|v| v.as_object()) {
        for (folder, entries) in folders {
            if !napp::pack::FOLDERS.contains(&folder.as_str()) {
                return Err(refuse(format!(
                    "unknown folder `{folder}`; a pack has only {}",
                    napp::pack::FOLDERS.join(", ")
                )));
            }
            let Some(entries) = entries.as_array() else { continue };
            let fdir = staging.join(folder);
            std::fs::create_dir_all(&fdir)?;
            for e in entries {
                let ename = e.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
                if !name_ok(ename) {
                    return Err(refuse(format!("entry in `{folder}` needs a `name` (letters, digits, hyphens, underscores)")));
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
                std::fs::write(fdir.join(format!("{ename}.md")), text)?;
                written += 1;
            }
        }
    }
    Ok(written)
}

fn add_from_path(input: &Value) -> Result<String, String> {
    let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("").trim();
    if path.is_empty() {
        return Err("`path` is required: a folder holding one marker file and its typed folders".into());
    }
    let src = PathBuf::from(path);
    let slug = napp::pack::load_pack(&src)
        .map_err(|e| format!("not a valid pack: {e}"))?
        .slug;
    let dest = PackTool::dir()?.join(&slug);
    // The same gate: the folder is copied into staging, read by the loader, and
    // only then replaces what is there — so a copy that fails halfway cannot
    // leave a broken pack where a whole one stood.
    let pack = napp::commit_change(&dest, |staging| napp::copy_tree(&src, staging))
        .map_err(|e| format!("not a valid pack: {e}"))?;
    Ok(format!(
        "{} pack `{}` added from {}. It is parked for the owner: every employee reads it when the owner applies the layer change.",
        pack.layer.as_str(),
        pack.slug,
        src.display()
    ))
}

/// The layer a pack directory declares, read off its marker file. This is
/// how a call that names a pack by path (`add`) or by slug (`remove`) says
/// which layer it touches, so putting a company pack in from a folder is the
/// same operation as writing one.
fn marker_layer(dir: &Path) -> Option<&'static str> {
    [
        ("COMPANY.md", "company"),
        ("FRANCHISE.md", "franchise"),
        ("INDUSTRY.md", "industry"),
    ]
    .into_iter()
    .find(|(marker, _)| dir.join(marker).is_file())
    .map(|(_, layer)| layer)
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
    Ok(format!("pack `{slug}` removed. It is parked for the owner: every employee drops what came only from it when the owner applies the layer change."))
}

impl DynTool for PackTool {
    fn name(&self) -> &str {
        "pack"
    }

    fn description(&self) -> String {
        "The industry, franchise, and company packs this company works by. \
         `create` writes a pack from parts (marker body + typed folders: vocabulary, parties, rules, laws, standards, workflows, reference); \
         `add` installs a pack folder from a path; `list`, `show`, `remove`. \
         A pack is knowledge, never a skill. A write is parked for the owner; when the owner applies it, every employee reads the change once and writes what matters to its own job into its context. \
         Laws carry `ceiling: [\"<capability.resource.action>\"]` in their frontmatter and are the only Blocked operations. \
         Rules may carry `always: true`. \
         A `standards/` entry is a settled value ONLY when it has a dotted `id` and a `value` and neither a `question:` nor a `kind:` key; an entry with no `id` is skipped entirely. \
         Any `question:` or `kind:` key makes the entry a question instead: write `question: <local_key>` — a string, the key the answer is stored under (`question: true` still loads as a question, but it is then named after the file) — with `scope: company|seat`, a `label:` written as the question, and `missing:` saying what the company does until it is answered. \
         A question that also carries a `value` (or a `default:`) uses it as its default; a money question is written with neither, so nothing is ever guessed."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["create", "add", "list", "show", "remove"] },
                "slug": { "type": "string", "description": "Pack id: lowercase, digits, hyphens. Required for create, show, remove." },
                "layer": { "type": "string", "enum": ["industry", "franchise", "company"], "description": "create: which layer this pack is (default industry). Writing or removing the company layer needs the owner's authority for this employee (Settings → the employee → Approvals); without it the owner is asked when the call runs." },
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
                "path": { "type": "string", "description": "add: absolute path of a pack folder." },
                "display": { "type": "string", "description": "REQUIRED when the call writes or removes a layer: ONE plain-language sentence for the owner's approval prompt, in words a non-technical person reads at a glance. Example: 'Write the company file for Acme Roofing: how the business runs, three rules and two numbers.'" }
            },
            "required": ["action"]
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    /// Which gated operation this call performs, for the runner's
    /// per-operation gate. Writing a layer is `layers.<layer>.write` and
    /// removing one `layers.<layer>.remove`; `list` and `show` read, so they
    /// perform none. The tool decides nothing here — `OperationPolicy` does.
    fn operation_performed(&self, input: &Value) -> Option<String> {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("list");
        let arg = |key: &str| {
            input
                .get(key)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let (layer, verb) = match action {
            "create" => (
                match input.get("layer").and_then(|v| v.as_str()).unwrap_or("industry") {
                    l @ ("industry" | "franchise" | "company") => l,
                    // `create` refuses any other layer, so nothing is performed.
                    _ => return None,
                },
                "write",
            ),
            "add" => {
                let path = arg("path");
                if path.is_empty() {
                    return None;
                }
                (marker_layer(Path::new(&path))?, "write")
            }
            "remove" => {
                let slug = arg("slug");
                if !slug_ok(&slug) {
                    return None;
                }
                (marker_layer(&Self::dir().ok()?.join(slug))?, "remove")
            }
            _ => return None,
        };
        Some(format!("layers.{layer}.{verb}"))
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
        let _g = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

        // The company layer is written by an employee the owner gave that
        // authority to, so this tool writes a valid pack and says which
        // operation the call performs; the per-operation gate decides whether
        // it may run (see the policy tests — if THAT inverts, any employee
        // rewrites the company's own rules). What is checked here is that the
        // call is reported as the critical company write, in both directions.
        let company = json!({
            "action": "create", "slug": "acme", "layer": "company", "name": "Acme",
            "body": "Fix roofs and get paid.",
        });
        assert_eq!(
            PackTool.operation_performed(&company).as_deref(),
            Some("layers.company.write"),
        );
        assert!(create(&company).is_ok());
        assert!(config::packs_dir().unwrap().join("acme").join("COMPANY.md").is_file());
        // The removal's layer is read off the marker on disk, so removing the
        // company pack by slug is the company's own removal operation.
        let drop_company = json!({"action": "remove", "slug": "acme"});
        assert_eq!(
            PackTool.operation_performed(&drop_company).as_deref(),
            Some("layers.company.remove"),
        );
        assert!(remove(&drop_company).is_ok());
        // Gone from disk, so there is no operation left to perform on it.
        assert_eq!(PackTool.operation_performed(&drop_company), None);

        assert!(remove(&json!({"slug": "sample-trade"})).is_ok());
        assert!(napp::pack::scan_packs(&config::packs_dir().unwrap()).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// What the runner's per-operation gate asks this tool. A create names the
    /// layer it writes; an `add` and a `remove` read the layer off the marker;
    /// reading a layer performs nothing, so the gate never fires on `list` or
    /// `show`. A wrong answer here is either an ungated company write or an
    /// approval prompt on a read.
    #[test]
    fn a_call_reports_the_layer_operation_it_performs() {
        let _g = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("nebo-packop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        // SAFETY: test-only override of the data dir for this process.
        unsafe { std::env::set_var("NEBO_HOME", &root) };
        let op = |v: Value| PackTool.operation_performed(&v);

        for layer in ["industry", "franchise", "company"] {
            assert_eq!(
                op(json!({"action": "create", "slug": "x", "layer": layer, "body": "b"})).as_deref(),
                Some(format!("layers.{layer}.write").as_str()),
            );
        }
        // `layer` defaults to industry, exactly as `create` defaults it.
        assert_eq!(
            op(json!({"action": "create", "slug": "x", "body": "b"})).as_deref(),
            Some("layers.industry.write"),
        );
        // A layer this tool refuses performs nothing at all.
        assert_eq!(op(json!({"action": "create", "slug": "x", "layer": "team"})), None);

        // Reads.
        assert_eq!(op(json!({"action": "list"})), None);
        assert_eq!(op(json!({"action": "show", "slug": "x"})), None);
        assert_eq!(op(json!({})), None, "no action reads the list");

        // `add` installs a folder that already carries its marker: putting a
        // company pack in by path is the same operation as writing one.
        let src = root.join("incoming");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("COMPANY.md"), "---\ntype: pack\nscope: company\ncompany: acme\n---\n\nHow we run.\n").unwrap();
        assert_eq!(
            op(json!({"action": "add", "path": src.to_string_lossy()})).as_deref(),
            Some("layers.company.write"),
        );
        assert_eq!(op(json!({"action": "add"})), None, "no path, nothing performed");

        // A removal names a pack on disk; a slug that is not one performs nothing.
        create(&json!({"action": "create", "slug": "trade", "layer": "industry", "body": "How it runs."})).unwrap();
        assert_eq!(
            op(json!({"action": "remove", "slug": "trade"})).as_deref(),
            Some("layers.industry.remove"),
        );
        assert_eq!(op(json!({"action": "remove", "slug": "nothing-here"})), None);
        assert_eq!(op(json!({"action": "remove", "slug": "../escape"})), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod skill_example_tests {
    use super::*;

    /// The `company-layers` skill teaches this tool by showing one call. That
    /// example is the first thing an employee copies, so it is held to the
    /// same bar as the tool: it is lifted out of the shipped skill and run.
    /// An example that no longer loads is a broken procedure, not a typo.
    fn example_call() -> Value {
        let skill = crate::skills::bundled::BUNDLED_SKILLS
            .iter()
            .find(|(k, _)| *k == "company-layers")
            .expect("company-layers is bundled")
            .1;
        let at = skill.find("\"action\": \"create\"").expect("the example calls create");
        let start = skill[..at].rfind('{').expect("the call opens with a brace");
        let mut depth = 0usize;
        let bytes = skill.as_bytes();
        let mut end = start;
        for (i, b) in bytes.iter().enumerate().skip(start) {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        serde_json::from_str(&skill[start..end]).expect("the example is JSON")
    }

    #[test]
    fn the_worked_example_in_the_skill_loads_as_a_company_pack() {
        let _g = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("nebo-packskill-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        // SAFETY: test-only override of the data dir for this process.
        unsafe { std::env::set_var("NEBO_HOME", &root) };
        let call = example_call();
        // The example writes the company layer, so it must report the critical
        // company write to the gate — an employee copying it is decided by the
        // owner's grant, not by which chat it is standing in.
        assert_eq!(
            PackTool.operation_performed(&call).as_deref(),
            Some("layers.company.write"),
        );
        let out = create(&call).expect("a valid company pack");
        assert!(out.contains("created"), "{out}");

        let packs = napp::pack::scan_packs(&config::packs_dir().unwrap());
        let pack = packs.iter().find(|p| p.slug == "bright-carpet").expect("the example pack");
        assert_eq!(pack.layer, napp::pack::PackLayer::Company);
        // The company's purpose is read from the marker's frontmatter, so the
        // example has to put it there and not only in the body.
        assert!(pack.frontmatter.get("purpose").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty()));
        // One law reserved to the owner, so it is the owner's hand and not a
        // blanket block; one money question with no default.
        assert_eq!(pack.reserved_ops(), vec!["spend.above_company_bounds".to_string()]);
        assert!(pack.ceilings().is_empty(), "{:?}", pack.ceilings());
        let q = pack.questions.iter().find(|q| q.money).expect("the unset money value");
        assert!(q.default.is_none() && q.missing.is_some());
        assert_eq!(q.label, "What may an employee refund without asking?");
        // The six reserved ids are read by id, so the example must supply one
        // spelled exactly.
        assert!(pack.defaults().contains_key("company.unattended.spend_per_day_cents"));

        // And it lands the same way twice: the example is a procedure, not a
        // one-shot, so re-running it over the existing pack replaces it.
        let out = create(&call).expect("the example is re-runnable");
        assert!(out.contains("updated"), "{out}");
        assert!(remove(&json!({"slug": "bright-carpet"})).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }
}
