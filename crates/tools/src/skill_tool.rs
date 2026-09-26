//! The skill tools: `use_skill` (core) loads a skill's instructions into the
//! conversation, and the deferred family finds, reads, saves, deletes,
//! installs, configures and reviews skills. They share one [`SkillCore`]
//! over the skill loader. The installed skills reach the model as the skill
//! listing (name + one line each), never through a tool.

use std::sync::Arc;

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

use crate::skills::{Loader, SkillSource};

/// Where the installed skill names are, after a "no such skill" line.
const LISTING_HINT: &str = "Installed skills are in the skill listing; find_skills searches them by what they do.";

/// Most of a skill's files named when it loads.
const LOADED_FILES_NAMED: usize = 20;

/// What every skill tool shares: the loader and the channels its writes
/// report through.
pub struct SkillCore {
    loader: Arc<Loader>,
    store: Option<Arc<db::Store>>,
    /// Live-broadcast callback (wired to ClientHub via the registry cell, same
    /// pattern as MessageTool). Used so a staged learned-skill write surfaces
    /// in the Inbox immediately, not on next load.
    notify_fn: Arc<std::sync::RwLock<Option<crate::message_tool::NotifyFn>>>,
    /// Optional reference to the live plugin registry. When set, a skill
    /// search that finds nothing can say when the query named a plugin
    /// rather than a skill. Runtime-driven — no hardcoded slugs.
    plugin_store: Option<Arc<napp::plugin::PluginStore>>,
    /// Shared canonical-installer cell (server-injected). `install_skill` delegates here so it
    /// goes through the ONE `codes::handle_code` pathway — never a direct API bypass.
    code_installer: Arc<std::sync::RwLock<Option<Arc<dyn crate::bot_tool::CodeInstaller>>>>,
}

impl SkillCore {
    pub fn new(loader: Arc<Loader>) -> Self {
        Self {
            loader,
            store: None,
            notify_fn: Arc::new(std::sync::RwLock::new(None)),
            plugin_store: None,
            code_installer: Arc::new(std::sync::RwLock::new(None)),
        }
    }

    pub fn with_store(mut self, store: Arc<db::Store>) -> Self {
        self.store = Some(store);
        self
    }

    /// Inject the shared broadcast cell (from the `Registry`).
    pub fn with_notify_fn(
        mut self,
        cell: Arc<std::sync::RwLock<Option<crate::message_tool::NotifyFn>>>,
    ) -> Self {
        self.notify_fn = cell;
        self
    }

    /// Inject the shared canonical-installer cell (from the `Registry`).
    pub fn with_code_installer(
        mut self,
        installer: Arc<std::sync::RwLock<Option<Arc<dyn crate::bot_tool::CodeInstaller>>>>,
    ) -> Self {
        self.code_installer = installer;
        self
    }

    pub fn with_plugin_store(mut self, plugin_store: Arc<napp::plugin::PluginStore>) -> Self {
        self.plugin_store = Some(plugin_store);
        self
    }

    /// Stage a learned-skill write for owner approval: pending_writes row +
    /// Inbox notification (`learn:<id>`) + live broadcast. The ONE staging
    /// pathway for every learned write when learning_mode = "staged".
    #[allow(clippy::too_many_arguments)]
    fn stage_learned_write(
        &self,
        agent_id: &str,
        action: &str,
        target: &str,
        content: Option<&str>,
        gist: &str,
        target_hash: &str,
        prior_content: Option<&str>,
    ) -> ToolResult {
        let Some(store) = self.store.as_ref() else {
            return ToolResult::error("Staging unavailable: the database is not initialized. Tell the user to restart Nebo.");
        };
        let pending_id = uuid::Uuid::new_v4().to_string();
        if let Err(e) = store.create_pending_write(
            &pending_id, agent_id, "skill", action, target, content, gist, target_hash,
            prior_content,
        ) {
            return ToolResult::error(format!("Failed to stage write: {}. Do not retry — this is a database error.", e));
        }
        let agent_name = store
            .get_agent(agent_id)
            .ok()
            .flatten()
            .map(|a| a.name)
            .unwrap_or_else(|| "An employee".to_string());
        let notif_id = format!("learn:{}", pending_id);
        let title = format!("{} wants to learn something new", agent_name);
        let notify = self.notify_fn.read().ok().and_then(|g| g.clone());
        let n = crate::owner_notify::OwnerNotification {
            id: &notif_id,
            kind: "approval",
            title: &title,
            body: Some(gist),
            action_url: Some("/inbox"),
            agent_id: Some(agent_id),
            loud: false,
        };
        match &notify {
            Some(f) => crate::owner_notify::emit(store, Some(&|ev, payload| f(ev, payload)), &n),
            None => crate::owner_notify::emit(store, None, &n),
        }
        ToolResult::ok(format!(
            "Staged for the owner's approval: {}. NOTHING has been saved yet — the owner reviews this from their Inbox. Report it as 'staged for approval', never as 'saved'.",
            gist
        ))
    }

