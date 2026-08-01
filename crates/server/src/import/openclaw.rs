//! Read-only walker for an OpenClaw install (`~/.openclaw`).
//!
//! Layout reference (docs.openclaw.ai): `openclaw.json` (JSON5; `mcp.servers`,
//! `agents.entries[]`, `env`), per-agent workspaces carrying `SOUL.md` /
//! `AGENTS.md` / `USER.md` / `MEMORY.md` / `memory/*.md`, skills under
//! `skills/` and `workspace*/skills/`, transcripts under
//! `agents/<id>/sessions/*.jsonl`, secrets in `.env` / `credentials/` /
//! `state/openclaw.sqlite`.
//!
//! OpenClaw is multi-agent: the default workspace agent plus every
//! `agents.entries[]` row each normalize to their own [`SourceAgent`].

use std::fs;
use std::path::{Path, PathBuf};

use super::apply::{ImportOutcome, SourceAgent, SourceConversation, SourceMessage};
use super::manifest::{ImportItem, ImportManifest, ItemKind, SourceKind, TrustTier};
use super::parse;

/// Walk an OpenClaw install root and build its dry-run manifest.
pub fn scan(root: &Path) -> ImportManifest {
    let mut m = ImportManifest::new(SourceKind::OpenClaw, root.display().to_string());
    let cfg = match config(root) {
        Ok(c) => c,
        Err(e) => {
            m.note(format!("openclaw.json could not be parsed: {e}"));
            return m;
        }
    };

    // MCP servers — same tiering as everywhere: local exec is Code.
    let mut discard = ImportOutcome::default();
    if let Some(block) = mcp_block(&cfg, &mut discard) {
        for s in crate::handlers::integrations::parse_mcp_servers_block(&block) {
            let is_stdio = s.server_type == "stdio";
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
                    .and_then(|v| v.get("command").and_then(|c| c.as_str()).map(str::to_string));
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
                source_path: "openclaw.json".into(),
            });
        }
    }
    for note in discard.skipped {
        m.note(note);
    }

    // Employees, their memory, and their history. Scan-side stays cheap:
    // personas and memory are small markdown reads; session logs are counted
    // by file and line, never JSON-parsed (that's the apply's job).
    for (id, name, workspace) in agent_rows(root, &cfg) {
        if !workspace.is_dir() {
            m.note(format!("agent {name}: workspace {} missing", workspace.display()));
            continue;
        }
        let persona_lines = fs::read_to_string(workspace.join("SOUL.md"))
            .map(|s| s.lines().count())
            .unwrap_or(0);
        m.push(ImportItem {
            kind: ItemKind::Agent,
            tier: TrustTier::Content,
            name: name.clone(),
            detail: format!("persona · {persona_lines} lines"),
            target: "Employee persona",
            source_path: "workspace".into(),
        });
        for (file, entries) in workspace_memory_files(&workspace) {
            m.push(ImportItem {
                kind: ItemKind::Memory,
                tier: TrustTier::Content,
                name: format!("{name} · {file}"),
                detail: format!("{} entries → parsed + re-embedded", entries.len()),
                target: "Nebo memory",
                source_path: file,
            });
        }
        let (files, lines) = session_counts(&root.join("agents").join(&id).join("sessions"));
        if files > 0 {
            m.push(ImportItem {
                kind: ItemKind::Session,
                tier: TrustTier::Content,
                name: format!("{name} · conversation history"),
                detail: format!("{files} conversations · ~{lines} log entries"),
                target: "Chats + messages",
                source_path: "agents/*/sessions".into(),
            });
        }
    }

    // Skills across both tiers.
    for skill_md in skill_dirs(root, &cfg) {
        let skill_dir = skill_md.parent().unwrap_or(root);
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
        let rel = skill_md
            .strip_prefix(root)
            .unwrap_or(skill_md.as_path())
            .display()
            .to_string();
        m.push(ImportItem {
            kind: ItemKind::Skill,
            tier,
            name,
            detail: if has_scripts {
                "SKILL.md · bundles scripts".to_string()
            } else {
                "SKILL.md".to_string()
            },
            target: "Nebo skill",
            source_path: rel,
        });
    }

    // Credentials: .env keys and the config env block — names only, never values.
    if let Ok(text) = fs::read_to_string(root.join(".env")) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, _)) = line.split_once('=') {
                let key = key.trim().trim_start_matches("export ").trim();
                if !key.is_empty() {
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
        }
    }
    if let Some(env) = cfg.get("env").and_then(|e| e.as_object()) {
        for key in env.keys() {
            m.push(ImportItem {
                kind: ItemKind::Credential,
                tier: TrustTier::Content,
                name: key.clone(),
                detail: "config env → provider profile".into(),
                target: "Encrypted credential",
                source_path: "openclaw.json".into(),
            });
        }
    }

    let mut discard = ImportOutcome::default();
    note_deferred(root, &cfg, &mut discard);
    for note in discard.skipped {
        m.note(note);
    }
    m
}

