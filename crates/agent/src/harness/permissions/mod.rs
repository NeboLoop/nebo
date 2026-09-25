//! The one permission check (Turn-Controller-Technical-Design §2.12).
//!
//! Every tool call, whatever its shape and whichever door it came through,
//! reaches [`Check`] from inside `tools::Registry::execute`: there is no
//! other way to run a tool. The order is fixed ([`decide`]): hard limits,
//! the run's fence and ceiling, the rules, the mode; the decision is
//! recorded with why. An ask parks only that step.

pub mod activity;
pub mod ask;
pub mod cases;
pub mod consent;
pub mod judgement;
pub mod limits;
pub mod migrate;
pub mod plan;
pub mod rules;

use std::sync::Arc;

use tools::{GateVerdict, PermissionGate, ResolvedCall, ToolContext, ToolResult};
use types::permissions::{AskCase, Decision, Effect, Grant, JudgementMode, Mode, Target, Verdict, Why};

pub use ask::{Answer, AnsweredVia, Ask, AskError, AskStatus, AskSurfaces, Asks, Settled};
pub use rules::RuleSet;

/// The permission check: the registry's gate.
pub struct Check {
    store: Arc<db::Store>,
    asks: Arc<Asks>,
}

impl Check {
    pub fn new(store: Arc<db::Store>) -> Self {
        let asks = Arc::new(Asks::new(store.clone()));
        Self { store, asks }
    }

    /// The asks this check parks: the server attaches the card's surfaces
    /// and answers them.
    pub fn asks(&self) -> Arc<Asks> {
        self.asks.clone()
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
    // An employee made by an employee works under its creator's grant until
    // the owner answers its card.
    grant.ceiling = consent::creator_ceiling(store, agent_id);
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
        let (mut decision, judged) = decide_judged(&cx, t);
        // The owner already said no to this same call in this session: it
        // is refused without a card.
        if matches!(decision, Decision::Ask { .. })
            && let Some(ask_id) = self.asks.declined_before(&ctx.session_key, t, call.input)
        {
            decision = Decision::Deny {
                reason: ask::declined_text(&call.tool.activity(call.input)),
                why: Why::Declined { ask_id },
            };
        }
        let ask_id = match &decision {
            Decision::Ask { case } => Some(self.asks.park(&cx, call, case)),
            _ => None,
        };
        let entry = activity::Entry {
            activity: call.tool.activity(call.input),
            decision: &decision,
            ask_id: ask_id.as_deref(),
            judged: judged.as_ref(),
        };
        if let Err(e) = activity::record(&self.store, &cx, t, &entry) {
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

    /// What a call that ran brought into being is the employee's own work
    /// from now on: a later delete or overwrite of it is not case 3.
    async fn ran(&self, ctx: &ToolContext, call: &ResolvedCall<'_>, result: &ToolResult) {
        let agent_id = match &ctx.grant {
            Some(g) => g.agent_id.clone(),
            None => types::keyparser::extract_agent_id(&ctx.session_key),
        };
        let mut created = call.target.effects.creates.clone();
        // A typed create names its new record in its result.
        if let Some(op) = &call.target.operation
            && let Some((resource, "create")) = tools::plugin_tool::port_suffix(op).rsplit_once('.')
            && let Some(id) = serde_json::from_str::<serde_json::Value>(&result.content)
                .ok()
                .as_ref()
                .and_then(tools::plugin_tool::record_id)
        {
            created.push(format!("{resource}:{id}"));
        }
        for target in created {
            if let Err(e) = self.store.add_employee_created(&agent_id, &target) {
                tracing::warn!(error = %e, "created work not recorded");
            }
        }
    }
}

/// A judge's verdict as the check applied it: recorded with the decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Judged {
    pub mode: JudgementMode,
    pub verdict: Verdict,
}

impl Judged {
    /// Neither judge could answer: the call ran unreviewed.
    pub fn unreviewed(&self) -> bool {
        self.verdict == Verdict::Unjudged
    }
}

/// What the code decided for one call: a decision, or a question for the
/// judgement with the answer that stands when no judge is asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Decided(Decision),
    Undecided { question: cases::Question, code: Decision },
}

/// The question the judgement is asked about this call, when the code
/// can't decide it (the tool round asks once for all of its calls).
pub fn question_for(cx: &CheckCx<'_>, t: &Target) -> Option<cases::Question> {
    match decide_code(cx, t) {
        Outcome::Undecided { question, .. } => Some(question),
        Outcome::Decided(_) => None,
    }
}

/// The decision for one call, in the fixed order.
pub fn decide(cx: &CheckCx<'_>, t: &Target) -> Decision {
    decide_judged(cx, t).0
}

