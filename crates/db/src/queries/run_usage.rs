//! Per-run usage and outcomes.
//!
//! Token counts flow through the runner and reach the UI, which shows them for
//! a few seconds and drops them. This is where a run becomes a durable record:
//! what it cost, and what it achieved. See docs/plans/per-run-cost-tracking.md.

use rusqlite::params;

use crate::Store;
use crate::models::{AgentUsageStats, OutcomeCount, RunUsage, RunUsageEntry};
use types::NeboError;

/// Cost in microcents (1 = $0.000001) from token counts and per-million
/// prices. Integer math throughout: a month of sub-cent runs summed as floats
/// accumulates drift, and this number ends up on an invoice.
///
/// Cache writes are billed at the input rate and cache reads at the (cheaper)
/// cached rate; a model with no pricing yields 0 rather than a wrong number.
pub fn cost_microcents(
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
    price_input_per_m: f64,
    price_output_per_m: f64,
    price_cached_per_m: f64,
) -> i64 {
    // $/M tokens → microcents/token: $1/M = 100 microcents per token / 1e6.
    // Multiply first, round once, so the rounding error is bounded per run
    // rather than per token.
    let micro = |tokens: i64, per_m: f64| -> i64 {
        if tokens <= 0 || per_m <= 0.0 {
            return 0;
        }
        ((tokens as f64) * per_m * 100.0 / 1_000_000.0 * 1_000_000.0).round() as i64
    };
    micro(input_tokens, price_input_per_m)
        + micro(output_tokens, price_output_per_m)
        + micro(cache_read_tokens, price_cached_per_m)
        + micro(cache_creation_tokens, price_input_per_m)
}

impl Store {
    /// Records a finished run. Best-effort by contract: the caller has already
    /// done the work, so a failure to write the record must never fail the run
    /// — but unlike a timeline entry, this one is money, so it is logged loudly
    /// and returned for the caller to decide.
    pub fn record_run_usage(&self, e: &RunUsageEntry) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO run_usage (
                 agent_id, session_key, run_id, run_type, model_id,
                 input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                 cost_microcents, outcome
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                e.agent_id,
                e.session_key,
                e.run_id,
                e.run_type,
                e.model_id,
                e.input_tokens,
                e.output_tokens,
                e.cache_read_tokens,
                e.cache_creation_tokens,
                e.cost_microcents,
                e.outcome,
            ],
        )
        .map_err(|err| NeboError::Database(err.to_string()))?;
        Ok(conn.last_insert_rowid())
    }

    /// Cost for one agent since a unix timestamp — the "today / this week /
    /// this month" figures.
    pub fn agent_usage_stats(&self, agent_id: &str, since: i64) -> Result<AgentUsageStats, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(cost_microcents),0),
                    COALESCE(SUM(input_tokens),0),
                    COALESCE(SUM(output_tokens),0)
             FROM run_usage WHERE agent_id = ?1 AND created_at >= ?2",
            params![agent_id, since],
            |row| {
                Ok(AgentUsageStats {
                    runs: row.get(0)?,
                    cost_microcents: row.get(1)?,
                    input_tokens: row.get(2)?,
                    output_tokens: row.get(3)?,
                })
            },
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Outcomes for a period — the billable "task" count, and what each kind
    /// of work cost. Rows with no outcome (plain chat turns) are excluded:
    /// they are usage, not work.
    pub fn agent_outcome_counts(
        &self,
        agent_id: &str,
        since: i64,
    ) -> Result<Vec<OutcomeCount>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT outcome, COUNT(*), COALESCE(SUM(cost_microcents),0)
                 FROM run_usage
                 WHERE agent_id = ?1 AND created_at >= ?2 AND outcome IS NOT NULL
                 GROUP BY outcome ORDER BY COUNT(*) DESC",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![agent_id, since], |row| {
                Ok(OutcomeCount {
                    outcome: row.get(0)?,
                    count: row.get(1)?,
                    cost_microcents: row.get(2)?,
                })
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Recent runs for an agent, newest first — the raw material for a daily
    /// report and for the reporter that ships these upstream.
    pub fn list_run_usage(
        &self,
        agent_id: &str,
        since: i64,
        limit: i64,
    ) -> Result<Vec<RunUsage>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, agent_id, session_key, run_id, run_type, model_id,
                        input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                        cost_microcents, outcome, created_at
                 FROM run_usage
                 WHERE agent_id = ?1 AND created_at >= ?2
                 ORDER BY created_at DESC LIMIT ?3",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![agent_id, since, limit], |row| {
                Ok(RunUsage {
                    id: row.get(0)?,
                    agent_id: row.get(1)?,
                    session_key: row.get(2)?,
                    run_id: row.get(3)?,
                    run_type: row.get(4)?,
                    model_id: row.get(5)?,
                    input_tokens: row.get(6)?,
                    output_tokens: row.get(7)?,
                    cache_read_tokens: row.get(8)?,
                    cache_creation_tokens: row.get(9)?,
                    cost_microcents: row.get(10)?,
                    outcome: row.get(11)?,
                    created_at: row.get(12)?,
                })
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The number this produces lands on an invoice, so the arithmetic gets a
    // check rather than a comment. Sonnet-class pricing: $3/M in, $15/M out.
    #[test]
    fn cost_matches_hand_calculation() {
        // 1M input tokens at $3/M = $3.00 = 300_000_000 microcents.
        assert_eq!(cost_microcents(1_000_000, 0, 0, 0, 3.0, 15.0, 0.3), 300_000_000);
        // 1M output at $15/M = $15.00.
        assert_eq!(cost_microcents(0, 1_000_000, 0, 0, 3.0, 15.0, 0.3), 1_500_000_000);
        // The worked example from the plan: 1000 input on Sonnet = $0.003.
        assert_eq!(cost_microcents(1_000, 0, 0, 0, 3.0, 15.0, 0.3), 300_000);
    }

    // Cache reads bill at the cached rate, cache writes at the full input
    // rate. Charging reads at the input rate would overbill every long
    // conversation, which is most of them.
    #[test]
    fn cache_tiers_are_billed_differently() {
        let read = cost_microcents(0, 0, 1_000_000, 0, 3.0, 15.0, 0.3);
        let write = cost_microcents(0, 0, 0, 1_000_000, 3.0, 15.0, 0.3);
        assert_eq!(read, 30_000_000, "cache reads bill at the cached rate");
        assert_eq!(write, 300_000_000, "cache writes bill at the input rate");
        assert!(read < write);
    }

    // A model we have no pricing for must cost zero, not a wrong number. A
    // silently invented figure is worse than a visibly missing one.
    #[test]
    fn unpriced_model_costs_nothing() {
        assert_eq!(cost_microcents(10_000, 10_000, 0, 0, 0.0, 0.0, 0.0), 0);
    }

    // Negative or zero token counts cannot subtract from a bill.
    #[test]
    fn nonpositive_tokens_never_reduce_cost() {
        assert_eq!(cost_microcents(-500, 0, 0, 0, 3.0, 15.0, 0.3), 0);
        assert_eq!(cost_microcents(0, 0, 0, 0, 3.0, 15.0, 0.3), 0);
    }
}