/// Parse `openclaw.json` (JSON5 — comments, trailing commas, unquoted keys).
pub(super) fn config(root: &Path) -> Result<serde_json::Value, String> {
    let text = fs::read_to_string(root.join("openclaw.json")).map_err(|e| e.to_string())?;
    json5::from_str(&text).map_err(|e| e.to_string())
}

/// `mcp.servers` → the canonical `mcpServers` block. OpenClaw's dialect:
/// `transport: "streamable-http" | "sse"` → `type`, `auth: "oauth"` → oauth,
/// an Authorization header without oauth → api_key. Disabled servers are
/// skipped with a note, not silently imported enabled.
pub(super) fn mcp_block(
    cfg: &serde_json::Value,
    out: &mut ImportOutcome,
) -> Option<serde_json::Value> {
    let servers = cfg.get("mcp")?.get("servers")?.as_object()?;
    let mut normalized = serde_json::Map::new();
    for (name, entry) in servers {
        if entry.get("enabled").and_then(|e| e.as_bool()) == Some(false) {
            out.skipped
                .push(format!("MCP server {name}: disabled in OpenClaw, not imported"));
            continue;
        }
        let mut e = entry.clone();
        if let Some(obj) = e.as_object_mut() {
            if let Some(transport) = obj.get("transport").and_then(|t| t.as_str()) {
                let ty = match transport {
                    "sse" => "sse",
                    _ => "http", // streamable-http and friends
                };
                obj.insert("type".into(), serde_json::json!(ty));
            }
            let auth = obj.get("auth").and_then(|a| a.as_str());
            let auth_type = match auth {
                Some("oauth") => "oauth",
                _ if obj
                    .get("headers")
                    .and_then(|h| h.get("Authorization"))
                    .is_some() =>
                {
                    "api_key"
                }
                _ => "none",
            };
            obj.insert("authType".into(), serde_json::json!(auth_type));
        }
        normalized.insert(name.clone(), e);
    }
    Some(serde_json::json!({ "mcpServers": normalized }))
}

/// Every SKILL.md across OpenClaw's skill tiers: the managed root `skills/`
/// and each agent workspace's `skills/`.
pub(super) fn skill_dirs(root: &Path, cfg: &serde_json::Value) -> Vec<PathBuf> {
    let mut found = Vec::new();
    super::hermes::collect_skill_md(&root.join("skills"), &mut found);
    for (_, _, workspace) in agent_rows(root, cfg) {
        super::hermes::collect_skill_md(&workspace.join("skills"), &mut found);
    }
    found
}

/// `(id, name, workspace)` for the default agent plus every `agents.entries[]`
/// row. Workspace resolution: absolute stays, `~/` expands, relative joins the
/// install root; entries default to `workspace-{id}` per OpenClaw convention.
fn agent_rows(root: &Path, cfg: &serde_json::Value) -> Vec<(String, String, PathBuf)> {
    let mut rows = Vec::new();
    if root.join("workspace").is_dir() {
        rows.push((
            "main".to_string(),
            "OpenClaw".to_string(),
            root.join("workspace"),
        ));
    }
    let entries = cfg
        .get("agents")
        .and_then(|a| a.get("entries"))
        .and_then(|e| e.as_array())
        .cloned()
        .unwrap_or_default();
    for entry in entries {
        let Some(id) = entry.get("id").and_then(|i| i.as_str()) else {
            continue;
        };
        let workspace = entry
            .get("workspace")
            .and_then(|w| w.as_str())
            .map(|w| resolve_path(root, w))
            .unwrap_or_else(|| root.join(format!("workspace-{id}")));
        let mut name: String = id.to_string();
        if let Some(first) = name.get(0..1) {
            let upper = first.to_uppercase();
            name.replace_range(0..1, &upper);
        }
        rows.push((id.to_string(), name, workspace));
    }
    rows
}

fn resolve_path(root: &Path, raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    let p = PathBuf::from(raw);
    if p.is_absolute() { p } else { root.join(raw) }
}

