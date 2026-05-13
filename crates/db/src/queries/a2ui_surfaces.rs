use rusqlite::params;

use crate::OptionalExt;
use crate::Store;
use crate::models::A2UISurface;
use types::NeboError;

fn row_to_surface(row: &rusqlite::Row) -> rusqlite::Result<A2UISurface> {
    Ok(A2UISurface {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        view_id: row.get(2)?,
        surface_type: row.get(3)?,
        components: row.get(4)?,
        data_model: row.get(5)?,
        window_geometry: row.get(6)?,
        is_active: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

impl Store {
    pub fn get_a2ui_surface(&self, id: &str) -> Result<Option<A2UISurface>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, agent_id, view_id, surface_type, components, data_model,
                    window_geometry, is_active, created_at, updated_at
             FROM a2ui_surfaces WHERE id = ?1",
            params![id],
            row_to_surface,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn list_a2ui_surfaces_by_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<A2UISurface>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, agent_id, view_id, surface_type, components, data_model,
                        window_geometry, is_active, created_at, updated_at
                 FROM a2ui_surfaces WHERE agent_id = ?1 ORDER BY created_at",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![agent_id], row_to_surface)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn list_active_a2ui_surfaces(&self) -> Result<Vec<A2UISurface>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, agent_id, view_id, surface_type, components, data_model,
                        window_geometry, is_active, created_at, updated_at
                 FROM a2ui_surfaces WHERE is_active = 1 ORDER BY created_at",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_surface)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn upsert_a2ui_surface(
        &self,
        id: &str,
        agent_id: &str,
        view_id: &str,
        surface_type: &str,
        components: Option<&str>,
        data_model: Option<&str>,
    ) -> Result<A2UISurface, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO a2ui_surfaces (id, agent_id, view_id, surface_type, components, data_model)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                components = COALESCE(excluded.components, a2ui_surfaces.components),
                data_model = COALESCE(excluded.data_model, a2ui_surfaces.data_model),
                is_active = 1,
                updated_at = unixepoch()
             RETURNING id, agent_id, view_id, surface_type, components, data_model,
                       window_geometry, is_active, created_at, updated_at",
            params![id, agent_id, view_id, surface_type, components, data_model],
            row_to_surface,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn update_a2ui_surface_components(
        &self,
        id: &str,
        components: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE a2ui_surfaces SET components = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, components],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_a2ui_surface_data_model(
        &self,
        id: &str,
        data_model: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE a2ui_surfaces SET data_model = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, data_model],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_a2ui_surface_geometry(&self, id: &str, geometry: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE a2ui_surfaces SET window_geometry = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, geometry],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn deactivate_a2ui_surface(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE a2ui_surfaces SET is_active = 0, updated_at = unixepoch() WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_a2ui_surface(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM a2ui_surfaces WHERE id = ?1", params![id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}
