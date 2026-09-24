//! The General Manager's `authority` tool: standing authority for seats,
//! inside the constitution.
//!
//! Authority is a permission rule on the seat: an allow on the operation,
//! with money limits. Only the owner widens: a grant or a widening is
//! written only when the owner answered this exact call (the operation
//! `authority.grant.*` asks). Narrowing and suspending are an employee
//! narrowing another's rules, which the store allows and nothing more.

use std::sync::Arc;

use types::permissions::{Effect, MoneyLimit, Rule, RuleKey, RuleSource, Scope, Writer};

use crate::origin::ToolContext;
use crate::policy::{Bounds, CompanyPolicy};
use crate::registry::{DynTool, ToolResult};

pub struct AuthorityTool {
    store: Arc<db::Store>,
}

/// The standing-grant bounds the tool takes, as money limits.
fn limit_of(input: &serde_json::Value) -> Result<MoneyLimit, String> {
    let b: Bounds = serde_json::from_value(input["bounds"].clone()).map_err(|e| format!("`bounds` is malformed: {e}"))?;
    Ok(MoneyLimit {
        per_action_cents: b.max_amount_cents,
        per_day_cents: b.per_day_cents,
        per_day_count: b.per_day_count,
        per_counterparty_day_cents: b.per_counterparty_day_cents,
    })
}

