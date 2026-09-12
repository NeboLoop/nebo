//! The General Manager's `authority` tool: standing authority for seats,
//! inside the constitution.
//!
//! It edits the ONE operation policy (`entity_config.operation_policy` on the
//! seat) through `OperationPolicy::apply_edit`, the same path the settings
//! page uses. A grant is refused when it exceeds the company policy, names a
//! reserved operation, or would change a locked rule (a ceiling or a law).

use std::sync::Arc;

use crate::origin::ToolContext;
use crate::policy::{Bounds, CompanyPolicy, OperationAccess, OperationPolicy, OperationRule};
use crate::registry::{DynTool, ToolResult};

pub struct AuthorityTool {
    store: Arc<db::Store>,
}

impl AuthorityTool {
    pub fn new(store: Arc<db::Store>) -> Self {
        Self { store }
    }

    fn seat(&self, agent: &str) -> Result<db::models::Agent, String> {
        if agent.is_empty() {
            return Err("`agent` (the seat's name or id) is required".into());
        }
        let found = self
            .store
            .get_agent(agent)
            .ok()
            .flatten()
            .or_else(|| self.store.get_agent_by_name(agent).ok().flatten())
            .or_else(|| self.store.get_agent_by_slug(agent).ok().flatten());
        found.ok_or_else(|| format!("no seat named '{agent}'"))
    }

    fn seat_policy(&self, agent_id: &str) -> OperationPolicy {
        let json = self
            .store
            .get_entity_config("agent", agent_id)
            .ok()
            .flatten()
            .and_then(|c| c.operation_policy);
        OperationPolicy::from_json(json.as_deref())
    }

    fn company(&self) -> CompanyPolicy {
        CompanyPolicy::from_json(self.store.get_company_policy().ok().flatten().as_deref())
    }

    fn save(&self, agent_id: &str, policy: &OperationPolicy) -> Result<(), String> {
        self.store
            .upsert_entity_config(
                "agent",
                agent_id,
                &serde_json::json!({ "operationPolicy": policy.to_json() }),
            )
            .map(|_| ())
            .map_err(|e| format!("saving the seat's policy: {e}"))
    }

    fn grant(&self, input: &serde_json::Value, widen: bool) -> ToolResult {
        let agent = input["agent"].as_str().unwrap_or("");
        let operation = input["operation"].as_str().unwrap_or("");
        if operation.is_empty() {
            return ToolResult::error("`operation` is required (e.g. ledger.billpayment.create)");
        }
        let evidence = input["evidence"].as_str().unwrap_or("").trim().to_string();
        if widen && evidence.is_empty() {
            return ToolResult::error(
                "widening needs evidence from the record: say what the seat did inside the current bound",
            );
        }
        let bounds: Bounds = match serde_json::from_value(input["bounds"].clone()) {
            Ok(b) => b,
            Err(e) => return ToolResult::error(format!("`bounds` is malformed: {e}")),
        };
        let seat = match self.seat(agent) {
            Ok(s) => s,
            Err(e) => return ToolResult::error(e),
        };
        let mut policy = self.seat_policy(&seat.id);
        let key = crate::plugin_tool::port_suffix(operation);
        if widen {
            let Some(existing) = policy.operations.get(&key).filter(|r| r.is_standing_grant()) else {
                return ToolResult::error(format!(
                    "{} holds no standing grant for {key}; grant first, widen later",
                    seat.name
                ));
            };
            if !bounds_wider(existing.bounds.as_ref().unwrap(), &bounds) {
                return ToolResult::error("widen must loosen at least one bound; to tighten, use narrow");
            }
        }
        let rule = OperationRule {
            access: OperationAccess::Always,
            bounds: Some(bounds),
            source: Some("general_manager".into()),
            evidence: (!evidence.is_empty()).then_some(evidence),
            granted_at: Some(chrono::Utc::now().timestamp()),
            locked: false,
        };
        if let Err(e) = self.company().permits(operation, &rule) {
            return ToolResult::error(format!(
                "refused: {e}. Only the owner can change the constitution."
            ));
        }
        if let Err(e) = policy.apply_edit(operation, rule.clone()) {
            return ToolResult::error(format!("refused: {e}"));
        }
        if let Err(e) = self.save(&seat.id, &policy) {
            return ToolResult::error(e);
        }
        ToolResult::ok(format!(
            "{} now runs {key} unattended inside {}. Recorded as standing authority granted by the General Manager{}.",
            seat.name,
            describe(rule.bounds.as_ref().unwrap()),
            rule.evidence.as_deref().map(|e| format!(" on: {e}")).unwrap_or_default()
        ))
    }

