use rusqlite::params;

use crate::models::{CommanderEdge, CommanderNodePosition, CommanderTeam, CommanderTeamMember};
use crate::Store;
use types::NeboError;

fn row_to_team(row: &rusqlite::Row) -> rusqlite::Result<CommanderTeam> {
    Ok(CommanderTeam {
        id: row.get(0)?,
        name: row.get(1)?,
        color: row.get(2)?,
        position_x: row.get(3)?,
        position_y: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn row_to_member(row: &rusqlite::Row) -> rusqlite::Result<CommanderTeamMember> {
    Ok(CommanderTeamMember {
        team_id: row.get(0)?,
        agent_id: row.get(1)?,
    })
}

fn row_to_position(row: &rusqlite::Row) -> rusqlite::Result<CommanderNodePosition> {
    Ok(CommanderNodePosition {
        node_id: row.get(0)?,
        position_x: row.get(1)?,
        position_y: row.get(2)?,
    })
}

fn row_to_edge(row: &rusqlite::Row) -> rusqlite::Result<CommanderEdge> {
    Ok(CommanderEdge {
        id: row.get(0)?,
        source_node_id: row.get(1)?,
        target_node_id: row.get(2)?,
        edge_type: row.get(3)?,
        label: row.get(4)?,
        created_at: row.get(5)?,
    })
}

impl Store {
    // ── Teams ──────────────────────────────────────────────────────────

    pub fn list_commander_teams(&self) -> Result<Vec<CommanderTeam>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, color, position_x, position_y, created_at, updated_at
                 FROM commander_teams ORDER BY created_at",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_team)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn create_commander_team(
        &self,
        id: &str,
        name: &str,
        color: &str,
    ) -> Result<CommanderTeam, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO commander_teams (id, name, color)
             VALUES (?1, ?2, ?3)
             RETURNING id, name, color, position_x, position_y, created_at, updated_at",
            params![id, name, color],
            row_to_team,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn update_commander_team(
        &self,
        id: &str,
        name: &str,
        color: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE commander_teams SET name = ?1, color = ?2, updated_at = unixepoch() WHERE id = ?3",
            params![name, color, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_commander_team_position(
        &self,
        id: &str,
        x: f64,
        y: f64,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE commander_teams SET position_x = ?1, position_y = ?2, updated_at = unixepoch() WHERE id = ?3",
            params![x, y, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_commander_team(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM commander_teams WHERE id = ?1", params![id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    // ── Team Members ──────────────────────────────────────────────────

    pub fn list_commander_team_members(&self) -> Result<Vec<CommanderTeamMember>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT team_id, agent_id FROM commander_team_members")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_member)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn set_commander_team_members(
        &self,
        team_id: &str,
        agent_ids: &[String],
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM commander_team_members WHERE team_id = ?1",
            params![team_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        for agent_id in agent_ids {
            conn.execute(
                "INSERT INTO commander_team_members (team_id, agent_id) VALUES (?1, ?2)",
                params![team_id, agent_id],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        Ok(())
    }

    // ── Node Positions ────────────────────────────────────────────────

    pub fn list_commander_node_positions(&self) -> Result<Vec<CommanderNodePosition>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT node_id, position_x, position_y FROM commander_node_positions")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_position)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn save_commander_node_positions(
        &self,
        positions: &[(String, f64, f64)],
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        for (node_id, x, y) in positions {
            conn.execute(
                "INSERT INTO commander_node_positions (node_id, position_x, position_y)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(node_id) DO UPDATE SET position_x = ?2, position_y = ?3, updated_at = unixepoch()",
                params![node_id, x, y],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        Ok(())
    }

    // ── Edges (user-drawn reporting/coordination links) ───────────────

    pub fn list_commander_edges(&self) -> Result<Vec<CommanderEdge>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, source_node_id, target_node_id, edge_type, label, created_at
                 FROM commander_edges ORDER BY created_at",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_edge)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn create_commander_edge(
        &self,
        id: &str,
        source_node_id: &str,
        target_node_id: &str,
        edge_type: &str,
        label: &str,
    ) -> Result<CommanderEdge, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO commander_edges (id, source_node_id, target_node_id, edge_type, label)
             VALUES (?1, ?2, ?3, ?4, ?5)
             RETURNING id, source_node_id, target_node_id, edge_type, label, created_at",
            params![id, source_node_id, target_node_id, edge_type, label],
            row_to_edge,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn delete_commander_edge(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM commander_edges WHERE id = ?1", params![id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}
