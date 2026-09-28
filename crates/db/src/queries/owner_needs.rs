use rusqlite::params;

use crate::{DbErrExt, Store};
use types::NeboError;

/// A need an employee stands on that the owner was told of (`owner_needs`).
#[derive(Debug, Clone, PartialEq)]
pub struct OwnerNeedRow {
    pub need_key: String,
    /// The Inbox item that told it.
    pub notice_id: String,
    /// Every duty (binding name) held on it, in the order they were held.
    pub duties: Vec<String>,
    /// What was installed and connected for the employee when it was told.
    pub basis: Vec<String>,
}

/// What telling the owner a need comes to ([`Store::tell_owner_need`]).
#[derive(Debug, Clone, PartialEq)]
pub enum ToldNeed {
    /// News: the row was made with the notice id handed in.
    New,
    /// Already told, and this duty is newly held on it: `duties` is every
    /// duty now held, for the item that told it (`notice_id`).
    Joined { notice_id: String, duties: Vec<String> },
    /// Already told, this duty included.
    Already,
}

fn strings(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
}

impl Store {
    /// The owner is about to be told `agent_id` stands on `need_key` for
    /// `duty`. Only the first of a need is news, whichever duty and source
    /// noticed it; each statement is one write, so two fires at once tell it
    /// once. A need is forgotten only when it is met
    /// ([`Store::forget_owner_need`]).
    pub fn tell_owner_need(
        &self,
        agent_id: &str,
        need_key: &str,
        duty: &str,
        notice_id: &str,
        basis: &[String],
        at: i64,
    ) -> Result<ToldNeed, NeboError> {
        let conn = self.conn()?;
        let basis = serde_json::to_string(basis).unwrap_or_else(|_| "[]".into());
        let made = conn
            .execute(
                "INSERT INTO owner_needs (agent_id, need_key, notice_id, duties, basis, told_at)
                 VALUES (?1, ?2, ?3, json_array(?4), ?5, ?6)
                 ON CONFLICT (agent_id, need_key) DO NOTHING",
                params![agent_id, need_key, notice_id, duty, basis, at],
            )
            .db_err("tell_owner_need")?;
        if made > 0 {
            return Ok(ToldNeed::New);
        }
        let joined = conn
            .execute(
                "UPDATE owner_needs SET duties = json_insert(duties, '$[#]', ?3)
                 WHERE agent_id = ?1 AND need_key = ?2
                   AND NOT EXISTS (SELECT 1 FROM json_each(duties) WHERE value = ?3)",
                params![agent_id, need_key, duty],
            )
            .db_err("tell_owner_need")?;
        if joined == 0 {
            return Ok(ToldNeed::Already);
        }
        let (notice_id, duties): (String, String) = conn
            .query_row(
                "SELECT notice_id, duties FROM owner_needs WHERE agent_id = ?1 AND need_key = ?2",
                params![agent_id, need_key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .db_err("tell_owner_need")?;
        Ok(ToldNeed::Joined { notice_id, duties: strings(&duties) })
    }

    /// Every need `agent_id` stands on that the owner was told of.
    pub fn owner_needs_of(&self, agent_id: &str) -> Result<Vec<OwnerNeedRow>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT need_key, notice_id, duties, basis FROM owner_needs
                 WHERE agent_id = ?1 ORDER BY told_at, need_key",
            )
            .db_err("owner_needs_of")?;
        let rows = stmt
            .query_map(params![agent_id], |r| {
                Ok(OwnerNeedRow {
                    need_key: r.get(0)?,
                    notice_id: r.get(1)?,
                    duties: strings(&r.get::<_, String>(2)?),
                    basis: strings(&r.get::<_, String>(3)?),
                })
            })
            .db_err("owner_needs_of")?;
        rows.collect::<Result<Vec<_>, _>>().db_err("owner_needs_of")
    }

    /// The need is met: forget it, so its return is news. True when this
    /// call forgot it (two at once resolve its item once).
    pub fn forget_owner_need(&self, agent_id: &str, need_key: &str) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let n = conn
            .execute(
                "DELETE FROM owner_needs WHERE agent_id = ?1 AND need_key = ?2",
                params![agent_id, need_key],
            )
            .db_err("forget_owner_need")?;
        Ok(n > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::ToldNeed;
    use crate::Store;

    fn store() -> Store {
        let path = std::env::temp_dir().join(format!("nebo-owner-needs-{}.db", uuid::Uuid::new_v4()));
        let s = Store::new(path.to_str().unwrap()).unwrap();
        s.create_agent("rcp", None, "Receptionist", "", "", "{}", None, None).unwrap();
        s
    }

    /// One row per employee per need: the first telling is news, a new duty
    /// joins it, a repeat is not; only forgetting it (the need met) makes
    /// its return news again, and only one of two forgets does it.
    #[test]
    fn a_need_is_told_once_per_employee_whatever_duty_holds_on_it() {
        let s = store();
        let basis = vec!["plugin:sheets".to_string()];
        assert_eq!(s.tell_owner_need("rcp", "capability:telephony", "answer-inbound", "need:1", &basis, 10).unwrap(), ToldNeed::New);
        assert_eq!(s.tell_owner_need("rcp", "capability:telephony", "answer-inbound", "need:2", &basis, 20).unwrap(), ToldNeed::Already);
        assert_eq!(
            s.tell_owner_need("rcp", "capability:telephony", "missed-sweep", "need:3", &[], 30).unwrap(),
            ToldNeed::Joined { notice_id: "need:1".into(), duties: vec!["answer-inbound".into(), "missed-sweep".into()] }
        );
        assert_eq!(s.tell_owner_need("rcp", "capability:telephony", "missed-sweep", "need:4", &[], 40).unwrap(), ToldNeed::Already);
        assert_eq!(s.tell_owner_need("rcp", "account:voiceline", "missed-sweep", "need:5", &[], 50).unwrap(), ToldNeed::New, "another need is news");
        let rows = s.owner_needs_of("rcp").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].notice_id.as_str(), rows[0].basis.clone()), ("need:1", basis), "the item and basis of the first telling are kept");

        assert!(s.forget_owner_need("rcp", "capability:telephony").unwrap());
        assert!(!s.forget_owner_need("rcp", "capability:telephony").unwrap(), "two forgets, one resolution");
        assert_eq!(s.tell_owner_need("rcp", "capability:telephony", "answer-inbound", "need:6", &[], 60).unwrap(), ToldNeed::New, "met, then back: news");

        // An employee let go takes its needs with it.
        s.conn_exec_for_test("DELETE FROM agents WHERE id = 'rcp'");
        assert!(s.owner_needs_of("rcp").unwrap().is_empty());
    }
}
