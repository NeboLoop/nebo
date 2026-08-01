//! Read-only walker for a Hermes install (`~/.hermes`).
//!
//! Produces an [`ImportManifest`] describing what a migration would adopt and
//! what each piece becomes in Nebo. Nothing here writes to Nebo or modifies the
//! source directory — this is the dry-run half of the importer.
//!
//! Layout reference (Hermes docs + `scripts/install.sh`):
//! `config.yaml` (`mcp_servers:`), `SOUL.md` (persona), `memories/{MEMORY,USER}.md`,
//! `skills/**/SKILL.md`, `cron/jobs.json`, `.env` + `auth.json` (secrets),
//! `state.db` (conversation history).

use std::fs;
use std::path::{Path, PathBuf};

use crate::handlers::integrations::parse_mcp_servers_block;

use super::manifest::{ImportItem, ImportManifest, ItemKind, SourceKind, TrustTier};

/// Walk a Hermes install root and build its dry-run manifest.
pub fn scan(root: &Path) -> ImportManifest {
    let mut m = ImportManifest::new(SourceKind::Hermes, root.display().to_string());
    scan_mcp(root, &mut m);
    scan_persona(root, &mut m);
    scan_memory(root, &mut m);
    scan_skills(root, &mut m);
    scan_cron(root, &mut m);
    scan_credentials(root, &mut m);
    scan_history(root, &mut m);
    m
}

/// `config.yaml` → `mcp_servers:` map. Hermes's dialect (`auth:` instead of
/// `authType:`, no explicit `type` on remotes) is normalized into the canonical
/// config block and handed to the one shared MCP parser, so the importer and
/// paste-import can never drift.
fn scan_mcp(root: &Path, m: &mut ImportManifest) {
    let block = match mcp_block(root) {
        Ok(Some(b)) => b,
        Ok(None) => return,
        Err(e) => {
            m.note(format!("config.yaml could not be parsed: {e}"));
            return;
        }
    };

    for s in parse_mcp_servers_block(&block) {
        let is_stdio = s.server_type == "stdio";
        // A local subprocess launch is Code tier (needs confirm); a remote
        // endpoint is Content (auto), matching the paste-import posture.
        let tier = if is_stdio {
            TrustTier::Code
        } else {
            TrustTier::Content
        };
        let detail = if is_stdio {
            let cmd = s
                .metadata
                .as_deref()
                .and_then(|md| serde_json::from_str::<serde_json::Value>(md).ok())
                .and_then(|v| {
                    v.get("command")
                        .and_then(|c| c.as_str())
                        .map(str::to_string)
                });
            match cmd {
                Some(c) => format!("stdio · {c}"),
                None => "stdio launch".to_string(),
            }
        } else {
            match &s.server_url {
                Some(url) => format!("{} · {url}", s.server_type),
                None => s.server_type.clone(),
            }
        };
        m.push(ImportItem {
            kind: ItemKind::McpServer,
            tier,
            name: s.name,
            detail,
            target: "MCP integration",
            source_path: "config.yaml".into(),
        });
    }
}

/// Read `config.yaml` and return its `mcp_servers:` map as a canonical
/// `mcpServers` block, ready for `parse_mcp_servers_block`. `Ok(None)` when the
/// file or key is absent, `Err` when the YAML doesn't parse. Shared by the
/// dry-run scan and the apply step so both see identical servers.
pub(super) fn mcp_block(root: &Path) -> Result<Option<serde_json::Value>, String> {
    let Ok(text) = fs::read_to_string(root.join("config.yaml")) else {
        return Ok(None);
    };
    let cfg: serde_json::Value = serde_yaml::from_str(&text).map_err(|e| e.to_string())?;
    Ok(cfg
        .get("mcp_servers")
        .and_then(|v| v.as_object())
        .map(normalize_mcp_block))
}

/// Normalize Hermes's `mcp_servers:` map into the canonical `mcpServers` block
/// consumed by `parse_mcp_servers_block`. Hermes spells the auth field `auth:`
/// with values `oauth` | `header`; Nebo's auth_type vocabulary is
/// `none` | `oauth` | `api_key`, so `header` maps to `api_key`.
fn normalize_mcp_block(servers: &serde_json::Map<String, serde_json::Value>) -> serde_json::Value {
    let mut normalized = serde_json::Map::new();
    for (name, entry) in servers {
        let mut e = entry.clone();
        if let Some(obj) = e.as_object_mut() {
            if !obj.contains_key("authType") {
                if let Some(auth) = obj.get("auth").and_then(|a| a.as_str()) {
                    let mapped = if auth == "header" { "api_key" } else { auth };
                    obj.insert("authType".into(), serde_json::json!(mapped));
                }
            }
        }
        normalized.insert(name.clone(), e);
    }
    serde_json::json!({ "mcpServers": normalized })
}

