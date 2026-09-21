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
    /// The question as the owner reads it: the entry's `label:` (or `title:`),
    /// else its first heading, else the key.
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
    /// Operation suffixes this law holds: Blocked for every seat, unless the
    /// law is `reserved_to: owner`, in which case they are the owner's own
    /// hand instead — reachable with the owner's approval, never grantable.
    pub ceiling: Vec<String>,
    /// `required` or `eventual` (Playbook PRD 6.4 freshness).
    pub freshness: Option<String>,
    /// `owner` when the law reserves its operations to the owner rather than
    /// blocking them outright. Only the company layer writes this.
    pub reserved_to: Option<String>,
}

impl PackLaw {
    /// The owner's own hand: `reserved_to: owner` in the law's frontmatter.
    pub fn is_owner_reserved(&self) -> bool {
        self.reserved_to.as_deref() == Some("owner")
    }
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
        &["rule", "law", "term", "party", "title", "label", "name", "question", "id"],
    )
    .map(str::to_string)
    .or_else(|| first_heading(&body))
    .unwrap_or_else(|| name.clone());
    Ok(PackEntry { name, title, frontmatter, body })
}

/// Markdown files directly in `dir` and in its subdirectories (`laws/by-state/`),
/// sorted by relative path so output is stable. A `README.md` at any depth is
/// skipped: it explains the folder to whoever opens it, and a seat that read it
/// as a law would read "Company-level laws" as policy.
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
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if path.is_dir() {
            collect_markdown(&path, out)?;
        } else if name.eq_ignore_ascii_case("README.md") {
            continue;
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
            let reserved_to = entry
                .frontmatter
                .get("reserved_to")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_ascii_lowercase());
            PackLaw { entry, ceiling, freshness, reserved_to }
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

/// Copy a tree, leaving a file whose bytes already match alone so an idempotent
/// install does not churn every mtime. The ONE tree copy every pack path uses.
pub fn copy_tree(src: &Path, dst: &Path) -> Result<(), PackError> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            let same = std::fs::read(&from)
                .ok()
                .zip(std::fs::read(&to).ok())
                .is_some_and(|(a, b)| a == b);
            if !same {
                std::fs::copy(&from, &to)?;
            }
        }
    }
    Ok(())
}