    /// Record an auto-mode learned write that was ALREADY applied to disk, as an
    /// approved audit row (+ an `info` Inbox notification). This is the revert
    /// anchor for `learning_mode = "auto"`: without it, auto-applied learned
    /// skills leave no trace and cannot be undone. The staged path already
    /// produces a row on approval; this is its auto-mode counterpart. Best
    /// effort — a failure here must not fail the write that already succeeded.
    #[allow(clippy::too_many_arguments)]
    fn record_applied_learning(
        &self,
        agent_id: &str,
        action: &str,
        target: &str,
        content: Option<&str>,
        gist: &str,
        target_hash: &str,
        prior_content: Option<&str>,
    ) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let pending_id = uuid::Uuid::new_v4().to_string();
        if let Err(e) = store.record_applied_write(
            &pending_id, agent_id, "skill", action, target, content, gist, target_hash,
            prior_content,
        ) {
            tracing::warn!(error = %e, "auto learned write: could not record audit row");
            return;
        }
        let user_id = store.ensure_local_user_id().unwrap_or_default();
        let agent_name = store
            .get_agent(agent_id)
            .ok()
            .flatten()
            .map(|a| a.name)
            .unwrap_or_else(|| "An employee".to_string());
        let notif_id = format!("learn:{}", pending_id);
        let title = format!("{} refined a skill", agent_name);
        if let Err(e) = store.create_notification_if_not_exists(
            &notif_id,
            &user_id,
            "info",
            &title,
            Some(gist),
            Some("/inbox"),
            None,
            Some(agent_id),
        ) {
            tracing::warn!(error = %e, "auto learned write: could not persist notification");
        }
        let notify = self.notify_fn.read().ok().and_then(|g| g.clone());
        if let Some(notify) = notify {
            notify(
                "notification_created",
                serde_json::json!({
                    "id": notif_id,
                    "type": "info",
                    "title": title,
                    "body": gist,
                    "actionUrl": "/inbox",
                    "agentId": agent_id,
                    "readAt": null,
                }),
            );
        }
    }

    /// Find an installed plugin whose slug matches `term` (case-insensitive
    /// exact or substring). Returns the canonical slug if matched.
    fn match_plugin_slug(&self, term: &str) -> Option<String> {
        let store = self.plugin_store.as_ref()?;
        let needle = term.trim().to_lowercase();
        if needle.is_empty() {
            return None;
        }
        let mut exact: Option<String> = None;
        let mut substring: Option<String> = None;
        for (slug, _ver, _path, _src) in store.list_installed() {
            let lower = slug.to_lowercase();
            if lower == needle {
                exact = Some(slug);
                break;
            }
            if substring.is_none() && (lower.contains(&needle) || needle.contains(&lower)) {
                substring = Some(slug);
            }
        }
        exact.or(substring)
    }

    /// The ONE "no such skill" line, then what to do next: `next` for a
    /// load miss (`load_miss`), else [`LISTING_HINT`].
    fn not_found(name: &str, next: &str) -> String {
        format!("No skill named '{name}'. {next}")
    }

    /// What a load of a skill that isn't installed hears: the installed
    /// skills that do what the name says, when there are any, and in every
    /// case that a missing skill is no missing capability. Gate 2026-09-26
    /// (correction-skill-miss-proceeds): the old line sent every run
    /// searching (find_tools, find_skills twice, an invented "install skill"
    /// name) before it read the file and did the sum.
    async fn load_miss(&self, scope: Scope<'_>, name: &str) -> String {
        // Close means a name that shares a word with the one asked for; a
        // description that happens to say "summary" is not close to
        // "quarterly-summary".
        let words: Vec<String> = name
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() >= 3)
            .map(str::to_string)
            .collect();
        let close: Vec<String> = self
            .loader
            .discover_summaries(name, scope.agent)
            .await
            .into_iter()
            .filter(|s| {
                let own = s.name.to_lowercase();
                words.iter().any(|w| own.split(|c: char| !c.is_alphanumeric()).any(|o| o == w))
            })
            .take(5)
            .map(|s| format!("{} ({})", s.name, s.description))
            .collect();
        let next = if close.is_empty() {
            "No installed skill has a name like it. The skill listing names every installed skill; if none \
             there fits, there is no skill for this task: do it with your other tools rather than searching again."
                .to_string()
        } else {
            format!(
                "Installed skills close to it: {}. Load one of those if it fits; otherwise do the task with your \
                 other tools.",
                close.join("; ")
            )
        };
        Self::not_found(name, &next)
    }

    fn user_skills_dir() -> Result<std::path::PathBuf, String> {
        config::user_dir()
            .map(|d| d.join("skills"))
            .map_err(|e| format!("data dir error: {}", e))
    }

    /// Validate a tool-input skill name: rejects path separators, `..`, or `.`
    /// so input like `../../foo` can never escape a skills directory. The ONE
    /// name check for both the user and learned write pathways.
    fn validate_skill_name(name: &str) -> Result<(), String> {
        if name.is_empty()
            || name == "."
            || name.contains("..")
            || name.contains('/')
            || name.contains('\\')
        {
            return Err(format!(
                "Invalid skill name '{}': names cannot be '.' and cannot contain '..', '/' or '\\'",
                name
            ));
        }
        Ok(())
    }

    /// Resolve a skill name to its directory under the user skills dir
    /// (save/read/enable/delete all go through here).
    fn user_skill_dir(name: &str) -> Result<std::path::PathBuf, String> {
        Self::validate_skill_name(name)?;
        let dir = Self::user_skills_dir()?;
        let skill_dir = dir.join(name);
        // Belt and braces: the joined path must still be inside the skills dir.
        if !skill_dir.starts_with(&dir) {
            return Err(format!("Invalid skill name '{}'", name));
        }
        Ok(skill_dir)
    }

    /// Resolve a skill name to its directory under the learned tree for
    /// `owner` — the review fork's write target. Same name validation as
    /// user_skill_dir; the base comes from the loader's learned root.
    fn learned_skill_dir(&self, owner: &str, name: &str) -> Result<std::path::PathBuf, String> {
        Self::validate_skill_name(name)?;
        let base = self
            .loader
            .learned_dir()
            .ok_or_else(|| "learned skills are not enabled on this install".to_string())?;
        let skill_dir = base.join(owner).join(name);
        if !skill_dir.starts_with(base) {
            return Err(format!("Invalid skill name '{}'", name));
        }
        Ok(skill_dir)
    }

    /// Load a skill's instructions: its expanded body (the review fork gets
    /// the full SKILL.md of its own learned skills, which it rewrites whole).
    /// A skill switched off on disk is switched on and loaded in the same
    /// call.
    async fn load(&self, ctx: &ToolContext, scope: Scope<'_>, name: &str, args: Option<&str>) -> ToolResult {
        if !self.loader.get(name, scope.agent).await.is_some_and(|s| s.enabled) {
            // Switched off on disk: switch it back on, then load it.
            let skill_dir = match Self::user_skill_dir(name) {
                Ok(d) => d,
                Err(e) => return ToolResult::error(e),
            };
            if !skill_dir.join("SKILL.md.disabled").exists() {
                return ToolResult::error(self.load_miss(scope, name).await);
            }
            if let Err(e) = std::fs::rename(skill_dir.join("SKILL.md.disabled"), skill_dir.join("SKILL.md")) {
                return ToolResult::error(format!("Failed to enable skill: {}. Do not retry — this is a filesystem error.", e));
            }
            // Make it live now instead of waiting for the watcher.
            self.loader.reload_from_disk().await;
        }
        let Some(skill) = self.loader.get(name, scope.agent).await.filter(|s| s.enabled) else {
            return ToolResult::error(self.load_miss(scope, name).await);
        };
        // Read mark for the review fork: a save or delete of a learned skill
        // requires it was loaded THIS run.
        if let Ok(mut read) = ctx.skills_read.lock() {
            read.insert(skill.name.clone());
        }
        // The review fork rewrites whole files — hand it the FULL source
        // (frontmatter + body) so a save preserves triggers/priority/version
        // instead of reconstructing frontmatter blind.
        if scope.learned_owner.is_some()
            && matches!(skill.source, SkillSource::Learned)
            && let Some(raw) = skill.source_path.as_deref().and_then(|p| std::fs::read_to_string(p).ok())
        {
            return ToolResult::ok(format!(
                "Loaded skill '{}'. CURRENT FULL SKILL.md (rewrite the whole file with save_skill, keeping frontmatter fields you don't mean to change):\n\n{}",
                skill.name, raw
            ));
        }
        let body = with_args(&self.loader.expand_template(&skill, self.store.as_deref()), args);
        let base = skill
            .base_dir
            .as_deref()
            .map(|d| format!("This skill's files are in: {}\n\n", d.display()))
            .unwrap_or_default();
        ToolResult::ok(format!(
            "Loaded skill '{}'. Follow its instructions:\n\n{base}{body}{}",
            skill.name,
            files_line(&skill.name, skill.list_resources().unwrap_or_default())
        ))
    }

    async fn find(&self, scope: Scope<'_>, query: &str) -> ToolResult {
        let matches = self.loader.discover_summaries(query, scope.agent).await;
        if !matches.is_empty() {
            let lines: Vec<String> =
                matches.iter().take(10).map(|s| format!("- {}: {}", s.name, s.description)).collect();
            return ToolResult::ok(format!(
                "Skills matching \"{}\":\n{}\n\nLoad one with use_skill, then follow its instructions.",
                query,
                lines.join("\n")
            ));
        }
        // If the query matches a registered plugin slug (channel plugins
        // like slack/discord, or any other installed plugin), the model
        // probably meant the plugin. Say so rather than a dead "no match."
        if let Some(slug) = self.match_plugin_slug(query) {
            // "slack" IS the plugin; "send the weekly report to slack"
            // merely contains its slug. Only the first shape gets the flat
            // "is a plugin" verdict.
            let q = query.trim();
            if q.eq_ignore_ascii_case(&slug) || !q.contains(char::is_whitespace) {
                return ToolResult::ok(format!(
                    "`{}` is a plugin, not a skill. Skills are local capability bundles; plugins are managed binaries. \
                     USE its tool {}: command \"help\" lists its commands. \
                     For channel messaging (upload/post/dm/reply), the bridge fills channel and thread from context; you only need the operation and its arguments.",
                    slug, crate::plugin_tools::plugin_tool_name(&slug)
                ));
            }
            return ToolResult::ok(format!(
                "No installed skill matches \"{}\". The installed plugin `{}` matches part of that query; \
                 if that is what you need, its tool {} lists its commands with command \"help\". \
                 Otherwise proceed with your other tools.",
                query, slug, crate::plugin_tools::plugin_tool_name(&slug)
            ));
        }
        // A keyword miss over installed skills is not a verdict on the
        // capability: the other tools handle most jobs with no skill at all.
        ToolResult::ok(format!(
            "No installed skill or plugin matches \"{}\". That only means no skill is installed for it; proceed with your other tools. If a marketplace skill would help, tell the user.",
            query
        ))
    }

    /// A skill's files: the list with no path, a file's text with one.
    async fn read_file(&self, scope: Scope<'_>, name: &str, path: &str) -> ToolResult {
        let Some(skill) = self.loader.get(name, scope.agent).await else {
            return ToolResult::error(Self::not_found(name, LISTING_HINT));
        };
        let mut resources = match skill.list_resources() {
            Ok(r) => r,
            Err(e) => return ToolResult::error(format!("Failed to list resources: {}. Do not retry — this is a filesystem error.", e)),
        };
        // A path that names a file reads it; one that names a folder lists
        // what is under it.
        if !path.is_empty() && resources.iter().any(|r| r == path) {
            return match skill.read_resource(path) {
                Ok(data) => match String::from_utf8(data.clone()) {
                    Ok(text) => ToolResult::ok(text),
                    Err(_) => ToolResult::ok(format!("binary file, {} bytes", data.len())),
                },
                Err(e) => ToolResult::error(e),
            };
        }
        if !path.is_empty() {
            let prefix = if path.ends_with('/') { path.to_string() } else { format!("{}/", path) };
            resources.retain(|r| r.starts_with(&prefix));
        }
        if resources.is_empty() {
            return if path.is_empty() {
                ToolResult::ok(format!("Skill '{}' has no files besides its instructions.", name))
            } else {
                ToolResult::ok(format!(
                    "No files under '{}/{}'. Leave out path to list the skill's files; pass a listed path to read one.",
                    name, path
                ))
            };
        }
        resources.sort();
        let listing: Vec<String> = resources
            .iter()
            .map(|r| {
                let size = skill
                    .base_dir
                    .as_ref()
                    .and_then(|base| std::fs::metadata(base.join(r)).ok())
                    .map(|m| format!(" ({} bytes)", m.len()))
                    .unwrap_or_default();
                format!("  {}{}", r, size)
            })
            .collect();
        ToolResult::ok(format!("Files in '{}':\n{}", name, listing.join("\n")))
    }

    /// Create the skill, or replace it when it exists.
    async fn save(&self, ctx: &ToolContext, scope: Scope<'_>, name: &str, content: &str) -> ToolResult {
        match self.loader.get(name, scope.agent).await {
            // The review fork saves learned skills only: a same-named skill
            // of another source is never its to replace.
            Some(existing) if scope.learned_owner.is_some() && !matches!(existing.source, SkillSource::Learned) => {
                ToolResult::error(format!(
                    "Skill '{}' already exists (source: {}). Save the lesson under another name.",
                    name,
                    source_words(&existing.source)
                ))
            }
            Some(skill) => self.update(ctx, scope, skill, content).await,
            None if scope.learned_owner.is_none()
                && Self::user_skill_dir(name).is_ok_and(|d| d.join("SKILL.md").exists()) =>
            {
                match Self::user_skill_dir(name) {
                    Ok(dir) => match std::fs::write(dir.join("SKILL.md"), content) {
                        Ok(_) => ToolResult::ok(format!("Updated skill '{}'", name)),
                        Err(e) => ToolResult::error(format!("Failed to update: {}. Do not retry — this is a filesystem error.", e)),
                    },
                    Err(e) => ToolResult::error(e),
                }
            }
            None => self.create(ctx, scope, name, content).await,
        }
    }

    async fn create(&self, ctx: &ToolContext, scope: Scope<'_>, name: &str, content_raw: &str) -> ToolResult {
        // LLMs often send literal \n instead of real newlines in tool call strings.
        let content = content_raw.replace("\\n", "\n");

        let skill_dir = match scope.learned_owner {
            // Review fork: create in the learned tree, never user/skills/.
            Some(owner) => match self.learned_skill_dir(owner, name) {
                Ok(d) => d,
                Err(e) => return ToolResult::error(e),
            },
            None => match Self::user_skill_dir(name) {
                Ok(d) => d,
                Err(e) => return ToolResult::error(e),
            },
        };

        // Always write as {name}/SKILL.md per Agent Skills spec
        let final_content = if content.trim_start().starts_with("---") {
            content.clone()
        } else {
            format!("---\nname: {}\ndescription: {}\n---\n{}", name, name, content)
        };

        // Staged learning: the create becomes a pending write for the owner
        // to approve from the Inbox.
        if ctx.learned_write_staged
            && let Some(owner) = scope.learned_owner
        {
            let gist = format!("Create learned skill '{}'", name);
            return self.stage_learned_write(owner, "create", name, Some(&final_content), &gist, "", None);
        }

        let path = skill_dir.join("SKILL.md");
        // Never overwrite silently: a file the loader could not read is
        // still someone's skill.
        if path.exists() {
            return ToolResult::error(format!(
                "Skill '{}' already exists at {} but could not be loaded. Pick another name, or fix that file.",
                name,
                path.display()
            ));
        }
        if let Err(e) = std::fs::create_dir_all(&skill_dir) {
            return ToolResult::error(format!("Failed to create skill dir: {}. Do not retry — this is a filesystem error.", e));
        }
        match std::fs::write(&path, &final_content) {
            Ok(_) => {
                // Make the skill (and its triggers) live NOW — the fs
                // watcher is not instant and first-call trigger tests race
                // it. Same pattern as the install paths.
                self.loader.reload_from_disk().await;
                // Auto-mode learned create: record the revert anchor
                // (prior_content None — nothing existed before). Skipped on a
                // re-apply (approve/revert already have a row).
                if let Some(owner) = scope.learned_owner.filter(|_| !ctx.learned_write_reapply) {
                    let gist = format!("Create learned skill '{}'", name);
                    self.record_applied_learning(owner, "create", name, Some(&final_content), &gist, "", None);
                }
                ToolResult::ok(format!("Created skill '{}' at {}", name, path.display()))
            }
            Err(e) => ToolResult::error(format!("Failed to write skill: {}. Do not retry — this is a filesystem error.", e)),
        }
    }

    async fn update(&self, ctx: &ToolContext, scope: Scope<'_>, skill: crate::skills::Skill, content: &str) -> ToolResult {
        let name = skill.name.clone();
        // Protect marketplace (installed) skills from modification
        if matches!(skill.source, SkillSource::Installed) {
            return ToolResult::error(format!(
                "Cannot change marketplace skill '{}'. It was installed from NeboAI and is read-only.",
                name
            ));
        }
        if matches!(skill.source, SkillSource::Learned) {
            // Only the review fork may rewrite learned skills, only its own,
            // and only after loading them THIS run (read-before-write:
            // rewrite from actual content, never a transcript-inferred
            // recollection).
            let Some(owner) = scope.learned_owner else {
                return ToolResult::error(format!(
                    "Cannot change learned skill '{}'. It is managed by the self-improvement loop; review changes from the Inbox.",
                    name
                ));
            };
            if skill.owner_agent_id.as_deref() != Some(owner) {
                return ToolResult::error(format!(
                    "Cannot change learned skill '{}': it belongs to a different employee.",
                    name
                ));
            }
            let read = ctx.skills_read.lock().map(|r| r.contains(&skill.name)).unwrap_or(false);
            if !read {
                return ToolResult::error(format!(
                    "Read-before-write: load skill '{}' first with use_skill and rewrite from its returned content, then save again.",
                    name
                ));
            }
            // Staged learning: normalize + validate now (so approve can't
            // fail parsing), then park the write.
            if ctx.learned_write_staged {
                let final_content = with_frontmatter(&skill, content);
                if let Err(e) = crate::skills::parse_skill_frontmatter(final_content.as_bytes()) {
                    return ToolResult::error(format!(
                        "Save rejected: content would not parse as a valid skill ({}). Send the FULL SKILL.md including the --- frontmatter block.",
                        e
                    ));
                }
                let hash = skill.source_path.as_deref().map(crate::skills::hash_skill_file).unwrap_or_default();
                let prior = skill.source_path.as_deref().and_then(|p| std::fs::read_to_string(p).ok());
                let gist = format!("Update learned skill '{}'", name);
                return self.stage_learned_write(owner, "update", &name, Some(&final_content), &gist, &hash, prior.as_deref());
            }
        }
        let Some(ref path) = skill.source_path else {
            return ToolResult::error(Self::not_found(&name, LISTING_HINT));
        };
        // Models routinely send the body without the YAML header; writing that
        // verbatim knocks the skill out of the loader on the next reload.
        // Re-wrap bare content with the skill's existing identity, then refuse
        // anything that still doesn't parse.
        let final_content = with_frontmatter(&skill, content);
        if let Err(e) = crate::skills::parse_skill_frontmatter(final_content.as_bytes()) {
            return ToolResult::error(format!(
                "Save rejected: content would not parse as a valid skill ({}). Send the FULL SKILL.md including the --- frontmatter block.",
                e
            ));
        }
        // Capture the restore point BEFORE overwriting, but only for an
        // auto-mode learned write (a user-skill edit is not a "learning" and
        // gets no revert anchor).
        let learned_write = matches!(skill.source, SkillSource::Learned).then_some(scope.learned_owner).flatten();
        let prior = learned_write.and_then(|_| std::fs::read_to_string(path).ok());
        let prior_hash = learned_write.map(|_| crate::skills::hash_skill_file(path)).unwrap_or_default();
        match std::fs::write(path, &final_content) {
            Ok(_) => {
                self.loader.reload_from_disk().await;
                if let Some(owner) = learned_write.filter(|_| !ctx.learned_write_reapply) {
                    let gist = format!("Update learned skill '{}'", name);
                    self.record_applied_learning(owner, "update", &name, Some(&final_content), &gist, &prior_hash, prior.as_deref());
                }
                ToolResult::ok(format!("Updated skill '{}'", name))
            }
            Err(e) => ToolResult::error(format!("Failed to update: {}. Do not retry — this is a filesystem error.", e)),
        }
    }

    async fn delete(&self, ctx: &ToolContext, scope: Scope<'_>, name: &str) -> ToolResult {
        // Protect marketplace (installed) skills from deletion
        if let Some(skill) = self.loader.get(name, scope.agent).await {
            if matches!(skill.source, SkillSource::Installed) {
                return ToolResult::error(format!(
                    "Cannot delete marketplace skill '{}'. It was installed from NeboAI and is read-only.",
                    name
                ));
            }
            if matches!(skill.source, SkillSource::Learned) {
                // Fork-only, own-skill-only, read-before-write — same rules
                // as a save. Deletes the learned dir.
                let Some(owner) = scope.learned_owner else {
                    return ToolResult::error(format!(
                        "Cannot delete learned skill '{}'. It is managed by the self-improvement loop; review changes from the Inbox.",
                        name
                    ));
                };
                if skill.owner_agent_id.as_deref() != Some(owner) {
                    return ToolResult::error(format!(
                        "Cannot delete learned skill '{}': it belongs to a different employee.",
                        name
                    ));
                }
                let read = ctx.skills_read.lock().map(|r| r.contains(&skill.name)).unwrap_or(false);
                if !read {
                    return ToolResult::error(format!(
                        "Read-before-write: load skill '{}' first with use_skill to confirm what you are deleting, then retry.",
                        name
                    ));
                }
                // The full SKILL.md is the delete's restore point — a revert
                // re-creates the skill from it.
                let prior = skill.source_path.as_deref().and_then(|p| std::fs::read_to_string(p).ok());
                let hash = skill.source_path.as_deref().map(crate::skills::hash_skill_file).unwrap_or_default();
                // Staged learning: park the delete for approval.
                if ctx.learned_write_staged {
                    let gist = format!("Delete learned skill '{}'", name);
                    return self.stage_learned_write(owner, "delete", name, None, &gist, &hash, prior.as_deref());
                }
                let dir = match self.learned_skill_dir(owner, name) {
                    Ok(d) => d,
                    Err(e) => return ToolResult::error(e),
                };
                if dir.is_dir()
                    && let Err(e) = std::fs::remove_dir_all(&dir)
                {
                    return ToolResult::error(format!(
                        "Failed to delete learned skill: {}. Do not retry — this is a filesystem error.",
                        e
                    ));
                }
                self.loader.reload_from_disk().await;
                // Auto-mode learned delete: record the revert anchor (skipped
                // on a re-apply — approve/revert own the row).
                if !ctx.learned_write_reapply {
                    let gist = format!("Delete learned skill '{}'", name);
                    self.record_applied_learning(owner, "delete", name, None, &gist, &hash, prior.as_deref());
                }
                return ToolResult::ok(format!("Deleted learned skill '{}'", name));
            }
        }

        let skill_dir = match Self::user_skill_dir(name) {
            Ok(d) => d,
            Err(e) => return ToolResult::error(e),
        };
        if !skill_dir.is_dir() {
            return ToolResult::error(format!("{} Nothing deleted.", Self::not_found(name, LISTING_HINT)));
        }
        if let Err(e) = std::fs::remove_dir_all(&skill_dir) {
            tracing::warn!(skill = %name, error = %e, "failed to remove skill directory");
            return ToolResult::error(format!("Failed to delete skill '{}': {}", name, e));
        }
        ToolResult::ok(format!("Deleted skill '{}'", name))
    }

    async fn install(&self, ctx: &ToolContext, code: &str) -> ToolResult {
        // Delegate to the ONE canonical install pathway (`codes::handle_code`):
        // redeem + persist + reload + cascade deps, identical to the WS code
        // flow. No direct API bypass.
        let installer = self.code_installer.read().unwrap().clone();
        match installer {
            Some(installer) => {
                let msg = installer.install(code, crate::InstalledBy::of(ctx)).await;
                // The installer trait returns one string for both outcomes; a
                // failure must not arrive as success.
                if install_failed(&msg) { ToolResult::error(msg) } else { ToolResult::ok(msg) }
            }
            None => ToolResult::error(
                "Installing from a code is not available in this context. Tell the user to install it from the Nebo app.",
            ),
        }
    }

    /// Set a secret (`key` + `value`), or show the declared secrets and
    /// which are set (neither).
    async fn configure(&self, scope: Scope<'_>, name: &str, key: &str, value: &str) -> ToolResult {
        let Some(store) = &self.store else {
            return ToolResult::error(
                "Skill secrets are not available — store not configured. The user needs to restart Nebo so the database initializes.",
            );
        };
        if key.is_empty() {
            return self.secrets(store, scope, name).await;
        }

        // Validate key name matches a declared secret in the skill
        if let Some(skill) = self.loader.get(name, scope.agent).await {
            let declarations = skill.secrets();
            if !declarations.is_empty() && !declarations.iter().any(|d| d.key == key) {
                let valid_keys: Vec<&str> = declarations.iter().map(|d| d.key.as_str()).collect();
                return ToolResult::error(format!(
                    "Unknown secret '{}' for skill '{}'. Declared secrets: {}",
                    key,
                    name,
                    valid_keys.join(", ")
                ));
            }
        }

        let encrypted = match auth::credential::encrypt(value) {
            Ok(v) => v,
            Err(e) => return ToolResult::error(format!("encryption failed: {}. Do not retry — this is a configuration error.", e)),
        };
        match store.set_skill_secret(name, key, &encrypted) {
            Ok(()) => ToolResult::ok(format!("Configured {} for skill '{}'. The value is stored encrypted.", key, name)),
            Err(e) => ToolResult::error(format!("failed to save secret: {}. Do not retry — this is a database error.", e)),
        }
    }

    async fn secrets(&self, store: &db::Store, scope: Scope<'_>, name: &str) -> ToolResult {
        let Some(skill) = self.loader.get(name, scope.agent).await else {
            return ToolResult::error(Self::not_found(name, LISTING_HINT));
        };
        let declarations = skill.secrets();
        if declarations.is_empty() {
            return ToolResult::ok(format!("Skill '{}' does not declare any secrets.", name));
        }
        let stored = store.list_skill_secrets(name).unwrap_or_default();
        let stored_keys: std::collections::HashSet<&str> = stored.iter().map(|(k, _)| k.as_str()).collect();
        let lines: Vec<String> = declarations
            .iter()
            .map(|d| {
                let status = if stored_keys.contains(d.key.as_str()) {
                    "configured"
                } else if d.required {
                    "MISSING (required)"
                } else {
                    "not set (optional)"
                };
                let label = if d.label.is_empty() { d.key.clone() } else { format!("{} ({})", d.label, d.key) };
                let hint = if d.hint.is_empty() { String::new() } else { format!("\n    {}", d.hint) };
                format!("- {} [{}]{}", label, status, hint)
            })
            .collect();
        ToolResult::ok(format!("Secrets for skill '{}':\n{}", name, lines.join("\n")))
    }

    /// The marketplace API, or the model-facing reason there is none.
    fn neboai_api(&self) -> Result<comm::api::NeboAIApi, String> {
        let Some(store) = &self.store else {
            return Err("Skill reviews are not available — store not configured. The user needs to restart Nebo so the database initializes.".to_string());
        };
        crate::build_neboai_api(store).map_err(|e| format!("NeboAI connection required: {}", e))
    }

    async fn reviews(&self, name: &str) -> ToolResult {
        let api = match self.neboai_api() {
            Ok(a) => a,
            Err(e) => return ToolResult::error(e),
        };
        match api.get_skill_reviews(name, None, None).await {
            Ok(resp) => {
                if resp.reviews.is_empty() {
                    return ToolResult::ok(format!("No reviews yet for skill '{}'.", name));
                }
                let lines: Vec<String> = resp
                    .reviews
                    .iter()
                    .map(|r| {
                        let who = if r.reviewer_type == "bot" {
                            // Bot slugs are already stored prefixed with `@`
                            // (e.g. `@bot_xyz`). `/` is reserved for slash
                            // commands — never use it for identities.
                            format!("🤖 {}", if r.reviewer_name.is_empty() { r.reviewer_slug.clone() } else { r.reviewer_name.clone() })
                        } else if !r.reviewer_name.is_empty() {
                            r.reviewer_name.clone()
                        } else {
                            "Anonymous".to_string()
                        };
                        let stars = "★".repeat(r.rating as usize);
                        format!("- {} {} — {}", who, stars, r.body)
                    })
                    .collect();
                ToolResult::ok(format!("Reviews for skill '{}':\n{}", name, lines.join("\n")))
            }
            Err(e) => ToolResult::error(format!("failed to fetch reviews: {}. Tell the user; do not retry in this turn.", e)),
        }
    }

    async fn rate(&self, name: &str, rating: i64, review: &str) -> ToolResult {
        let api = match self.neboai_api() {
            Ok(a) => a,
            Err(e) => return ToolResult::error(e),
        };
        let body = serde_json::json!({ "rating": rating, "review": review });
        match api.submit_skill_review(name, &body).await {
            Ok(_) => ToolResult::ok(format!("Posted {}★ review on skill '{}'.", rating, name)),
            Err(e) => ToolResult::error(format!("failed to post review: {}. Tell the user; do not retry in this turn.", e)),
        }
    }
}

