use rusqlite::params;

use crate::models::ProviderModel;
use crate::Store;
use types::NeboError;

impl Store {
    pub fn list_all_provider_models(&self) -> Result<Vec<ProviderModel>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM provider_models ORDER BY provider, display_name")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_provider_model)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn list_provider_models(&self, provider: &str) -> Result<Vec<ProviderModel>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM provider_models WHERE provider = ?1 ORDER BY display_name")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![provider], row_to_provider_model)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn list_active_provider_models(
        &self,
        provider: &str,
    ) -> Result<Vec<ProviderModel>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM provider_models WHERE provider = ?1 AND is_active = 1 ORDER BY display_name",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![provider], row_to_provider_model)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_default_provider_model(
        &self,
        provider: &str,
    ) -> Result<Option<ProviderModel>, NeboError> {
        let conn = self.conn()?;
        match conn.query_row(
            "SELECT * FROM provider_models WHERE provider = ?1 AND is_default = 1 LIMIT 1",
            params![provider],
            row_to_provider_model,
        ) {
            Ok(m) => Ok(Some(m)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    pub fn get_provider_model(&self, id: &str) -> Result<Option<ProviderModel>, NeboError> {
        let conn = self.conn()?;
        match conn.query_row(
            "SELECT * FROM provider_models WHERE id = ?1",
            params![id],
            row_to_provider_model,
        ) {
            Ok(m) => Ok(Some(m)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    pub fn get_provider_model_by_model_id(
        &self,
        provider: &str,
        model_id: &str,
    ) -> Result<Option<ProviderModel>, NeboError> {
        let conn = self.conn()?;
        match conn.query_row(
            "SELECT * FROM provider_models WHERE provider = ?1 AND model_id = ?2",
            params![provider, model_id],
            row_to_provider_model,
        ) {
            Ok(m) => Ok(Some(m)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    pub fn create_provider_model(
        &self,
        id: &str,
        provider: &str,
        model_id: &str,
        display_name: &str,
        is_active: i64,
        is_default: i64,
        context_window: Option<i64>,
        input_price: Option<f64>,
        output_price: Option<f64>,
        capabilities: Option<&str>,
        kind: Option<&str>,
        preferred: i64,
        seeded_version: Option<&str>,
    ) -> Result<ProviderModel, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO provider_models (id, provider, model_id, display_name, is_active, is_default, context_window, input_price, output_price, capabilities, kind, preferred, seeded_version, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, unixepoch(), unixepoch()) RETURNING *",
            params![id, provider, model_id, display_name, is_active, is_default, context_window, input_price, output_price, capabilities, kind, preferred, seeded_version],
            row_to_provider_model,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Upsert a provider model: insert if not exists, update metadata if exists.
    /// Preserves user's is_active, is_default, and preferred choices.
    pub fn upsert_provider_model(
        &self,
        id: &str,
        provider: &str,
        model_id: &str,
        display_name: &str,
        context_window: Option<i64>,
        input_price: Option<f64>,
        output_price: Option<f64>,
        capabilities: Option<&str>,
        kind: Option<&str>,
        seeded_version: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO provider_models (id, provider, model_id, display_name, is_active, is_default, context_window, input_price, output_price, capabilities, kind, preferred, seeded_version, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 1, 0, ?5, ?6, ?7, ?8, ?9, 0, ?10, unixepoch(), unixepoch())
             ON CONFLICT(provider, model_id) DO UPDATE SET
                 display_name = excluded.display_name,
                 context_window = excluded.context_window,
                 input_price = excluded.input_price,
                 output_price = excluded.output_price,
                 capabilities = excluded.capabilities,
                 kind = excluded.kind,
                 seeded_version = excluded.seeded_version,
                 updated_at = unixepoch()",
            params![id, provider, model_id, display_name, context_window, input_price, output_price, capabilities, kind, seeded_version],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_provider_model_active(
        &self,
        id: &str,
        is_active: i64,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE provider_models SET is_active = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, is_active],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_provider_model_preferred(
        &self,
        id: &str,
        preferred: i64,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE provider_models SET preferred = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, preferred],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_default_provider_model(
        &self,
        id: &str,
        provider: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE provider_models SET is_default = CASE WHEN id = ?1 THEN 1 ELSE 0 END, updated_at = unixepoch() WHERE provider = ?2",
            params![id, provider],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_provider_model(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM provider_models WHERE id = ?1", params![id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_provider_models_by_provider(
        &self,
        provider: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM provider_models WHERE provider = ?1",
            params![provider],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Mark models as inactive if they were seeded but are no longer in the catalog.
    pub fn deactivate_stale_models(
        &self,
        provider: &str,
        active_model_ids: &[String],
    ) -> Result<(), NeboError> {
        if active_model_ids.is_empty() {
            return Ok(());
        }
        let conn = self.conn()?;
        // Build placeholders for the IN clause
        let placeholders: Vec<String> = (0..active_model_ids.len())
            .map(|i| format!("?{}", i + 2))
            .collect();
        let sql = format!(
            "UPDATE provider_models SET is_active = 0, updated_at = unixepoch() WHERE provider = ?1 AND model_id NOT IN ({}) AND seeded_version IS NOT NULL",
            placeholders.join(", ")
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        stmt.raw_bind_parameter(1, provider)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        for (i, model_id) in active_model_ids.iter().enumerate() {
            stmt.raw_bind_parameter(i + 2, model_id.as_str())
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        stmt.raw_execute()
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}

fn row_to_provider_model(row: &rusqlite::Row) -> rusqlite::Result<ProviderModel> {
    Ok(ProviderModel {
        id: row.get("id")?,
        provider: row.get("provider")?,
        model_id: row.get("model_id")?,
        display_name: row.get("display_name")?,
        is_active: row.get("is_active")?,
        is_default: row.get("is_default")?,
        context_window: row.get("context_window")?,
        input_price: row.get("input_price")?,
        output_price: row.get("output_price")?,
        capabilities: row.get("capabilities")?,
        kind: row.get("kind")?,
        preferred: row.get("preferred")?,
        seeded_version: row.get("seeded_version")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}
