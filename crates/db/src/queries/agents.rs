use rusqlite::params;

use crate::{DbErrExt, OptionalExt};
use crate::Store;
use crate::models::{Agent, AgentWorkflow, EmitSource};
use types::NeboError;

/// An employee's public name: lowercased, runs of anything that isn't a letter
/// or digit collapsed to one dash. Two names with the same slug are the same
/// name, so uniqueness and the API's `employee/<slug>` both key on this.
pub fn agent_slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut dash = false;
    for c in name.trim().chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod slug_tests {
    use super::agent_slug;

    #[test]
    fn slug_collapses_case_space_and_punctuation() {
        assert_eq!(agent_slug("Executive Assistant"), "executive-assistant");
        assert_eq!(agent_slug("  Frontend Designer/Coder Agent "), "frontend-designer-coder-agent");
        assert_eq!(agent_slug("Sales-Advisor"), agent_slug("sales advisor"));
        assert_eq!(agent_slug("Nebo!!"), "nebo");
        assert_eq!(agent_slug("---"), "");
    }
}

impl Store {
    pub fn list_agents(&self, limit: i64, offset: i64) -> Result<Vec<Agent>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, kind, name, description, agent_md, frontmatter,
                        pricing_model, pricing_cost, is_enabled, installed_at, updated_at,
                        napp_path, input_values, is_app, app_ui_path, app_binary_path, app_window_config, soul, rules, handle, color, loop_exposed, loop_agent_id, department, voice, name_locked, context_stamp, reports_to, department_locked
                 FROM agents ORDER BY installed_at DESC LIMIT ?1 OFFSET ?2",
            )
            .db_err("list_agents prepare")?;
        let rows = stmt
            .query_map(params![limit, offset], row_to_agent)
            .db_err("list_agents query")?;
        rows.collect::<Result<Vec<_>, _>>()
            .db_err("list_agents collect")
    }

    pub fn count_agents(&self) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.query_row("SELECT COUNT(*) FROM agents", [], |row| row.get(0))
            .db_err("count_agents")
    }

    pub fn get_agent(&self, id: &str) -> Result<Option<Agent>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, kind, name, description, agent_md, frontmatter,
                    pricing_model, pricing_cost, is_enabled, installed_at, updated_at,
                    napp_path, input_values, is_app, app_ui_path, app_binary_path, app_window_config, soul, rules, handle, color, loop_exposed, loop_agent_id, department, voice, name_locked, context_stamp, reports_to, department_locked
             FROM agents WHERE id = ?1",
            params![id],
            row_to_agent,
        )
        .optional()
        .db_err("get_agent")
    }

    /// The employee named like `name`, matched on slug so "Sales Advisor" and
    /// "sales-advisor" are the same name. `except_id` is the row being renamed.
    pub fn agent_name_taken(&self, name: &str, except_id: Option<&str>) -> Result<Option<String>, NeboError> {
        let slug = agent_slug(name);
        if slug.is_empty() {
            return Ok(None);
        }
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT id, name FROM agents").db_err("agent_name_taken")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .db_err("agent_name_taken")?;
        for row in rows.flatten() {
            if except_id != Some(row.0.as_str()) && agent_slug(&row.1) == slug {
                return Ok(Some(row.1));
            }
        }
        Ok(None)
    }

    /// The employee whose name slugs to `slug` — how the public API names one.
    pub fn get_agent_by_slug(&self, slug: &str) -> Result<Option<Agent>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, kind, name, description, agent_md, frontmatter,
                        pricing_model, pricing_cost, is_enabled, installed_at, updated_at,
                        napp_path, input_values, is_app, app_ui_path, app_binary_path, app_window_config, soul, rules, handle, color, loop_exposed, loop_agent_id, department, voice, name_locked, context_stamp, reports_to, department_locked
                 FROM agents",
            )
            .db_err("get_agent_by_slug")?;
        let rows = stmt.query_map([], row_to_agent).db_err("get_agent_by_slug")?;
        Ok(rows.flatten().find(|a| agent_slug(&a.name) == slug))
    }

    /// Give later duplicates a numbered name so every employee name is unique;
    /// the renamed rows lock their name so a manifest sync can't undo it.
    /// Returns (id, new name) per rename. Idempotent: no duplicates, no writes.
    pub fn dedupe_agent_names(&self) -> Result<Vec<(String, String)>, NeboError> {
        let conn = self.conn()?;
        let rows: Vec<(String, String)> = {
            let mut stmt = conn
                .prepare("SELECT id, name FROM agents ORDER BY installed_at, rowid")
                .db_err("dedupe_agent_names")?;
            let it = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .db_err("dedupe_agent_names")?;
            it.flatten().collect()
        };
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut renamed = Vec::new();
        for (id, name) in rows {
            let slug = agent_slug(&name);
            if seen.insert(slug.clone()) {
                continue;
            }
            let fresh = (2..)
                .map(|n| format!("{} {n}", name.trim()))
                .find(|candidate| !seen.contains(&agent_slug(candidate)))
                .expect("an unbounded counter finds a free name");
            conn.execute(
                "UPDATE agents SET name = ?2, name_locked = 1, updated_at = unixepoch() WHERE id = ?1",
                params![id, fresh],
            )
            .db_err("dedupe_agent_names")?;
            seen.insert(agent_slug(&fresh));
            renamed.push((id, fresh));
        }
        Ok(renamed)
    }

    pub fn create_agent(
        &self,
        id: &str,
        kind: Option<&str>,
        name: &str,
        description: &str,
        agent_md: &str,
        frontmatter: &str,
        pricing_model: Option<&str>,
        pricing_cost: Option<f64>,
    ) -> Result<Agent, NeboError> {
        if let Some(other) = self.agent_name_taken(name, None)? {
            return Err(NeboError::Validation(format!("An employee named \"{other}\" already exists. Pick a different name.")));
        }
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO agents (id, kind, name, description, agent_md, frontmatter,
                    pricing_model, pricing_cost)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             RETURNING id, kind, name, description, agent_md, frontmatter,
                       pricing_model, pricing_cost, is_enabled, installed_at, updated_at,
                       napp_path, input_values, is_app, app_ui_path, app_binary_path, app_window_config, soul, rules, handle, color, loop_exposed, loop_agent_id, department, voice, name_locked, context_stamp, reports_to, department_locked",
            params![id, kind, name, description, agent_md, frontmatter, pricing_model, pricing_cost],
            row_to_agent,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn update_agent(
        &self,
        id: &str,
        name: &str,
        description: &str,
        agent_md: &str,
        frontmatter: &str,
        pricing_model: Option<&str>,
        pricing_cost: Option<f64>,
        soul: Option<&str>,
        rules: Option<&str>,
        handle: Option<&str>,
        color: Option<&str>,
        loop_exposed: Option<bool>,
        voice: Option<&str>,
        // The part of the company this seat sits in. `Some("")` clears it.
        // Setting it locks it: the owner's department outlives every later
        // package sync (`set_agent_department` respects the lock).
        department: Option<&str>,
        // The seat this one answers to, by local agent id. `Some("")` clears
        // the line (answers to the owner). Refused when it would close a loop.
        reports_to: Option<&str>,
    ) -> Result<(), NeboError> {
        if !name.trim().is_empty() {
            if let Some(other) = self.agent_name_taken(name, Some(id))? {
                return Err(NeboError::Validation(format!("An employee named \"{other}\" already exists. Pick a different name.")));
            }
        }
        // The reporting line is refused HERE, not in a client: every door that
        // writes an employee's fields comes through this one call.
        if let Some(manager) = reports_to.map(str::trim).filter(|m| !m.is_empty()) {
            self.check_reporting_line(id, manager)?;
        }
        let conn = self.conn()?;
        // A blank name is never written and never locks: locking one would leave
        // the row nameless with no way back, since sync_agent_identity only
        // restores the manifest name while name_locked = 0. Same guard shape the
        // sync uses, so the two agree on what counts as a name.
        conn.execute(
            "UPDATE agents SET name_locked = CASE WHEN TRIM(?1) != '' AND ?1 != name THEN 1 ELSE name_locked END,
                    name = CASE WHEN TRIM(?1) != '' THEN ?1 ELSE name END,
                    description = ?2, agent_md = ?3,
                    frontmatter = ?4, pricing_model = ?5, pricing_cost = ?6,
                    soul = COALESCE(?7, soul),
                    rules = COALESCE(?8, rules),
                    handle = COALESCE(?9, handle),
                    color = COALESCE(?10, color),
                    loop_exposed = COALESCE(?11, loop_exposed),
                    voice = COALESCE(?12, voice),
                    -- Owner-wins, the name_locked contract applied to the
                    -- department: an explicit value (blank = none) is the
                    -- owner's and locks the column against package syncs.
                    department_locked = CASE WHEN ?13 IS NOT NULL THEN 1 ELSE department_locked END,
                    department = CASE
                        WHEN ?13 IS NULL THEN department
                        WHEN TRIM(?13) = '' THEN NULL
                        ELSE TRIM(?13) END,
                    reports_to = CASE
                        WHEN ?14 IS NULL THEN reports_to
                        WHEN TRIM(?14) = '' THEN NULL
                        ELSE TRIM(?14) END,
                    updated_at = unixepoch()
             WHERE id = ?15",
            params![
                name,
                description,
                agent_md,
                frontmatter,
                pricing_model,
                pricing_cost,
                soul,
                rules,
                handle,
                color,
                loop_exposed.map(|b| b as i32),
                voice,
                department,
                reports_to,
                id
            ],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Set an agent's "Expose to Loop" flag. Used to seed the primary agent's
    /// default (ON) at row creation; the toggle save path uses `update_agent`.
    pub fn set_loop_exposed(&self, id: &str, exposed: bool) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET loop_exposed = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, exposed as i32],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Sync filesystem-owned columns: content (agent_md, frontmatter) and
    /// manifest identity (name, description). The manifest is the source of
    /// truth for display name — without this, agents get stuck with slug names.
    ///
    /// Owner-set runtime state rides IN the DB frontmatter and the filesystem
    /// knows nothing about it, so a content refresh must carry it forward:
    /// `memory.context_isolated` (the isolation toggle) previously vanished on
    /// every server restart, silently un-isolating employees. The owner's DB
    /// value always wins over the publisher's shipped default.
    pub fn sync_agent_content(
        &self,
        id: &str,
        agent_md: &str,
        frontmatter: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let existing: Option<String> = conn
            .query_row(
                "SELECT frontmatter FROM agents WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .db_err("sync_agent_content read")?;
        // ONE merge for both package-delivery paths (this sync and the
        // marketplace install/update in `persist_agent_from_api`): the owner's
        // own declaration entries and the isolation toggle are held, everything
        // else is the package's to change. See `crate::declaration`.
        let merged = match existing {
            Some(ours) => crate::declaration::merge_package_declaration(&ours, frontmatter),
            None => frontmatter.to_string(),
        };
        conn.execute(
            "UPDATE agents SET agent_md = ?1, frontmatter = ?2, updated_at = unixepoch()
             WHERE id = ?3",
            params![agent_md, merged, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Flip `memory.context_isolated` in the DB frontmatter and return the
    /// merged frontmatter so the caller can mirror it to agent.json. Used
    /// when a phone line is attached: a receptionist's callers must never
    /// share memory, so the line forces isolation on.
    /// The owner's per-run spending limit for this employee, in cents
    /// (`budget.run_spend_cap_cents` in the frontmatter); 0 = no limit. A
    /// package's `token_budget` figures are the author's cost estimate and
    /// are never enforced — this is the only ceiling a run has.
    pub fn agent_run_spend_cap_cents(&self, id: &str) -> i64 {
        let Ok(conn) = self.conn() else { return 0 };
        let fm: Option<String> = conn
            .query_row("SELECT frontmatter FROM agents WHERE id = ?1", params![id], |r| r.get(0))
            .ok();
        fm.and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|v| v.pointer("/budget/run_spend_cap_cents").and_then(|c| c.as_i64()))
            .unwrap_or(0)
            .max(0)
    }

    pub fn set_agent_context_isolated(&self, id: &str, isolated: bool) -> Result<serde_json::Value, NeboError> {
        let conn = self.conn()?;
        let current: String = conn
            .query_row("SELECT frontmatter FROM agents WHERE id = ?1", params![id], |r| r.get(0))
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let mut fm: serde_json::Value = serde_json::from_str(&current).unwrap_or(serde_json::json!({}));
        if !fm.is_object() {
            fm = serde_json::json!({});
        }
        let mem = fm
            .as_object_mut()
            .unwrap()
            .entry("memory")
            .or_insert_with(|| serde_json::json!({}));
        if !mem.is_object() {
            *mem = serde_json::json!({});
        }
        mem.as_object_mut().unwrap().insert("context_isolated".into(), serde_json::json!(isolated));
        conn.execute(
            "UPDATE agents SET frontmatter = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, fm.to_string()],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(fm)
    }

    /// Sync display name and description from the manifest.
    /// Only updates if the manifest provides non-empty values, and never
    /// overwrites an owner-renamed (name_locked) name — the boot FS→DB sync
    /// runs on every restart and used to revert christened names to the
    /// bundled manifest's default.
    pub fn sync_agent_identity(
        &self,
        id: &str,
        name: &str,
        description: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET name = CASE WHEN ?2 != '' AND name_locked = 0 THEN ?2 ELSE name END,
                    description = CASE WHEN ?3 != '' THEN ?3 ELSE description END,
                    updated_at = unixepoch()
             WHERE id = ?1",
            params![id, name, description],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Get an agent by name (case-insensitive).
    pub fn get_agent_by_name(&self, name: &str) -> Result<Option<Agent>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, kind, name, description, agent_md, frontmatter,
                    pricing_model, pricing_cost, is_enabled, installed_at, updated_at,
                    napp_path, input_values, is_app, app_ui_path, app_binary_path, app_window_config, soul, rules, handle, color, loop_exposed, loop_agent_id, department, voice, name_locked, context_stamp, reports_to, department_locked
             FROM agents WHERE LOWER(name) = LOWER(?1)",
            params![name],
            row_to_agent,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Check if an agent is installed by matching its name (case-insensitive).
    pub fn agent_installed_by_name(&self, name: &str) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agents WHERE LOWER(name) = LOWER(?1)",
                params![name],
                |row| row.get(0),
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(count > 0)
    }

    pub fn delete_agent(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM agents WHERE id = ?1", params![id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        // Clean up per-agent state with no FK cascade so deleting an agent doesn't
        // leave orphans: its entity_config rows (tracked bug) and its
        // artifact-update-tracking row. Best-effort — a failure here must not block
        // the delete itself.
        let _ = conn.execute("DELETE FROM entity_config WHERE entity_id = ?1", params![id]);
        let _ = conn.execute(
            "DELETE FROM artifact_update_prefs WHERE artifact_id = ?1 AND artifact_type = 'agent'",
            params![id],
        );
        Ok(())
    }

    pub fn set_agent_app_fields(
        &self,
        id: &str,
        is_app: bool,
        app_ui_path: Option<&str>,
        app_binary_path: Option<&str>,
        app_window_config: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET is_app = ?1, app_ui_path = ?2, app_binary_path = ?3,
                    app_window_config = ?4, updated_at = unixepoch()
             WHERE id = ?5",
            params![
                is_app as i32,
                app_ui_path,
                app_binary_path,
                app_window_config,
                id
            ],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// The PACKAGE's department, written on install and re-install. Never
    /// overwrites an owner-set (department_locked) department — the marketplace
    /// employee arrives in "sales", the owner moves it to "Revenue", and the
    /// next sync must leave it there. Exactly the guard `sync_agent_identity`
    /// uses for a locked name; the owner's door is `update_agent`.
    pub fn set_agent_department(&self, id: &str, department: Option<&str>) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET department = ?1, updated_at = unixepoch()
             WHERE id = ?2 AND department_locked = 0",
            params![department, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// The seats an employee answers to, nearest manager first, as
    /// `(id, name)`. Ends at the owner (no manager), at a manager id that no
    /// longer resolves (a deleted employee reads as "answers to the owner"),
    /// or — if the stored data is already cyclic — at the first seat that
    /// repeats, so a caller walking the line can never loop forever.
    ///
    /// The ONE reading of the reporting line: the cycle refusal below and
    /// every escalation that takes work upwards use this, not their own walk.
    pub fn manager_chain(&self, agent_id: &str) -> Result<Vec<(String, String)>, NeboError> {
        let conn = self.conn()?;
        let mut chain: Vec<(String, String)> = Vec::new();
        let mut seen: Vec<String> = vec![agent_id.to_string()];
        let mut current = agent_id.to_string();
        loop {
            let next: Option<(String, String)> = conn
                .query_row(
                    "SELECT m.id, m.name FROM agents a
                     JOIN agents m ON m.id = a.reports_to
                     WHERE a.id = ?1",
                    params![current],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|e| NeboError::Database(e.to_string()))?;
            let Some((id, name)) = next else { return Ok(chain) };
            let closes = seen.contains(&id);
            chain.push((id.clone(), name));
            if closes {
                return Ok(chain);
            }
            seen.push(id.clone());
            current = id;
        }
    }

    /// Refuse a reporting line that would close a loop, naming the loop.
    /// Called by `update_agent` — the owner's one door — so no surface can
    /// write a cycle even if its UI would have allowed it.
    fn check_reporting_line(&self, id: &str, manager_id: &str) -> Result<(), NeboError> {
        let name_of = |who: &str| -> String {
            self.get_agent(who)
                .ok()
                .flatten()
                .map(|a| a.name)
                .unwrap_or_else(|| who.to_string())
        };
        let me = name_of(id);
        if manager_id == id {
            return Err(NeboError::Validation(format!(
                "{me} cannot report to itself. Pick another employee, or leave the reporting line empty to answer to you."
            )));
        }
        if self.get_agent(manager_id)?.is_none() {
            return Err(NeboError::Validation(format!(
                "No employee with id {manager_id} to report to."
            )));
        }
        // Walking UP from the proposed manager must never reach this seat: if
        // it does, this seat is already somewhere above it and the new line
        // would close the loop.
        let chain = self.manager_chain(manager_id)?;
        if let Some(pos) = chain.iter().position(|(cid, _)| cid == id) {
            let mut hops: Vec<String> = vec![name_of(manager_id)];
            hops.extend(chain[..=pos].iter().map(|(_, n)| n.clone()));
            return Err(NeboError::Validation(format!(
                "{me} cannot report to {}: that closes a loop — {}. Move one of those employees first, \
                 or leave {me}'s reporting line empty to answer to you.",
                name_of(manager_id),
                hops.join(" answers to ")
            )));
        }
        Ok(())
    }

    /// The seat reviewed the packages and rewrote the package part of its
    /// Rules: store the merged Rules and mark the stamp written.
    pub fn set_agent_rules_from_packages(&self, id: &str, rules: &str, stamp: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET rules = ?1, context_stamp = ?2, updated_at = unixepoch() WHERE id = ?3",
            params![rules, stamp, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// The update run was launched (or failed): record what it is against.
    pub fn set_agent_context_stamp(&self, id: &str, stamp: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET context_stamp = ?1 WHERE id = ?2",
            params![stamp, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_agent_napp_path(&self, id: &str, napp_path: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET napp_path = ?1, updated_at = unixepoch() WHERE id = ?2",
            params![napp_path, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_agent_input_values(&self, id: &str, input_values: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET input_values = ?1, updated_at = unixepoch() WHERE id = ?2",
            params![input_values, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn toggle_agent(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET is_enabled = NOT is_enabled, updated_at = unixepoch() WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_agent_enabled(&self, id: &str, enabled: bool) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET is_enabled = ?1, updated_at = unixepoch() WHERE id = ?2",
            params![enabled as i32, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Set (or clear, with None) the NeboAI loop agent UUID for an agent.
    /// Kept separate from `update_agent` so reconcile can capture/backfill/clear
    /// this field without clobbering user-edited identity columns.
    pub fn set_agent_loop_agent_id(
        &self,
        id: &str,
        loop_agent_id: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET loop_agent_id = ?1 WHERE id = ?2",
            params![loop_agent_id, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Look up a local agent by its NeboAI loop agent UUID. Used by the channel
    /// branch to resolve an `<@{loop_agent_id}>` mention token to a local agent.
    pub fn get_agent_by_loop_agent_id(
        &self,
        loop_agent_id: &str,
    ) -> Result<Option<Agent>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, kind, name, description, agent_md, frontmatter,
                    pricing_model, pricing_cost, is_enabled, installed_at, updated_at,
                    napp_path, input_values, is_app, app_ui_path, app_binary_path, app_window_config, soul, rules, handle, color, loop_exposed, loop_agent_id, department, voice, name_locked, context_stamp, reports_to, department_locked
             FROM agents WHERE loop_agent_id = ?1",
            params![loop_agent_id],
            row_to_agent,
        )
        .optional()
        .db_err("get_agent_by_loop_agent_id")
    }

    /// Persist the NeboAI agent-space conversation id observed for an agent.
    /// Durable side of the in-memory ConvMaps conv→agent association so inbound
    /// DMs still resolve after a restart (before the join repopulates ConvMaps).
    pub fn set_agent_loop_conv_id(&self, id: &str, loop_conv_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET loop_conv_id = ?1 WHERE id = ?2",
            params![loop_conv_id, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Resolve a local agent id from a NeboAI agent-space conversation id.
    /// Fallback used by the inbound DM path when ConvMaps has no entry yet.
    pub fn get_agent_id_by_loop_conv_id(
        &self,
        loop_conv_id: &str,
    ) -> Result<Option<String>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id FROM agents WHERE loop_conv_id = ?1",
            params![loop_conv_id],
            |row| row.get(0),
        )
        .optional()
        .db_err("get_agent_id_by_loop_conv_id")
    }

    // ── Agent Workflow Bindings ──

    /// `owner_modified` says who is writing: `true` for the owner's surfaces
    /// (API PUT/POST, the work tool) — the write always lands and flags the
    /// row; `false` for package sync (boot FS→DB, legacy migration) — the
    /// write lands only on rows the owner has never touched. A cloud bot's
    /// image roll re-extracts the packaged agent.json, and an unconditional
    /// upsert silently reverted live owner edits on every restart.
    pub fn upsert_agent_workflow(
        &self,
        agent_id: &str,
        binding_name: &str,
        trigger_type: &str,
        trigger_config: &str,
        description: Option<&str>,
        inputs: Option<&str>,
        emit: Option<&str>,
        activities: Option<&str>,
        connections: Option<&str>,
        owner_modified: bool,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO agent_workflows (agent_id, binding_name,
                    trigger_type, trigger_config, description, inputs, emit, activities, connections, owner_modified)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(agent_id, binding_name) DO UPDATE SET
                trigger_type = excluded.trigger_type,
                trigger_config = excluded.trigger_config,
                description = excluded.description,
                inputs = excluded.inputs,
                emit = excluded.emit,
                activities = excluded.activities,
                connections = excluded.connections,
                owner_modified = MAX(agent_workflows.owner_modified, excluded.owner_modified)
             WHERE agent_workflows.owner_modified = 0 OR excluded.owner_modified = 1",
            params![agent_id, binding_name,
                    trigger_type, trigger_config, description, inputs, emit, activities, connections,
                    owner_modified as i64],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Every active heartbeat binding of every enabled agent — the engine's
    /// arming worklist for binding heartbeats.
    pub fn list_active_heartbeat_workflows(&self) -> Result<Vec<AgentWorkflow>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT aw.id, aw.agent_id, aw.binding_name,
                        aw.trigger_type, aw.trigger_config, aw.description, aw.inputs, aw.is_active, aw.emit, aw.activities, aw.last_fired, aw.connections
                 FROM agent_workflows aw JOIN agents a ON aw.agent_id = a.id
                 WHERE aw.trigger_type = 'heartbeat' AND aw.is_active = 1 AND a.is_enabled = 1
                 ORDER BY aw.agent_id, aw.binding_name",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_agent_workflow)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn list_agent_workflows(&self, agent_id: &str) -> Result<Vec<AgentWorkflow>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, agent_id, binding_name,
                        trigger_type, trigger_config, description, inputs, is_active, emit, activities, last_fired, connections
                 FROM agent_workflows WHERE agent_id = ?1",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![agent_id], row_to_agent_workflow)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn delete_single_agent_workflow(
        &self,
        agent_id: &str,
        binding_name: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM agent_workflows WHERE agent_id = ?1 AND binding_name = ?2",
            params![agent_id, binding_name],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn toggle_agent_workflow(
        &self,
        agent_id: &str,
        binding_name: &str,
    ) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agent_workflows SET is_active = NOT is_active WHERE agent_id = ?1 AND binding_name = ?2",
            params![agent_id, binding_name],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        // Return new is_active state
        let is_active: i64 = conn
            .query_row(
                "SELECT is_active FROM agent_workflows WHERE agent_id = ?1 AND binding_name = ?2",
                params![agent_id, binding_name],
                |row| row.get(0),
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(is_active != 0)
    }

    pub fn delete_agent_workflows(&self, agent_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM agent_workflows WHERE agent_id = ?1",
            params![agent_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Check if an agent workflow is active AND its parent agent is enabled.
    pub fn is_agent_workflow_active(
        &self,
        agent_id: &str,
        binding_name: &str,
    ) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_workflows aw
                 JOIN agents a ON aw.agent_id = a.id
                 WHERE aw.agent_id = ?1 AND aw.binding_name = ?2
                   AND aw.is_active = 1 AND a.is_enabled = 1",
                params![agent_id, binding_name],
                |row| row.get(0),
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(count > 0)
    }

    pub fn list_active_event_triggers(&self) -> Result<Vec<AgentWorkflow>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT aw.id, aw.agent_id, aw.binding_name,
                        aw.trigger_type, aw.trigger_config, aw.description, aw.inputs, aw.is_active, aw.emit, aw.activities, aw.last_fired, aw.connections
                 FROM agent_workflows aw
                 JOIN agents a ON aw.agent_id = a.id
                 WHERE aw.trigger_type = 'event' AND aw.is_active = 1 AND a.is_enabled = 1",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_agent_workflow)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn update_agent_workflow_last_fired(
        &self,
        agent_id: &str,
        binding_name: &str,
        fired_at: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agent_workflows SET last_fired = ?1
             WHERE agent_id = ?2 AND binding_name = ?3",
            params![fired_at, agent_id, binding_name],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Watch-trigger configs of active bindings, with the owning agent's
    /// name — used to surface `{plugin}.{event}` auto-emission sources.
    pub fn list_watch_trigger_configs(&self) -> Result<Vec<(String, String, String)>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT a.name, aw.binding_name, aw.trigger_config
                 FROM agent_workflows aw
                 JOIN agents a ON aw.agent_id = a.id
                 WHERE aw.trigger_type = 'watch' AND aw.is_active = 1",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn list_emit_sources(&self) -> Result<Vec<EmitSource>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT aw.emit, a.name AS agent_name, aw.binding_name, aw.description
                 FROM agent_workflows aw
                 JOIN agents a ON aw.agent_id = a.id
                 WHERE aw.emit IS NOT NULL AND aw.emit != '' AND aw.is_active = 1",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(EmitSource {
                    emit: row.get(0)?,
                    agent_name: row.get(1)?,
                    binding_name: row.get(2)?,
                    description: row.get(3)?,
                })
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn delete_cron_jobs_by_prefix(&self, prefix: &str) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        let pattern = format!("{}%", prefix);
        let count = conn
            .execute("DELETE FROM cron_jobs WHERE name LIKE ?1", params![pattern])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(count as i64)
    }

    // ── Agent data cleanup (on delete) ──

    /// Delete all chats belonging to agent sessions.
    /// Must be called BEFORE delete_agent_sessions (uses session_name pattern).
    /// Chat messages cascade-delete via FK.
    pub fn delete_agent_chats(&self, agent_id: &str) -> Result<usize, NeboError> {
        let conn = self.conn()?;
        let pattern = format!("agent:{}:%", agent_id);
        conn.execute(
            "DELETE FROM chats WHERE session_name LIKE ?1",
            params![pattern],
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Delete all sessions scoped to this agent.
    pub fn delete_agent_sessions(&self, agent_id: &str) -> Result<usize, NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM sessions WHERE scope = 'agent' AND scope_id = ?1",
            params![agent_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Delete all memories extracted during conversations with this agent.
    /// Memory user_id format: "{user_id}:agent:{agent_id}".
    /// Memory chunks cascade-delete via FK.
    pub fn delete_agent_memories(&self, agent_id: &str) -> Result<usize, NeboError> {
        let conn = self.conn()?;
        // Both the base agent scope AND its isolation contexts — the old
        // base-only pattern left every sealed matter's memories alive after
        // the employee was deleted. Chunks/embeddings cascade via FK.
        let base = format!("%:agent:{}", agent_id);
        let ctx = format!("%:agent:{}:ctx:%", agent_id);
        let n = conn
            .execute(
                "DELETE FROM memories WHERE user_id LIKE ?1 OR user_id LIKE ?2",
                params![base, ctx],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(n)
    }

    /// The other half of "deleted things leave memory": memories in ANY scope
    /// whose key or value mentions the deleted thing by name get a tombstone
    /// note appended (never destroyed — a memory may hold other facts), and
    /// their chunks are cleared so the boot backfill re-embeds the corrected
    /// text. Deterministic, no LLM. Names shorter than 4 chars are skipped —
    /// a substring sweep on "Al" would maul unrelated memories.
    pub fn tombstone_memories_mentioning(
        &self,
        name: &str,
        note: &str,
    ) -> Result<usize, NeboError> {
        let name = name.trim();
        if name.len() < 4 {
            return Ok(0);
        }
        let conn = self.conn()?;
        let pattern = format!("%{}%", name.to_lowercase());
        let ids: Vec<i64> = {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM memories
                     WHERE (LOWER(value) LIKE ?1 OR LOWER(key) LIKE ?1)
                       AND value NOT LIKE '%' || ?2 || '%'",
                )
                .map_err(|e| NeboError::Database(e.to_string()))?;
            let rows = stmt
                .query_map(params![pattern, note], |r| r.get(0))
                .map_err(|e| NeboError::Database(e.to_string()))?;
            rows.filter_map(|r| r.ok()).collect()
        };
        for id in &ids {
            conn.execute(
                "UPDATE memories SET value = value || ' [' || ?2 || ']', updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
                params![id, note],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
            conn.execute(
                "DELETE FROM memory_chunks WHERE memory_id = ?1",
                params![id],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        Ok(ids.len())
    }

}

fn row_to_agent_workflow(row: &rusqlite::Row) -> rusqlite::Result<AgentWorkflow> {
    let activities_str: Option<String> = row.get(9)?;
    let activities = activities_str.and_then(|s| serde_json::from_str(&s).ok());
    let connections_str: Option<String> = row.get(11)?;
    let connections = connections_str.and_then(|s| serde_json::from_str(&s).ok());
    Ok(AgentWorkflow {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        binding_name: row.get(2)?,
        trigger_type: row.get(3)?,
        trigger_config: row.get(4)?,
        description: row.get(5)?,
        inputs: row.get(6)?,
        is_active: row.get(7)?,
        emit: row.get(8)?,
        activities,
        last_fired: row.get(10)?,
        connections,
    })
}

fn row_to_agent(row: &rusqlite::Row) -> rusqlite::Result<Agent> {
    Ok(Agent {
        id: row.get(0)?,
        kind: row.get(1)?,
        name: row.get(2)?,
        description: row.get(3)?,
        agent_md: row.get(4)?,
        frontmatter: row.get(5)?,
        pricing_model: row.get(6)?,
        pricing_cost: row.get(7)?,
        is_enabled: row.get(8)?,
        installed_at: row.get(9)?,
        updated_at: row.get(10)?,
        napp_path: row.get(11)?,
        input_values: row
            .get::<_, Option<String>>(12)?
            .unwrap_or_else(|| "{}".to_string()),
        is_app: row.get(13)?,
        app_ui_path: row.get(14)?,
        app_binary_path: row.get(15)?,
        app_window_config: row.get(16)?,
        soul: row.get(17)?,
        rules: row.get(18)?,
        handle: row.get(19)?,
        color: row.get(20)?,
        loop_exposed: row.get(21)?,
        loop_agent_id: row.get(22)?,
        department: row.get(23)?,
        voice: row.get(24)?,
        name_locked: row.get(25)?,
        context_stamp: row.get(26)?,
        reports_to: row.get(27)?,
        department_locked: row.get(28)?,
    })
}

#[cfg(test)]
mod owner_modified_tests {
    use crate::Store;

    fn store() -> Store {
        let path = std::env::temp_dir()
            .join(format!("nebo-wfown-test-{}.db", uuid::Uuid::new_v4()));
        let s = Store::new(&path.to_string_lossy()).expect("store");
        s.create_agent("a1", None, "Test", "", "", "{}", None, None)
            .expect("agent row");
        s
    }

    fn activities(s: &Store) -> String {
        s.list_agent_workflows("a1").unwrap()[0]
            .activities
            .as_ref()
            .map(|v| v.to_string())
            .unwrap_or_default()
    }

    /// The Biss incident: a pod restart re-synced the packaged agent.json
    /// over a live owner restructure. Package sync (owner_modified=false)
    /// must never touch a row the owner has written.
    #[test]
    fn package_sync_cannot_clobber_owner_edits() {
        let s = store();
        // Package install seeds the binding.
        s.upsert_agent_workflow("a1", "order-intake", "watch", "{}", None, None, None,
            Some(r#"[{"id":"v1"}]"#), None, false).unwrap();
        assert!(activities(&s).contains("v1"));
        // Owner restructures it.
        s.upsert_agent_workflow("a1", "order-intake", "watch", "{}", None, None, None,
            Some(r#"[{"id":"v2-owner"}]"#), None, true).unwrap();
        // Boot re-sync from the packaged file: must be a no-op.
        s.upsert_agent_workflow("a1", "order-intake", "watch", "{}", None, None, None,
            Some(r#"[{"id":"v1"}]"#), None, false).unwrap();
        assert!(activities(&s).contains("v2-owner"), "package sync reverted an owner edit");
        // A later owner edit still lands.
        s.upsert_agent_workflow("a1", "order-intake", "watch", "{}", None, None, None,
            Some(r#"[{"id":"v3-owner"}]"#), None, true).unwrap();
        assert!(activities(&s).contains("v3-owner"));
    }

    /// Untouched package rows keep following the package.
    #[test]
    fn package_sync_still_updates_pristine_rows() {
        let s = store();
        s.upsert_agent_workflow("a1", "daily-brief", "schedule", "0 0 8 * * * *", None, None, None,
            Some(r#"[{"id":"pkg1"}]"#), None, false).unwrap();
        s.upsert_agent_workflow("a1", "daily-brief", "schedule", "0 0 9 * * * *", None, None, None,
            Some(r#"[{"id":"pkg2"}]"#), None, false).unwrap();
        let rows = s.list_agent_workflows("a1").unwrap();
        assert_eq!(rows[0].trigger_config, "0 0 9 * * * *");
        assert!(activities(&s).contains("pkg2"));
    }
}

/// Structure on an employee: the department the owner claims from the package,
/// and a reporting line that can never close a loop.
#[cfg(test)]
mod structure_tests {
    use crate::Store;

    fn store() -> Store {
        let path =
            std::env::temp_dir().join(format!("nebo-structure-test-{}.db", uuid::Uuid::new_v4()));
        Store::new(&path.to_string_lossy()).expect("store")
    }

    fn seat(s: &Store, id: &str, name: &str) {
        s.create_agent(id, None, name, "", "", "{}", None, None)
            .expect("agent row");
    }

    /// Set only the structure, leaving every other field as it is — what the
    /// settings page's save amounts to.
    fn set_structure(
        s: &Store,
        id: &str,
        department: Option<&str>,
        reports_to: Option<&str>,
    ) -> Result<(), types::NeboError> {
        let a = s.get_agent(id).unwrap().unwrap();
        s.update_agent(
            id,
            &a.name,
            &a.description,
            &a.agent_md,
            &a.frontmatter,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            department,
            reports_to,
        )
    }

    /// A line is refused whether it closes the loop in one hop or in four, and
    /// the refusal names the loop rather than saying "invalid".
    #[test]
    fn a_reporting_line_can_never_close_a_loop() {
        let s = store();
        for (id, name) in [
            ("a", "Anna"),
            ("b", "Ben"),
            ("c", "Cara"),
            ("d", "Dev"),
            ("e", "Eve"),
        ] {
            seat(&s, id, name);
        }

        // Anna ← Ben ← Cara ← Dev ← Eve: a chain four deep.
        set_structure(&s, "b", None, Some("a")).unwrap();
        set_structure(&s, "c", None, Some("b")).unwrap();
        set_structure(&s, "d", None, Some("c")).unwrap();
        set_structure(&s, "e", None, Some("d")).unwrap();
        assert_eq!(
            s.manager_chain("e").unwrap(),
            vec![
                ("d".to_string(), "Dev".to_string()),
                ("c".to_string(), "Cara".to_string()),
                ("b".to_string(), "Ben".to_string()),
                ("a".to_string(), "Anna".to_string()),
            ]
        );

        // Depth 0: answering to yourself.
        let err = set_structure(&s, "a", None, Some("a")).unwrap_err().to_string();
        assert!(err.contains("cannot report to itself"), "{err}");

        // Depth 1: Ben already answers to Anna.
        let err = set_structure(&s, "a", None, Some("b")).unwrap_err().to_string();
        assert!(err.contains("Ben answers to Anna"), "{err}");

        // Depth 4: the far end of the chain, which is where a check that only
        // looks one hop up would let the loop through.
        let err = set_structure(&s, "a", None, Some("e")).unwrap_err().to_string();
        assert!(
            err.contains("Eve answers to Dev answers to Cara answers to Ben answers to Anna"),
            "the refusal must name the whole loop: {err}"
        );

        // And a loop that closes in the middle of the chain, not at the top.
        let err = set_structure(&s, "c", None, Some("e")).unwrap_err().to_string();
        assert!(err.contains("Eve answers to Dev answers to Cara"), "{err}");

        // Nothing was written by any refusal.
        assert_eq!(s.get_agent("a").unwrap().unwrap().reports_to, None);
        assert_eq!(
            s.get_agent("c").unwrap().unwrap().reports_to,
            Some("b".to_string())
        );

        // A line that does not close a loop is fine: Eve moves from the bottom
        // of the chain to answering to Anna directly.
        set_structure(&s, "e", None, Some("a")).unwrap();
        assert_eq!(
            s.get_agent("e").unwrap().unwrap().reports_to,
            Some("a".to_string())
        );

        // And the check is about the line, not the names: once Ben answers to
        // nobody, Anna may answer to Cara, which was refused a moment ago.
        set_structure(&s, "a", None, Some("c")).unwrap_err();
        set_structure(&s, "b", None, Some("")).unwrap();
        set_structure(&s, "a", None, Some("c")).unwrap();
        assert_eq!(
            s.get_agent("a").unwrap().unwrap().reports_to,
            Some("c".to_string())
        );
        set_structure(&s, "a", None, Some("")).unwrap();

        // Clearing a line puts the seat back under the owner.
        set_structure(&s, "e", None, Some("")).unwrap();
        assert_eq!(s.get_agent("e").unwrap().unwrap().reports_to, None);
        assert!(s.manager_chain("e").unwrap().is_empty());
    }

    /// A manager id that stopped resolving (the employee was deleted) reads as
    /// "answers to the owner" — never a walk that fails or hangs.
    #[test]
    fn a_deleted_manager_reads_as_answering_to_the_owner() {
        let s = store();
        seat(&s, "boss", "Boss");
        seat(&s, "report", "Report");
        set_structure(&s, "report", None, Some("boss")).unwrap();
        s.delete_agent("boss").unwrap();
        assert!(s.manager_chain("report").unwrap().is_empty());
        // The stale id is still on the row; nothing pretends it was cleaned up.
        assert_eq!(
            s.get_agent("report").unwrap().unwrap().reports_to,
            Some("boss".to_string())
        );
    }

    /// The same contract, applied to the seat's declaration, through the real
    /// sync path. The owner gives a packaged employee a capability and writes a
    /// question their trade needs; a package update lands; both survive, and
    /// something the owner never touched does get the update.
    #[test]
    fn the_owners_declaration_survives_a_package_sync() {
        let s = store();
        seat(&s, "bk", "Bookkeeper");

        // Install: what the package ships.
        let shipped = r#"{"requires": {"interfaces": ["ledger"]},
                          "inputs": [{"id": "finance.ap.mailbox", "key": "mailbox", "label": "Which mailbox?"}],
                          "ceiling": {"ledger.payment.apply": "approval"}}"#;
        s.sync_agent_content("bk", "# Bookkeeper", shipped).unwrap();

        // The owner authors on top of it. This is what the settings page's save
        // writes: the declaration, plus the baseline noting the package's word.
        let on_row: serde_json::Value =
            serde_json::from_str(&s.get_agent("bk").unwrap().unwrap().frontmatter).unwrap();
        let mut owned = on_row.clone();
        owned["requires"]["interfaces"] = serde_json::json!(["ledger", "mail"]);
        owned["inputs"] = serde_json::json!([
            {"id": "finance.ap.mailbox", "key": "mailbox", "label": "Which mailbox?"},
            {"id": "trade.permit_number", "key": "permit", "label": "What is your permit number?"}
        ]);
        crate::declaration::note_owner_edit(
            &mut owned,
            &on_row,
            crate::declaration::OWNER_AUTHORED_FIELDS,
        );
        let a = s.get_agent("bk").unwrap().unwrap();
        s.update_agent(
            "bk", &a.name, &a.description, &a.agent_md, &owned.to_string(),
            None, None, None, None, None, None, None, None, None, None,
        )
        .unwrap();

        // The package updates: it corrects a label, adds a question and a
        // ceiling operation, and has never heard of the owner's additions.
        let update = r#"{"requires": {"interfaces": ["ledger"]},
                         "inputs": [{"id": "finance.ap.mailbox", "key": "mailbox", "label": "Which mailbox do bills arrive in?"},
                                    {"id": "finance.ap.terms", "key": "terms", "label": "What terms do you offer?"}],
                         "ceiling": {"ledger.payment.apply": "approval", "ledger.invoice.send": "approval"}}"#;
        s.sync_agent_content("bk", "# Bookkeeper", update).unwrap();

        let fm: serde_json::Value =
            serde_json::from_str(&s.get_agent("bk").unwrap().unwrap().frontmatter).unwrap();

        // The owner's edits survived.
        assert!(
            fm["requires"]["interfaces"].as_array().unwrap().iter().any(|v| v == "mail"),
            "the capability the owner gave it survived: {}",
            fm["requires"]["interfaces"]
        );
        let ids: Vec<&str> = fm["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|q| q["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"trade.permit_number"), "the owner's question survived: {ids:?}");

        // And the update was not a no-op.
        assert!(ids.contains(&"finance.ap.terms"), "the package's new question arrived: {ids:?}");
        assert_eq!(
            fm["inputs"].as_array().unwrap().iter()
                .find(|q| q["id"] == "finance.ap.mailbox").unwrap()["label"],
            "Which mailbox do bills arrive in?",
            "a question the owner never edited took the correction"
        );
        assert_eq!(fm["ceiling"]["ledger.invoice.send"], "approval", "the new ceiling operation arrived");

        // Superseded, not deleted.
        assert_eq!(
            fm[crate::declaration::PACKAGE_BASELINE]["requires"]["interfaces"],
            serde_json::json!(["ledger"]),
            "the record still shows what the package said"
        );
    }

    /// The package declares a department, the owner moves the seat, and the
    /// next package sync leaves the owner's answer alone — the `name_locked`
    /// contract, applied to the department.
    #[test]
    fn the_owners_department_survives_a_package_sync() {
        let s = store();
        seat(&s, "bk", "Bookkeeper");

        // Install: the package's declared department, nothing owner-set yet.
        s.set_agent_department("bk", Some("finance")).unwrap();
        let a = s.get_agent("bk").unwrap().unwrap();
        assert_eq!(a.department.as_deref(), Some("finance"));
        assert_eq!(a.department_locked, 0);

        // A re-install before the owner touches it still tracks the package.
        s.set_agent_department("bk", Some("accounting")).unwrap();
        assert_eq!(
            s.get_agent("bk").unwrap().unwrap().department.as_deref(),
            Some("accounting")
        );

        // The owner moves the seat.
        set_structure(&s, "bk", Some("Back Office"), None).unwrap();
        let a = s.get_agent("bk").unwrap().unwrap();
        assert_eq!(a.department.as_deref(), Some("Back Office"));
        assert_eq!(a.department_locked, 1);

        // Every later sync — boot, an update, a re-install — leaves it.
        s.set_agent_department("bk", Some("accounting")).unwrap();
        s.set_agent_department("bk", Some("finance")).unwrap();
        s.set_agent_department("bk", None).unwrap();
        assert_eq!(
            s.get_agent("bk").unwrap().unwrap().department.as_deref(),
            Some("Back Office")
        );

        // A save that does not mention the department leaves it too.
        set_structure(&s, "bk", None, None).unwrap();
        assert_eq!(
            s.get_agent("bk").unwrap().unwrap().department.as_deref(),
            Some("Back Office")
        );

        // "No department" is an answer the owner can give, and it stays theirs.
        set_structure(&s, "bk", Some(""), None).unwrap();
        let a = s.get_agent("bk").unwrap().unwrap();
        assert_eq!(a.department, None);
        assert_eq!(a.department_locked, 1);
        s.set_agent_department("bk", Some("finance")).unwrap();
        assert_eq!(s.get_agent("bk").unwrap().unwrap().department, None);
    }
}