/// Whose skills a call sees and writes.
#[derive(Clone, Copy)]
struct Scope<'a> {
    /// The seat whose own skills (package and learned) the call sees beside
    /// the shared ones.
    agent: Option<&'a str>,
    /// Set on the review fork: saves and deletes target this seat's learned
    /// tree, with read-before-write.
    learned_owner: Option<&'a str>,
}

/// `content` as a whole SKILL.md: bare instructions get the skill's existing
/// name and description as frontmatter.
fn with_frontmatter(skill: &crate::skills::Skill, content: &str) -> String {
    let normalized = content.replace("\\n", "\n");
    if normalized.trim_start().starts_with("---") {
        normalized
    } else {
        format!("---\nname: {}\ndescription: {}\n---\n{}", skill.name, skill.description, normalized)
    }
}

/// The skill's instructions with the call's arguments: in place of
/// `$ARGUMENTS` where the skill has it, otherwise after the instructions.
fn with_args(body: &str, args: Option<&str>) -> String {
    match args.map(str::trim).filter(|a| !a.is_empty()) {
        None => body.to_string(),
        Some(args) if body.contains("$ARGUMENTS") => body.replace("$ARGUMENTS", args),
        Some(args) => format!("{body}\n\nARGUMENTS: {args}"),
    }
}

