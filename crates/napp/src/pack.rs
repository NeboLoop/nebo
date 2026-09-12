//! Packs — the industry, franchise, and company layers as packages on disk.
//!
//! A pack is a directory `<packs_dir>/<slug>/` holding exactly one marker file,
//! `INDUSTRY.md`, `FRANCHISE.md`, or `COMPANY.md` (YAML frontmatter plus a
//! markdown body), and the typed folders beside it: `vocabulary/`, `parties/`,
//! `rules/`, `laws/`, `standards/`, `workflows/`, `reference/`. Every file in a
//! folder is markdown with optional YAML frontmatter; the frontmatter carries
//! the structure (a law's `ceiling`, a rule's `always`, a standard's `id` and
//! `value`), the body carries the prose.
//!
//! A pack is knowledge, never procedure: a package that carries a `SKILL.md`
//! or a `skills/` directory is refused (Playbook PRD invariant 9).
//!
//! The loader is a reader, not a runtime. Its output is the text a seat reads
//! in its update run (`Pack::prompt_text`, `Pack::diff_text`), the ceilings the
//! policy stack takes at load (`Pack::ceilings`), and the defaults the resolved
//! value algorithm falls through to (`Pack::defaults`). Nothing here is
//! consulted at work time (Playbook PRD 6.8).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use crate::agent::split_frontmatter;

/// The three marker files, in precedence order from lowest to highest.
pub const MARKERS: &[(&str, PackLayer)] = &[
    ("INDUSTRY.md", PackLayer::Industry),
    ("FRANCHISE.md", PackLayer::Franchise),
    ("COMPANY.md", PackLayer::Company),
];

/// The typed folders a pack may carry, in the order a seat reads them.
pub const FOLDERS: &[&str] = &[
    "vocabulary",
    "parties",
    "rules",
    "laws",
    "standards",
    "workflows",
    "reference",
];

#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error("no marker file: a pack holds exactly one of INDUSTRY.md, FRANCHISE.md, COMPANY.md")]
    NoMarker,
    #[error("more than one marker file: {0:?}")]
    ManyMarkers(Vec<String>),
    #[error("a pack never contains a skill; found {0}")]
    ContainsSkill(String),
    #[error("{0}: {1}")]
    File(String, String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackLayer {
    Industry,
    Franchise,
    Company,
}

impl PackLayer {
    pub fn as_str(&self) -> &'static str {
        match self {
            PackLayer::Industry => "industry",
            PackLayer::Franchise => "franchise",
            PackLayer::Company => "company",
        }
    }

    /// Precedence: company above franchise above industry.
    pub fn rank(&self) -> u8 {
        match self {
            PackLayer::Industry => 0,
            PackLayer::Franchise => 1,
            PackLayer::Company => 2,
        }
    }
}

/// One markdown file in a typed folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackEntry {
    /// The file stem (`dunning-cadence`).
    pub name: String,
    /// `rule:` / `law:` / `term:` / `party:` / `title:` / `name:` from the
    /// frontmatter, else the first markdown heading, else the stem.
    pub title: String,
    /// The frontmatter as JSON (an object; `{}` when absent).
    pub frontmatter: serde_json::Value,
    /// The markdown body after the frontmatter.
    pub body: String,
}

