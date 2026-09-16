use rusqlite::params;

use crate::Store;
use types::NeboError;

/// Today's tallies for one rule key, as stored in `operation_counters`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OperationCounters {
    pub count: i64,
    pub cents: i64,
    pub counterparty_cents: i64,
}

/// The rule key for the company-wide daily totals.
pub const COMPANY_COUNTER_KEY: &str = "company";

impl Store {
    /// The constitution as stored: the CompanyPolicy JSON, or None when the
    /// owner has not written one.
    pub fn get_company_policy(&self) -> Result<Option<String>, NeboError> {
        let conn = self.conn()?;
        match conn.query_row("SELECT policy FROM company_policy WHERE id = 1", [], |row| {
            row.get::<_, String>(0)
        }) {
            Ok(s) => Ok(Some(s)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    /// Write the constitution wholesale. The only writer is the owner.
    pub fn set_company_policy(&self, policy_json: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO company_policy (id, policy, updated_at) VALUES (1, ?1, unixepoch())
             ON CONFLICT(id) DO UPDATE SET policy = excluded.policy, updated_at = excluded.updated_at",
            params![policy_json],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Today's tallies for a rule: its own totals (the '' counterparty row)
    /// plus the named counterparty's cents. Zero when nothing ran.
    pub fn day_counters(
        &self,
        rule_key: &str,
        day: &str,
        counterparty: &str,
    ) -> Result<OperationCounters, NeboError> {
        let conn = self.conn()?;
        let (count, cents): (i64, i64) = conn
            .query_row(
                "SELECT count, cents FROM operation_counters
                 WHERE rule_key = ?1 AND day = ?2 AND counterparty = ''",
                params![rule_key, day],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok((0, 0)),
                e => Err(e),
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let counterparty_cents: i64 = if counterparty.is_empty() {
            0
        } else {
            conn.query_row(
                "SELECT counterparty_cents FROM operation_counters
                 WHERE rule_key = ?1 AND day = ?2 AND counterparty = ?3",
                params![rule_key, day, counterparty],
                |row| row.get(0),
            )
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(0),
                e => Err(e),
            })
            .map_err(|e| NeboError::Database(e.to_string()))?
        };
        Ok(OperationCounters { count, cents, counterparty_cents })
    }

    /// Record one operation against a rule for the day: the rule's own row
    /// gains one count and the cents; the counterparty's row (when named)
    /// gains the cents. One statement per row, in one transaction, so a
    /// crash cannot half-record it.
    pub fn bump_counters(
        &self,
        rule_key: &str,
        day: &str,
        counterparty: &str,
        cents: i64,
    ) -> Result<(), NeboError> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction()
            .map_err(|e| NeboError::Database(e.to_string()))?;
        tx.execute(
            "INSERT INTO operation_counters (rule_key, day, counterparty, count, cents, counterparty_cents)
             VALUES (?1, ?2, '', 1, ?3, 0)
             ON CONFLICT(rule_key, day, counterparty)
             DO UPDATE SET count = count + 1, cents = cents + excluded.cents",
            params![rule_key, day, cents],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        if !counterparty.is_empty() {
            tx.execute(
                "INSERT INTO operation_counters (rule_key, day, counterparty, count, cents, counterparty_cents)
                 VALUES (?1, ?2, ?3, 0, 0, ?4)
                 ON CONFLICT(rule_key, day, counterparty)
                 DO UPDATE SET counterparty_cents = counterparty_cents + excluded.counterparty_cents",
                params![rule_key, day, counterparty, cents],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        tx.commit().map_err(|e| NeboError::Database(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nebo-company-policy-test.db");
        let store = Store::new(&path.to_string_lossy()).expect("store");
        (dir, store)
    }

    #[test]
    fn counters_tally_the_rule_and_the_counterparty_separately() {
        let (_dir, store) = store();
        assert_eq!(store.day_counters("a:x", "2026-09-12", "v1").unwrap(), OperationCounters::default());
        store.bump_counters("a:x", "2026-09-12", "v1", 500).unwrap();
        store.bump_counters("a:x", "2026-09-12", "v2", 700).unwrap();
        store.bump_counters("a:x", "2026-09-12", "", 100).unwrap();
        let c = store.day_counters("a:x", "2026-09-12", "v1").unwrap();
        assert_eq!(c, OperationCounters { count: 3, cents: 1300, counterparty_cents: 500 });
        // Another day starts at zero.
        assert_eq!(store.day_counters("a:x", "2026-09-13", "v1").unwrap(), OperationCounters::default());
    }

    #[test]
    fn the_constitution_is_one_row_written_wholesale() {
        let (_dir, store) = store();
        assert_eq!(store.get_company_policy().unwrap(), None);
        store.set_company_policy(r#"{"purpose":"a"}"#).unwrap();
        store.set_company_policy(r#"{"purpose":"b"}"#).unwrap();
        assert_eq!(store.get_company_policy().unwrap().as_deref(), Some(r#"{"purpose":"b"}"#));
    }
}
