//! Apply an import: turn a scanned foreign install into real Nebo artifacts.
//!
//! The per-source entry points ([`apply_hermes`], [`apply_openclaw`]) are
//! synchronous and take only a [`db::Store`] plus target directories, so they
//! are directly testable. Both feed the same shared importers below — one
//! funnel, per-source walkers — and the [`apply`] wrapper resolves Nebo's real
//! directories and runs the canonical post-install steps (loader reloads,
//! agent finalization, MCP bridge connect, embedding backfill).
//!
//! Idempotent by skip: anything that already exists (same integration name,
//! skill directory, agent directory, provider profile, memory key, chat id) is
//! left untouched and recorded in `skipped`, so re-running an import never
//! clobbers or duplicates. The source directory is never written to.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use types::NeboError;

use super::hermes;
use super::manifest::SourceKind;
use super::openclaw;

/// `.env` keys that map onto Nebo LLM provider profiles. Everything else
/// (channel tokens, service secrets) belongs to later slices and is reported
/// as skipped rather than guessed at.
const PROVIDER_ENV_KEYS: &[(&str, &str)] = &[
    ("ANTHROPIC_API_KEY", "anthropic"),
    ("OPENAI_API_KEY", "openai"),
    ("GEMINI_API_KEY", "gemini"),
    ("GOOGLE_API_KEY", "gemini"),
    ("DEEPSEEK_API_KEY", "deepseek"),
];

/// What an apply actually did — the receipt shown after the confirm.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    pub agents: usize,
    pub skills: usize,
    pub mcp_servers: usize,
    pub auth_profiles: usize,
    pub memories: usize,
    pub chats: usize,
    pub chat_messages: usize,
    /// Id + name of the first created employee, when one was created.
    pub agent_id: Option<String>,
    pub agent_name: Option<String>,
    /// Everything found but not imported, with the reason — honesty over silence.
    pub skipped: Vec<String>,
    /// All employees created this run, for post-install finalization.
    #[serde(skip)]
    pub created_agents: Vec<(String, String)>,
}

/// Where the import writes. Split from `AppState` so the cores are testable
/// against a temp store and temp dirs.
pub struct ApplyTargets<'a> {
    pub store: &'a db::Store,
    /// `<data_dir>/user/agents`
    pub user_agents_dir: PathBuf,
    /// `<data_dir>/user/skills`
    pub user_skills_dir: PathBuf,
}

/// One conversation normalized out of a foreign install, ready to become a
/// Nebo chat. Both walkers produce this shape.
pub(super) struct SourceConversation {
    /// Stable per-source id — becomes part of the deterministic chat id.
    pub id: String,
    /// First user message, truncated. Empty = "Imported conversation".
    pub title: String,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub messages: Vec<SourceMessage>,
}

pub(super) struct SourceMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Option<String>,
    pub timestamp: Option<i64>,
}

/// One employee normalized out of a foreign install.
pub(super) struct SourceAgent {
    pub name: String,
    pub description: String,
    /// Persona prose (SOUL.md) — becomes the AGENT.md body and `soul`.
    pub persona: String,
    /// Behavior rules (AGENTS.md), when the source splits them out.
    pub rules: Option<String>,
    /// Memory files as `(file_name, entries)`.
    pub memory_files: Vec<(String, Vec<String>)>,
    pub conversations: Vec<SourceConversation>,
}

// ─── Per-source entry points ────────────────────────────────────────────────