/// A question the pack declares (a `standards/` file with `question:` or a
/// `kind: value` frontmatter) — the Onboarding Specialist asks it, the answer
/// becomes a fact under `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackQuestion {
    /// Durable semantic id (`software.managed.deployment_days`).
    pub id: String,
    /// Artifact-local key (`deployment_days`).
    pub key: String,
    /// `company` or `seat`.
    pub scope: String,
    pub label: String,
    pub missing: Option<String>,
    pub money: bool,
    pub default: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackRule {
    pub entry: PackEntry,
    /// A pinned rule holds regardless of relevance.
    pub always: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackLaw {
    pub entry: PackEntry,
    /// Operation suffixes this law blocks for every seat that can perform them.
    pub ceiling: Vec<String>,
    /// `required` or `eventual` (Playbook PRD 6.4 freshness).
    pub freshness: Option<String>,
}

/// A resolved value the pack supplies (`standards/` with `id` and `value`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackStandard {
    pub id: String,
    pub value: serde_json::Value,
    pub entry: PackEntry,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pack {
    pub layer: PackLayer,
    /// Directory name under `packs/`.
    pub slug: String,
    pub version: String,
    /// Display name: `industry:` / `company:` / `franchise:` / `name:` from
    /// the marker frontmatter, else the slug.
    pub name: String,
    /// The marker's frontmatter as JSON.
    pub frontmatter: serde_json::Value,
    /// The marker's markdown body.
    pub body: String,
    pub capabilities: Vec<String>,
    pub questions: Vec<PackQuestion>,
    pub vocabulary: Vec<PackEntry>,
    pub parties: Vec<PackEntry>,
    pub rules: Vec<PackRule>,
    pub laws: Vec<PackLaw>,
    pub standards: Vec<PackStandard>,
    pub workflows: Vec<PackEntry>,
    pub reference: Vec<PackEntry>,
    pub source_path: PathBuf,
    /// SHA-256 over every file's relative path and bytes; changes when any
    /// file changes.
    pub content_hash: String,
}

/// Frontmatter is hand-written. Strict YAML first; when a value trips the
/// YAML scanner (a bare `@org/...` reference, a stray `:`), fall back to a
/// line-wise `key: value` read so the pack still loads and the prose is not
/// lost. Bracketed lists become arrays; everything else stays a string.
fn yaml_to_json(yaml: &str) -> Result<serde_json::Value, String> {
    if yaml.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    match serde_yaml::from_str::<serde_yaml::Value>(yaml) {
        Ok(v) => serde_json::to_value(v).map_err(|e| e.to_string()),
        Err(strict) => {
            let mut map = serde_json::Map::new();
            for line in yaml.lines() {
                let Some((k, v)) = line.split_once(':') else { continue };
                let k = k.trim();
                if k.is_empty() || k.starts_with('#') || k.contains(' ') {
                    continue;
                }
                let v = v.trim();
                let value = if v.starts_with('[') && v.ends_with(']') {
                    serde_json::from_str::<serde_json::Value>(v).unwrap_or_else(|_| {
                        serde_json::Value::Array(
                            v[1..v.len() - 1]
                                .split(',')
                                .map(|p| p.trim().trim_matches('"').trim_matches('\''))
                                .filter(|p| !p.is_empty())
                                .map(|p| serde_json::Value::String(p.to_string()))
                                .collect(),
                        )
                    })
                } else if let Ok(n) = v.parse::<i64>() {
                    serde_json::Value::from(n)
                } else if let Ok(f) = v.parse::<f64>() {
                    serde_json::Value::from(f)
                } else if v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("false") {
                    serde_json::Value::Bool(v.eq_ignore_ascii_case("true"))
                } else {
                    serde_json::Value::String(v.trim_matches('"').to_string())
                };
                map.insert(k.to_string(), value);
            }
            if map.is_empty() {
                return Err(strict.to_string());
            }
            debug!(error = %strict, "frontmatter is not strict YAML; read line-wise");
            Ok(serde_json::Value::Object(map))
        }
    }
}

fn str_field<'a>(fm: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| fm.get(*k).and_then(|v| v.as_str()))
}