/// The decision, and the judgement it applied when the call carried one.
/// Shadow records the verdict and keeps the code's answer; enforce acts on
/// it. With no verdict on the call (no judge was asked: a door outside the
/// tool round) the code's answer stands.
fn decide_judged(cx: &CheckCx<'_>, t: &Target) -> (Decision, Option<Judged>) {
    let code = match decide_code(cx, t) {
        Outcome::Decided(d) => return (d, None),
        Outcome::Undecided { code, .. } => code,
    };
    let Some(verdict) = cx.ctx.judgement.clone() else {
        return (code, None);
    };
    let mode = cx.store.permission_judgement_mode().unwrap_or_default();
    let decision = match (mode, &verdict) {
        (JudgementMode::Shadow, _) => code,
        (JudgementMode::Enforce, Verdict::Allow { by, reason }) => {
            Decision::Allow { why: Why::Judged { by: by.as_str().to_string(), reason: reason.clone() } }
        }
        (JudgementMode::Enforce, Verdict::Ask { case, .. }) => Decision::Ask { case: case.clone() },
        (JudgementMode::Enforce, Verdict::Unjudged) => {
            Decision::Allow { why: Why::Unreviewed { reason: types::permissions::UNREVIEWED_REASON.to_string() } }
        }
    };
    (decision, Some(Judged { mode, verdict }))
}

/// The code's decision for one call, in the fixed order (§2.12.3).
fn decide_code(cx: &CheckCx<'_>, t: &Target) -> Outcome {
    Outcome::Decided(match decide_rules(cx, t) {
        Ok(d) => d,
        Err(Automatic { rules, by_rule }) => return automatic(cx, t, &rules, by_rule),
    })
}

/// Past the rules and the mode: a call the cases decide.
struct Automatic {
    rules: RuleSet,
    by_rule: Why,
}

/// Hard limits, the fence and ceiling, the rules and the mode. `Err` is a
/// call the surfaced cases decide.
fn decide_rules(cx: &CheckCx<'_>, t: &Target) -> Result<Decision, Automatic> {
    // 1. Hard limits: never asked, never lifted.
    if let Some(d) = limits::hard_limits(cx, t) {
        return Ok(d);
    }
    // 2. The run's own fence (an isolated helper's copy) and the ceiling it
    //    runs under (its parent's grant): it can only narrow.
    if let Some(fence) = &cx.grant.fence
        && let Some(reason) = rules::outside(fence, t, cx.input)
    {
        return Ok(Decision::Deny { reason, why: Why::Ceiling });
    }
    if let Some(ceiling) = &cx.grant.ceiling
        && let Some(reason) = beyond(ceiling.grant(), cx, t)
    {
        return Ok(Decision::Deny { reason, why: Why::Ceiling });
    }
    // 3. Rules.
    let rules = RuleSet::of(cx.grant);
    let decided = rules.decide(t);
    if let Some((rule, Effect::Deny)) = decided {
        return Ok(Decision::Deny { reason: refusal(t, rule), why: Why::Rule { rule_id: rule.id.clone() } });
    }
    if let Some(ask_id) = &cx.ctx.answered_ask {
        return Ok(Decision::Allow { why: Why::AnsweredOnce { ask_id: ask_id.clone() } });
    }
    // Only the owner gives an employee more room, in every mode.
    if t.effects.widens {
        return Ok(Decision::Ask { case: AskCase::Widens });
    }
    let full = cx.grant.mode == Mode::FullAccess;
    if let Some((rule, Effect::Ask)) = decided
        && !full
    {
        return Ok(Decision::Ask { case: AskCase::AskRule { rule_id: rule.id.clone() } });
    }
    // 4. The mode.
    let by_rule = || match decided {
        Some((rule, _)) => Why::Rule { rule_id: rule.id.clone() },
        None => Why::BasicWork,
    };
    match cx.grant.mode {
        Mode::FullAccess => return Ok(Decision::Allow { why: Why::Mode { mode: Mode::FullAccess } }),
        Mode::Plan if !t.read_only => {
            return Ok(Decision::Deny { reason: plan::REFUSAL.to_string(), why: Why::Mode { mode: Mode::Plan } });
        }
        Mode::Plan => return Ok(Decision::Allow { why: Why::Mode { mode: Mode::Plan } }),
        Mode::Ask if !t.read_only && !rules.owner_allowed(t) => {
            return Ok(Decision::Ask { case: AskCase::AskMode });
        }
        // A change the owner allowed, or a read: the surfaced cases still
        // apply, as in Automatic.
        Mode::Ask | Mode::Automatic => {}
    }
    let by_rule = by_rule();
    Err(Automatic { rules, by_rule })
}

/// The five surfaced cases (`cases.rs`), then the call runs.
fn automatic(cx: &CheckCx<'_>, t: &Target, rules: &RuleSet, by_rule: Why) -> Outcome {
    let gathered = cases::Gathered::load(cx, rules, t);
    let facts = gathered.facts(&cx.ctx.run_taint, cx.input);
    match cases::surfaced(t, rules, &facts) {
        cases::CaseVerdict::Clear => Outcome::Decided(Decision::Allow { why: by_rule }),
        cases::CaseVerdict::Ask(case) => Outcome::Decided(Decision::Ask { case }),
        cases::CaseVerdict::Undecided(question) => {
            Outcome::Undecided { question, code: Decision::Allow { why: by_rule } }
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

pub(super) fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
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
mod proof;
#[cfg(test)]
mod tests;