    fn narrow(&self, input: &serde_json::Value, suspend: bool) -> ToolResult {
        let agent = input["agent"].as_str().unwrap_or("");
        let operation = input["operation"].as_str().unwrap_or("");
        if operation.is_empty() {
            return ToolResult::error("`operation` is required");
        }
        let seat = match self.seat(agent) {
            Ok(s) => s,
            Err(e) => return ToolResult::error(e),
        };
        let mut policy = self.seat_policy(&seat.id);
        let key = crate::plugin_tool::port_suffix(operation);
        let Some(existing) = policy.operations.get(&key).cloned() else {
            return ToolResult::error(format!("{} holds no rule for {key}", seat.name));
        };
        if existing.locked {
            return ToolResult::error(format!("{key} is a ceiling or a law on {}; it cannot change", seat.name));
        }
        let reason = input["evidence"].as_str().unwrap_or("").trim().to_string();
        let rule = if suspend {
            OperationRule {
                access: OperationAccess::Approval,
                bounds: None,
                source: Some("general_manager".into()),
                evidence: (!reason.is_empty()).then_some(reason.clone()),
                granted_at: Some(chrono::Utc::now().timestamp()),
                locked: false,
            }
        } else {
            let bounds: Bounds = match serde_json::from_value(input["bounds"].clone()) {
                Ok(b) => b,
                Err(e) => return ToolResult::error(format!("`bounds` is malformed: {e}")),
            };
            if let Some(cur) = existing.bounds.as_ref() {
                if bounds_wider(cur, &bounds) {
                    return ToolResult::error("narrow must not loosen any bound; to loosen, use widen with evidence");
                }
            }
            OperationRule {
                access: OperationAccess::Always,
                bounds: Some(bounds),
                source: Some("general_manager".into()),
                evidence: (!reason.is_empty()).then_some(reason.clone()),
                granted_at: Some(chrono::Utc::now().timestamp()),
                locked: false,
            }
        };
        if let Err(e) = self.company().permits(operation, &rule) {
            return ToolResult::error(format!("refused: {e}"));
        }
        if let Err(e) = policy.apply_edit(operation, rule.clone()) {
            return ToolResult::error(format!("refused: {e}"));
        }
        if let Err(e) = self.save(&seat.id, &policy) {
            return ToolResult::error(e);
        }
        ToolResult::ok(if suspend {
            format!("{}'s standing authority for {key} is suspended; it asks until a new grant.", seat.name)
        } else {
            format!(
                "{} now runs {key} unattended inside {}.",
                seat.name,
                describe(rule.bounds.as_ref().unwrap())
            )
        })
    }

    fn list(&self, input: &serde_json::Value) -> ToolResult {
        let agent = input["agent"].as_str().unwrap_or("");
        let seat = match self.seat(agent) {
            Ok(s) => s,
            Err(e) => return ToolResult::error(e),
        };
        let policy = self.seat_policy(&seat.id);
        let mut lines: Vec<String> = policy
            .operations
            .iter()
            .map(|(op, r)| {
                let what = match r.access {
                    OperationAccess::Always => match r.bounds.as_ref() {
                        Some(b) => format!("unattended inside {}", describe(b)),
                        None => "unattended, unbounded (owner's setting)".to_string(),
                    },
                    OperationAccess::Approval => "asks".to_string(),
                    OperationAccess::Blocked => "blocked".to_string(),
                };
                format!(
                    "- {op}: {what}{}{}{}",
                    r.source.as_deref().map(|s| format!(" [{s}]")).unwrap_or_default(),
                    if r.locked { " (locked)" } else { "" },
                    r.evidence.as_deref().map(|e| format!(" — {e}")).unwrap_or_default()
                )
            })
            .collect();
        lines.sort();
        ToolResult::ok(format!(
            "{}: default {}\n{}",
            seat.name,
            policy.default.as_str(),
            if lines.is_empty() { "- no per-operation rules".to_string() } else { lines.join("\n") }
        ))
    }

