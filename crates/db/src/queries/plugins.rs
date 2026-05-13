use rusqlite::params;

use crate::Store;
use crate::models::{PluginRegistry, PluginSetting};
use types::NeboError;

impl Store {
    /// List all .napp-installed plugins (slug is non-empty).
    pub fn list_installed_plugins(&self) -> Result<Vec<PluginRegistry>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, plugin_type, display_name, description, icon, version,
                        is_enabled, is_installed, settings_manifest, connection_status,
                        last_connected_at, last_error, metadata, created_at, updated_at,
                        slug, author, source, binary_path, manifest_hash, signature_status
                 FROM plugin_registry WHERE slug != '' ORDER BY slug",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(PluginRegistry {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    plugin_type: row.get(2)?,
                    display_name: row.get(3)?,
                    description: row.get(4)?,
                    icon: row.get(5)?,
                    version: row.get(6)?,
                    is_enabled: row.get(7)?,
                    is_installed: row.get(8)?,
                    settings_manifest: row.get(9)?,
                    connection_status: row.get(10)?,
                    last_connected_at: row.get(11)?,
                    last_error: row.get(12)?,
                    metadata: row.get(13)?,
                    created_at: row.get(14)?,
                    updated_at: row.get(15)?,
                    slug: row.get(16)?,
                    author: row.get(17)?,
                    source: row.get(18)?,
                    binary_path: row.get(19)?,
                    manifest_hash: row.get(20)?,
                    signature_status: row.get(21)?,
                })
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;

        let mut plugins = Vec::new();
        for row in rows {
            plugins.push(row.map_err(|e| NeboError::Database(e.to_string()))?);
        }
        Ok(plugins)
    }

    /// Get a single installed plugin by slug.
    pub fn get_plugin_by_slug(&self, slug: &str) -> Result<Option<PluginRegistry>, NeboError> {
        let conn = self.conn()?;
        match conn.query_row(
            "SELECT id, name, plugin_type, display_name, description, icon, version,
                    is_enabled, is_installed, settings_manifest, connection_status,
                    last_connected_at, last_error, metadata, created_at, updated_at,
                    slug, author, source, binary_path, manifest_hash, signature_status
             FROM plugin_registry WHERE slug = ?1",
            params![slug],
            |row| {
                Ok(PluginRegistry {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    plugin_type: row.get(2)?,
                    display_name: row.get(3)?,
                    description: row.get(4)?,
                    icon: row.get(5)?,
                    version: row.get(6)?,
                    is_enabled: row.get(7)?,
                    is_installed: row.get(8)?,
                    settings_manifest: row.get(9)?,
                    connection_status: row.get(10)?,
                    last_connected_at: row.get(11)?,
                    last_error: row.get(12)?,
                    metadata: row.get(13)?,
                    created_at: row.get(14)?,
                    updated_at: row.get(15)?,
                    slug: row.get(16)?,
                    author: row.get(17)?,
                    source: row.get(18)?,
                    binary_path: row.get(19)?,
                    manifest_hash: row.get(20)?,
                    signature_status: row.get(21)?,
                })
            },
        ) {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    /// Upsert an installed .napp plugin into the registry.
    /// Uses slug as the conflict key.
    pub fn upsert_installed_plugin(
        &self,
        slug: &str,
        name: &str,
        version: &str,
        author: &str,
        binary_path: &str,
        manifest_hash: &str,
        signature_status: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let now = chrono::Utc::now().timestamp();
        let id = format!("napp-{slug}");

        conn.execute(
            "INSERT INTO plugin_registry (
                id, name, plugin_type, display_name, description, icon, version,
                is_enabled, is_installed, settings_manifest, connection_status,
                metadata, created_at, updated_at,
                slug, author, source, binary_path, manifest_hash, signature_status
            ) VALUES (
                ?1, ?2, 'napp', ?3, '', '', ?4,
                1, 1, '{}', 'connected',
                '{}', ?5, ?5,
                ?6, ?7, 'installed', ?8, ?9, ?10
            ) ON CONFLICT(slug) WHERE slug != '' DO UPDATE SET
                name = excluded.name,
                display_name = excluded.display_name,
                version = excluded.version,
                is_installed = 1,
                binary_path = excluded.binary_path,
                manifest_hash = excluded.manifest_hash,
                signature_status = excluded.signature_status,
                author = excluded.author,
                updated_at = excluded.updated_at",
            params![
                id,
                slug,
                name,
                version,
                now,
                slug,
                author,
                binary_path,
                manifest_hash,
                signature_status
            ],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;

        Ok(())
    }

    /// Delete an installed plugin from the registry.
    pub fn delete_installed_plugin(&self, slug: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM plugin_registry WHERE slug = ?1", params![slug])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Enable or disable a plugin.
    pub fn set_plugin_enabled(&self, slug: &str, enabled: bool) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "UPDATE plugin_registry SET is_enabled = ?1, updated_at = ?2 WHERE slug = ?3",
            params![enabled as i64, now, slug],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// List all settings for a plugin by its slug (not id).
    pub fn list_plugin_settings_by_slug(
        &self,
        slug: &str,
    ) -> Result<Vec<PluginSetting>, NeboError> {
        let conn = self.conn()?;
        let plugin_id = format!("napp-{slug}");
        let mut stmt = conn
            .prepare(
                "SELECT id, plugin_id, setting_key, setting_value, is_secret, created_at, updated_at
                 FROM plugin_settings WHERE plugin_id = ?1 ORDER BY setting_key",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(params![plugin_id], |row| {
                Ok(PluginSetting {
                    id: row.get(0)?,
                    plugin_id: row.get(1)?,
                    setting_key: row.get(2)?,
                    setting_value: row.get(3)?,
                    is_secret: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;

        let mut settings = Vec::new();
        for row in rows {
            settings.push(row.map_err(|e| NeboError::Database(e.to_string()))?);
        }
        Ok(settings)
    }

    /// Upsert a plugin setting.
    pub fn upsert_plugin_setting_by_slug(
        &self,
        slug: &str,
        key: &str,
        value: &str,
        is_secret: bool,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let now = chrono::Utc::now().timestamp();
        let plugin_id = format!("napp-{slug}");
        let id = format!("{plugin_id}:{key}");

        conn.execute(
            "INSERT INTO plugin_settings (id, plugin_id, setting_key, setting_value, is_secret, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
             ON CONFLICT(plugin_id, setting_key) DO UPDATE SET
                setting_value = excluded.setting_value,
                is_secret = excluded.is_secret,
                updated_at = excluded.updated_at",
            params![id, plugin_id, key, value, is_secret as i64, now],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;

        Ok(())
    }
}