fn scalar_string(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn first_heading(body: &str) -> Option<String> {
    body.lines()
        .map(str::trim)
        .find(|l| l.starts_with('#'))
        .map(|l| l.trim_start_matches('#').trim().to_string())
        .filter(|s| !s.is_empty())
}

fn read_entry(path: &Path) -> Result<PackEntry, PackError> {
    let rel = path.display().to_string();
    let content = std::fs::read_to_string(path)?;
    let (yaml, body) =
        split_frontmatter(&content).map_err(|e| PackError::File(rel.clone(), e.to_string()))?;
    let frontmatter = yaml_to_json(&yaml).map_err(|e| PackError::File(rel.clone(), e))?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let title = str_field(
        &frontmatter,
        &["rule", "law", "term", "party", "title", "name", "question", "id"],
    )
    .map(str::to_string)
    .or_else(|| first_heading(&body))
    .unwrap_or_else(|| name.clone());
    Ok(PackEntry { name, title, frontmatter, body })
}

/// Markdown files directly in `dir` and in its subdirectories (`laws/by-state/`),
/// sorted by relative path so output is stable.
fn read_folder(dir: &Path) -> Result<Vec<PackEntry>, PackError> {
    let mut files = Vec::new();
    if dir.is_dir() {
        collect_markdown(dir, &mut files)?;
    }
    files.sort();
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        out.push(read_entry(&f)?);
    }
    Ok(out)
}

fn collect_markdown(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), PackError> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_markdown(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }
    Ok(())
}

fn string_list(v: Option<&serde_json::Value>) -> Vec<String> {
    match v {
        Some(serde_json::Value::Array(items)) => {
            items.iter().filter_map(|i| i.as_str().map(str::to_string)).collect()
        }
        Some(serde_json::Value::String(s)) => {
            s.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
        }
        _ => Vec::new(),
    }
}

/// Refuse a pack that carries a skill anywhere in its tree (invariant 9).
fn refuse_skills(dir: &Path) -> Result<(), PackError> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.eq_ignore_ascii_case("SKILL.md") || (path.is_dir() && name == "skills") {
            return Err(PackError::ContainsSkill(path.display().to_string()));
        }
        if path.is_dir() {
            refuse_skills(&path)?;
        }
    }
    Ok(())
}

