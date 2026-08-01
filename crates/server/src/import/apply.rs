//! Apply an import: turn a scanned foreign install into real Nebo artifacts.
//!
//! The core ([`apply_hermes`]) is synchronous and takes only a [`db::Store`]
//! plus target directories, so it is directly testable. The [`apply`] wrapper
//! resolves Nebo's real directories and runs the canonical post-install steps
//! (loader reloads, agent finalization, MCP bridge connect).
//!
//! Idempotent by skip: anything that already exists (same integration name,
//! skill directory, agent directory, provider profile) is left untouched and
//! recorded in `skipped`, so re-running an import never clobbers or duplicates.
//! The source directory is never written to.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use types::NeboError;

use super::hermes;
use super::manifest::SourceKind;

/// Hermes `.env` keys that map onto Nebo LLM provider profiles. Everything
/// else in `.env` (channel tokens, service secrets) belongs to later slices
/// and is reported as skipped rather than guessed at.
const PROVIDER_ENV_KEYS: &[(&str, &str)] = &[
    ("ANTHROPIC_API_KEY", "anthropic"),
    ("OPENAI_API_KEY", "openai"),
    ("GEMINI_API_KEY", "gemini"),
    ("GOOGLE_API_KEY", "gemini"),
    ("DEEPSEEK_API_KEY", "deepseek"),
];

/// What an apply actually did — the receipt shown after the confirm.
#[derive(Debug, Default, Serialize)]
pub struct ImportOutcome {
    pub agents: usize,
    pub skills: usize,
    pub mcp_servers: usize,
    pub auth_profiles: usize,
    /// Id + name of the created employee, when one was created.
    pub agent_id: Option<String>,
    pub agent_name: Option<String>,
    /// Everything found but not imported, with the reason — honesty over silence.
    pub skipped: Vec<String>,
}

/// Where the import writes. Split from `AppState` so the core is testable
/// against a temp store and temp dirs.
pub struct ApplyTargets<'a> {
    pub store: &'a db::Store,
    /// `<data_dir>/user/agents`
    pub user_agents_dir: PathBuf,
    /// `<data_dir>/user/skills`
    pub user_skills_dir: PathBuf,
}

/// Apply a Hermes install to Nebo: MCP integrations, skills, the employee
/// persona, and LLM provider keys. Memory, history, cron, and channel tokens
/// are later slices and are reported in `skipped`.
pub fn apply_hermes(t: &ApplyTargets, root: &Path) -> Result<ImportOutcome, NeboError> {
    if super::detect(root) != Some(SourceKind::Hermes) {
        return Err(NeboError::Validation(format!(
            "{} is not a Hermes install directory",
            root.display()
        )));
    }
    let mut out = ImportOutcome::default();
    apply_mcp(t, root, &mut out)?;
    apply_skills(t, root, &mut out);
    apply_agent(t, root, &mut out)?;
    apply_provider_keys(t, root, &mut out)?;
    note_deferred(root, &mut out);
    Ok(out)
}