/// Apply a Hermes install: MCP integrations, skills, the employee persona,
/// memory, conversation history, and LLM provider keys.
pub fn apply_hermes(t: &ApplyTargets, root: &Path) -> Result<ImportOutcome, NeboError> {
    if super::detect(root) != Some(SourceKind::Hermes) {
        return Err(NeboError::Validation(format!(
            "{} is not a Hermes install directory",
            root.display()
        )));
    }
    let mut out = ImportOutcome::default();

    match hermes::mcp_block(root) {
        Ok(Some(block)) => import_mcp_block(t, &block, &mut out)?,
        Ok(None) => {}
        Err(e) => out
            .skipped
            .push(format!("MCP servers: config.yaml unparseable ({e})")),
    }

    let mut skill_dirs = Vec::new();
    hermes::collect_skill_md(&root.join("skills"), &mut skill_dirs);
    import_skill_dirs(t, &skill_dirs, &mut out);

    let agent = hermes::source_agent(root, &mut out);
    if let Some(agent) = agent {
        import_agent(t, &agent, "hermes", &mut out)?;
    }

    if let Ok(text) = fs::read_to_string(root.join(".env")) {
        import_provider_env(t, &text, "Hermes", &mut out)?;
    }

    if root.join("cron").join("jobs.json").is_file() {
        out.skipped
            .push("cron jobs: imported by the scheduling slice".into());
    }
    Ok(out)
}

/// Apply an OpenClaw install. OpenClaw is genuinely multi-agent: every entry
/// in `agents.entries[]` (plus the default workspace agent) becomes its own
/// Nebo employee with its own persona, memory, and conversation history.
pub fn apply_openclaw(t: &ApplyTargets, root: &Path) -> Result<ImportOutcome, NeboError> {
    if super::detect(root) != Some(SourceKind::OpenClaw) {
        return Err(NeboError::Validation(format!(
            "{} is not an OpenClaw install directory",
            root.display()
        )));
    }
    let mut out = ImportOutcome::default();

    let cfg = match openclaw::config(root) {
        Ok(c) => c,
        Err(e) => {
            return Err(NeboError::Validation(format!(
                "openclaw.json unparseable: {e}"
            )));
        }
    };

    if let Some(block) = openclaw::mcp_block(&cfg, &mut out) {
        import_mcp_block(t, &block, &mut out)?;
    }

    import_skill_dirs(t, &openclaw::skill_dirs(root, &cfg), &mut out);

    for agent in openclaw::source_agents(root, &cfg, &mut out) {
        import_agent(t, &agent, "openclaw", &mut out)?;
    }

    if let Ok(text) = fs::read_to_string(root.join(".env")) {
        import_provider_env(t, &text, "OpenClaw", &mut out)?;
    }
    // openclaw.json can also carry env inline.
    if let Some(env) = cfg.get("env").and_then(|e| e.as_object()) {
        let text: String = env
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|s| format!("{k}={s}\n")))
            .collect();
        import_provider_env(t, &text, "OpenClaw", &mut out)?;
    }

    openclaw::note_deferred(root, &cfg, &mut out);
    Ok(out)
}

// ─── Shared importers (the one funnel) ──────────────────────────────────────

/// Canonical `mcpServers` block → `mcp_integrations` rows. Existing names are
/// skipped so a re-import can't duplicate or clobber configured servers.
fn import_mcp_block(
    t: &ApplyTargets,
    block: &serde_json::Value,
    out: &mut ImportOutcome,
) -> Result<(), NeboError> {
    let existing: HashSet<String> = t
        .store
        .list_mcp_integrations()?
        .into_iter()
        .map(|i| i.name)
        .collect();
    for s in crate::handlers::integrations::parse_mcp_servers_block(block) {
        if existing.contains(&s.name) {
            out.skipped
                .push(format!("MCP server {}: already configured", s.name));
            continue;
        }
        let id = uuid::Uuid::new_v4().to_string();
        t.store.create_mcp_integration(
            &id,
            &s.name,
            &s.server_type,
            s.server_url.as_deref(),
            &s.auth_type,
            s.metadata.as_deref(),
            None,
        )?;
        out.mcp_servers += 1;
    }
    Ok(())
}