/// Where a skill came from, in words the model can repeat to the user.
fn source_words(source: &SkillSource) -> &'static str {
    match source {
        SkillSource::Installed => "installed from the marketplace",
        SkillSource::User => "a user-created skill",
        SkillSource::Learned => "a learned skill of this employee",
    }
}

/// The canonical installer returns one string for success and failure; these
/// are the failure shapes it produces (`codes::handle_code_text` and the
/// code-format check in the server's `CodeInstallerImpl`).
fn install_failed(msg: &str) -> bool {
    msg.starts_with("Failed to install") || msg.contains("is not a valid install code")
}

fn str_field<'a>(input: &'a serde_json::Value, key: &str) -> &'a str {
    input.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

/// The skill tools, one purpose each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    UseSkill,
    FindSkills,
    ReadSkillFile,
    SaveSkill,
    DeleteSkill,
    InstallSkill,
    ConfigureSkill,
    RateSkill,
    ReadSkillReviews,
}

const KINDS: &[Kind] = &[
    Kind::UseSkill,
    Kind::FindSkills,
    Kind::ReadSkillFile,
    Kind::SaveSkill,
    Kind::DeleteSkill,
    Kind::InstallSkill,
    Kind::ConfigureSkill,
    Kind::RateSkill,
    Kind::ReadSkillReviews,
];