fn hash_tree(dir: &Path) -> Result<String, PackError> {
    let mut files = Vec::new();
    collect_all(dir, &mut files)?;
    files.sort();
    let mut hasher = Sha256::new();
    for f in files {
        let rel = f.strip_prefix(dir).unwrap_or(&f).display().to_string();
        hasher.update(rel.as_bytes());
        hasher.update([0u8]);
        hasher.update(std::fs::read(&f)?);
        hasher.update([0u8]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn collect_all(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), PackError> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_all(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// Load one pack directory.
pub fn load_pack(dir: &Path) -> Result<Pack, PackError> {
    let present: Vec<(&str, PackLayer)> = MARKERS
        .iter()
        .copied()
        .filter(|(m, _)| dir.join(m).is_file())
        .collect();
    let (marker, layer) = match present.as_slice() {
        [] => return Err(PackError::NoMarker),
        [one] => *one,
        many => {
            return Err(PackError::ManyMarkers(
                many.iter().map(|(m, _)| m.to_string()).collect(),
            ))
        }
    };
    refuse_skills(dir)?;

    let marker_entry = read_entry(&dir.join(marker))?;
    let fm = &marker_entry.frontmatter;
    let slug = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let version = fm
        .get("version")
        .and_then(scalar_string)
        .unwrap_or_else(|| "0.0.0".to_string());
    let name = str_field(fm, &["name", "company", "franchise", "brand", "industry"])
        .map(str::to_string)
        .unwrap_or_else(|| slug.clone());
    let capabilities = string_list(fm.get("capabilities"));

    let vocabulary = read_folder(&dir.join("vocabulary"))?;
    let parties = read_folder(&dir.join("parties"))?;
    let rules = read_folder(&dir.join("rules"))?
        .into_iter()
        .map(|entry| {
            let always = entry
                .frontmatter
                .get("always")
                .map(|v| v.as_bool().unwrap_or(false) || v.as_str() == Some("True"))
                .unwrap_or(false);
            PackRule { entry, always }
        })
        .collect();
    let laws = read_folder(&dir.join("laws"))?
        .into_iter()
        .map(|entry| {
            let ceiling = string_list(entry.frontmatter.get("ceiling"));
            let freshness = entry
                .frontmatter
                .get("freshness")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            PackLaw { entry, ceiling, freshness }
        })
        .collect();

    let mut questions = Vec::new();
    let mut standards = Vec::new();
    for entry in read_folder(&dir.join("standards"))? {
        let fm = &entry.frontmatter;
        let id = fm.get("id").and_then(|v| v.as_str()).map(str::to_string);
        let key = fm
            .get("question")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| entry.name.clone());
        let is_question = fm.get("question").is_some() || fm.get("kind").is_some();
        match (id, fm.get("value")) {
            (Some(id), Some(value)) if !is_question => standards.push(PackStandard {
                id,
                value: value.clone(),
                entry,
            }),
            (Some(id), value) => questions.push(PackQuestion {
                id,
                key,
                scope: fm
                    .get("scope")
                    .and_then(|v| v.as_str())
                    .unwrap_or("company")
                    .to_string(),
                label: entry.title.clone(),
                missing: fm.get("missing").and_then(|v| v.as_str()).map(str::to_string),
                money: fm.get("money").and_then(|v| v.as_bool()).unwrap_or(false),
                default: value.cloned().or_else(|| fm.get("default").cloned()),
            }),
            (None, _) => {
                warn!(file = %entry.name, pack = %slug, "standards entry has no semantic id; skipped");
            }
        }
    }
    let workflows = read_folder(&dir.join("workflows"))?;
    let reference = read_folder(&dir.join("reference"))?;
    let content_hash = hash_tree(dir)?;

    Ok(Pack {
        layer,
        slug,
        version,
        name,
        frontmatter: marker_entry.frontmatter,
        body: marker_entry.body,
        capabilities,
        questions,
        vocabulary,
        parties,
        rules,
        laws,
        standards,
        workflows,
        reference,
        source_path: dir.to_path_buf(),
        content_hash,
    })
}

/// Every loadable pack under `packs_dir`, sorted by layer rank then slug. A
/// directory that fails to load is logged and skipped, never fatal.
pub fn scan_packs(packs_dir: &Path) -> Vec<Pack> {
    let mut packs = Vec::new();
    let Ok(entries) = std::fs::read_dir(packs_dir) else {
        return packs;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        match load_pack(&path) {
            Ok(pack) => {
                debug!(slug = %pack.slug, layer = pack.layer.as_str(), "loaded pack");
                packs.push(pack);
            }
            Err(e) => warn!(dir = %path.display(), error = %e, "pack not loaded"),
        }
    }
    packs.sort_by(|a, b| a.layer.rank().cmp(&b.layer.rank()).then(a.slug.cmp(&b.slug)));
    packs
}

fn render_entries(out: &mut String, heading: &str, entries: &[&PackEntry], inline: bool) {
    if entries.is_empty() {
        return;
    }
    out.push_str(&format!("\n## {heading}\n"));
    for e in entries {
        if inline {
            out.push_str(&format!("\n### {}\n\n{}\n", e.title, e.body.trim()));
        } else {
            out.push_str(&format!("- {}\n", e.title));
        }
    }
}

impl Pack {
    /// A stable label for records and stamps: `industry:<slug>@<version>`.
    pub fn stamp(&self) -> String {
        format!("{}:{}@{}", self.layer.as_str(), self.slug, self.version)
    }

    /// The whole pack as the text a seat reads in its update run. The marker
    /// body first, then each folder with every entry's title and body.
    /// `reference/` is listed by title only; it stays in recall.
    pub fn prompt_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "# {} ({} layer, version {})\n\n{}\n",
            self.name,
            self.layer.as_str(),
            self.version,
            self.body.trim()
        ));
        let v: Vec<&PackEntry> = self.vocabulary.iter().collect();
        render_entries(&mut out, "Vocabulary", &v, true);
        let p: Vec<&PackEntry> = self.parties.iter().collect();
        render_entries(&mut out, "Parties", &p, true);
        if !self.rules.is_empty() {
            out.push_str("\n## Rules\n");
            for r in &self.rules {
                let pin = if r.always { " (always)" } else { "" };
                out.push_str(&format!("\n### {}{}\n\n{}\n", r.entry.title, pin, r.entry.body.trim()));
            }
        }
        if !self.laws.is_empty() {
            out.push_str("\n## Laws\n");
            for l in &self.laws {
                out.push_str(&format!("\n### {}\n\n{}\n", l.entry.title, l.entry.body.trim()));
                if !l.ceiling.is_empty() {
                    out.push_str(&format!(
                        "\nBlocked for every seat, not the seat's to decide: {}\n",
                        l.ceiling.join(", ")
                    ));
                }
            }
        }
        if !self.standards.is_empty() || !self.questions.is_empty() {
            out.push_str("\n## Standards\n\n");
            for s in &self.standards {
                out.push_str(&format!("- {} = {}\n", s.id, s.value));
            }
            for q in &self.questions {
                let d = q
                    .default
                    .as_ref()
                    .map(|v| format!(" (default {v})"))
                    .unwrap_or_default();
                out.push_str(&format!("- {} asks: {}{}\n", q.id, q.label, d));
            }
        }
        let w: Vec<&PackEntry> = self.workflows.iter().collect();
        render_entries(&mut out, "Workflows", &w, true);
        let r: Vec<&PackEntry> = self.reference.iter().collect();
        render_entries(&mut out, "Reference (available on request, not inlined)", &r, false);
        out
    }

    fn entries_by_folder(&self) -> BTreeMap<&'static str, BTreeMap<String, &PackEntry>> {
        let mut m: BTreeMap<&'static str, BTreeMap<String, &PackEntry>> = BTreeMap::new();
        for e in &self.vocabulary {
            m.entry("vocabulary").or_default().insert(e.name.clone(), e);
        }
        for e in &self.parties {
            m.entry("parties").or_default().insert(e.name.clone(), e);
        }
        for r in &self.rules {
            m.entry("rules").or_default().insert(r.entry.name.clone(), &r.entry);
        }
        for l in &self.laws {
            m.entry("laws").or_default().insert(l.entry.name.clone(), &l.entry);
        }
        for s in &self.standards {
            m.entry("standards").or_default().insert(s.entry.name.clone(), &s.entry);
        }
        for e in &self.workflows {
            m.entry("workflows").or_default().insert(e.name.clone(), e);
        }
        for e in &self.reference {
            m.entry("reference").or_default().insert(e.name.clone(), e);
        }
        m
    }

    /// What changed from `previous` to `self`, by folder and entry: added and
    /// changed entries carry their new body; removed entries are named. The
    /// marker body is included when it changed. Empty when nothing changed.
    pub fn diff_text(&self, previous: &Pack) -> String {
        let mut out = String::new();
        if self.body.trim() != previous.body.trim() {
            out.push_str(&format!("## {} (changed)\n\n{}\n", self.name, self.body.trim()));
        }
        let now = self.entries_by_folder();
        let then = previous.entries_by_folder();
        for folder in FOLDERS {
            let empty = BTreeMap::new();
            let a = now.get(folder).unwrap_or(&empty);
            let b = then.get(folder).unwrap_or(&empty);
            let mut lines = String::new();
            for (name, e) in a {
                match b.get(name) {
                    None => lines.push_str(&format!("\n### {} (added)\n\n{}\n", e.title, e.body.trim())),
                    Some(old) if old.body.trim() != e.body.trim() || old.frontmatter != e.frontmatter => {
                        lines.push_str(&format!("\n### {} (changed)\n\n{}\n", e.title, e.body.trim()))
                    }
                    Some(_) => {}
                }
            }
            for (name, old) in b {
                if !a.contains_key(name) {
                    lines.push_str(&format!("\n### {} (removed)\n", old.title));
                }
            }
            if !lines.is_empty() {
                out.push_str(&format!("\n## {}\n{}", folder, lines));
            }
        }
        out
    }

    /// Every operation a law blocks, with the law that blocks it.
    pub fn ceilings(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for l in &self.laws {
            for op in &l.ceiling {
                out.push((op.clone(), l.entry.title.clone()));
            }
        }
        out
    }

    /// Values this pack supplies by semantic id: standards' values and
    /// questions' defaults.
    pub fn defaults(&self) -> HashMap<String, serde_json::Value> {
        let mut out = HashMap::new();
        for s in &self.standards {
            out.insert(s.id.clone(), s.value.clone());
        }
        for q in &self.questions {
            if let Some(d) = &q.default {
                out.entry(q.id.clone()).or_insert_with(|| d.clone());
            }
        }
        out
    }
}

