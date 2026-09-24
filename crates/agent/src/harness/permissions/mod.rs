//! The one permission check (Turn-Controller-Technical-Design §2.12).
//!
//! Every tool call, whatever its shape and whichever door it came through,
//! reaches [`Check`] from inside `tools::Registry::execute`: there is no
//! other way to run a tool. The order is fixed ([`decide`]): hard limits,
//! the run's fence and ceiling, the rules, the mode; the decision is
//! recorded with why. An ask parks only that step.

pub mod activity;
pub mod ask;
pub mod limits;
pub mod migrate;
pub mod plan;
pub mod rules;

use std::sync::Arc;

use tools::{GateVerdict, PermissionGate, ResolvedCall, ToolContext, ToolResult};
use types::permissions::{AskCase, Decision, Effect, Grant, Mode, Target, Why};

pub use rules::RuleSet;

/// The permission check: the registry's gate.
pub struct Check {
    store: Arc<db::Store>,
}

impl Check {
    pub fn new(store: Arc<db::Store>) -> Self {
        Self { store }
    }
}

/// What one decision reads: the call's context and input, the grant it runs
/// under, and the store for money spent today.
pub struct CheckCx<'a> {
    pub ctx: &'a ToolContext,
    pub input: &'a serde_json::Value,
    pub grant: &'a Grant,
    pub store: &'a db::Store,
}

/// The grant an employee holds: its rules (company and its own) and its
/// mode, `mode` overriding it for one run. A store that can't be read
/// yields no rules, so everything outside basic work asks: never less.
pub fn resolve_grant(store: &db::Store, agent_id: &str, mode: Option<Mode>) -> Grant {
    let mut grant = Grant::new(agent_id, mode.unwrap_or_else(|| rules::mode_of(store, agent_id).unwrap_or_default()));
    match store.permission_rules(agent_id) {
        Ok(r) => grant.rules = r,
        Err(e) => tracing::warn!(agent = %agent_id, error = %e, "permission rules unreadable; the run holds none"),
    }
    grant
}

#[async_trait::async_trait]
impl PermissionGate for Check {
    async fn check(&self, ctx: &ToolContext, call: &ResolvedCall<'_>) -> GateVerdict {
        let loaded;
        let grant: &Grant = match &ctx.grant {
            Some(g) => g,
            None => {
                let agent_id = types::keyparser::extract_agent_id(&ctx.session_key);
                loaded = resolve_grant(&self.store, &agent_id, None);
                &loaded
            }
        };
        let cx = CheckCx { ctx, input: call.input, grant, store: &self.store };
        let t = &call.target;
        let decision = decide(&cx, t);
        let ask_id = match &decision {
            Decision::Ask { case } => Some(ask::park(&cx, call, case)),
            _ => None,
        };
        if let Err(e) = activity::record(&self.store, &cx, t, call.tool.activity(call.input), &decision, ask_id.as_deref()) {
            tracing::warn!(tool = %t.tool, error = %e, "permission decision not recorded");
        }
        match decision {
            Decision::Allow { why } => {
                spend(&cx, t);
                GateVerdict::Run(why)
            }
            Decision::Deny { reason, .. } => GateVerdict::Refuse(ToolResult::error(reason)),
            Decision::Ask { case } => {
                let sentence = call.tool.activity(call.input);
                let mut parked = ToolResult::error(ask::parked_text(&sentence, &case));
                parked.parked_ask = ask_id;
                GateVerdict::Parked(parked)
            }
        }
    }
}