/// Normalize every OpenClaw agent: persona from its workspace SOUL.md (+
/// AGENTS.md as rules), memory from workspace markdown + daily notes, history
/// from `agents/<id>/sessions/*.jsonl`.
pub(super) fn source_agents(
    root: &Path,
    cfg: &serde_json::Value,
    out: &mut ImportOutcome,
) -> Vec<SourceAgent> {
    let mut agents = Vec::new();
    for (id, name, workspace) in agent_rows(root, cfg) {
        if !workspace.is_dir() {
            out.skipped
                .push(format!("agent {name}: workspace {} missing", workspace.display()));
            continue;
        }
        let persona = fs::read_to_string(workspace.join("SOUL.md")).unwrap_or_default();
        let rules = fs::read_to_string(workspace.join("AGENTS.md")).ok();

        agents.push(SourceAgent {
            name,
            description: "Imported from an OpenClaw install".to_string(),
            persona,
            rules,
            memory_files: workspace_memory_files(&workspace),
            conversations: read_sessions(&root.join("agents").join(&id).join("sessions"), out),
        });
    }
    agents
}

/// Memory files in an agent workspace: `MEMORY.md`, `USER.md`, and daily
/// notes under `memory/*.md`, parsed into discrete entries. Shared by scan
/// (counts) and apply (writes).
fn workspace_memory_files(workspace: &Path) -> Vec<(String, Vec<String>)> {
    let mut memory_files: Vec<(String, Vec<String>)> = Vec::new();
    for file in ["MEMORY.md", "USER.md"] {
        if let Ok(text) = fs::read_to_string(workspace.join(file)) {
            let entries = parse::memory_text_entries(&text);
            if !entries.is_empty() {
                memory_files.push((file.to_string(), entries));
            }
        }
    }
    if let Ok(dir) = fs::read_dir(workspace.join("memory")) {
        let mut daily: Vec<PathBuf> = dir
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
            .collect();
        daily.sort();
        for p in daily {
            if let Ok(text) = fs::read_to_string(&p) {
                let entries = parse::memory_text_entries(&text);
                if !entries.is_empty() {
                    let fname = p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("daily")
                        .to_string();
                    memory_files.push((format!("memory/{fname}"), entries));
                }
            }
        }
    }
    memory_files
}

/// File + line counts for a sessions directory — the scan-side stand-in for
/// [`read_sessions`], cheap enough for big installs.
fn session_counts(dir: &Path) -> (usize, usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut files = 0usize;
    let mut lines = 0usize;
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("jsonl") {
            files += 1;
            if let Ok(text) = fs::read_to_string(&p) {
                lines += text.lines().filter(|l| !l.trim().is_empty()).count();
            }
        }
    }
    (files, lines)
}

/// Read `sessions/*.jsonl` transcripts into conversations. Lines are parsed
/// tolerantly — `{role, content}` at the top level or under `message`, with
/// Anthropic-style content-block arrays flattened to text; anything else is
/// counted and skipped, never guessed at.
fn read_sessions(dir: &Path, out: &mut ImportOutcome) -> Vec<SourceConversation> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .collect();
    files.sort();

    let mut conversations = Vec::new();
    let mut unparsed = 0usize;
    for file in files {
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        let id = file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("session")
            .to_string();
        let mut conv = SourceConversation {
            id,
            title: String::new(),
            started_at: None,
            ended_at: None,
            messages: Vec::new(),
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                unparsed += 1;
                continue;
            };
            let node = v.get("message").unwrap_or(&v);
            let Some(role) = node.get("role").and_then(|r| r.as_str()) else {
                unparsed += 1;
                continue;
            };
            let content = flatten_content(node.get("content"));
            let timestamp = v.get("timestamp").and_then(parse::value_to_epoch);
            if conv.title.is_empty() && role == "user" && !content.trim().is_empty() {
                conv.title = parse::truncate_title(&content);
            }
            if let Some(ts) = timestamp {
                if conv.started_at.is_none() {
                    conv.started_at = Some(ts);
                }
                conv.ended_at = Some(ts);
            }
            conv.messages.push(SourceMessage {
                role: role.to_string(),
                content,
                tool_calls: node
                    .get("tool_calls")
                    .filter(|t| !t.is_null())
                    .map(|t| t.to_string()),
                timestamp,
            });
        }
        if !conv.messages.is_empty() {
            conversations.push(conv);
        }
    }
    if unparsed > 0 {
        out.skipped.push(format!(
            "conversation history: {unparsed} non-message lines in session logs skipped"
        ));
    }
    conversations
}