/// A limit may not state more than the company allows unattended: no day
/// figure above the company's, and a per-operation bound no larger than the
/// company's single-operation cap (required when the company has one).
fn within_company(limit: &MoneyLimit, company: &CompanyPolicy) -> Result<(), String> {
    let d = &company.daily;
    let no_larger = |name: &str, inner: Option<i64>, outer: Option<i64>| match (inner, outer) {
        (Some(i), Some(o)) if i > o => Err(format!("{name} {i} exceeds the company's {o}")),
        _ => Ok(()),
    };
    no_larger("per_day_cents", limit.per_day_cents, d.per_day_cents)?;
    no_larger("per_day_count", limit.per_day_count, d.per_day_count)?;
    no_larger("per_counterparty_day_cents", limit.per_counterparty_day_cents, d.per_counterparty_day_cents)?;
    match (limit.per_action_cents, d.max_amount_cents.or(d.per_day_cents)) {
        (None, Some(o)) => Err(format!("max_amount_cents is unbounded; the company allows at most {o} per operation")),
        (Some(i), Some(o)) if i > o => Err(format!("max_amount_cents {i} exceeds the company's {o}")),
        _ => Ok(()),
    }
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

    fn company(&self) -> CompanyPolicy {
        CompanyPolicy::from_json(self.store.get_company_policy().ok().flatten().as_deref())
    }

    /// The seat's own rule on an operation.
    fn seat_rule(&self, seat_id: &str, key: &RuleKey) -> Option<Rule> {
        self.store
            .permission_rules_in(&Scope::Employee(seat_id.to_string()))
            .ok()?
            .into_iter()
            .find(|r| &r.key == key && r.field.is_none())
    }

    fn write(&self, seat_id: &str, key: RuleKey, effect: Effect, money: Option<MoneyLimit>, source: RuleSource, by: &Writer) -> Result<(), String> {
        let rule = Rule {
            id: uuid::Uuid::new_v4().to_string(),
            scope: Scope::Employee(seat_id.to_string()),
            key,
            field: None,
            effect,
            money,
            source,
            locked: false,
            created_at: chrono::Utc::now().timestamp(),
        };
        self.store.write_permission_rule(&rule, by).map(|_| ()).map_err(|e| format!("refused: {e}"))
    }

    fn grant(&self, ctx: &ToolContext, input: &serde_json::Value, widen: bool) -> ToolResult {
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
        // Only the owner widens: this runs only as the owner's answer to
        // this exact call.
        let Some(ask_id) = ctx.answered_ask.clone() else {
            return ToolResult::error(
                "Only the owner can give a seat more authority. Tell the owner what you would grant, \
                 to whom, within what bounds, and why; they decide.",
            );
        };
        let limit = match limit_of(input) {
            Ok(l) => l,
            Err(e) => return ToolResult::error(e),
        };
        let seat = match self.seat(agent) {
            Ok(s) => s,
            Err(e) => return ToolResult::error(e),
        };
        let suffix = crate::plugin_tool::port_suffix(operation);
        let key = RuleKey::Operation(suffix.clone());
        let company = self.company();
        if company.is_reserved(&suffix) {
            return ToolResult::error(format!("refused: {suffix} is reserved to the owner. Only the owner can change the constitution."));
        }
        if let Err(e) = within_company(&limit, &company) {
            return ToolResult::error(format!("refused: {e}. Only the owner can change the constitution."));
        }
        if widen {
            let current = self.seat_rule(&seat.id, &key).filter(|r| r.effect == Effect::Allow && r.money.is_some());
            let Some(current) = current else {
                return ToolResult::error(format!("{} holds no standing grant for {suffix}; grant first, widen later", seat.name));
            };
            if limit.within(current.money.as_ref().unwrap()) {
                return ToolResult::error("widen must loosen at least one bound; to tighten, use narrow");
            }
        }
        if let Err(e) = self.write(&seat.id, key, Effect::Allow, Some(limit.clone()), RuleSource::AllowAlways { ask_id }, &Writer::Owner) {
            return ToolResult::error(e);
        }
        ToolResult::ok(format!(
            "{} now runs {suffix} unattended inside {}. Recorded as standing authority the owner allowed{}.",
            seat.name,
            describe(&limit),
            if evidence.is_empty() { String::new() } else { format!(" on: {evidence}") }
        ))
    }

    fn narrow(&self, ctx: &ToolContext, input: &serde_json::Value, suspend: bool) -> ToolResult {
        let agent = input["agent"].as_str().unwrap_or("");
        let operation = input["operation"].as_str().unwrap_or("");
        if operation.is_empty() {
            return ToolResult::error("`operation` is required");
        }
        let seat = match self.seat(agent) {
            Ok(s) => s,
            Err(e) => return ToolResult::error(e),
        };
        let suffix = crate::plugin_tool::port_suffix(operation);
        let key = RuleKey::Operation(suffix.clone());
        if self.seat_rule(&seat.id, &key).is_none() {
            return ToolResult::error(format!("{} holds no rule for {suffix}", seat.name));
        }
        let narrower = Writer::Employee { agent_id: ctx.grant.as_ref().map(|g| g.agent_id.clone()).unwrap_or_default() };
        let result = if suspend {
            self.write(&seat.id, key, Effect::Ask, None, RuleSource::Owner, &narrower)
        } else {
            match limit_of(input) {
                Ok(limit) => self.write(&seat.id, key, Effect::Allow, Some(limit), RuleSource::Owner, &narrower),
                Err(e) => Err(e),
            }
        };
        if let Err(e) = result {
            return ToolResult::error(e);
        }
        ToolResult::ok(if suspend {
            format!("{}'s standing authority for {suffix} is suspended; it asks until the owner grants it again.", seat.name)
        } else {
            format!("{}'s standing authority for {suffix} is narrowed.", seat.name)
        })
    }

    fn list(&self, input: &serde_json::Value) -> ToolResult {
        let agent = input["agent"].as_str().unwrap_or("");
        let seat = match self.seat(agent) {
            Ok(s) => s,
            Err(e) => return ToolResult::error(e),
        };
        let rules = self.store.permission_rules_in(&Scope::Employee(seat.id.clone())).unwrap_or_default();
        let mut lines: Vec<String> = rules
            .iter()
            .filter_map(|r| match &r.key {
                RuleKey::Operation(op) => Some((op, r)),
                _ => None,
            })
            .map(|(op, r)| {
                let what = match (r.effect, r.money.as_ref()) {
                    (Effect::Allow, Some(m)) => format!("unattended inside {}", describe(m)),
                    (Effect::Allow, None) => "unattended, unbounded (owner's setting)".to_string(),
                    (Effect::Ask, _) => "asks".to_string(),
                    (Effect::Deny, _) => "blocked".to_string(),
                };
                format!("- {op}: {what}{}", if r.locked { " (locked)" } else { "" })
            })
            .collect();
        lines.sort();
        ToolResult::ok(format!(
            "{}\n{}",
            seat.name,
            if lines.is_empty() { "- no per-operation rules".to_string() } else { lines.join("\n") }
        ))
    }

    fn constitution(&self) -> ToolResult {
        match self.store.get_company_policy() {
            Ok(Some(json)) => {
                let c = CompanyPolicy::from_json(Some(&json));
                let d = &c.daily;
                ToolResult::ok(format!(
                    "Purpose: {}\nCompany-wide per day: {}\nReserved to the owner: {}",
                    if c.purpose.is_empty() { "(unset)" } else { &c.purpose },
                    describe(&MoneyLimit {
                        per_action_cents: d.max_amount_cents,
                        per_day_cents: d.per_day_cents,
                        per_day_count: d.per_day_count,
                        per_counterparty_day_cents: d.per_counterparty_day_cents,
                    }),
                    if c.reserved.is_empty() { "nothing".to_string() } else { c.reserved.join(", ") },
                ))
            }
            Ok(None) => ToolResult::ok(
                "No constitution is written yet. Until the owner writes one, no standing authority can be granted.",
            ),
            Err(e) => ToolResult::error(format!("reading the constitution: {e}")),
        }
    }
}