/// Skill directories (each containing SKILL.md) → `<user>/skills/<name>/`,
/// copied wholesale so bundled resources survive. Source nesting flattens to
/// Nebo's flat user tier; a name collision skips rather than merges.
fn import_skill_dirs(t: &ApplyTargets, skill_mds: &[PathBuf], out: &mut ImportOutcome) {
    let mut seen: HashSet<String> = HashSet::new();
    for skill_md in skill_mds {
        let Some(skill_dir) = skill_md.parent() else {
            continue;
        };
        let name = skill_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("skill")
            .to_string();
        if !seen.insert(name.clone()) {
            continue;
        }
        let dest = t.user_skills_dir.join(&name);
        if dest.exists() {
            out.skipped.push(format!("skill {name}: already installed"));
            continue;
        }
        match copy_dir(skill_dir, &dest) {
            Ok(()) => out.skills += 1,
            Err(e) => out.skipped.push(format!("skill {name}: copy failed ({e})")),
        }
    }
}

/// One normalized [`SourceAgent`] → employee + memory + history. The on-disk
/// layout mirrors the user-created agent pathway (AGENT.md + agent.json +
/// manifest.json under `<user>/agents/<name>/`, napp_path set) so the agent FS
/// watcher treats it exactly like a hand-made employee.
fn import_agent(
    t: &ApplyTargets,
    agent: &SourceAgent,
    source_tag: &str,
    out: &mut ImportOutcome,
) -> Result<(), NeboError> {
    let agent_dir = t.user_agents_dir.join(&agent.name);
    let agent_id = if agent_dir.exists() {
        out.skipped.push(format!(
            "agent {}: already exists, persona untouched",
            agent.name
        ));
        match existing_agent_id(t, &agent.name)? {
            Some(id) => id,
            None => {
                out.skipped.push(format!(
                    "agent {}: directory exists but no DB row — memory and history not attached",
                    agent.name
                ));
                return Ok(());
            }
        }
    } else {
        let agent_md = format!(
            "---\nname: {}\ndescription: {}\n---\n\n{}",
            agent.name, agent.description, agent.persona
        );
        let frontmatter = serde_json::json!({ "workflows": {}, "skills": [] }).to_string();
        let id = uuid::Uuid::new_v4().to_string();
        t.store.create_agent(
            &id,
            None,
            &agent.name,
            &agent.description,
            &agent_md,
            &frontmatter,
            None,
            None,
        )?;
        // Persona layers: SOUL.md is the soul verbatim; a separate rules file
        // (OpenClaw AGENTS.md) lands in the rules column.
        if agent.rules.is_some() || !agent.persona.is_empty() {
            t.store.update_agent(
                &id,
                &agent.name,
                &agent.description,
                &agent_md,
                &frontmatter,
                None,
                None,
                Some(&agent.persona),
                agent.rules.as_deref(),
                None,
                None,
                None,
                None,
            )?;
        }

        fs::create_dir_all(&agent_dir)?;
        fs::write(agent_dir.join("AGENT.md"), &agent_md)?;
        fs::write(agent_dir.join("agent.json"), &frontmatter)?;
        let manifest = serde_json::json!({
            "id": id,
            "name": agent.name,
            "version": "1.0.0",
            "type": "agent",
            "description": agent.description,
        });
        fs::write(
            agent_dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap_or_default(),
        )?;
        t.store
            .set_agent_napp_path(&id, &agent_dir.to_string_lossy())?;

        out.agents += 1;
        if out.agent_id.is_none() {
            out.agent_id = Some(id.clone());
            out.agent_name = Some(agent.name.clone());
        }
        out.created_agents.push((id.clone(), agent.name.clone()));
        id
    };

    import_memory_files(t, &agent_id, &agent.memory_files, source_tag, out)?;
    import_conversations(t, &agent_id, source_tag, &agent.conversations, out)?;
    Ok(())
}