/// `SOUL.md` → the employee's persona.
fn scan_persona(root: &Path, m: &mut ImportManifest) {
    let path = root.join("SOUL.md");
    if !path.is_file() {
        return;
    }
    let lines = fs::read_to_string(&path)
        .map(|t| t.lines().count())
        .unwrap_or(0);
    m.push(ImportItem {
        kind: ItemKind::Agent,
        tier: TrustTier::Content,
        name: "Hermes agent".into(),
        detail: format!("persona · {lines} lines"),
        target: "Employee persona",
        source_path: "SOUL.md".into(),
    });
}

/// `memories/*.md` → Nebo memory, one manifest row per file with the real
/// parsed entry count (the same parse the apply step uses).
fn scan_memory(root: &Path, m: &mut ImportManifest) {
    for (file, entries) in memory_entries(root) {
        m.push(ImportItem {
            kind: ItemKind::Memory,
            tier: TrustTier::Content,
            name: file.clone(),
            detail: format!("{} entries → parsed + re-embedded", entries.len()),
            target: "Nebo memory",
            source_path: format!("memories/{file}"),
        });
    }
}

/// Parse every `memories/*.md` into discrete entries: `(file_name, entries)`.
/// Hermes delimits entries with the section sign (`§`); files without `§`
/// fall back to blank-line paragraphs. Markdown headings are structure, not
/// memories, and are dropped. Shared by scan (counts) and apply (writes) so
/// the dry-run can never promise a different import than the apply performs.
pub(super) fn memory_entries(root: &Path) -> Vec<(String, Vec<String>)> {
    let dir = root.join("memories");
    let Ok(dir_entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut paths: Vec<PathBuf> = dir_entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
        .collect();
    paths.sort();
    for path in paths {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let raw: Vec<&str> = if text.contains('§') {
            text.split('§').collect()
        } else {
            text.split("\n\n").collect()
        };
        let entries: Vec<String> = raw
            .into_iter()
            .map(|chunk| {
                // Drop heading lines; keep the prose.
                chunk
                    .lines()
                    .filter(|l| !l.trim_start().starts_with('#'))
                    .collect::<Vec<_>>()
                    .join("\n")
                    .trim()
                    .to_string()
            })
            .filter(|e| !e.is_empty())
            .collect();
        if entries.is_empty() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("memory")
            .to_string();
        out.push((name, entries));
    }
    out
}

/// `skills/**/SKILL.md` → Nebo skills. A skill that bundles a `scripts/` dir is
/// Code tier (it can run), everything else is Content.
fn scan_skills(root: &Path, m: &mut ImportManifest) {
    let dir = root.join("skills");
    if !dir.is_dir() {
        return;
    }
    let mut found = Vec::new();
    collect_skill_md(&dir, &mut found);
    for skill_md in found {
        let skill_dir = skill_md.parent().unwrap_or(&dir);
        let name = skill_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("skill")
            .to_string();
        let has_scripts = skill_dir.join("scripts").is_dir();
        let tier = if has_scripts {
            TrustTier::Code
        } else {
            TrustTier::Content
        };
        let desc = read_frontmatter_description(&skill_md).unwrap_or_else(|| "SKILL.md".into());
        let detail = if has_scripts {
            format!("{desc} · bundles scripts")
        } else {
            desc
        };
        let rel = skill_md
            .strip_prefix(root)
            .unwrap_or(&skill_md)
            .display()
            .to_string();
        m.push(ImportItem {
            kind: ItemKind::Skill,
            tier,
            name,
            detail,
            target: "Nebo skill",
            source_path: rel,
        });
    }
}

/// `cron/jobs.json` → scheduled events. The file is either a bare array of jobs
/// or an object with a `jobs` array.
fn scan_cron(root: &Path, m: &mut ImportManifest) {
    let path = root.join("cron").join("jobs.json");
    let Ok(text) = fs::read_to_string(&path) else {
        return;
    };
    let val: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            m.note(format!("cron/jobs.json could not be parsed: {e}"));
            return;
        }
    };
    let jobs = val
        .as_array()
        .cloned()
        .or_else(|| val.get("jobs").and_then(|j| j.as_array()).cloned())
        .unwrap_or_default();
    for job in jobs {
        let name = job
            .get("name")
            .or_else(|| job.get("id"))
            .and_then(|n| n.as_str())
            .unwrap_or("job")
            .to_string();
        let sched = job
            .get("schedule")
            .and_then(|s| s.get("display").or_else(|| s.get("expr")))
            .and_then(|d| d.as_str())
            .unwrap_or("schedule")
            .to_string();
        m.push(ImportItem {
            kind: ItemKind::Cron,
            tier: TrustTier::Content,
            name,
            detail: sched,
            target: "Scheduled event",
            source_path: "cron/jobs.json".into(),
        });
    }
}