/// The ONE gate every change to a pack passes through (CODE_AUDITOR 8.1).
///
/// `build` is handed an empty staging directory beside `dest` and writes the pack
/// it wants there — a whole pack from parts, a copy of an uploaded folder, or a
/// copy of the pack that stands with one file changed. The real loader then reads
/// the staging copy. Only a pack that loads replaces `dest`; a change that would
/// stop the pack loading comes back as the loader's own error with the pack the
/// seats work from untouched.
///
/// Every path that changes a pack lands through here — the owner's layers screen,
/// a seat's `pack` tool, an org install, an uploaded zip — which is why the loader
/// can be trusted as the gate: there is no way around it, and no path can drift
/// past it a forgotten step at a time.
pub fn commit_change<F>(dest: &Path, build: F) -> Result<Pack, PackError>
where
    F: FnOnce(&Path) -> Result<(), PackError>,
{
    let parent = dest
        .parent()
        .ok_or_else(|| PackError::File(dest.display().to_string(), "has no parent".into()))?;
    let name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| PackError::File(dest.display().to_string(), "has no name".into()))?;
    let staging = parent.join(format!(".staging-{name}"));
    let retired = parent.join(format!(".retiring-{name}"));
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_dir_all(&retired);
    std::fs::create_dir_all(&staging)?;

    let built = build(&staging).and_then(|()| load_pack(&staging));
    let pack = match built {
        Ok(pack) => pack,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    };

    // Two renames rather than a delete and a copy: the window in which the pack
    // is not on disk is a metadata operation wide, not a recursive delete wide.
    if dest.exists() {
        std::fs::rename(dest, &retired)?;
    }
    let landed = std::fs::rename(&staging, dest);
    if landed.is_err() && retired.exists() {
        let _ = std::fs::rename(&retired, dest);
    }
    landed?;
    let _ = std::fs::remove_dir_all(&retired);
    // The loader read the pack at the staging path, so its slug and its source
    // are the staging directory's. Both are the destination's now — a caller that
    // trusted either would look for the pack where it no longer is.
    Ok(Pack {
        slug: name.to_string(),
        source_path: dest.to_path_buf(),
        ..pack
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
        // A dot-directory is never a pack: `commit_change` stages beside the
        // pack it is replacing, and a half-written staging copy must not load as
        // a pack of its own while it is being built.
        let hidden = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'));
        if !path.is_dir() || hidden {
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

/// How many unchanged lines a hunk carries either side of a change — the three
/// every pull request shows.
const DIFF_CONTEXT: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Keep,
    Del,
    Add,
}

/// A longest-common-subsequence edit script over lines. The common prefix and
/// suffix are taken off first, so a one-word change in a long file leaves a
/// table of a line or two; only a genuinely rewritten file pays for the whole
/// table, and past a few million cells the file is simply reported as replaced.
/// `Keep` and `Del` index into `a`, `Add` into `b`.
fn edit_script(a: &[&str], b: &[&str]) -> Vec<(Op, usize)> {
    let lead = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
    let max_tail = a.len().min(b.len()) - lead;
    let tail = (0..max_tail)
        .take_while(|t| a[a.len() - 1 - t] == b[b.len() - 1 - t])
        .count();
    let (am, bm) = (&a[lead..a.len() - tail], &b[lead..b.len() - tail]);
    let mut out: Vec<(Op, usize)> = (0..lead).map(|i| (Op::Keep, i)).collect();
    if am.len().saturating_mul(bm.len()) > 4_000_000 {
        out.extend((0..am.len()).map(|i| (Op::Del, lead + i)));
        out.extend((0..bm.len()).map(|i| (Op::Add, lead + i)));
    } else {
        // lcs[i][j] is the longest common subsequence of am[i..] and bm[j..].
        let (n, m) = (am.len(), bm.len());
        let w = m + 1;
        let mut lcs = vec![0u32; (n + 1) * w];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i * w + j] = if am[i] == bm[j] {
                    lcs[(i + 1) * w + j + 1] + 1
                } else {
                    lcs[(i + 1) * w + j].max(lcs[i * w + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0usize, 0usize);
        while i < n && j < m {
            if am[i] == bm[j] {
                out.push((Op::Keep, lead + i));
                i += 1;
                j += 1;
            } else if lcs[(i + 1) * w + j] >= lcs[i * w + j + 1] {
                out.push((Op::Del, lead + i));
                i += 1;
            } else {
                out.push((Op::Add, lead + j));
                j += 1;
            }
        }
        while i < n {
            out.push((Op::Del, lead + i));
            i += 1;
        }
        while j < m {
            out.push((Op::Add, lead + j));
            j += 1;
        }
    }
    out.extend((0..tail).map(|t| (Op::Keep, a.len() - tail + t)));
    out
}

/// One file as a unified diff: a header naming it, `@@` hunks, three lines of
/// context, `-` and `+` on the lines that moved. An empty `old` reads as an
/// added file and an empty `new` as a removed one, against `/dev/null`, the way
/// `diff` writes it. Empty when the two are the same.
pub fn unified_diff(path: &str, old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let a: Vec<&str> = if old.is_empty() { Vec::new() } else { old.lines().collect() };
    let b: Vec<&str> = if new.is_empty() { Vec::new() } else { new.lines().collect() };

    struct Row<'t> {
        op: Op,
        text: &'t str,
        old_no: usize,
        new_no: usize,
    }
    // Number every row in both files as the walk consumes it; a hunk header is
    // the first line number on each side and how many lines it covers.
    let mut rows: Vec<Row> = Vec::new();
    let (mut co, mut cn) = (0usize, 0usize);
    for (op, idx) in edit_script(&a, &b) {
        match op {
            Op::Keep => {
                co += 1;
                cn += 1;
                rows.push(Row { op, text: a[idx], old_no: co, new_no: cn });
            }
            Op::Del => {
                co += 1;
                rows.push(Row { op, text: a[idx], old_no: co, new_no: cn });
            }
            Op::Add => {
                cn += 1;
                rows.push(Row { op, text: b[idx], old_no: co, new_no: cn });
            }
        }
    }

    // Every changed line takes three rows of context either side; windows that
    // touch become one hunk.
    let mut groups: Vec<(usize, usize)> = Vec::new();
    for (k, _) in rows.iter().enumerate().filter(|(_, r)| r.op != Op::Keep) {
        let start = k.saturating_sub(DIFF_CONTEXT);
        let end = (k + 1 + DIFF_CONTEXT).min(rows.len());
        match groups.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => groups.push((start, end)),
        }
    }

    let mut out = String::new();
    let from = if a.is_empty() { "/dev/null".to_string() } else { format!("a/{path}") };
    let to = if b.is_empty() { "/dev/null".to_string() } else { format!("b/{path}") };
    out.push_str(&format!("--- {from}\n+++ {to}\n"));
    for (s, e) in groups {
        let g = &rows[s..e];
        let old_count = g.iter().filter(|r| r.op != Op::Add).count();
        let new_count = g.iter().filter(|r| r.op != Op::Del).count();
        let old_start = g.iter().find(|r| r.op != Op::Add).map_or(g[0].old_no, |r| r.old_no);
        let new_start = g.iter().find(|r| r.op != Op::Del).map_or(g[0].new_no, |r| r.new_no);
        out.push_str(&format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@\n"));
        for r in g {
            out.push(match r.op {
                Op::Keep => ' ',
                Op::Del => '-',
                Op::Add => '+',
            });
            out.push_str(r.text);
            out.push('\n');
        }
    }
    out
}

/// One pack file the way it reads on disk: the frontmatter between fences, then
/// the body. Reconstructed rather than re-read, because the pack a diff compares
/// against is the one that stood before the owner's edit and is no longer there.
fn file_text(frontmatter: &serde_json::Value, body: &str) -> String {
    let mut out = String::new();
    if frontmatter.as_object().is_some_and(|m| !m.is_empty()) {
        let yaml = serde_yaml::to_string(frontmatter).unwrap_or_default();
        out.push_str("---\n");
        out.push_str(yaml.trim_start_matches("---\n").trim_end());
        out.push_str("\n---\n");
    }
    out.push_str(body.trim_start_matches('\n').trim_end());
    out.push('\n');
    out
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
                    let holding = if self.reserves_to_owner(l) {
                        "The owner's own hand, never yours and never granted to you"
                    } else {
                        "Blocked for every seat, not the seat's to decide"
                    };
                    out.push_str(&format!("\n{}: {}\n", holding, l.ceiling.join(", ")));
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

    /// The marker file's name for this pack's layer.
    pub fn marker_file(&self) -> &'static str {
        MARKERS
            .iter()
            .find(|(_, l)| *l == self.layer)
            .map_or("COMPANY.md", |(m, _)| *m)
    }

    /// Every file in the pack as text, keyed by its path relative to the pack
    /// root, in the order a seat reads them: the marker, then each typed
    /// folder. The frontmatter is rendered back into the file so a changed
    /// `ceiling` or `value` shows up in a diff as its own line — the structure
    /// matters more than the prose around it.
    fn files_text(&self) -> Vec<(String, String)> {
        let mut out = vec![(
            self.marker_file().to_string(),
            file_text(&self.frontmatter, &self.body),
        )];
        let by_folder = self.entries_by_folder();
        for folder in FOLDERS {
            let Some(entries) = by_folder.get(folder) else { continue };
            for (name, e) in entries {
                out.push((format!("{folder}/{name}.md"), file_text(&e.frontmatter, &e.body)));
            }
        }
        out
    }

    /// What changed from `previous` to `self`, as a unified diff per changed
    /// file: the header naming the file, `@@` hunks, three lines of context,
    /// `-` and `+` on the lines that moved. A one-word change reads as a hunk
    /// and not as a reprint of the file, because a seat pays for every line it
    /// reads. Added and removed files come whole, against `/dev/null`. Empty
    /// when nothing changed.
    pub fn diff_text(&self, previous: &Pack) -> String {
        let then: BTreeMap<String, String> = previous.files_text().into_iter().collect();
        let now = self.files_text();
        let mut out = String::new();
        for (path, text) in &now {
            match then.get(path) {
                None => out.push_str(&unified_diff(path, "", text)),
                Some(old) if old != text => out.push_str(&unified_diff(path, old, text)),
                Some(_) => {}
            }
        }
        let present: std::collections::HashSet<&str> =
            now.iter().map(|(p, _)| p.as_str()).collect();
        for (path, old) in &then {
            if !present.contains(path.as_str()) {
                out.push_str(&unified_diff(path, old, ""));
            }
        }
        out
    }

    /// Whether a law's operations are the owner's own hand rather than blocked.
    ///
    /// `reserved_to: owner` holds on the company layer and nowhere else, because
    /// the runtime reads the reserved list from the company pack alone. On an
    /// industry or franchise pack the key would produce an operation that is
    /// neither blocked nor reserved — held in the author's mind and wide open in
    /// fact. Off the company layer the key is ignored and the law blocks, so a
    /// mistaken `reserved_to` fails closed and nothing has to be refused.
    fn reserves_to_owner(&self, law: &PackLaw) -> bool {
        self.layer == PackLayer::Company && law.is_owner_reserved()
    }

    /// Every operation a law blocks, with the law that blocks it. A law the
    /// company layer reserves to the owner is not here: it is not blocked, it is
    /// the owner's, and `reserved_ops` carries it.
    pub fn ceilings(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for l in self.laws.iter().filter(|l| !self.reserves_to_owner(l)) {
            for op in &l.ceiling {
                out.push((op.clone(), l.entry.title.clone()));
            }
        }
        out
    }

    /// Every operation a law reserves to the owner's own hand. No standing
    /// grant may cover these and the General Manager may never grant them;
    /// they reach the owner as a decision. Only the company pack is ever asked
    /// for this — `company_policy_from` reads the company layer and nothing
    /// else — which is why `ceilings` is the side that has to check the layer.
    pub fn reserved_ops(&self) -> Vec<String> {
        self.laws
            .iter()
            .filter(|l| l.is_owner_reserved())
            .flat_map(|l| l.ceiling.iter().cloned())
            .collect()
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

/// Watch `packs_dir` and call `on_change` after a burst of file changes
/// settles (same coalescing debounce as the agent loader).
///
/// It reports THAT the folder changed, never what it now holds: the reader
/// scans, so what a reader records is the directory as it stands when it
/// records it. A scan handed over here is already a description of the past
/// by the time anyone takes a lock on it.
///
/// Returns the task handle; dropping it detaches the watch.
pub fn watch_packs(
    packs_dir: PathBuf,
    on_change: impl Fn() + Send + 'static,
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

        // A write: something in the folder changed and the packs must be read
        // again. The watch also reports reads — on Linux, an open and an
        // access for every file anyone looks at — and those change nothing.
        let is_write = |event: &Event| {
            matches!(
                event.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            )
        };

        let debounce = std::time::Duration::from_secs(1);
        while let Some(result) = rx.recv().await {
            match result {
                Ok(event) => {
                    if !is_write(&event) {
                        continue;
                    }
                    // A pack arrives as a burst: a folder copied in, a pack
                    // written file by file. The first event is the first file,
                    // and reading the disk on it parks half a pack — a company
                    // layer missing the laws that had not landed yet. Wait for
                    // the burst to stop before reading, and only then read.
                    //
                    // Only a WRITE says the burst is still going. Counting
                    // every message here counted the reads too, and the loudest
                    // reader is whoever is waiting for this rescan: the layers
                    // screen scanning the folder on each `GET /layers`. On
                    // Linux that is ~480 events a second, so the burst never
                    // ended, every change ran the full 30 rounds, and the
                    // rescan landed at 30.03s — just past the 30s the waiter
                    // allowed it. (macOS has no read events, which is why this
                    // only ever showed on the Linux CI runner.)
                    for _ in 0..30 {
                        tokio::time::sleep(debounce).await;
                        let mut more = false;
                        while let Ok(next) = rx.try_recv() {
                            if next.as_ref().is_ok_and(is_write) {
                                more = true;
                            }
                        }
                        if !more {
                            break;
                        }
                    }
                    info!("packs directory changed");
                    on_change();
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
            "---\nquestion: deposit_pct\nid: trade.deposit_pct\nkind: value\nmoney: true\nlabel: \"What deposit is collected?\"\nmissing: Deposits are not quoted until set.\n---\n\nDeposit\n",
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
        // `laws/by-state/README.md` explains the folder; it is not a law.
        assert_eq!(p.laws.len(), 1);
        let law = p.laws.iter().find(|l| l.entry.name == "payments").unwrap();
        assert_eq!(law.ceiling.len(), 2);
        assert_eq!(law.freshness.as_deref(), Some("required"));
        assert_eq!(p.standards.len(), 1);
        assert_eq!(p.standards[0].id, "trade.support.first_response_hours");
        assert_eq!(p.questions.len(), 2);
        let money = p.questions.iter().find(|q| q.key == "deposit_pct").unwrap();
        assert!(money.money && money.default.is_none() && money.missing.is_some());
        // The question the owner answers is the one the file writes. Without
        // this the label falls through to the key or the id, and the owner is
        // asked `trade.deposit_pct`.
        assert_eq!(money.label, "What deposit is collected?");
        assert_eq!(p.reference[0].title, "The long guide");
        assert_eq!(p.stamp(), "industry:sample-trade@0.1.0");
        assert_eq!(p.content_hash.len(), 64);
    }

    /// `reserved_to: owner` is the company's to write and no one else's. The
    /// runtime reads the reserved list from the company pack alone, so the same
    /// key on an industry or franchise pack would leave the operation neither
    /// blocked nor reserved — the author would believe they had held it while it
    /// was wide open. Off the company layer it blocks instead: fail closed.
    #[test]
    fn a_law_reserved_to_the_owner_holds_only_on_the_company_layer() {
        let tmp = tempfile::tempdir().unwrap();
        let law = "---\nlaw: Equity\nceiling: [\"equity.issue\"]\nreserved_to: owner\n---\n\nThe owner's.\n";

        // The company layer: reserved, never Blocked, or the owner could not
        // approve it either.
        let co = tmp.path().join("acme");
        write(&co, "COMPANY.md", "---\ntype: company\ncompany: Acme\nversion: 1.0.0\n---\n\n# Acme\n");
        write(&co, "laws/equity.md", law);
        let p = load_pack(&co).unwrap();
        assert_eq!(p.reserved_ops(), vec!["equity.issue".to_string()]);
        let blocked: Vec<String> = p.ceilings().into_iter().map(|(op, _)| op).collect();
        assert!(!blocked.contains(&"equity.issue".to_string()), "reserved is not blocked: {blocked:?}");
        assert!(p.prompt_text().contains("The owner's own hand"));

        // The same law on an industry pack blocks, as it would without the key.
        let ind = sample_pack(tmp.path(), "split");
        write(&ind, "laws/equity.md", law);
        let p = load_pack(&ind).unwrap();
        let blocked: Vec<String> = p.ceilings().into_iter().map(|(op, _)| op).collect();
        assert!(
            blocked.contains(&"equity.issue".to_string()),
            "an industry pack cannot reserve to the owner; it must block: {blocked:?}"
        );
        // The sample pack's own law has no `reserved_to`, so it still blocks.
        assert!(blocked.contains(&"ledger.payment.create".to_string()), "{blocked:?}");
        assert!(!p.prompt_text().contains("The owner's own hand"));
    }

    /// The owner's real company package explains each folder in a `README.md`.
    /// The loader used to read `laws/README.md` as a law called "Company-level
    /// laws" with an empty ceiling, and every employee read it as policy.
    #[test]
    fn a_folder_readme_is_not_an_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("acme");
        write(&d, "COMPANY.md", "---\ntype: company\ncompany: Acme\nversion: 1.0.0\n---\n\n# Acme\n");
        write(&d, "laws/README.md", "# Company-level laws\n\nWhat goes in this folder.\n");
        write(&d, "laws/by-state/readme.md", "# Per state\n\nNotes.\n");
        write(&d, "laws/pay.md", "---\nlaw: Payments\nceiling: [\"ledger.payment.send\"]\n---\n\nNo seat pays unattended.\n");
        write(&d, "rules/README.md", "# Rules\n\nWhat goes here.\n");
        let p = load_pack(&d).unwrap();
        assert_eq!(p.laws.len(), 1, "only the law is a law: {:?}", p.laws.iter().map(|l| &l.entry.title).collect::<Vec<_>>());
        assert_eq!(p.laws[0].entry.title, "Payments");
        assert!(p.rules.is_empty());
        let text = p.prompt_text();
        assert!(!text.contains("Company-level laws"), "a folder note is not policy: {text}");
        assert!(!text.contains("What goes in this folder"), "{text}");
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
    fn the_diff_is_a_unified_diff_per_changed_file() {
        let tmp = tempfile::tempdir().unwrap();
        let before = load_pack(&sample_pack(tmp.path(), "v1")).unwrap();
        let d2 = sample_pack(tmp.path(), "v2");
        write(&d2, "rules/deductible.md", "---\nrule: Deductible\nclass: policy\nalways: True\n---\n\nThe deductible is never waived, ever.\n");
        write(&d2, "rules/photos.md", "---\nrule: Photos\n---\n\nPhotos before estimates.\n");
        std::fs::remove_file(d2.join("vocabulary/supplement.md")).unwrap();
        let after = load_pack(&d2).unwrap();
        let diff = after.diff_text(&before);

        // The shape anyone who has read a pull request already knows.
        assert!(diff.contains("--- a/rules/deductible.md"), "{diff}");
        assert!(diff.contains("+++ b/rules/deductible.md"), "{diff}");
        assert!(diff.contains("@@ -"), "{diff}");
        assert!(diff.contains("-The deductible is never waived."), "{diff}");
        assert!(diff.contains("+The deductible is never waived, ever."), "{diff}");
        // An added file comes whole, against /dev/null; so does a removed one.
        assert!(diff.contains("--- /dev/null\n+++ b/rules/photos.md"), "{diff}");
        assert!(diff.contains("+Photos before estimates."), "{diff}");
        assert!(diff.contains("--- a/vocabulary/supplement.md\n+++ /dev/null"), "{diff}");
        assert!(diff.contains("-An addition to an approved estimate."), "{diff}");
        // Files that did not change are not in the diff at all.
        assert!(!diff.contains("parties/carrier.md"), "{diff}");
        assert!(before.diff_text(&before).is_empty());
    }

    /// A one-word change used to reprint the whole file, which cost every seat
    /// a page of tokens to learn that one word moved. It must be a hunk.
    #[test]
    fn a_one_word_change_is_a_hunk_not_a_reprint() {
        let tmp = tempfile::tempdir().unwrap();
        let long: String = (1..=40).map(|i| format!("Paragraph {i} of the standing guidance.\n")).collect();
        let d1 = tmp.path().join("v1");
        write(&d1, "COMPANY.md", "---\ntype: company\ncompany: Acme\nversion: 1.0.0\n---\n\n# Acme\n");
        write(&d1, "rules/long.md", &format!("---\nrule: Long\n---\n\n{long}"));
        let before = load_pack(&d1).unwrap();

        let d2 = tmp.path().join("v2");
        write(&d2, "COMPANY.md", "---\ntype: company\ncompany: Acme\nversion: 1.0.0\n---\n\n# Acme\n");
        write(&d2, "rules/long.md", &format!("---\nrule: Long\n---\n\n{}", long.replace("Paragraph 20 of", "Paragraph 20 now of")));
        let after = load_pack(&d2).unwrap();

        let diff = after.diff_text(&before);
        // One hunk, one line out and one line in, six lines of context.
        assert_eq!(diff.matches("@@ -").count(), 1, "{diff}");
        assert_eq!(diff.lines().filter(|l| l.starts_with('-') && !l.starts_with("---")).count(), 1, "{diff}");
        assert_eq!(diff.lines().filter(|l| l.starts_with('+') && !l.starts_with("+++")).count(), 1, "{diff}");
        assert!(diff.contains("@@ -20,7 +20,7 @@"), "{diff}");
        assert!(!diff.contains("Paragraph 1 of"), "untouched prose is not reprinted: {diff}");
        assert!(diff.lines().count() < 14, "a hunk, not a page: {diff}");
    }

    /// The frontmatter is in the comparison: a changed ceiling or value matters
    /// more to a seat than the prose around it, and it must show as its own line.
    #[test]
    fn a_changed_ceiling_shows_as_a_frontmatter_line() {
        let tmp = tempfile::tempdir().unwrap();
        let before = load_pack(&sample_pack(tmp.path(), "c1")).unwrap();
        let d2 = sample_pack(tmp.path(), "c2");
        write(
            &d2,
            "laws/payments.md",
            "---\nlaw: Unattended payments\nclass: policy\nceiling: [\"ledger.payment.create\"]\nfreshness: required\n---\n\nNo seat pays unattended.\n",
        );
        let after = load_pack(&d2).unwrap();
        let diff = after.diff_text(&before);
        assert!(diff.contains("--- a/laws/payments.md"), "{diff}");
        assert!(diff.contains("-- ledger.payment.send"), "the dropped ceiling entry: {diff}");
        assert!(!diff.contains("No seat pays unattended"), "unchanged prose stays out: {diff}");
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

    /// The one gate: a change that would stop the pack loading never lands, and
    /// the pack that stands is untouched. Every path that writes a pack — the
    /// owner's layers screen, a seat's `pack` tool, an install, an upload — goes
    /// through here, so this is the only place the loader has to be the gate.
    #[test]
    fn commit_change_is_the_gate_and_a_refused_change_leaves_the_pack_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("acme");

        // A first commit creates the pack.
        let pack = commit_change(&dest, |staged| {
            std::fs::write(
                staged.join("COMPANY.md"),
                "---\ncompany: Acme\nversion: 1.0.0\n---\n\n# Acme\n\nFix roofs.\n",
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(pack.slug, "acme");
        assert_eq!(pack.source_path, dest, "the pack is where it landed, not in staging");
        assert!(dest.join("COMPANY.md").is_file());

        // A change that does not load is refused, with the loader's own message,
        // and nothing about the pack on disk moves.
        let err = commit_change(&dest, |staged| {
            copy_tree(&dest, staged)?;
            std::fs::write(staged.join("INDUSTRY.md"), "---\nindustry: roofing\n---\n\n# Roofing\n")?;
            Ok(())
        })
        .expect_err("two markers is not a pack");
        assert!(matches!(err, PackError::ManyMarkers(_)), "{err}");
        assert!(!dest.join("INDUSTRY.md").exists(), "the refused change never landed");
        assert_eq!(load_pack(&dest).unwrap().name, "Acme");

        // Nothing is left behind for `scan_packs` to trip over.
        assert!(!tmp.path().join(".staging-acme").exists());
        assert!(!tmp.path().join(".retiring-acme").exists());
        assert_eq!(scan_packs(tmp.path()).len(), 1);

        // And a change that loads replaces the pack whole.
        commit_change(&dest, |staged| {
            copy_tree(&dest, staged)?;
            std::fs::write(staged.join("rules/one.md"), "x").ok();
            std::fs::create_dir_all(staged.join("rules")).ok();
            std::fs::write(
                staged.join("rules/one.md"),
                "---\nrule: Deposits\n---\n\nHalf up front.\n",
            )?;
            Ok(())
        })
        .unwrap();
        let after = load_pack(&dest).unwrap();
        assert_eq!(after.rules.len(), 1);
        assert_eq!(after.rules[0].entry.title, "Deposits");
    }

    /// A staging directory must never load as a pack of its own while it is being
    /// built, or the owner's screen shows a `.staging-acme` layer for a second.
    #[test]
    fn a_dot_directory_is_never_a_pack() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join(".staging-acme");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("COMPANY.md"), "---\ncompany: Acme\n---\n\n# Acme\n").unwrap();
        assert!(scan_packs(tmp.path()).is_empty());
    }
}
