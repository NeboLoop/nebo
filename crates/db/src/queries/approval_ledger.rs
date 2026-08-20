//! The approval ledger — the evidence that earns and revokes autonomy.
//!
//! Every gated-operation decision is appended here at resolution time. The
//! summary answers the graduation question directly: how long is the current
//! clean streak, across how many distinct assets, over how many separate
//! review sessions — because "30 approvals" from one asset in one sitting is
//! one experiment repeated thirty times, not thirty pieces of evidence.

use rusqlite::params;

use crate::Store;
use types::NeboError;

/// One ledger row, as recorded at decision time.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalEntry {
    pub id: i64,
    pub agent_id: String,
    pub operation: String,
    pub kind: String,
    pub edited: bool,
    pub run_id: String,
    pub binding: String,
    pub asset: String,
    pub display: String,
    pub decided_at: String,
}

/// The graduation evidence for one (agent, operation) pair.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalSummary {
    pub operation: String,
    /// Consecutive clean approvals since the last denial, edit, or rollback.
    /// This is THE graduation counter; anything negative resets it to zero.
    pub clean_streak: i64,
    /// Distinct assets within the current clean streak.
    pub streak_assets: i64,
    /// Distinct review sessions within the current clean streak. A session
    /// is a cluster of decisions separated by more than an hour — the spread
    /// requirement that stops one sitting from buying autonomy.
    pub streak_sessions: i64,
    pub total_approved: i64,
    pub total_denied: i64,
    pub total_rollbacks: i64,
    pub last_decided_at: Option<String>,
}

impl Store {
    /// Append a decision. Never updates — a decision is a historical fact.
    #[allow(clippy::too_many_arguments)]
    pub fn record_approval_decision(
        &self,
        agent_id: &str,
        operation: &str,
        kind: &str,
        edited: bool,
        run_id: &str,
        binding: &str,
        asset: &str,
        display: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO approval_ledger
             (agent_id, operation, kind, edited, run_id, binding, asset, display)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![agent_id, operation, kind, edited as i64, run_id, binding, asset, display],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// The recent ledger for one agent+operation, newest first.
    pub fn approval_ledger(
        &self,
        agent_id: &str,
        operation: &str,
        limit: i64,
    ) -> Result<Vec<ApprovalEntry>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, agent_id, operation, kind, edited, run_id, binding, asset, display, decided_at
                 FROM approval_ledger
                 WHERE agent_id = ?1 AND operation = ?2
                 ORDER BY id DESC LIMIT ?3",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![agent_id, operation, limit], row_to_entry)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Graduation evidence for every operation this agent has ledger rows for.
    pub fn approval_summaries(&self, agent_id: &str) -> Result<Vec<ApprovalSummary>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, agent_id, operation, kind, edited, run_id, binding, asset, display, decided_at
                 FROM approval_ledger WHERE agent_id = ?1 ORDER BY operation, id ASC",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![agent_id], row_to_entry)
            .map_err(|e| NeboError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))?;

        let mut out: Vec<ApprovalSummary> = Vec::new();
        let mut i = 0;
        while i < rows.len() {
            let op = rows[i].operation.clone();
            let mut j = i;
            while j < rows.len() && rows[j].operation == op {
                j += 1;
            }
            out.push(summarize(&op, &rows[i..j]));
            i = j;
        }
        Ok(out)
    }
}

/// Compute the graduation evidence from one operation's rows (oldest first).
///
/// Free function rather than a Store method so the streak semantics are
/// testable without a database — the rules here ARE the autonomy policy.
pub fn summarize(operation: &str, rows: &[ApprovalEntry]) -> ApprovalSummary {
    let mut total_approved = 0i64;
    let mut total_denied = 0i64;
    let mut total_rollbacks = 0i64;
    for r in rows {
        match r.kind.as_str() {
            "approved" => total_approved += 1,
            "denied" => total_denied += 1,
            "rollback" => total_rollbacks += 1,
            _ => {}
        }
    }

    // The clean streak runs backward from the newest row until anything
    // negative: a denial, a rollback, or an approval the human had to edit.
    let mut streak: Vec<&ApprovalEntry> = Vec::new();
    for r in rows.iter().rev() {
        let clean = r.kind == "approved" && !r.edited;
        if !clean {
            break;
        }
        streak.push(r);
    }

    let mut assets = std::collections::HashSet::new();
    for r in &streak {
        if !r.asset.is_empty() {
            assets.insert(r.asset.as_str());
        }
    }

    // Sessions: decisions more than an hour apart are separate sittings.
    // Timestamps are SQLite 'YYYY-MM-DD HH:MM:SS' UTC strings, which sort
    // and diff lexically only per-field, so parse to epoch minutes crudely.
    let mut sessions = 0i64;
    let mut last: Option<i64> = None;
    for r in streak.iter().rev() {
        let t = minutes(&r.decided_at);
        match (t, last) {
            (Some(t), Some(prev)) if t - prev <= 60 => {}
            (Some(_), _) | (None, None) => sessions += 1,
            (None, Some(_)) => {}
        }
        if t.is_some() {
            last = t;
        }
    }

    ApprovalSummary {
        operation: operation.to_string(),
        clean_streak: streak.len() as i64,
        streak_assets: assets.len() as i64,
        streak_sessions: sessions,
        total_approved,
        total_denied,
        total_rollbacks,
        last_decided_at: rows.last().map(|r| r.decided_at.clone()),
    }
}