/// `config.yaml` `mcp_servers:` → `mcp_integrations` rows. Existing names are
/// skipped so a re-import can't duplicate or clobber configured servers.
fn apply_mcp(t: &ApplyTargets, root: &Path, out: &mut ImportOutcome) -> Result<(), NeboError> {
    let block = match hermes::mcp_block(root) {
        Ok(Some(b)) => b,
        Ok(None) => return Ok(()),
        Err(e) => {
            out.skipped.push(format!("MCP servers: config.yaml unparseable ({e})"));
            return Ok(());
        }
    };
    let existing: std::collections::HashSet<String> = t
        .store
        .list_mcp_integrations()?
        .into_iter()
        .map(|i| i.name)
        .collect();
    for s in crate::handlers::integrations::parse_mcp_servers_block(&block) {
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

/// `skills/**/SKILL.md` → `<user>/skills/<name>/`, copied wholesale so bundled
/// resources (scripts/, references/, …) survive. Hermes nests skills under
/// category directories; Nebo's user tier is flat, so the skill directory name
/// is the key and a name collision skips rather than merges.
fn apply_skills(t: &ApplyTargets, root: &Path, out: &mut ImportOutcome) {
    let src = root.join("skills");
    if !src.is_dir() {
        return;
    }
    let mut found = Vec::new();
    hermes::collect_skill_md(&src, &mut found);
    for skill_md in found {
        let Some(skill_dir) = skill_md.parent() else {
            continue;
        };
        let name = skill_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("skill");
        let dest = t.user_skills_dir.join(name);
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

/// `SOUL.md` → one Nebo employee. The persona becomes the AGENT.md body; the
/// on-disk layout mirrors the user-created agent pathway (AGENT.md +
/// agent.json + manifest.json under `<user>/agents/<name>/`, napp_path set)
/// so the agent FS watcher treats it exactly like a hand-made employee.
fn apply_agent(t: &ApplyTargets, root: &Path, out: &mut ImportOutcome) -> Result<(), NeboError> {
    let soul = match fs::read_to_string(root.join("SOUL.md")) {
        Ok(s) => s,
        Err(_) => {
            out.skipped
                .push("agent: no SOUL.md found, no employee created".into());
            return Ok(());
        }
    };
    let name = "Hermes";
    let agent_dir = t.user_agents_dir.join(name);
    if agent_dir.exists() {
        out.skipped
            .push(format!("agent {name}: already exists, persona untouched"));
        return Ok(());
    }

    let description = "Imported from a Hermes install";
    let agent_md = format!("---\nname: {name}\ndescription: {description}\n---\n\n{soul}");
    let frontmatter = serde_json::json!({ "workflows": {}, "skills": [] }).to_string();

    let id = uuid::Uuid::new_v4().to_string();
    t.store
        .create_agent(&id, None, name, description, &agent_md, &frontmatter, None, None)?;

    fs::create_dir_all(&agent_dir)?;
    fs::write(agent_dir.join("AGENT.md"), &agent_md)?;
    fs::write(agent_dir.join("agent.json"), &frontmatter)?;
    let manifest = serde_json::json!({
        "id": id,
        "name": name,
        "version": "1.0.0",
        "type": "agent",
        "description": description,
    });
    fs::write(
        agent_dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).unwrap_or_default(),
    )?;
    t.store
        .set_agent_napp_path(&id, &agent_dir.to_string_lossy())?;

    out.agents += 1;
    out.agent_id = Some(id);
    out.agent_name = Some(name.to_string());
    Ok(())
}

/// Known LLM provider keys from `.env` → `auth_profiles`, matching the
/// existing provider-settings pathway (which stores `api_key` as given —
/// readers pass it straight to providers, so an encrypted value would break
/// auth). Providers that already have a profile are skipped. Unknown keys are
/// reported, not guessed at.
fn apply_provider_keys(
    t: &ApplyTargets,
    root: &Path,
    out: &mut ImportOutcome,
) -> Result<(), NeboError> {
    let Ok(text) = fs::read_to_string(root.join(".env")) else {
        return Ok(());
    };
    let existing: std::collections::HashSet<String> = t
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
                    &format!("Imported from Hermes ({provider})"),
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

/// Record what this slice deliberately defers, so the receipt never implies a
/// clean copy of things that didn't move.
fn note_deferred(root: &Path, out: &mut ImportOutcome) {
    if root.join("memories").is_dir() {
        out.skipped
            .push("memory: imported by the memory slice (parse + re-embed)".into());
    }
    if root.join("state.db").is_file() {
        out.skipped
            .push("conversation history: imported by the history slice".into());
    }
    if root.join("cron").join("jobs.json").is_file() {
        out.skipped
            .push("cron jobs: imported by the scheduling slice".into());
    }
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

/// Full apply against the live server: run the core, then the canonical
/// post-install steps — skill loader reload, agent finalization (same routine
/// as a marketplace hire), and the MCP bridge connect pass.
pub async fn apply(state: &crate::state::AppState, root: &Path) -> Result<ImportOutcome, NeboError> {
    let user = config::user_dir()?;
    let targets = ApplyTargets {
        store: &state.store,
        user_agents_dir: user.join("agents"),
        user_skills_dir: user.join("skills"),
    };
    let outcome = apply_hermes(&targets, root)?;

    if outcome.skills > 0 {
        state.skill_loader.reload_from_disk().await;
    }
    if let (Some(id), Some(name)) = (&outcome.agent_id, &outcome.agent_name) {
        crate::codes::finalize_agent_install(state, id, name).await;
    }
    if outcome.mcp_servers > 0 {
        crate::handlers::integrations::sync_bridge(state).await;
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

    fn setup() -> Fixture {
        let source = hermes::hermes_fixture();
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

        // Employee created: DB row + on-disk trio + napp_path.
        let id = out.agent_id.as_deref().unwrap();
        let agent = f.store.get_agent(id).unwrap().unwrap();
        assert_eq!(agent.name, "Hermes");
        assert!(agent.agent_md.contains("You are Atlas."));
        let dir = f.agents_dir.join("Hermes");
        assert!(dir.join("AGENT.md").is_file());
        assert!(dir.join("agent.json").is_file());
        assert!(dir.join("manifest.json").is_file());
        assert_eq!(agent.napp_path.as_deref(), Some(&*dir.to_string_lossy()));

        // Provider key imported to the matching profile; channel token deferred.
        let profiles = f.store.list_auth_profiles().unwrap();
        let anthropic = profiles.iter().find(|p| p.provider == "anthropic").unwrap();
        assert_eq!(anthropic.api_key, "sk-secretxxx");
        assert!(out.skipped.iter().any(|s| s.starts_with("TELEGRAM_TOKEN")));

        // Deferred slices are named, not silently dropped.
        assert!(out.skipped.iter().any(|s| s.starts_with("memory:")));
        assert!(out.skipped.iter().any(|s| s.starts_with("conversation history:")));
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
