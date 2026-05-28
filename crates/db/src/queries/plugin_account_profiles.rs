use rusqlite::params;

use crate::Store;
use types::NeboError;

/// A per-agent plugin account profile. Maps an (agent, plugin, account) to the
/// config directory the plugin uses for that account's credentials. The plugin
/// owns the actual tokens inside `config_dir`; this row is just the mapping.
#[derive(Debug, Clone)]
pub struct PluginAccountProfile {
    pub id: String,
    pub agent_id: String,
    pub plugin_slug: String,
    pub account_label: String,
    pub config_dir: String,
    pub is_primary: bool,
}

impl Store {
    /// List all account profiles an agent has for a plugin.
    pub fn list_plugin_account_profiles(
        &self,
        agent_id: &str,
        plugin_slug: &str,
    ) -> Result<Vec<PluginAccountProfile>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, agent_id, plugin_slug, account_label, config_dir, is_primary
                 FROM plugin_account_profiles
                 WHERE agent_id = ?1 AND plugin_slug = ?2
                 ORDER BY is_primary DESC, account_label ASC",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![agent_id, plugin_slug], row_to_profile)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| NeboError::Database(e.to_string()))?);
        }
        Ok(out)
    }

    /// List every account profile an agent has across all plugins, ordered by
    /// plugin so callers can group rows by `plugin_slug`.
    pub fn list_all_plugin_account_profiles_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<PluginAccountProfile>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, agent_id, plugin_slug, account_label, config_dir, is_primary
                 FROM plugin_account_profiles
                 WHERE agent_id = ?1
                 ORDER BY plugin_slug ASC, is_primary DESC, account_label ASC",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![agent_id], row_to_profile)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| NeboError::Database(e.to_string()))?);
        }
        Ok(out)
    }

    /// Resolve a specific account for an agent+plugin. When `account_label` is
    /// `None`, returns the primary (or the only) profile. Returns `None` if the
    /// agent has no profile for this plugin (caller falls back to global creds).
    pub fn resolve_plugin_account_profile(
        &self,
        agent_id: &str,
        plugin_slug: &str,
        account_label: Option<&str>,
    ) -> Result<Option<PluginAccountProfile>, NeboError> {
        let profiles = self.list_plugin_account_profiles(agent_id, plugin_slug)?;
        if let Some(label) = account_label {
            return Ok(profiles.into_iter().find(|p| p.account_label == label));
        }
        // No explicit account: prefer the primary, else the first (list is
        // ordered primary-first), else none.
        Ok(profiles.into_iter().next())
    }

    /// Insert or update an account profile. If this profile is the agent's
    /// first for the plugin, it is marked primary automatically.
    pub fn upsert_plugin_account_profile(
        &self,
        id: &str,
        agent_id: &str,
        plugin_slug: &str,
        account_label: &str,
        config_dir: &str,
    ) -> Result<(), NeboError> {
        let existing = self.list_plugin_account_profiles(agent_id, plugin_slug)?;
        let is_primary = existing.is_empty();
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO plugin_account_profiles
                 (id, agent_id, plugin_slug, account_label, config_dir, is_primary)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(agent_id, plugin_slug, account_label)
             DO UPDATE SET config_dir = excluded.config_dir, updated_at = unixepoch()",
            params![id, agent_id, plugin_slug, account_label, config_dir, is_primary as i32],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Mark one account as the agent's primary for a plugin (clears the others).
    pub fn set_primary_plugin_account(
        &self,
        agent_id: &str,
        plugin_slug: &str,
        account_label: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE plugin_account_profiles SET is_primary = 0
             WHERE agent_id = ?1 AND plugin_slug = ?2",
            params![agent_id, plugin_slug],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        conn.execute(
            "UPDATE plugin_account_profiles SET is_primary = 1, updated_at = unixepoch()
             WHERE agent_id = ?1 AND plugin_slug = ?2 AND account_label = ?3",
            params![agent_id, plugin_slug, account_label],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Remove an account profile.
    pub fn delete_plugin_account_profile(
        &self,
        agent_id: &str,
        plugin_slug: &str,
        account_label: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM plugin_account_profiles
             WHERE agent_id = ?1 AND plugin_slug = ?2 AND account_label = ?3",
            params![agent_id, plugin_slug, account_label],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}

fn row_to_profile(row: &rusqlite::Row) -> rusqlite::Result<PluginAccountProfile> {
    Ok(PluginAccountProfile {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        plugin_slug: row.get(2)?,
        account_label: row.get(3)?,
        config_dir: row.get(4)?,
        is_primary: row.get::<_, i32>(5)? != 0,
    })
}