/// The name of the always-loaded skill tool.
pub const USE_SKILL: &str = "use_skill";

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::UseSkill => USE_SKILL,
            Kind::FindSkills => "find_skills",
            Kind::ReadSkillFile => "read_skill_file",
            Kind::SaveSkill => "save_skill",
            Kind::DeleteSkill => "delete_skill",
            Kind::InstallSkill => "install_skill",
            Kind::ConfigureSkill => "configure_skill",
            Kind::RateSkill => "rate_skill",
            Kind::ReadSkillReviews => "read_skill_reviews",
        }
    }

    fn search_hint(self) -> &'static str {
        match self {
            Kind::UseSkill => "load a skill's instructions to follow",
            Kind::FindSkills => "search installed skills by what they do",
            Kind::ReadSkillFile => "read or list a skill's files",
            Kind::SaveSkill => "create or rewrite a skill",
            Kind::DeleteSkill => "delete a skill you made",
            Kind::InstallSkill => "install a marketplace skill from a code",
            Kind::ConfigureSkill => "set a skill's API key secret",
            Kind::RateSkill => "rate and review a marketplace skill",
            Kind::ReadSkillReviews => "read a marketplace skill's reviews",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Kind::UseSkill => "Loads a skill: packaged instructions for a kind of work. Available skills are listed in reminders, one line each.\n\
                - When the task matches a listed skill, load it first and follow its instructions.\n\
                - Load only the skills the task needs, several in one response. To learn many, delegate.\n\
                - Only listed names are valid; find_skills searches them by what they do.\n\
                - A skill loaded earlier in this conversation is already here: follow it instead of loading it again.",
            Kind::FindSkills => "Searches the installed skills by what they do and returns matching names with one line each.\n\
                - The skill listing in reminders already names every skill; search when its lines don't settle which one fits.\n\
                - No match only means no skill is installed for it: do the work with your other tools.",
            Kind::ReadSkillFile => "Reads a file a skill ships beside its instructions: a script, template or reference.\n\
                - Leave out `path` to list the skill's files.\n\
                - `path` is relative to the skill's folder, as the list shows it.",
            Kind::SaveSkill => "Creates a skill, or replaces one you made, from a whole SKILL.md.\n\
                - `content` is the full file: a --- frontmatter block with name and description, then the instructions.\n\
                - To change an existing skill, load it with use_skill first and rewrite from what it returned.\n\
                - Marketplace skills are read-only.",
            Kind::DeleteSkill => "Deletes a skill you made. Marketplace skills are read-only.",
            Kind::InstallSkill => "Installs a skill from the NeboAI marketplace with its install code (SKIL-XXXX-XXXX).",
            Kind::ConfigureSkill => "Sets a secret a skill needs, such as an API key; the value is stored encrypted.\n\
                - With `key` and `value`: saves that secret.\n\
                - With only `name`: lists the secrets the skill declares and which are set.",
            Kind::RateSkill => "Leaves a 1–5 star review on a marketplace skill: what worked and what didn't.",
            Kind::ReadSkillReviews => "Reads the reviews of a marketplace skill.",
        }
    }

    fn schema(self) -> serde_json::Value {
        let name = |desc: &str| serde_json::json!({ "type": "string", "description": desc });
        match self {
            Kind::UseSkill => serde_json::json!({
                "type": "object",
                "properties": {
                    "name": name("Exact name from the skill listing."),
                    "args": { "type": "string", "description": "Arguments for the skill, when it takes any." }
                },
                "required": ["name"]
            }),
            Kind::FindSkills => serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What you're trying to do, in a few words." }
                },
                "required": ["query"]
            }),
            Kind::ReadSkillFile => serde_json::json!({
                "type": "object",
                "properties": {
                    "name": name("The skill's name."),
                    "path": { "type": "string", "description": "A file or folder inside the skill, e.g. scripts/recalc.py." }
                },
                "required": ["name"]
            }),
            Kind::SaveSkill => serde_json::json!({
                "type": "object",
                "properties": {
                    "name": name("The skill's name: lowercase words joined by hyphens."),
                    "content": { "type": "string", "description": "The whole SKILL.md: frontmatter, then instructions." }
                },
                "required": ["name", "content"]
            }),
            Kind::DeleteSkill => serde_json::json!({
                "type": "object",
                "properties": { "name": name("The skill's name.") },
                "required": ["name"]
            }),
            Kind::InstallSkill => serde_json::json!({
                "type": "object",
                "properties": {
                    "code": { "type": "string", "description": "The marketplace install code, SKIL-XXXX-XXXX." }
                },
                "required": ["code"]
            }),
            Kind::ConfigureSkill => serde_json::json!({
                "type": "object",
                "properties": {
                    "name": name("The skill's name."),
                    "key": { "type": "string", "description": "The secret's name the skill declares, e.g. BRAVE_API_KEY." },
                    "value": { "type": "string", "description": "The secret's value." }
                },
                "required": ["name"]
            }),
            Kind::RateSkill => serde_json::json!({
                "type": "object",
                "properties": {
                    "name": name("The skill's name."),
                    "rating": { "type": "integer", "minimum": 1, "maximum": 5, "description": "Stars, 1–5." },
                    "review": { "type": "string", "description": "What worked and what didn't." }
                },
                "required": ["name", "rating"]
            }),
            Kind::ReadSkillReviews => serde_json::json!({
                "type": "object",
                "properties": { "name": name("The skill's name.") },
                "required": ["name"]
            }),
        }
    }

    fn read_only(self, input: &serde_json::Value) -> bool {
        match self {
            Kind::UseSkill | Kind::FindSkills | Kind::ReadSkillFile | Kind::ReadSkillReviews => true,
            Kind::ConfigureSkill => str_field(input, "key").is_empty(),
            Kind::SaveSkill | Kind::DeleteSkill | Kind::InstallSkill | Kind::RateSkill => false,
        }
    }

    fn validate(self, input: &serde_json::Value) -> Result<(), String> {
        let blank = |k: &str| str_field(input, k).trim().is_empty();
        match self {
            Kind::FindSkills if blank("query") => Err("`query` is empty: say what you're trying to do.".to_string()),
            Kind::FindSkills => Ok(()),
            Kind::InstallSkill if !str_field(input, "code").starts_with("SKIL-") => {
                Err("`code` must be a skill install code starting with SKIL- (e.g. SKIL-XXXX-XXXX).".to_string())
            }
            Kind::InstallSkill => Ok(()),
            _ if blank("name") => Err("`name` is empty: pass the skill's name.".to_string()),
            Kind::SaveSkill if blank("content") => Err("`content` is empty: pass the whole SKILL.md.".to_string()),
            Kind::ConfigureSkill if blank("key") != blank("value") => {
                Err("Pass `key` and `value` together to set a secret, or neither to list the skill's secrets.".to_string())
            }
            _ => Ok(()),
        }
    }

    /// (activity, outcome) for the owner.
    fn labels(self, input: &serde_json::Value) -> (String, String) {
        let name = str_field(input, "name");
        let skill = if name.is_empty() { "a skill".to_string() } else { format!("the {name} skill") };
        match self {
            Kind::UseSkill => (format!("loading {skill}"), format!("Loaded {skill}")),
            Kind::FindSkills => ("searching skills".into(), "Searched skills".into()),
            Kind::ReadSkillFile => (format!("reading {skill}'s files"), format!("Read {skill}'s files")),
            Kind::SaveSkill => (format!("saving {skill}"), format!("Saved {skill}")),
            Kind::DeleteSkill => (format!("deleting {skill}"), format!("Deleted {skill}")),
            Kind::InstallSkill => ("installing a skill".into(), "Installed a skill".into()),
            Kind::ConfigureSkill => (format!("setting up {skill}"), format!("Set up {skill}")),
            Kind::RateSkill => (format!("reviewing {skill}"), format!("Reviewed {skill}")),
            Kind::ReadSkillReviews => (format!("reading reviews of {skill}"), format!("Read reviews of {skill}")),
        }
    }
}