/// Parse 'YYYY-MM-DD HH:MM:SS' into minutes-since-epoch-ish (good enough for
/// gap detection; days are approximated at 31 — a gap is a gap).
fn minutes(ts: &str) -> Option<i64> {
    let b = ts.as_bytes();
    if b.len() < 16 {
        return None;
    }
    let num = |s: &str| s.parse::<i64>().ok();
    let (y, mo, d) = (num(ts.get(0..4)?)?, num(ts.get(5..7)?)?, num(ts.get(8..10)?)?);
    let (h, mi) = (num(ts.get(11..13)?)?, num(ts.get(14..16)?)?);
    Some((((y * 12 + mo) * 31 + d) * 24 + h) * 60 + mi)
}

fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<ApprovalEntry> {
    Ok(ApprovalEntry {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        operation: row.get(2)?,
        kind: row.get(3)?,
        edited: row.get::<_, i64>(4)? != 0,
        run_id: row.get(5)?,
        binding: row.get(6)?,
        asset: row.get(7)?,
        display: row.get(8)?,
        decided_at: row.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: &str, edited: bool, asset: &str, decided_at: &str) -> ApprovalEntry {
        ApprovalEntry {
            id: 0,
            agent_id: "a".into(),
            operation: "cms.page.create".into(),
            kind: kind.into(),
            edited,
            run_id: String::new(),
            binding: String::new(),
            asset: asset.into(),
            display: String::new(),
            decided_at: decided_at.into(),
        }
    }

    // A denial resets the streak — everything after it counts, nothing
    // before it does. This is the rule that makes graduation evidence-based.
    #[test]
    fn denial_resets_the_streak() {
        let rows = vec![
            entry("approved", false, "asset-1", "2026-08-01 10:00:00"),
            entry("denied", false, "asset-1", "2026-08-02 10:00:00"),
            entry("approved", false, "asset-2", "2026-08-03 10:00:00"),
            entry("approved", false, "asset-3", "2026-08-04 10:00:00"),
        ];
        let s = summarize("cms.page.create", &rows);
        assert_eq!(s.clean_streak, 2);
        assert_eq!(s.streak_assets, 2);
        assert_eq!(s.total_denied, 1);
    }

    // A rollback is the strongest negative signal: it resets the streak even
    // though the original decision was an approval.
    #[test]
    fn rollback_resets_the_streak() {
        let rows = vec![
            entry("approved", false, "asset-1", "2026-08-01 10:00:00"),
            entry("approved", false, "asset-1", "2026-08-01 10:05:00"),
            entry("rollback", false, "asset-1", "2026-08-05 10:00:00"),
        ];
        let s = summarize("cms.page.create", &rows);
        assert_eq!(s.clean_streak, 0);
        assert_eq!(s.total_rollbacks, 1);
    }

    // An edited approval is not a clean approval: the human had to fix the
    // work, and "I keep correcting it" is evidence against autonomy.
    #[test]
    fn edited_approval_breaks_the_streak() {
        let rows = vec![
            entry("approved", false, "asset-1", "2026-08-01 10:00:00"),
            entry("approved", true, "asset-1", "2026-08-02 10:00:00"),
            entry("approved", false, "asset-2", "2026-08-03 10:00:00"),
        ];
        let s = summarize("cms.page.create", &rows);
        assert_eq!(s.clean_streak, 1);
    }

    // Decisions within an hour are one sitting; a later cluster is another.
    // One marathon session on one asset must read as sessions=1, assets=1 —
    // visibly short of any graduation threshold.
    #[test]
    fn sessions_cluster_by_hour_gaps() {
        let rows = vec![
            entry("approved", false, "asset-1", "2026-08-01 10:00:00"),
            entry("approved", false, "asset-1", "2026-08-01 10:20:00"),
            entry("approved", false, "asset-1", "2026-08-01 10:40:00"),
            entry("approved", false, "asset-2", "2026-08-02 09:00:00"),
            entry("approved", false, "asset-3", "2026-08-02 09:30:00"),
            entry("approved", false, "asset-4", "2026-08-03 15:00:00"),
        ];
        let s = summarize("cms.page.create", &rows);
        assert_eq!(s.clean_streak, 6);
        assert_eq!(s.streak_assets, 4);
        assert_eq!(s.streak_sessions, 3);
    }
}