/// Message content as text: a plain string, or an Anthropic-style array of
/// blocks whose `text` fields are concatenated.
fn flatten_content(content: Option<&serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// What deliberately doesn't transfer from OpenClaw, named honestly.
pub(super) fn note_deferred(root: &Path, cfg: &serde_json::Value, out: &mut ImportOutcome) {
    if root.join("credentials").is_dir() || root.join("state").join("openclaw.sqlite").is_file() {
        out.skipped.push(
            "channel credentials (WhatsApp sessions, allowlists, OAuth state): imported with the channels slice"
                .into(),
        );
    }
    if cfg.get("cron").is_some() {
        out.skipped
            .push("cron jobs: imported by the scheduling slice".into());
    }
}

/// Build a realistic OpenClaw install in a temp dir. Shared by the walker
/// tests and the apply tests so both halves exercise one fixture.
#[cfg(test)]
pub(super) fn openclaw_fixture() -> tempfile::TempDir {
    fn write(path: PathBuf, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    let dir = tempfile::tempdir().unwrap();
    {
        let r = dir.path();
        write(
            r.join("openclaw.json"),
            r#"{
  // OpenClaw config — JSON5 with comments and trailing commas
  agents: {
    defaults: { workspace: "workspace" },
    entries: [
      { id: "scout", workspace: "workspace-scout", },
    ],
  },
  mcp: {
    servers: {
      context7: { command: "uvx", args: ["context7-mcp"], env: { API_BASE: "https://internal.example" } },
      docs: { url: "https://mcp.example.com", transport: "streamable-http", auth: "oauth" },
      old: { url: "https://old.example.com/sse", transport: "sse", headers: { Authorization: "Bearer sek-token" } },
      off: { command: "npx", args: ["dead-server"], enabled: false, },
    },
  },
  env: { OPENAI_API_KEY: "sk-oc-inline" },
  cron: { jobs: [] },
}"#,
        );
        write(r.join("workspace/SOUL.md"), "# Soul\nYou are Claw.\n");
        write(
            r.join("workspace/AGENTS.md"),
            "# Rules\nAlways cite sources.\n",
        );
        write(
            r.join("workspace/MEMORY.md"),
            "standup is at 9\n\nprefers short replies\n",
        );
        write(r.join("workspace/USER.md"), "works at Acme\n");
        write(
            r.join("workspace/memory/2026-01-05.md"),
            "# Daily\nshipped the beta\n",
        );
        write(
            r.join("workspace/skills/notes/SKILL.md"),
            "---\nname: notes\ndescription: Take notes\n---\nTake notes.\n",
        );
        write(
            r.join("skills/tools/websearch/SKILL.md"),
            "---\nname: websearch\ndescription: Search\n---\nSearch.\n",
        );
        write(r.join("workspace-scout/SOUL.md"), "You are Scout.\n");
        write(
            r.join("agents/main/sessions/m1.jsonl"),
            concat!(
                r#"{"role":"user","content":"Find the report","timestamp":1700200000}"#,
                "\n",
                r#"{"message":{"role":"assistant","content":[{"type":"text","text":"Found it."}]},"timestamp":1700200060}"#,
                "\n",
                r#"{"type":"debug","note":"not a message"}"#,
                "\n",
            ),
        );
        write(
            r.join("agents/scout/sessions/s1.jsonl"),
            concat!(
                r#"{"role":"user","content":"Watch the feed","timestamp":1700300000}"#,
                "\n",
                r#"{"role":"assistant","content":"Watching.","timestamp":1700300030}"#,
                "\n",
            ),
        );
        write(r.join(".env"), "DEEPSEEK_API_KEY=sk-oc-env\n");
        write(r.join("credentials/whatsapp/acc1/creds.json"), "{}");
        fs::create_dir_all(r.join("state")).unwrap();
        fs::write(r.join("state/openclaw.sqlite"), "sqlite-bytes").unwrap();
    }
    dir
}

#[cfg(test)]
mod tests {
    use super::super::{detect, scan as scan_root};
    use super::*;

    #[test]
    fn detects_openclaw() {
        let f = openclaw_fixture();
        assert_eq!(detect(f.path()), Some(SourceKind::OpenClaw));
    }

    #[test]
    fn manifest_covers_openclaw_artifacts() {
        let f = openclaw_fixture();
        let m = scan_root(f.path()).unwrap();
        assert_eq!(m.source, SourceKind::OpenClaw);
        assert_eq!(m.count(ItemKind::Agent), 2);
        assert_eq!(m.count(ItemKind::McpServer), 3);
        assert_eq!(m.count(ItemKind::Skill), 2);
        assert_eq!(m.count(ItemKind::Memory), 3);
        assert_eq!(m.count(ItemKind::Session), 2);
        // .env key + config-env key
        assert_eq!(m.count(ItemKind::Credential), 2);
        // stdio server is Code tier; disabled server surfaced as a note.
        let ctx = m.items.iter().find(|i| i.name == "context7").unwrap();
        assert_eq!(ctx.tier, TrustTier::Code);
        assert!(m.notes.iter().any(|n| n.contains("off")));
        // Secrets never appear.
        for item in &m.items {
            assert!(!item.detail.contains("sk-oc-env"));
            assert!(!item.detail.contains("sk-oc-inline"));
            assert!(!item.detail.contains("sek-token"));
        }
    }
}