/// Look up a previously imported employee by name (re-import runs).
fn existing_agent_id(t: &ApplyTargets, name: &str) -> Result<Option<String>, NeboError> {
    Ok(t.store
        .list_agents(i64::MAX, 0)?
        .into_iter()
        .find(|a| a.name == name)
        .map(|a| a.id))
}

/// Memory files → discrete Nebo memories scoped to the employee — the
/// high-fidelity path: each parsed entry becomes its own searchable row
/// through the same `upsert_memory` pathway native extraction uses, and the
/// embedding backfill vectors them like any other memory. `USER.md` (user
/// profile) lands in `tacit/preferences`; everything else in `tacit/general`.
fn import_memory_files(
    t: &ApplyTargets,
    agent_id: &str,
    files: &[(String, Vec<String>)],
    source_tag: &str,
    out: &mut ImportOutcome,
) -> Result<(), NeboError> {
    if files.is_empty() {
        return Ok(());
    }
    let owner = t.store.ensure_local_user_id()?;
    let scope = agent::memory::agent_memory_scope(&owner, agent_id);
    let tags = format!(r#"["imported","{source_tag}"]"#);
    let metadata = serde_json::json!({
        "confidence": 0.7,
        "source": format!("{source_tag}-import"),
    })
    .to_string();

    let mut already = 0usize;
    for (file, entries) in files {
        let namespace = if file == "USER.md" {
            "tacit/preferences"
        } else {
            "tacit/general"
        };
        for entry in entries {
            let value = agent::sanitize::sanitize_memory_value(entry);
            let key = agent::sanitize::sanitize_memory_key(&memory_key(&value));
            if t.store
                .get_memory_by_key_and_user(namespace, &key, &scope)?
                .is_some()
            {
                already += 1;
                continue;
            }
            t.store
                .upsert_memory(namespace, &key, &value, Some(&tags), Some(&metadata), &scope)?;
            out.memories += 1;
        }
    }
    if already > 0 {
        out.skipped
            .push(format!("memory: {already} entries already imported"));
    }
    Ok(())
}

/// Conversations → one Nebo chat each, threaded under the employee
/// (`agent:{id}:thread:{chat_id}` — the same session-name shape native threads
/// use, so they list like any other conversation). Original timestamps are
/// preserved via the imported-insert variants.
fn import_conversations(
    t: &ApplyTargets,
    agent_id: &str,
    source_tag: &str,
    conversations: &[SourceConversation],
    out: &mut ImportOutcome,
) -> Result<(), NeboError> {
    if conversations.is_empty() {
        return Ok(());
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut already = 0usize;
    let mut skipped_roles = 0usize;
    for c in conversations {
        let chat_id = format!(
            "{source_tag}-{}",
            c.id.chars()
                .map(|ch| if ch.is_alphanumeric() || ch == '-' { ch } else { '-' })
                .collect::<String>()
        );
        if t.store.get_chat(&chat_id)?.is_some() {
            already += 1;
            continue;
        }
        let created = c.started_at.unwrap_or(now);
        let updated = c.ended_at.unwrap_or(created);
        let title = if c.title.is_empty() {
            "Imported conversation"
        } else {
            &c.title
        };
        let session_name = format!("agent:{agent_id}:thread:{chat_id}");
        t.store
            .create_chat_imported(&chat_id, &session_name, title, created, updated)?;
        for (i, msg) in c.messages.iter().enumerate() {
            // chat_messages constrains role; anything else is source-internal
            // (compression markers etc.) and doesn't belong in the transcript.
            if !matches!(msg.role.as_str(), "user" | "assistant" | "system" | "tool") {
                skipped_roles += 1;
                continue;
            }
            let ts = msg.timestamp.unwrap_or(created + i as i64);
            t.store.create_chat_message_imported(
                &format!("{chat_id}-m{i}"),
                &chat_id,
                &msg.role,
                &msg.content,
                msg.tool_calls.as_deref(),
                None,
                ts,
            )?;
            out.chat_messages += 1;
        }
        out.chats += 1;
    }
    if already > 0 {
        out.skipped.push(format!(
            "conversation history: {already} conversations already imported"
        ));
    }
    if skipped_roles > 0 {
        out.skipped.push(format!(
            "conversation history: {skipped_roles} internal (non-transcript) messages skipped"
        ));
    }
    Ok(())
}

/// Known LLM provider keys from dotenv-style text → `auth_profiles`, matching
/// the existing provider-settings pathway (which stores `api_key` as given —
/// readers pass it straight to providers, so an encrypted value would break
/// auth). Providers that already have a profile are skipped. Unknown keys are
/// reported, not guessed at.
fn import_provider_env(
    t: &ApplyTargets,
    text: &str,
    source_label: &str,
    out: &mut ImportOutcome,
) -> Result<(), NeboError> {
    let existing: HashSet<String> = t
        .store
        .list_auth_profiles()?
        .into_iter()
        .map(|p| p.provider)
        .collect();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_start_matches("export ").trim();
        let value = value.trim().trim_matches('"').trim_matches('\'');
        if value.is_empty() {
            continue;
        }
        match PROVIDER_ENV_KEYS.iter().find(|(k, _)| *k == key) {
            Some((_, provider)) => {
                if existing.contains(*provider) {
                    out.skipped
                        .push(format!("{key}: {provider} profile already exists"));
                    continue;
                }
                let id = uuid::Uuid::new_v4().to_string();
                t.store.create_auth_profile(
                    &id,
                    &format!("Imported from {source_label} ({provider})"),
                    provider,
                    value,
                    None,
                    None,
                    50,
                    1,
                    None,
                    None,
                )?;
                out.auth_profiles += 1;
            }
            None => out
                .skipped
                .push(format!("{key}: not imported (channel/service tokens come later)")),
        }
    }
    Ok(())
}

/// Deterministic memory key from an entry's content: a short slug plus a
/// content hash, so re-importing the same entry hits the same row (idempotent)
/// while distinct entries can never collide.
fn memory_key(value: &str) -> String {
    let slug: String = value
        .split_whitespace()
        .take(5)
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in value.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{}-{hash:08x}", slug.trim_matches('-'))
}

/// Recursive copy, used to adopt skill directories with their resources.
fn copy_dir(src: &Path, dest: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Full apply against the live server: detect the source, run its core, then
/// the canonical post-install steps — skill loader reload, agent finalization
/// (same routine as a marketplace hire) for every created employee, the MCP
/// bridge connect pass, and the embedding backfill.
pub async fn apply(state: &crate::state::AppState, root: &Path) -> Result<ImportOutcome, NeboError> {
    let user = config::user_dir()?;
    let targets = ApplyTargets {
        store: &state.store,
        user_agents_dir: user.join("agents"),
        user_skills_dir: user.join("skills"),
    };
    let outcome = match super::detect(root) {
        Some(SourceKind::Hermes) => apply_hermes(&targets, root)?,
        Some(SourceKind::OpenClaw) => apply_openclaw(&targets, root)?,
        None => {
            return Err(NeboError::Validation(format!(
                "{} is not a recognized Hermes or OpenClaw install directory",
                root.display()
            )));
        }
    };

    if outcome.skills > 0 {
        state.skill_loader.reload_from_disk().await;
    }
    for (id, name) in &outcome.created_agents {
        crate::codes::finalize_agent_install(state, id, name).await;
    }
    if outcome.mcp_servers > 0 {
        crate::handlers::integrations::sync_bridge(state).await;
    }
    // Vector the imported memories through the SAME backfill boot runs —
    // batched and rate-limited inside. Without a configured embedding
    // provider FTS recall still works immediately; vectors arrive when a
    // provider is configured and the next boot backfill runs.
    if outcome.memories > 0 {
        if let Some(ep) = state.embedding_provider.clone() {
            let store = state.store.clone();
            tokio::spawn(async move {
                agent::memory::backfill_missing_embeddings(store, ep).await;
            });
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    struct Fixture {
        _source: tempfile::TempDir,
        _nebo: tempfile::TempDir,
        root: PathBuf,
        store: db::Store,
        agents_dir: PathBuf,
        skills_dir: PathBuf,
    }

    fn setup_with(source: tempfile::TempDir) -> Fixture {
        let nebo = tempdir().unwrap();
        let store = db::Store::new(nebo.path().join("nebo.db").to_str().unwrap()).unwrap();
        let agents_dir = nebo.path().join("user/agents");
        let skills_dir = nebo.path().join("user/skills");
        let root = source.path().to_path_buf();
        Fixture {
            _source: source,
            _nebo: nebo,
            root,
            store,
            agents_dir,
            skills_dir,
        }
    }

    fn setup() -> Fixture {
        setup_with(hermes::hermes_fixture())
    }

    fn targets(f: &Fixture) -> ApplyTargets<'_> {
        ApplyTargets {
            store: &f.store,
            user_agents_dir: f.agents_dir.clone(),
            user_skills_dir: f.skills_dir.clone(),
        }
    }

    #[test]
    fn applies_all_artifact_kinds() {
        let f = setup();
        let out = apply_hermes(&targets(&f), &f.root).unwrap();

        assert_eq!(out.mcp_servers, 3);
        assert_eq!(out.skills, 2);
        assert_eq!(out.agents, 1);
        assert_eq!(out.auth_profiles, 1);
        assert_eq!(out.memories, 3);
        assert_eq!(out.chats, 2);
        assert_eq!(out.chat_messages, 4);

        // MCP rows landed with the right transport + normalized auth.
        let integrations = f.store.list_mcp_integrations().unwrap();
        let by_name = |n: &str| integrations.iter().find(|i| i.name == n).unwrap();
        assert_eq!(by_name("filesystem").server_type, "stdio");
        assert!(by_name("filesystem")
            .metadata
            .as_deref()
            .unwrap()
            .contains("npx"));
        assert_eq!(by_name("linear").auth_type, "oauth");
        assert_eq!(by_name("company_api").auth_type, "api_key");

        // Skill copied with its resources; nested category flattened.
        assert!(f.skills_dir.join("deploy/SKILL.md").is_file());
        assert!(f.skills_dir.join("deploy/scripts/run.sh").is_file());
        assert!(f.skills_dir.join("quick/SKILL.md").is_file());

        // Employee created: DB row + on-disk trio + napp_path + soul.
        let id = out.agent_id.as_deref().unwrap();
        let agent = f.store.get_agent(id).unwrap().unwrap();
        assert_eq!(agent.name, "Hermes");
        assert!(agent.agent_md.contains("You are Atlas."));
        assert!(agent.soul.as_deref().unwrap().contains("You are Atlas."));
        let dir = f.agents_dir.join("Hermes");
        assert!(dir.join("AGENT.md").is_file());
        assert!(dir.join("agent.json").is_file());
        assert!(dir.join("manifest.json").is_file());
        assert_eq!(agent.napp_path.as_deref(), Some(&*dir.to_string_lossy()));

        // Memories parsed into discrete rows scoped to the imported employee,
        // in the right namespaces, carrying provenance.
        let owner = f.store.ensure_local_user_id().unwrap();
        let scope = agent::memory::agent_memory_scope(&owner, id);
        let in_scope = |ns: &str| {
            f.store
                .list_memories_by_namespace(ns, 100, 0)
                .unwrap()
                .into_iter()
                .filter(|m| m.user_id == scope)
                .collect::<Vec<_>>()
        };
        let general = in_scope("tacit/general");
        assert_eq!(general.len(), 2);
        assert!(general.iter().any(|m| m.value.contains("prefers dark mode")));
        let prefs = in_scope("tacit/preferences");
        assert_eq!(prefs.len(), 1);
        assert!(prefs[0].value.contains("name is Sam"));
        assert!(prefs[0].metadata.as_deref().unwrap().contains("hermes-import"));

        // History became real chats threaded under the employee, with original
        // timestamps and titles; Hermes-internal roles were filtered.
        let chat = f.store.get_chat("hermes-sess-1").unwrap().unwrap();
        assert_eq!(chat.title, "Help me plan the launch");
        assert_eq!(chat.created_at, 1700000000);
        assert_eq!(
            chat.session_name.as_deref(),
            Some(format!("agent:{id}:thread:hermes-sess-1").as_str())
        );
        let msgs = f.store.get_chat_messages("hermes-sess-1").unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].created_at, 1700000000);
        let sess2 = f.store.get_chat_messages("hermes-sess-2").unwrap();
        assert!(sess2.iter().any(|m| m.tool_calls.is_some()));
        assert!(out
            .skipped
            .iter()
            .any(|s| s.contains("internal (non-transcript)")));

        // Provider key imported to the matching profile; channel token deferred.
        let profiles = f.store.list_auth_profiles().unwrap();
        let anthropic = profiles.iter().find(|p| p.provider == "anthropic").unwrap();
        assert_eq!(anthropic.api_key, "sk-secretxxx");
        assert!(out.skipped.iter().any(|s| s.starts_with("TELEGRAM_TOKEN")));

        // Deferred slices are named, not silently dropped.
        assert!(out.skipped.iter().any(|s| s.starts_with("cron jobs:")));
    }

    #[test]
    fn reapply_skips_everything_and_duplicates_nothing() {
        let f = setup();
        let first = apply_hermes(&targets(&f), &f.root).unwrap();
        assert_eq!(first.mcp_servers, 3);

        let second = apply_hermes(&targets(&f), &f.root).unwrap();
        assert_eq!(second.mcp_servers, 0);
        assert_eq!(second.skills, 0);
        assert_eq!(second.agents, 0);
        assert_eq!(second.auth_profiles, 0);
        assert_eq!(second.memories, 0);
        assert_eq!(second.chats, 0);
        assert_eq!(second.chat_messages, 0);

        assert_eq!(f.store.list_mcp_integrations().unwrap().len(), 3);
        assert_eq!(f.store.count_agents().unwrap(), 1);
        assert_eq!(
            f.store
                .list_auth_profiles()
                .unwrap()
                .iter()
                .filter(|p| p.provider == "anthropic")
                .count(),
            1
        );
    }

    #[test]
    fn rejects_non_hermes_directory() {
        let f = setup();
        let empty = tempdir().unwrap();
        assert!(apply_hermes(&targets(&f), empty.path()).is_err());
    }

    #[test]
    fn source_directory_is_never_modified() {
        let f = setup();
        let before = snapshot(&f.root);
        apply_hermes(&targets(&f), &f.root).unwrap();
        assert_eq!(before, snapshot(&f.root));
    }

    #[test]
    fn openclaw_imports_multiple_employees() {
        let f = setup_with(openclaw::openclaw_fixture());
        let out = apply_openclaw(&targets(&f), &f.root).unwrap();

        // Two employees: the default workspace agent and the "scout" entry.
        assert_eq!(out.agents, 2);
        assert_eq!(out.created_agents.len(), 2);
        let agents = f.store.list_agents(100, 0).unwrap();
        let oc = agents.iter().find(|a| a.name == "OpenClaw").unwrap();
        assert!(oc.soul.as_deref().unwrap().contains("You are Claw."));
        assert!(oc.rules.as_deref().unwrap().contains("Always cite sources"));
        let scout = agents.iter().find(|a| a.name == "Scout").unwrap();
        assert!(scout.agent_md.contains("You are Scout."));

        // MCP: stdio + oauth + header servers imported, disabled one skipped.
        assert_eq!(out.mcp_servers, 3);
        let integrations = f.store.list_mcp_integrations().unwrap();
        let by_name = |n: &str| integrations.iter().find(|i| i.name == n).unwrap();
        assert_eq!(by_name("context7").server_type, "stdio");
        assert_eq!(by_name("docs").server_type, "http");
        assert_eq!(by_name("docs").auth_type, "oauth");
        assert_eq!(by_name("old").server_type, "sse");
        assert_eq!(by_name("old").auth_type, "api_key");
        assert!(out.skipped.iter().any(|s| s.contains("off") && s.contains("disabled")));

        // Skills from both tiers, deduped and flattened.
        assert_eq!(out.skills, 2);
        assert!(f.skills_dir.join("notes/SKILL.md").is_file());
        assert!(f.skills_dir.join("websearch/SKILL.md").is_file());

        // Memory scoped per employee: default agent got workspace files,
        // including a daily note; scout has none.
        assert_eq!(out.memories, 4);
        let owner = f.store.ensure_local_user_id().unwrap();
        let scope = agent::memory::agent_memory_scope(&owner, &oc.id);
        let general: Vec<_> = f
            .store
            .list_memories_by_namespace("tacit/general", 100, 0)
            .unwrap()
            .into_iter()
            .filter(|m| m.user_id == scope)
            .collect();
        assert_eq!(general.len(), 3);
        assert!(general.iter().any(|m| m.value.contains("standup is at 9")));

        // History: one conversation per agent from sessions/*.jsonl, with
        // Anthropic-style content blocks flattened and junk lines skipped.
        assert_eq!(out.chats, 2);
        assert_eq!(out.chat_messages, 4);
        let chat = f.store.get_chat("openclaw-m1").unwrap().unwrap();
        assert_eq!(chat.title, "Find the report");
        assert_eq!(
            chat.session_name.as_deref(),
            Some(format!("agent:{}:thread:openclaw-m1", oc.id).as_str())
        );
        let msgs = f.store.get_chat_messages("openclaw-m1").unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1].content, "Found it.");
        assert_eq!(msgs[0].created_at, 1700200000);

        // Provider keys from BOTH .env and the config env block.
        let profiles = f.store.list_auth_profiles().unwrap();
        assert!(profiles.iter().any(|p| p.provider == "deepseek"));
        assert!(profiles.iter().any(|p| p.provider == "openai"));

        // Channel credentials and cron honestly deferred.
        assert!(out.skipped.iter().any(|s| s.contains("channel credentials")));
        assert!(out.skipped.iter().any(|s| s.starts_with("cron jobs:")));
    }

    #[test]
    fn openclaw_reapply_duplicates_nothing() {
        let f = setup_with(openclaw::openclaw_fixture());
        apply_openclaw(&targets(&f), &f.root).unwrap();
        let second = apply_openclaw(&targets(&f), &f.root).unwrap();
        assert_eq!(second.agents, 0);
        assert_eq!(second.skills, 0);
        assert_eq!(second.mcp_servers, 0);
        assert_eq!(second.memories, 0);
        assert_eq!(second.chats, 0);
        assert_eq!(f.store.count_agents().unwrap(), 2);
    }

    #[test]
    fn openclaw_source_never_modified() {
        let f = setup_with(openclaw::openclaw_fixture());
        let before = snapshot(&f.root);
        apply_openclaw(&targets(&f), &f.root).unwrap();
        assert_eq!(before, snapshot(&f.root));
    }

    fn snapshot(root: &Path) -> Vec<(PathBuf, u64)> {
        let mut v = Vec::new();
        fn walk(dir: &Path, v: &mut Vec<(PathBuf, u64)>) {
            let Ok(entries) = fs::read_dir(dir) else { return };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, v);
                } else {
                    let len = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                    v.push((p, len));
                }
            }
        }
        walk(root, &mut v);
        v.sort();
        v
    }
}