    fn constitution(&self) -> ToolResult {
        match self.store.get_company_policy() {
            Ok(Some(json)) => {
                let c = CompanyPolicy::from_json(Some(&json));
                ToolResult::ok(format!(
                    "Purpose: {}\nCompany-wide per day: {}\nReserved to the owner: {}\nPer-operation: {}",
                    if c.purpose.is_empty() { "(unset)" } else { &c.purpose },
                    describe(&c.daily),
                    if c.reserved.is_empty() { "nothing".to_string() } else { c.reserved.join(", ") },
                    if c.operations.is_empty() {
                        "none".to_string()
                    } else {
                        let mut v: Vec<String> = c
                            .operations
                            .iter()
                            .map(|(op, r)| {
                                format!(
                                    "{op} {}{}",
                                    r.access.as_str(),
                                    r.bounds.as_ref().map(|b| format!(" inside {}", describe(b))).unwrap_or_default()
                                )
                            })
                            .collect();
                        v.sort();
                        v.join("; ")
                    }
                ))
            }
            Ok(None) => ToolResult::ok(
                "No constitution is written yet. Until the owner writes one, no standing authority can be granted.",
            ),
            Err(e) => ToolResult::error(format!("reading the constitution: {e}")),
        }
    }
}

/// True when `next` loosens at least one axis of `cur` (a larger number, or
/// a bound removed).
fn bounds_wider(cur: &Bounds, next: &Bounds) -> bool {
    fn looser(c: Option<i64>, n: Option<i64>) -> bool {
        match (c, n) {
            (Some(c), Some(n)) => n > c,
            (Some(_), None) => true,
            _ => false,
        }
    }
    looser(cur.max_amount_cents, next.max_amount_cents)
        || looser(cur.per_day_cents, next.per_day_cents)
        || looser(cur.per_day_count, next.per_day_count)
        || looser(cur.per_counterparty_day_cents, next.per_counterparty_day_cents)
        || (cur.counterparty_class.is_some() && next.counterparty_class.is_none())
}

fn describe(b: &Bounds) -> String {
    let mut parts = Vec::new();
    if let Some(v) = b.max_amount_cents {
        parts.push(format!("${:.2} per operation", v as f64 / 100.0));
    }
    if let Some(v) = b.per_day_cents {
        parts.push(format!("${:.2} per day", v as f64 / 100.0));
    }
    if let Some(v) = b.per_day_count {
        parts.push(format!("{v} per day"));
    }
    if let Some(v) = b.per_counterparty_day_cents {
        parts.push(format!("${:.2} per counterparty per day", v as f64 / 100.0));
    }
    if let Some(c) = b.counterparty_class.as_deref() {
        parts.push(format!("counterparties with a {c}"));
    }
    if let Some(s) = b.freshness_secs {
        parts.push(format!("policy fresh within {}h", s / 3600));
    }
    if parts.is_empty() { "no bounds".to_string() } else { parts.join(", ") }
}

impl DynTool for AuthorityTool {
    fn name(&self) -> &str {
        "authority"
    }