/// One skill tool (see [`Kind`] for the family).
pub struct SkillTool {
    core: Arc<SkillCore>,
    kind: Kind,
}

/// Every skill tool, sharing one core.
pub fn tools(core: SkillCore) -> Vec<SkillTool> {
    let core = Arc::new(core);
    KINDS.iter().map(|&kind| SkillTool { core: core.clone(), kind }).collect()
}

impl DynTool for SkillTool {
    fn name(&self) -> &str {
        self.kind.name()
    }

    fn description(&self) -> String {
        self.kind.description().to_string()
    }

    fn schema(&self) -> serde_json::Value {
        self.kind.schema()
    }

    fn search_hint(&self) -> &str {
        self.kind.search_hint()
    }

    /// `use_skill` is core; the rest load through `find_tools`.
    fn should_defer(&self) -> bool {
        self.kind != Kind::UseSkill
    }

    fn read_only(&self, input: &serde_json::Value) -> bool {
        self.kind.read_only(input)
    }

    fn validate_input(&self, input: &serde_json::Value) -> Result<(), String> {
        self.kind.validate(input)
    }

    /// Loaded instructions are the conversation's working text: never
    /// swapped for a saved-to-disk preview.
    fn max_result_chars(&self, _input: &serde_json::Value) -> Option<usize> {
        match self.kind {
            Kind::UseSkill => None,
            _ => Some(crate::registry::DEFAULT_MAX_RESULT_CHARS),
        }
    }