/// Watch `packs_dir` and call `on_change` with the full rescan after a burst
/// of file changes settles (same coalescing debounce as the agent loader).
/// Returns the task handle; dropping it stops the watch.
pub fn watch_packs(
    packs_dir: PathBuf,
    on_change: impl Fn(Vec<Pack>) + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        use notify::{Event, EventKind, RecursiveMode, Watcher};
        use tokio::sync::mpsc;

        let (tx, mut rx) = mpsc::unbounded_channel::<notify::Result<Event>>();
        let mut watcher = match notify::RecommendedWatcher::new(
            move |res| {
                let _ = tx.send(res);
            },
            notify::Config::default().with_poll_interval(std::time::Duration::from_secs(2)),
        ) {
            Ok(w) => w,
            Err(e) => {
                warn!(error = %e, "failed to create filesystem watcher for packs");
                return;
            }
        };
        if let Err(e) = std::fs::create_dir_all(&packs_dir) {
            warn!(error = %e, dir = %packs_dir.display(), "cannot create packs dir");
            return;
        }
        if let Err(e) = watcher.watch(&packs_dir, RecursiveMode::Recursive) {
            warn!(error = %e, dir = %packs_dir.display(), "failed to watch packs dir");
            return;
        }

        let mut last_reload = std::time::Instant::now();
        let debounce = std::time::Duration::from_secs(1);
        while let Some(result) = rx.recv().await {
            match result {
                Ok(event) => {
                    if !matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                    ) {
                        continue;
                    }
                    let since = last_reload.elapsed();
                    if since < debounce {
                        tokio::time::sleep(debounce - since).await;
                    }
                    while rx.try_recv().is_ok() {}
                    last_reload = std::time::Instant::now();
                    let packs = scan_packs(&packs_dir);
                    info!(count = packs.len(), "packs directory changed, rescanned");
                    on_change(packs);
                }
                Err(e) => warn!(error = %e, "filesystem watch error (packs)"),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    fn sample_pack(root: &Path, slug: &str) -> PathBuf {
        let d = root.join(slug);
        write(
            &d,
            "INDUSTRY.md",
            "---\ntype: playbook\nscope: industry\nindustry: sample-trade\nversion: 0.1.0\ncapabilities: [\"mail\", \"ledger\"]\n---\n\n# A sample trade\n\nHow this trade runs.\n",
        );
        write(
            &d,
            "vocabulary/supplement.md",
            "---\nterm: Supplement\nclass: observation\n---\n\nAn addition to an approved estimate.\n",
        );
        write(
            &d,
            "parties/carrier.md",
            "---\nparty: Carrier\nclass: observation\nentity_kind: company\n---\n\nPays the claim.\n",
        );
        write(
            &d,
            "rules/deductible.md",
            "---\nrule: Deductible\nclass: policy\nalways: True\n---\n\nThe deductible is never waived.\n",
        );
        write(
            &d,
            "laws/payments.md",
            "---\nlaw: Unattended payments\nclass: policy\nceiling: [\"ledger.payment.create\", \"ledger.payment.send\"]\nfreshness: required\n---\n\nNo seat pays unattended.\n",
        );
        write(
            &d,
            "laws/by-state/README.md",
            "---\nlaw: By state\nceiling: []\n---\n\nPer-state notes live here.\n",
        );
        write(
            &d,
            "standards/first_response_hours.md",
            "---\nid: trade.support.first_response_hours\nscope: company\nvalue: 4\n---\n\n`first_response_hours`\n",
        );
        write(
            &d,
            "standards/deployment_days.md",
            "---\nquestion: deployment_days\nid: trade.deployment_days\nkind: value\ntype: number\nscope: company\ndefault: 14\n---\n\nDeployment target\n",
        );
        write(
            &d,
            "standards/deposit_pct.md",
            "---\nquestion: deposit_pct\nid: trade.deposit_pct\nkind: value\nmoney: true\nmissing: Deposits are not quoted until set.\n---\n\nDeposit\n",
        );
        write(&d, "reference/long-guide.md", "# The long guide\n\nPages of it.\n");
        d
    }

    #[test]
    fn loads_every_folder_with_structure_from_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let d = sample_pack(tmp.path(), "sample-trade");
        let p = load_pack(&d).unwrap();
        assert_eq!(p.layer, PackLayer::Industry);
        assert_eq!(p.slug, "sample-trade");
        assert_eq!(p.version, "0.1.0");
        assert_eq!(p.name, "sample-trade");
        assert_eq!(p.capabilities, vec!["mail", "ledger"]);
        assert_eq!(p.vocabulary[0].title, "Supplement");
        assert_eq!(p.parties[0].title, "Carrier");
        assert!(p.rules[0].always);
        assert_eq!(p.laws.len(), 2);
        let law = p.laws.iter().find(|l| l.entry.name == "payments").unwrap();
        assert_eq!(law.ceiling.len(), 2);
        assert_eq!(law.freshness.as_deref(), Some("required"));
        assert_eq!(p.standards.len(), 1);
        assert_eq!(p.standards[0].id, "trade.support.first_response_hours");
        assert_eq!(p.questions.len(), 2);
        let money = p.questions.iter().find(|q| q.key == "deposit_pct").unwrap();
        assert!(money.money && money.default.is_none() && money.missing.is_some());
        assert_eq!(p.reference[0].title, "The long guide");
        assert_eq!(p.stamp(), "industry:sample-trade@0.1.0");
        assert_eq!(p.content_hash.len(), 64);
    }

    #[test]
    fn a_pack_with_a_skill_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let d = sample_pack(tmp.path(), "bad");
        write(&d, "reference/SKILL.md", "# nope\n");
        assert!(matches!(load_pack(&d), Err(PackError::ContainsSkill(_))));
        let d2 = sample_pack(tmp.path(), "bad2");
        write(&d2, "skills/x.md", "# nope\n");
        assert!(matches!(load_pack(&d2), Err(PackError::ContainsSkill(_))));
    }

    #[test]
    fn markers_are_exactly_one() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("empty");
        std::fs::create_dir_all(&d).unwrap();
        assert!(matches!(load_pack(&d), Err(PackError::NoMarker)));
        write(&d, "INDUSTRY.md", "# a\n");
        write(&d, "COMPANY.md", "# b\n");
        assert!(matches!(load_pack(&d), Err(PackError::ManyMarkers(_))));
    }

    #[test]
    fn ceilings_and_defaults_come_from_laws_and_standards() {
        let tmp = tempfile::tempdir().unwrap();
        let p = load_pack(&sample_pack(tmp.path(), "s")).unwrap();
        let c = p.ceilings();
        assert!(c.contains(&("ledger.payment.create".into(), "Unattended payments".into())));
        assert_eq!(c.len(), 2);
        let d = p.defaults();
        assert_eq!(d["trade.support.first_response_hours"], serde_json::json!(4));
        assert_eq!(d["trade.deployment_days"], serde_json::json!(14));
        assert!(!d.contains_key("trade.deposit_pct"), "money questions never default");
    }

    #[test]
    fn prompt_text_inlines_everything_but_reference() {
        let tmp = tempfile::tempdir().unwrap();
        let p = load_pack(&sample_pack(tmp.path(), "s")).unwrap();
        let t = p.prompt_text();
        assert!(t.contains("An addition to an approved estimate."));
        assert!(t.contains("Deductible (always)"));
        assert!(t.contains("ledger.payment.create"));
        assert!(t.contains("trade.support.first_response_hours = 4"));
        assert!(t.contains("- The long guide"));
        assert!(!t.contains("Pages of it."));
    }

    #[test]
    fn diff_names_added_changed_and_removed_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let before = load_pack(&sample_pack(tmp.path(), "v1")).unwrap();
        let d2 = sample_pack(tmp.path(), "v2");
        write(&d2, "rules/deductible.md", "---\nrule: Deductible\nalways: True\n---\n\nThe deductible is never waived, ever.\n");
        write(&d2, "rules/photos.md", "---\nrule: Photos\n---\n\nPhotos before estimates.\n");
        std::fs::remove_file(d2.join("vocabulary/supplement.md")).unwrap();
        let after = load_pack(&d2).unwrap();
        let diff = after.diff_text(&before);
        assert!(diff.contains("Photos (added)"));
        assert!(diff.contains("Deductible (changed)"));
        assert!(diff.contains("Supplement (removed)"));
        assert!(!diff.contains("Carrier"));
        assert!(before.diff_text(&before).is_empty());
    }

    /// Loads real packs from `NEBO_PACK_FIXTURES=<dir with industry/ and company/>`
    /// when set (developer check against the org folder); a no-op otherwise.
    #[test]
    fn real_packs_load_when_fixtures_are_given() {
        let Ok(root) = std::env::var("NEBO_PACK_FIXTURES") else { return };
        let root = PathBuf::from(root);
        let ind = load_pack(&root.join("industry")).unwrap();
        assert_eq!(ind.layer, PackLayer::Industry);
        assert!(!ind.laws.is_empty() && !ind.rules.is_empty() && !ind.vocabulary.is_empty());
        assert!(!ind.ceilings().is_empty());
        assert!(!ind.defaults().is_empty());
        let co = load_pack(&root.join("company")).unwrap();
        assert_eq!(co.layer, PackLayer::Company);
        assert!(!co.rules.is_empty() && !co.standards.is_empty());
        assert!(co.prompt_text().len() > 1000);
    }

    #[test]
    fn hand_written_frontmatter_that_is_not_strict_yaml_still_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("co");
        write(
            &d,
            "COMPANY.md",
            "---\ntype: company\ncompany: Sample Co\nindustry: @org/playbooks/sample\nnebos: [\"a\", \"b\"]\n---\n\n# Sample Co\n\nHow it runs.\n",
        );
        let p = load_pack(&d).unwrap();
        assert_eq!(p.name, "Sample Co");
        assert_eq!(p.frontmatter["industry"], "@org/playbooks/sample");
        assert_eq!(p.frontmatter["nebos"], serde_json::json!(["a", "b"]));
    }

    #[test]
    fn scan_skips_a_broken_directory_and_orders_by_layer() {
        let tmp = tempfile::tempdir().unwrap();
        sample_pack(tmp.path(), "ind");
        let c = tmp.path().join("co");
        write(&c, "COMPANY.md", "---\ntype: company\ncompany: Sample Co\n---\n\n# Sample Co\n");
        std::fs::create_dir_all(tmp.path().join("broken")).unwrap();
        let packs = scan_packs(tmp.path());
        assert_eq!(packs.len(), 2);
        assert_eq!(packs[0].layer, PackLayer::Industry);
        assert_eq!(packs[1].layer, PackLayer::Company);
        assert_eq!(packs[1].name, "Sample Co");
    }
}