/// The decision for one call, in the fixed order.
pub fn decide(cx: &CheckCx<'_>, t: &Target) -> Decision {
    // 1. Hard limits: never asked, never lifted.
    if let Some(d) = limits::hard_limits(cx, t) {
        return d;
    }
    // 2. The run's own fence (an isolated helper's copy) and the ceiling it
    //    runs under (its parent's grant): it can only narrow.
    if let Some(fence) = &cx.grant.fence
        && let Some(reason) = rules::outside(fence, t, cx.input)
    {
        return Decision::Deny { reason, why: Why::Ceiling };
    }
    if let Some(ceiling) = &cx.grant.ceiling
        && let Some(reason) = beyond(ceiling.grant(), cx, t)
    {
        return Decision::Deny { reason, why: Why::Ceiling };
    }
    // 3. Rules.
    let rules = RuleSet::of(cx.grant);
    let decided = rules.decide(t);
    if let Some((rule, Effect::Deny)) = decided {
        return Decision::Deny { reason: refusal(t, rule), why: Why::Rule { rule_id: rule.id.clone() } };
    }
    if let Some(ask_id) = &cx.ctx.answered_ask {
        return Decision::Allow { why: Why::AnsweredOnce { ask_id: ask_id.clone() } };
    }
    let full = cx.grant.mode == Mode::FullAccess;
    if let Some((rule, Effect::Ask)) = decided
        && !full
    {
        return Decision::Ask { case: AskCase::AskRule { rule_id: rule.id.clone() } };
    }
    if !full && let Some(case) = over_money(cx, &rules, t) {
        return Decision::Ask { case };
    }
    // 4. The mode.
    let by_rule = || match decided {
        Some((rule, _)) => Why::Rule { rule_id: rule.id.clone() },
        None => Why::BasicWork,
    };
    match cx.grant.mode {
        Mode::FullAccess => return Decision::Allow { why: Why::Mode { mode: Mode::FullAccess } },
        Mode::Plan if !t.read_only => {
            return Decision::Deny { reason: plan::REFUSAL.to_string(), why: Why::Mode { mode: Mode::Plan } };
        }
        Mode::Plan => return Decision::Allow { why: Why::Mode { mode: Mode::Plan } },
        Mode::Ask if !t.read_only && !rules.owner_allowed(t) => {
            return Decision::Ask { case: AskCase::AskMode };
        }
        Mode::Ask | Mode::Automatic => {}
    }
    // Automatic: someone else's words in the run never spend a gated
    // operation unasked.
    let gated = t.operation.as_deref().is_some_and(tools::interface_catalog::is_gated);
    if gated && (!cx.ctx.origin.is_trusted() || cx.ctx.untrusted_input) {
        let source = if cx.ctx.untrusted_input {
            "the run's input".to_string()
        } else {
            limits::origin_label(cx.ctx.origin).to_string()
        };
        return Decision::Ask { case: AskCase::UntrustedInput { source } };
    }
    if rules.in_job(t, cx.input) {
        Decision::Allow { why: by_rule() }
    } else {
        Decision::Ask {
            case: AskCase::OutsideJob { capability: t.capability.clone().unwrap_or_else(|| t.key.clone()) },
        }
    }
}

/// Why `t` goes beyond `ceiling` (the parent's grant), or `None`.
fn beyond(ceiling: &Grant, cx: &CheckCx<'_>, t: &Target) -> Option<String> {
    let name = if ceiling.agent_id.is_empty() { "the run that started this one" } else { "the employee this work is for" };
    let refusal = || {
        Some(format!(
            "This is beyond what {name} may do, so it isn't allowed here. Say what you needed and stop; do not retry."
        ))
    };
    if let Some(fence) = &ceiling.fence
        && rules::outside(fence, t, cx.input).is_some()
    {
        return refusal();
    }
    let rs = RuleSet::of(ceiling);
    match rs.decide(t) {
        Some((_, Effect::Deny)) => return refusal(),
        _ if ceiling.mode == Mode::Plan && !t.read_only => return refusal(),
        _ if ceiling.mode != Mode::FullAccess && !rs.in_job(t, cx.input) => return refusal(),
        _ => {}
    }
    match &ceiling.ceiling {
        Some(up) => beyond(up.grant(), cx, t),
        None => None,
    }
}

/// The plain-words refusal for a deny rule.
fn refusal(t: &Target, rule: &types::permissions::Rule) -> String {
    let what = match &rule.key {
        types::permissions::RuleKey::Capability(c) => {
            format!("The \"{}\" permission is off for this employee", tools::capabilities::capability_label(c))
        }
        _ => format!("'{}' is turned off for this employee", t.key),
    };
    format!(
        "{what}, so this didn't run. Tell the user in plain words what you needed it for, then stop. \
         Do not try other tools or workarounds to get around it."
    )
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// The money case: a standing allow's limits against today's spend.
fn over_money(cx: &CheckCx<'_>, rules: &RuleSet, t: &Target) -> Option<AskCase> {
    let limit = rules.money_limit(t)?;
    let (rule, _) = rules.decide(t)?;
    let cents = t.effects.money_cents.unwrap_or(0);
    let counterparty = t.effects.counterparty.clone().unwrap_or_default();
    let spent = cx
        .store
        .permission_spend(&cx.grant.agent_id, &today(), rule.key.value(), &counterparty)
        .unwrap_or_default();
    let over = |limit: Option<i64>, used: i64| limit.is_some_and(|l| used > l);
    let exceeded = over(limit.per_action_cents, cents)
        || over(limit.per_day_count, spent.count + 1)
        || over(limit.per_day_cents, spent.cents + cents)
        || (!counterparty.is_empty() && over(limit.per_counterparty_day_cents, spent.counterparty_cents + cents));
    exceeded.then(|| AskCase::Money { cents, limit_cents: limit.per_action_cents.or(limit.per_day_cents) })
}

/// Count an allowed call against a standing allow's money limits, before it
/// runs, so a crash between the decision and the call never under-counts.
fn spend(cx: &CheckCx<'_>, t: &Target) {
    let rules = RuleSet::of(cx.grant);
    if rules.money_limit(t).is_none() {
        return;
    }
    let Some((rule, _)) = rules.decide(t) else { return };
    let counterparty = t.effects.counterparty.clone().unwrap_or_default();
    if let Err(e) = cx.store.add_permission_spend(
        &cx.grant.agent_id,
        &today(),
        rule.key.value(),
        &counterparty,
        t.effects.money_cents.unwrap_or(0),
    ) {
        tracing::warn!(error = %e, "spend not counted");
    }
}

#[cfg(test)]
mod tests;