/// `.env` + `auth.json` → the encrypted credential store. Only key *names* are
/// surfaced in the manifest — a secret value must never appear in a dry-run.
fn scan_credentials(root: &Path, m: &mut ImportManifest) {
    if let Ok(text) = fs::read_to_string(root.join(".env")) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, _value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim().trim_start_matches("export ").trim();
            if key.is_empty() {
                continue;
            }
            m.push(ImportItem {
                kind: ItemKind::Credential,
                tier: TrustTier::Content,
                name: key.to_string(),
                detail: "secret → encrypted store".into(),
                target: "Encrypted credential",
                source_path: ".env".into(),
            });
        }
    }
    if root.join("auth.json").is_file() {
        m.push(ImportItem {
            kind: ItemKind::Credential,
            tier: TrustTier::Content,
            name: "OAuth credentials".into(),
            detail: "full OAuth records → encrypted store".into(),
            target: "Encrypted credential",
            source_path: "auth.json".into(),
        });
    }
}

/// `state.db` → chats + messages. The SQLite file itself is only noted here;
/// the apply step reads history through Hermes's own `sessions export` (JSONL).
fn scan_history(root: &Path, m: &mut ImportManifest) {
    let db = root.join("state.db");
    if !db.is_file() {
        return;
    }
    let size = fs::metadata(&db).map(|md| md.len()).unwrap_or(0);
    m.push(ImportItem {
        kind: ItemKind::Session,
        tier: TrustTier::Content,
        name: "conversation history".into(),
        detail: format!("state.db · {} — imported via export", human_size(size)),
        target: "Chats + messages",
        source_path: "state.db".into(),
    });
}

/// Recursively collect every `SKILL.md` under `dir`, skipping hidden folders.
/// Shared by the dry-run scan and the apply step.
pub(super) fn collect_skill_md(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let hidden = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with('.'))
                .unwrap_or(false);
            if !hidden {
                collect_skill_md(&path, out);
            }
        } else if path.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") {
            out.push(path);
        }
    }
}

/// Pull the `description:` field out of a SKILL.md YAML frontmatter block.
fn read_frontmatter_description(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        let t = line.trim();
        if t == "---" {
            break;
        }
        if let Some(rest) = t.strip_prefix("description:") {
            let d = rest.trim().trim_matches('"').trim_matches('\'').to_string();
            if !d.is_empty() {
                return Some(d);
            }
        }
    }
    None
}

fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * KB;
    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Build a realistic Hermes install in a temp dir. Shared by the scan tests