fn describe(b: &MoneyLimit) -> String {
    let mut parts = Vec::new();
    if let Some(v) = b.per_action_cents {
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
         bounds: {max_amount_cents, per_day_cents, per_day_count, per_counterparty_day_cents}\n\n\
         Examples:\n  \
         authority(resource: \"grant\", action: \"grant\", agent: \"Bookkeeper\", operation: \"ledger.billpayment.create\", \
         bounds: {max_amount_cents: 250000, per_day_count: 20}, evidence: \"thirty clean days of approvals\")\n  \
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
                        "per_counterparty_day_cents": { "type": "integer" }
                    }
                },
                "evidence": { "type": "string", "description": "What in the record this is granted, widened, or narrowed on. Required for widen." },
                "display": { "type": "string", "description": "REQUIRED for grant and widen: ONE plain-language sentence for the owner's approval prompt, in words a non-technical person reads at a glance. Example: 'Let the Bookkeeper pay bills up to $2,500.00, 20 a day, to vendors already in the ledger.'" }
            },
            "required": ["resource", "action"]
        })
    }


    /// Granting a seat standing authority — and widening it — is how an
    /// employee comes to act unattended at all, so both are gated operations
    /// the owner stands behind: the permission check asks for them like any
    /// other (`authority.grant.*`, critical in the interface catalog).
    /// Narrowing, suspending, and reading make an employee less powerful or
    /// nothing at all, and are never gated.
    fn operation_performed(&self, input: &serde_json::Value) -> Option<String> {
        match (
            input["resource"].as_str().unwrap_or("grant"),
            input["action"].as_str().unwrap_or(""),
        ) {
            ("grant", "grant") => Some("authority.grant.grant".to_string()),
            ("grant", "widen") => Some("authority.grant.widen".to_string()),
            _ => None,
        }
    }

    fn search_hint(&self) -> &str {
        "standing authority grants constitution"
    }

    fn read_only(&self, input: &serde_json::Value) -> bool {
        matches!(input["action"].as_str(), Some("list") | Some("show"))
    }

    fn rule_key(&self, input: &serde_json::Value) -> String {
        if input["resource"].as_str() == Some("constitution") {
            return "read_constitution".to_string();
        }
        match input["action"].as_str().unwrap_or("") {
            "narrow" | "suspend" => "narrow_authority",
            "list" | "show" => "list_authority",
            _ => "grant_authority",
        }
        .to_string()
    }

    /// Pre-interface: it settles its own call shapes (see
    /// `DynTool::validates_input`).
    fn validates_input(&self) -> bool {
        false
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let resource = input["resource"].as_str().unwrap_or("grant");
            let action = input["action"].as_str().unwrap_or("");
            match (resource, action) {
                ("constitution", _) => self.constitution(),
                ("grant", "grant") => self.grant(ctx, &input, false),
                ("grant", "widen") => self.grant(ctx, &input, true),
                ("grant", "narrow") => self.narrow(ctx, &input, false),
                ("grant", "suspend") => self.narrow(ctx, &input, true),
                ("grant", "list") => self.list(&input),
                _ => ToolResult::error(format!("unknown {resource}/{action}")),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> (tempfile::TempDir, AuthorityTool, Arc<db::Store>) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(&dir.path().join("a.db").to_string_lossy()).expect("store"));
        (dir, AuthorityTool::new(store.clone()), store)
    }

    /// A seat that can grant authority could otherwise grant itself
    /// everything the company's bounds allow, unsupervised. Granting and
    /// widening are catalog-gated operations; narrowing, suspending and
    /// reading are not.
    #[test]
    fn granting_is_an_operation_and_narrowing_is_not() {
        let (_d, tool, _s) = tool();
        let op = |resource: &str, action: &str| {
            tool.operation_performed(&serde_json::json!({"resource": resource, "action": action}))
        };
        assert_eq!(op("grant", "grant").as_deref(), Some("authority.grant.grant"));
        assert_eq!(op("grant", "widen").as_deref(), Some("authority.grant.widen"));
        for action in ["narrow", "suspend", "list"] {
            assert_eq!(op("grant", action), None, "{action} is not gated");
        }
        assert_eq!(op("constitution", "show"), None);
        assert!(crate::interface_catalog::is_critical("authority.grant.grant"));
    }

    /// A grant may not state more than the company allows unattended.
    #[test]
    fn a_grant_beyond_the_company_is_refused() {
        let company = CompanyPolicy {
            daily: Bounds { per_day_count: Some(20), max_amount_cents: Some(250_000), ..Default::default() },
            ..Default::default()
        };
        let inside = MoneyLimit { per_action_cents: Some(250_000), per_day_count: Some(20), ..Default::default() };
        assert!(within_company(&inside, &company).is_ok());
        let over = MoneyLimit { per_action_cents: Some(300_000), per_day_count: Some(20), ..Default::default() };
        assert!(within_company(&over, &company).is_err());
        let unbounded = MoneyLimit::default();
        assert!(within_company(&unbounded, &company).is_err(), "an unbounded grant where the company bounds the day");
    }

    /// Only the owner widens: without the owner's answer to this exact call,
    /// nothing is written; with it, the grant is an allow with money limits.
    #[tokio::test]
    async fn a_grant_is_written_only_as_the_owners_answer() {
        let (_d, tool, store) = tool();
        store
            .create_agent("seat-1", None, "Bookkeeper", "", "", "{}", None, None)
            .expect("seat");
        let input = serde_json::json!({
            "resource": "grant", "action": "grant", "agent": "seat-1",
            "operation": "ledger.billpayment.create", "bounds": {"max_amount_cents": 250000, "per_day_count": 20}
        });
        let refused = tool.execute_dyn(&ToolContext::default(), input.clone()).await;
        assert!(refused.is_error && refused.content.contains("Only the owner"), "{}", refused.content);
        assert!(store.permission_rules_in(&Scope::Employee("seat-1".into())).unwrap().is_empty());

        let answered = ToolContext { answered_ask: Some("ask-1".into()), ..Default::default() };
        let granted = tool.execute_dyn(&answered, input).await;
        assert!(!granted.is_error, "{}", granted.content);
        let rules = store.permission_rules_in(&Scope::Employee("seat-1".into())).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].effect, Effect::Allow);
        assert_eq!(rules[0].money.as_ref().and_then(|m| m.per_action_cents), Some(250000));

        // Narrowing needs no answer, and cannot loosen.
        let narrow = serde_json::json!({
            "resource": "grant", "action": "narrow", "agent": "seat-1",
            "operation": "ledger.billpayment.create", "bounds": {"max_amount_cents": 100000, "per_day_count": 20}
        });
        assert!(!tool.execute_dyn(&ToolContext::default(), narrow).await.is_error);
        let loosen = serde_json::json!({
            "resource": "grant", "action": "narrow", "agent": "seat-1",
            "operation": "ledger.billpayment.create", "bounds": {"max_amount_cents": 900000}
        });
        assert!(tool.execute_dyn(&ToolContext::default(), loosen).await.is_error);
    }
}