    fn activity(&self, input: &serde_json::Value) -> String {
        self.kind.labels(input).0
    }

    fn outcome(&self, input: &serde_json::Value) -> String {
        self.kind.labels(input).1
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            // Per-employee skill scope: runs bound to an agent (session key
            // "agent:<id>:...") also see that agent's own skills. On the
            // review fork its learned tree is also the write target.
            let agent_scope =
                Some(types::keyparser::extract_agent_id(&ctx.session_key)).filter(|id| !id.is_empty());
            let learned_owner = ctx.learned_write_agent.as_deref();
            let scope = Scope {
                agent: learned_owner.or(agent_scope.as_deref()),
                learned_owner,
            };
            let core = &self.core;
            let name = str_field(&input, "name");
            match self.kind {
                Kind::UseSkill => {
                    core.load(ctx, scope, name, input.get("args").and_then(|v| v.as_str())).await
                }
                Kind::FindSkills => core.find(scope, str_field(&input, "query")).await,
                Kind::ReadSkillFile => core.read_file(scope, name, str_field(&input, "path")).await,
                Kind::SaveSkill => core.save(ctx, scope, name, str_field(&input, "content")).await,
                Kind::DeleteSkill => core.delete(ctx, scope, name).await,
                Kind::InstallSkill => core.install(ctx, str_field(&input, "code")).await,
                Kind::ConfigureSkill => {
                    core.configure(scope, name, str_field(&input, "key"), str_field(&input, "value")).await
                }
                Kind::RateSkill => {
                    let rating = input.get("rating").and_then(|v| v.as_i64()).unwrap_or(0);
                    if !(1..=5).contains(&rating) {
                        return ToolResult::error(format!("rating must be an integer 1-5 (got {})", input["rating"]));
                    }
                    core.rate(name, rating, str_field(&input, "review")).await
                }
                Kind::ReadSkillReviews => core.reviews(name).await,
            }
        })
    }
}