    fn description(&self) -> String {
        "Standing authority for seats, inside the constitution. The General Manager grants a seat \
         the right to run a gated operation unattended within bounds, widens it on evidence from the \
         record, narrows it on one bad outcome, or suspends it. Nothing here can exceed the \
         constitution or change a ceiling or a law; only the owner changes the constitution.\n\n\
         Resources:\n\
         - grant: grant | widen | narrow | suspend | list (agent, operation, bounds, evidence)\n\
         - constitution: show\n\n\
         bounds: {max_amount_cents, per_day_cents, per_day_count, per_counterparty_day_cents, \
         counterparty_class: \"ledger_id\", freshness_secs}\n\n\
         Examples:\n  \
         authority(resource: \"grant\", action: \"grant\", agent: \"Bookkeeper\", operation: \"ledger.billpayment.create\", \
         bounds: {max_amount_cents: 250000, per_day_count: 20, counterparty_class: \"ledger_id\"}, evidence: \"thirty clean days of approvals\")\n  \
         authority(resource: \"grant\", action: \"widen\", agent: \"Bookkeeper\", operation: \"ledger.billpayment.create\", \
         bounds: {max_amount_cents: 500000, per_day_count: 20}, evidence: \"41 invoices under $2,500 in 30 days, zero reversals\")\n  \
         authority(resource: \"grant\", action: \"narrow\", agent: \"Bookkeeper\", operation: \"ledger.billpayment.create\", bounds: {max_amount_cents: 100000}, evidence: \"one reversal on 2026-09-12\")\n  \
         authority(resource: \"grant\", action: \"list\", agent: \"Bookkeeper\")\n  \
         authority(resource: \"constitution\", action: \"show\")"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": { "type": "string", "enum": ["grant", "constitution"] },
                "action": { "type": "string", "enum": ["grant", "widen", "narrow", "suspend", "list", "show"] },
                "agent": { "type": "string", "description": "The seat's name or id" },
                "operation": { "type": "string", "description": "Operation suffix, e.g. ledger.billpayment.create" },
                "bounds": {
                    "type": "object",
                    "properties": {
                        "max_amount_cents": { "type": "integer" },
                        "per_day_cents": { "type": "integer" },
                        "per_day_count": { "type": "integer" },
                        "per_counterparty_day_cents": { "type": "integer" },
                        "counterparty_class": { "type": "string", "description": "ledger_id: only counterparties carrying a source-system id" },
                        "freshness_secs": { "type": "integer" }
                    }
                },
                "evidence": { "type": "string", "description": "What in the record this is granted, widened, or narrowed on. Required for widen." }
            },
            "required": ["resource", "action"]
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn is_concurrent_safe(&self, input: &serde_json::Value) -> bool {
        matches!(input["action"].as_str(), Some("list") | Some("show"))
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let resource = input["resource"].as_str().unwrap_or("grant");
            let action = input["action"].as_str().unwrap_or("");
            match (resource, action) {
                ("constitution", _) => self.constitution(),
                ("grant", "grant") => self.grant(&input, false),
                ("grant", "widen") => self.grant(&input, true),
                ("grant", "narrow") => self.narrow(&input, false),
                ("grant", "suspend") => self.narrow(&input, true),
                ("grant", "list") => self.list(&input),
                _ => ToolResult::error(format!("unknown {resource}/{action}")),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wider_means_a_looser_axis_or_a_dropped_class() {
        let cur = Bounds { max_amount_cents: Some(100), per_day_count: Some(5), counterparty_class: Some("ledger_id".into()), ..Default::default() };
        assert!(bounds_wider(&cur, &Bounds { max_amount_cents: Some(200), per_day_count: Some(5), counterparty_class: Some("ledger_id".into()), ..Default::default() }));
        assert!(bounds_wider(&cur, &Bounds { max_amount_cents: Some(100), per_day_count: None, counterparty_class: Some("ledger_id".into()), ..Default::default() }));
        assert!(bounds_wider(&cur, &Bounds { max_amount_cents: Some(100), per_day_count: Some(5), ..Default::default() }));
        assert!(!bounds_wider(&cur, &Bounds { max_amount_cents: Some(50), per_day_count: Some(5), counterparty_class: Some("ledger_id".into()), ..Default::default() }));
    }
}
