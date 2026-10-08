//! The intelligence packs the owner makes (`types::packs`). The built-in
//! Nebo AI pack is never a row: `list` puts it first.

use rusqlite::{OptionalExtension, params};

use crate::Store;
use types::NeboError;
use types::packs::{LevelEffort, Pack, PackLanes, PackLevels, nebo_ai};

fn db_err(e: impl std::fmt::Display) -> NeboError {
    NeboError::Database(e.to_string())
}

fn pack_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<Pack> {
    let json = |col: &str| row.get::<_, String>(col);
    Ok(Pack {
        id: row.get("id")?,
        name: row.get("name")?,
        levels: serde_json::from_str::<PackLevels>(&json("levels")?).unwrap_or_default(),
        fallback: row.get::<_, i64>("fallback")? != 0,
        built_in: false,
        route_through_janus: row.get::<_, i64>("route_through_janus")? != 0,
        lanes: serde_json::from_str::<PackLanes>(&json("lanes")?).unwrap_or_default(),
        level_effort: serde_json::from_str::<LevelEffort>(&json("level_effort")?).unwrap_or_default(),
    })
}

const COLUMNS: &str = "id, name, levels, fallback, route_through_janus, lanes, level_effort";

impl Store {
    /// Every pack: Nebo AI first, then the owner's by name.
    pub fn list_intelligence_packs(&self) -> Result<Vec<Pack>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!("SELECT {COLUMNS} FROM intelligence_packs ORDER BY name COLLATE NOCASE"))
            .map_err(db_err)?;
        let rows = stmt.query_map([], pack_of).map_err(db_err)?;
        let mut packs = vec![nebo_ai()];
        packs.extend(rows.collect::<Result<Vec<_>, _>>().map_err(db_err)?);
        Ok(packs)
    }

    /// One pack by id; the built-in for its id.
    pub fn get_intelligence_pack(&self, id: &str) -> Result<Option<Pack>, NeboError> {
        if id == types::packs::NEBO_AI_ID {
            return Ok(Some(nebo_ai()));
        }
        let conn = self.conn()?;
        conn.query_row(
            &format!("SELECT {COLUMNS} FROM intelligence_packs WHERE id = ?1"),
            params![id],
            pack_of,
        )
        .optional()
        .map_err(db_err)
    }

    /// Create or replace one of the owner's packs. The built-in is refused.
    pub fn save_intelligence_pack(&self, pack: &Pack) -> Result<(), NeboError> {
        if pack.id == types::packs::NEBO_AI_ID {
            return Err(NeboError::Validation("Nebo AI is built in and can't be changed.".into()));
        }
        let levels = serde_json::to_string(&pack.levels).map_err(db_err)?;
        let lanes = serde_json::to_string(&pack.lanes).map_err(db_err)?;
        let effort = serde_json::to_string(&pack.level_effort).map_err(db_err)?;
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO intelligence_packs (id, name, levels, fallback, route_through_janus, lanes, level_effort)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, levels = excluded.levels,
                 fallback = excluded.fallback, route_through_janus = excluded.route_through_janus,
                 lanes = excluded.lanes, level_effort = excluded.level_effort, updated_at = unixepoch()",
            params![pack.id, pack.name, levels, pack.fallback as i64, pack.route_through_janus as i64, lanes, effort],
        )
        .map_err(db_err)?;
        Ok(())
    }

    /// Remove one of the owner's packs; true when it existed. The built-in
    /// is refused.
    pub fn delete_intelligence_pack(&self, id: &str) -> Result<bool, NeboError> {
        if id == types::packs::NEBO_AI_ID {
            return Err(NeboError::Validation("Nebo AI is built in and can't be removed.".into()));
        }
        let conn = self.conn()?;
        let n = conn.execute("DELETE FROM intelligence_packs WHERE id = ?1", params![id]).map_err(db_err)?;
        Ok(n > 0)
    }
}

#[cfg(test)]
mod tests {
    use types::packs::{Effort, Pack, PackLevels, NEBO_AI_ID};

    fn store() -> (tempfile::TempDir, crate::Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap();
        (dir, store)
    }

    #[test]
    fn packs_are_saved_listed_after_nebo_ai_and_removed() {
        let (_d, store) = store();
        let mine = Pack {
            id: "p1".into(),
            name: "My Claude".into(),
            levels: PackLevels { medium: Some("anthropic/claude-sonnet".into()), ..Default::default() },
            fallback: true,
            built_in: false,
            route_through_janus: true,
            lanes: types::packs::PackLanes { heartbeat: Some(Effort::Instant), ..Default::default() },
            level_effort: Default::default(),
        };
        store.save_intelligence_pack(&mine).unwrap();
        let listed = store.list_intelligence_packs().unwrap();
        assert_eq!(listed[0].id, NEBO_AI_ID);
        assert!(listed[0].built_in);
        assert_eq!(listed[1], mine);
        let got = store.get_intelligence_pack("p1").unwrap().unwrap();
        assert_eq!(got.model_for(Effort::Max), Some("anthropic/claude-sonnet"));
        assert!(store.delete_intelligence_pack("p1").unwrap());
        assert!(store.get_intelligence_pack("p1").unwrap().is_none());
    }

    #[test]
    fn the_built_in_is_never_changed_or_removed() {
        let (_d, store) = store();
        let mut nebo = types::packs::nebo_ai();
        nebo.name = "Mine now".into();
        assert!(store.save_intelligence_pack(&nebo).is_err());
        assert!(store.delete_intelligence_pack(NEBO_AI_ID).is_err());
        assert_eq!(store.get_intelligence_pack(NEBO_AI_ID).unwrap().unwrap().name, "Nebo AI");
    }
}