/// The skill's other files, named at the end of its instructions with the
/// call that reads one: its instructions point at them by relative path
/// ("see reference/actions.md"), and on 2026-09-26 runs asked use_skill for
/// one twice (args "action: browse, path: …") and got the instructions
/// again. Empty when it has none.
fn files_line(name: &str, mut files: Vec<String>) -> String {
    if files.is_empty() {
        return String::new();
    }
    files.sort();
    let more = files.len().saturating_sub(LOADED_FILES_NAMED);
    files.truncate(LOADED_FILES_NAMED);
    let more = if more > 0 { format!(" and {more} more") } else { String::new() };
    format!(
        "\n\nFiles in it: {}{more}. Read one with read_skill_file(name: \"{name}\", path: \"<file>\").",
        files.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn family(dir: &std::path::Path) -> Vec<SkillTool> {
        let loader = Arc::new(Loader::new(dir.join("installed"), dir.join("user")));
        tools(SkillCore::new(loader))
    }

    fn tool<'a>(family: &'a [SkillTool], name: &str) -> &'a SkillTool {
        family.iter().find(|t| t.name() == name).unwrap()
    }

    #[test]
    fn install_failures_are_detected_and_sources_are_words() {
        assert!(install_failed("Failed to install skill: not found"));
        assert!(install_failed("'SKIL-1' is not a valid install code (e.g. ...)"));
        assert!(!install_failed("Installed skill 'foo' (v1.2)"));
        assert_eq!(source_words(&SkillSource::Learned), "a learned skill of this employee");
        assert!(SkillCore::not_found("x", LISTING_HINT).contains("find_skills"));
    }

    /// One tool loads, the rest are deferred, and each is one purpose with
    /// no action enum. use_skill loads only what the task needs, several in
    /// one response (they run together: it only reads), and a survey of
    /// many skills goes to a helper.
    #[test]
    fn use_skill_is_core_and_the_family_is_deferred() {
        let dir = tempfile::tempdir().unwrap();
        let family = family(dir.path());
        let names: Vec<&str> = family.iter().map(|t| t.name()).collect();
        assert_eq!(
            names,
            [
                "use_skill", "find_skills", "read_skill_file", "save_skill", "delete_skill",
                "install_skill", "configure_skill", "rate_skill", "read_skill_reviews"
            ]
        );
        for t in &family {
            assert_eq!(t.should_defer(), t.name() != USE_SKILL, "{}", t.name());
            assert!(t.schema()["properties"].get("action").is_none(), "{}", t.name());
        }
        let use_skill = tool(&family, USE_SKILL);
        let d = use_skill.description();
        for rule in [
            "Load only the skills the task needs, several in one response. To learn many, delegate.",
        ] {
            assert!(d.contains(rule), "{rule:?} missing from:\n{d}");
        }
        assert!(use_skill.read_only(&json!({"name": "x"})));
        assert!(use_skill.concurrency_safe(&json!({"name": "x"})));
        assert_eq!(use_skill.max_result_chars(&json!({})), None);
        let configure = tool(&family, "configure_skill");
        assert!(configure.read_only(&json!({"name": "x"})));
        assert!(!configure.read_only(&json!({"name": "x", "key": "K", "value": "v"})));
        assert!(!tool(&family, "save_skill").read_only(&json!({"name": "x", "content": "y"})));
    }

    #[test]
    fn inputs_are_checked_before_the_handler() {
        let dir = tempfile::tempdir().unwrap();
        let family = family(dir.path());
        assert!(tool(&family, USE_SKILL).validate_input(&json!({"name": " "})).is_err());
        assert!(tool(&family, "install_skill").validate_input(&json!({"code": "PLUG-1"})).is_err());
        assert!(tool(&family, "install_skill").validate_input(&json!({"code": "SKIL-AB12-CD34"})).is_ok());
        let configure = tool(&family, "configure_skill");
        assert!(configure.validate_input(&json!({"name": "x", "key": "K"})).is_err());
        assert!(configure.validate_input(&json!({"name": "x"})).is_ok());
        assert!(tool(&family, "save_skill").validate_input(&json!({"name": "x", "content": ""})).is_err());
    }

    /// An unknown name answers with where the names are, not a schema error.
    #[tokio::test]
    async fn an_unknown_skill_points_at_the_listing() {
        let dir = tempfile::tempdir().unwrap();
        let family = family(dir.path());
        let ctx = ToolContext::default();
        let result = tool(&family, "read_skill_file").execute_dyn(&ctx, json!({"name": "no-such-skill"})).await;
        assert!(result.is_error, "{}", result.content);
        assert!(result.content.contains("no-such-skill"), "{}", result.content);
        assert!(result.content.contains("skill listing"), "{}", result.content);
    }

    /// A load of a skill that isn't installed names the installed skills
    /// that do what its name says, or settles that there is none: either
    /// way the task goes on with the other tools, and nothing sends the
    /// model searching again.
    #[tokio::test]
    async fn a_load_miss_names_what_is_close_or_says_to_proceed() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("installed").join("revenue-report");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: revenue-report\ndescription: Totals revenue by quarter\n---\nAdd it up.\n",
        )
        .unwrap();
        let loader = Arc::new(Loader::new(dir.path().join("installed"), dir.path().join("user")));
        loader.load_all().await;
        let family = tools(SkillCore::new(loader));
        let ctx = ToolContext::default();
        let use_skill = tool(&family, USE_SKILL);

        let none = use_skill.execute_dyn(&ctx, json!({"name": "calendar-sync"})).await;
        assert!(none.is_error);
        assert_eq!(
            none.content,
            "No skill named 'calendar-sync'. No installed skill has a name like it. The skill listing names every \
             installed skill; if none there fits, there is no skill for this task: do it with your other tools \
             rather than searching again."
        );

        // Its description says "quarter"; its name doesn't: not close.
        let described = use_skill.execute_dyn(&ctx, json!({"name": "quarter-summary"})).await;
        assert!(described.content.contains("No installed skill has a name like it."), "{}", described.content);

        let close = use_skill.execute_dyn(&ctx, json!({"name": "quarterly-revenue"})).await;
        assert!(close.is_error);
        assert!(
            close.content.contains("Installed skills close to it: revenue-report (Totals revenue by quarter)."),
            "{}",
            close.content
        );
        assert!(close.content.ends_with("otherwise do the task with your other tools."), "{}", close.content);
    }

    /// Loading returns the instructions, with the base directory and the
    /// call's arguments; a file read and a listing reach the skill's files.
    #[tokio::test]
    async fn a_load_brings_the_instructions_and_the_files_are_readable() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("installed").join("invoicing");
        std::fs::create_dir_all(skill_dir.join("scripts")).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: invoicing\ndescription: Draft an invoice\n---\nBill the client for $ARGUMENTS.\n",
        )
        .unwrap();
        std::fs::write(skill_dir.join("scripts").join("total.py"), "print(1)\n").unwrap();
        let loader = Arc::new(Loader::new(dir.path().join("installed"), dir.path().join("user")));
        loader.load_all().await;
        let family = tools(SkillCore::new(loader));
        let ctx = ToolContext::default();

        let loaded = tool(&family, USE_SKILL)
            .execute_dyn(&ctx, json!({"name": "invoicing", "args": "March"}))
            .await;
        assert!(!loaded.is_error, "{}", loaded.content);
        assert!(loaded.content.contains("Bill the client for March."), "{}", loaded.content);
        assert!(loaded.content.contains("This skill's files are in:"), "{}", loaded.content);
        assert!(
            loaded.content.ends_with(
                "Files in it: scripts/total.py. Read one with read_skill_file(name: \"invoicing\", path: \"<file>\")."
            ),
            "the files it points at are named with the call that reads them: {}",
            loaded.content
        );

        let read = tool(&family, "read_skill_file");
        let files = read.execute_dyn(&ctx, json!({"name": "invoicing"})).await;
        assert!(files.content.contains("scripts/total.py"), "{}", files.content);
        let file = read.execute_dyn(&ctx, json!({"name": "invoicing", "path": "scripts/total.py"})).await;
        assert_eq!(file.content, "print(1)\n");

        let found = tool(&family, "find_skills").execute_dyn(&ctx, json!({"query": "invoice"})).await;
        assert!(found.content.contains("- invoicing: Draft an invoice"), "{}", found.content);
    }

    #[test]
    fn arguments_fill_the_placeholder_or_follow_the_instructions() {
        assert_eq!(with_args("Do $ARGUMENTS now", Some("x")), "Do x now");
        assert_eq!(with_args("Do it", Some("x")), "Do it\n\nARGUMENTS: x");
        assert_eq!(with_args("Do it", Some("  ")), "Do it");
        assert_eq!(with_args("Do it", None), "Do it");
    }

    #[test]
    fn test_user_skill_dir_rejects_traversal_names() {
        // Names that could escape the skills directory must be rejected
        // before any filesystem operation (esp. delete's remove_dir_all).
        for bad in ["../../foo", "..", "a/../b", "foo/bar", "foo\\bar", "/etc", ".", ""] {
            assert!(SkillCore::user_skill_dir(bad).is_err(), "expected rejection for {:?}", bad);
        }
    }
}