/// here and the apply tests in `apply.rs` so both halves exercise one fixture.
#[cfg(test)]
pub(super) fn hermes_fixture() -> tempfile::TempDir {
    fn write(path: PathBuf, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    let dir = tempfile::tempdir().unwrap();
    {
        let r = dir.path();
        write(
            r.join("config.yaml"),
            "model: anthropic/claude\n\
             mcp_servers:\n\
             \x20 filesystem:\n\
             \x20   command: npx\n\
             \x20   args: [\"-y\", \"@modelcontextprotocol/server-filesystem\", \"/tmp\"]\n\
             \x20 linear:\n\
             \x20   url: https://mcp.linear.app/mcp\n\
             \x20   auth: oauth\n\
             \x20 company_api:\n\
             \x20   url: https://mcp.internal.example.com\n\
             \x20   auth: header\n\
             \x20   headers:\n\
             \x20     Authorization: \"Bearer tok-secretyyy\"\n",
        );
        write(r.join("SOUL.md"), "# Persona\nYou are Atlas.\n");
        write(r.join("memories/MEMORY.md"), "§ prefers dark mode\n§ works in PST\n");
        write(r.join("memories/USER.md"), "§ name is Sam\n");
        write(
            r.join("skills/research/quick/SKILL.md"),
            "---\nname: quick\ndescription: Fast research\n---\nDo research.\n",
        );
        write(
            r.join("skills/deploy/SKILL.md"),
            "---\nname: deploy\ndescription: Ship it\n---\nDeploy.\n",
        );
        write(r.join("skills/deploy/scripts/run.sh"), "echo hi\n");
        write(
            r.join("cron/jobs.json"),
            "[{\"name\":\"daily-brief\",\"schedule\":{\"display\":\"every day at 9am\"},\"prompt\":\"brief me\"}]",
        );
        write(r.join(".env"), "# secrets\nANTHROPIC_API_KEY=sk-secretxxx\nTELEGRAM_TOKEN=123:abc\n\n");
        write(r.join("state.db"), "sqlite-bytes");
    }
    dir
}

#[cfg(test)]
mod tests {
    use super::super::{detect, scan as scan_root};
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn detects_hermes() {
        let f = hermes_fixture();
        assert_eq!(detect(f.path()), Some(SourceKind::Hermes));
    }

    #[test]
    fn empty_dir_is_unrecognized() {
        let d = tempdir().unwrap();
        assert!(detect(d.path()).is_none());
        assert!(scan_root(d.path()).is_err());
    }

    #[test]
    fn manifest_covers_all_artifact_kinds() {
        let f = hermes_fixture();
        let m = scan_root(f.path()).unwrap();
        assert_eq!(m.source, SourceKind::Hermes);
        assert_eq!(m.count(ItemKind::McpServer), 3);
        assert_eq!(m.count(ItemKind::Agent), 1);
        assert_eq!(m.count(ItemKind::Memory), 2);
        assert_eq!(m.count(ItemKind::Skill), 2);
        assert_eq!(m.count(ItemKind::Cron), 1);
        assert_eq!(m.count(ItemKind::Credential), 2);
        assert_eq!(m.count(ItemKind::Session), 1);
    }

    #[test]
    fn stdio_mcp_and_script_skill_are_code_tier() {
        let f = hermes_fixture();
        let m = scan_root(f.path()).unwrap();
        let fs_srv = m.items.iter().find(|i| i.name == "filesystem").unwrap();
        assert_eq!(fs_srv.tier, TrustTier::Code);
        let linear = m.items.iter().find(|i| i.name == "linear").unwrap();
        assert_eq!(linear.tier, TrustTier::Content);
        let deploy = m.items.iter().find(|i| i.name == "deploy").unwrap();
        assert_eq!(deploy.tier, TrustTier::Code);
        let quick = m.items.iter().find(|i| i.name == "quick").unwrap();
        assert_eq!(quick.tier, TrustTier::Content);
        assert!(m.needs_confirmation());
    }

    #[test]
    fn credentials_expose_key_names_never_values() {
        let f = hermes_fixture();
        let m = scan_root(f.path()).unwrap();
        for item in &m.items {
            assert!(!item.detail.contains("sk-secretxxx"));
            assert!(!item.detail.contains("123:abc"));
            assert!(!item.detail.contains("tok-secretyyy"));
            assert!(!item.name.contains("sk-secretxxx"));
        }
        assert!(m.items.iter().any(|i| i.name == "ANTHROPIC_API_KEY"));
        assert!(m.items.iter().any(|i| i.name == "TELEGRAM_TOKEN"));
    }

    #[test]
    fn hermes_auth_header_maps_to_api_key() {
        let servers = serde_json::json!({
            "linear": { "url": "https://mcp.linear.app/mcp", "auth": "oauth" },
            "company_api": { "url": "https://mcp.internal.example.com", "auth": "header" },
        });
        let block = normalize_mcp_block(servers.as_object().unwrap());
        let parsed = parse_mcp_servers_block(&block);
        let auth_of = |name: &str| {
            parsed
                .iter()
                .find(|s| s.name == name)
                .map(|s| s.auth_type.clone())
                .unwrap()
        };
        assert_eq!(auth_of("linear"), "oauth");
        assert_eq!(auth_of("company_api"), "api_key");
    }
}
