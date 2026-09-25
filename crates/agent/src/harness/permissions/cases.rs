//! Automatic mode's five surfaced cases (Turn-Controller-Technical-Design
//! §2.12.4, PRD-Permissions §4.4). A call the rules let run still asks when
//! it spends beyond its grant, speaks for the owner somewhere new, removes
//! or replaces something the employee didn't make, is outside its job, or
//! acts outward on words that came from outside.
//!
//! Every case is decided here, by code, from facts the employee's history
//! and the call's own effects hold: spend against the limit, recipients
//! against the people it works with, deletes against what it created, the
//! call's capability against the job, the run's taint. Only a call whose
//! outward effect its input can't show (`Knowable::Unknown`) is handed to
//! the judgement as a [`Question`]; a case code can decide never reaches a
//! judge, so a judge outage never lets one through.

use std::collections::HashSet;
use std::path::Path;

use types::permissions::{AskCase, Effect, Knowable, RuleField, Target};
use types::provenance::ProvenanceClass;

use super::{CheckCx, RuleSet};

/// Today's spend against the key a money limit sits on.
pub type SpendToday = db::PermissionSpend;

/// The people, among a call's recipients, the employee already works with
/// (address keys, see [`types::permissions::address_key`]).
#[derive(Debug, Clone, Default)]
pub struct Counterparties(pub HashSet<String>);

/// What, among a call's deletes and overwrites, the employee created.
#[derive(Debug, Clone, Default)]
pub struct CreatedLedger(pub HashSet<String>);

/// The people the conversation the run serves is with: a reply to them is
/// a reply in the thread.
#[derive(Debug, Clone, Default)]
pub struct ThreadRef {
    pub people: Vec<String>,
}

/// What the cases read beside the call itself.
pub struct Facts<'a> {
    pub spend: &'a SpendToday,
    pub counterparties: &'a Counterparties,
    pub created: &'a CreatedLedger,
    pub taint: &'a [ProvenanceClass],
    pub thread: Option<&'a ThreadRef>,
    /// Outside words the run carries that taint doesn't name: an untrusted
    /// origin, or a workflow started by an inbound payload.
    pub outside_source: Option<&'a str>,
    /// The call's input (the job's folders are matched on it).
    pub input: &'a serde_json::Value,
}

/// A call the code could not decide: whether it publishes, or speaks for
/// the owner to someone outside, isn't in its input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub tool: String,
    /// The owner-facing line for the call.
    pub activity: String,
    /// The call's input, with secrets removed.
    pub input: serde_json::Value,
    /// The outside words the run carries, when it carries any: a "yes" is
    /// then case 5 rather than case 2.
    pub untrusted: Option<String>,
}

impl Question {
    /// The ask a "yes, it goes outside" answer raises.
    pub fn ask_case(&self) -> AskCase {
        match &self.untrusted {
            Some(source) => AskCase::UntrustedInput { source: source.clone() },
            None => AskCase::NewCounterparty { who: PUBLIC.to_string() },
        }
    }
}

/// Who a publish reaches.
pub const PUBLIC: &str = "the public";

/// What the cases decided for one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaseVerdict {
    Clear,
    Ask(AskCase),
    Undecided(Question),
}

/// The five cases, in order 1-5. The first case code decides wins; a call
/// no case asks about but whose outward effect is unknown is undecided.
pub fn surfaced(t: &Target, rules: &RuleSet, f: &Facts<'_>) -> CaseVerdict {
    if let Some(case) = money(t, rules, f.spend) {
        return CaseVerdict::Ask(case);
    }
    let undecided = match new_counterparty(t, rules, f) {
        Ok(Some(case)) => return CaseVerdict::Ask(case),
        Ok(None) => None,
        Err(q) => Some(q),
    };
    if let Some(case) = irreversible(t, rules, f) {
        return CaseVerdict::Ask(case);
    }
    if let Some(case) = outside_job(t, rules, f.input) {
        return CaseVerdict::Ask(case);
    }
    if let Some(case) = untrusted_input(t, rules, f) {
        return CaseVerdict::Ask(case);
    }
    match undecided {
        Some(q) => CaseVerdict::Undecided(q),
        None => CaseVerdict::Clear,
    }
}

/// Case 1: a standing allow's money limits against today's spend.
pub fn money(t: &Target, rules: &RuleSet, spend: &SpendToday) -> Option<AskCase> {
    let limit = rules.money_limit(t)?;
    let cents = t.effects.money_cents.unwrap_or(0);
    let named = t.effects.counterparty.as_deref().is_some_and(|c| !c.is_empty());
    let over = |limit: Option<i64>, used: i64| limit.is_some_and(|l| used > l);
    let exceeded = over(limit.per_action_cents, cents)
        || over(limit.per_day_count, spend.count + 1)
        || over(limit.per_day_cents, spend.cents + cents)
        || (named && over(limit.per_counterparty_day_cents, spend.counterparty_cents + cents));
    exceeded.then(|| AskCase::Money { cents, limit_cents: limit.per_action_cents.or(limit.per_day_cents) })
}

/// Case 2: a first message to someone the employee doesn't work with, or a
/// publish. `Err` when the input can't show whether the call publishes.
pub fn new_counterparty(t: &Target, rules: &RuleSet, f: &Facts<'_>) -> Result<Option<AskCase>, Question> {
    if let Some(who) = t.effects.recipients.iter().find(|r| !known(r, rules, f)) {
        return Ok(Some(AskCase::NewCounterparty { who: who.clone() }));
    }
    match t.effects.publishes {
        Knowable::Yes if published_before(t, rules) => Ok(None),
        Knowable::Yes => Ok(Some(AskCase::NewCounterparty { who: PUBLIC.to_string() })),
        Knowable::No => Ok(None),
        // A read changes nothing outside; its effects are never unknown in
        // a way that matters.
        Knowable::Unknown if t.read_only || !t.effects.recipients.is_empty() => Ok(None),
        Knowable::Unknown => Err(Question {
            tool: t.tool.clone(),
            activity: String::new(),
            input: super::judgement::redact(f.input),
            untrusted: untrusted_source(f),
        }),
    }
}

/// Whether the owner answered "Allow always" to a publish by this call's
/// key: the answer's rule names the public as the recipient.
fn published_before(t: &Target, rules: &RuleSet) -> bool {
    let mut public = t.clone();
    public.field = Some(RuleField::Recipient(PUBLIC.to_string()));
    rules.answered_always(&public)
}

/// Whether a recipient is someone the employee already works with: in the
/// thread, in its history, or named by an allow rule ("Allow always" on an
/// earlier case 2 ask writes one).
fn known(recipient: &str, rules: &RuleSet, f: &Facts<'_>) -> bool {
    let Some(key) = types::permissions::address_key(recipient) else {
        return false;
    };
    if f.counterparties.0.contains(&key) || f.thread.is_some_and(|th| th.people.contains(&key)) {
        return true;
    }
    let domain = key.rsplit_once('@').map(|(_, d)| d);
    rules.all().filter(|r| r.effect == Effect::Allow).any(|r| match &r.field {
        Some(RuleField::Recipient(who)) => types::permissions::address_key(who).as_deref() == Some(key.as_str()),
        Some(RuleField::Domain(d)) => domain.is_some_and(|dom| {
            let d = d.to_ascii_lowercase();
            dom == d || dom.ends_with(&format!(".{d}"))
        }),
        _ => false,
    })
}

/// Case 3: deleting what the employee didn't create, or replacing it
/// outside the folders its job works in.
pub fn irreversible(t: &Target, rules: &RuleSet, f: &Facts<'_>) -> Option<AskCase> {
    // "Allow always" on an earlier ask for this key and field.
    if rules.answered_always(t) {
        return None;
    }
    if let Some(what) = t.effects.deletes.iter().find(|d| !f.created.0.contains(*d)) {
        return Some(AskCase::Irreversible { what: plain(what) });
    }
    let folders = rules.folders();
    let in_job_folders = |what: &str| {
        what.strip_prefix("file:").is_some_and(|p| {
            let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
            folders.iter().any(|folder| abs(Path::new(p)).starts_with(abs(folder)))
        })
    };
    t.effects
        .overwrites
        .iter()
        .find(|o| !f.created.0.contains(*o) && !in_job_folders(o))
        .map(|what| AskCase::Irreversible { what: plain(what) })
}

/// `workflow:weekly report` → `the workflow "weekly report"`.
fn plain(named: &str) -> String {
    match named.split_once(':') {
        Some(("file", path)) => format!("the file {path}"),
        Some((kind, name)) => format!("the {} \"{name}\"", kind.rsplit('.').next().unwrap_or(kind)),
        None => named.to_string(),
    }
}

/// Case 4: a capability the job doesn't include.
pub fn outside_job(t: &Target, rules: &RuleSet, input: &serde_json::Value) -> Option<AskCase> {
    (!rules.in_job(t, input))
        .then(|| AskCase::OutsideJob { capability: t.capability.clone().unwrap_or_else(|| t.key.clone()) })
}

/// Case 5: the run read outside words and now wants to send, pay, publish
/// or delete. A reply inside the thread the words came from is not this
/// case.
pub fn untrusted_input(t: &Target, rules: &RuleSet, f: &Facts<'_>) -> Option<AskCase> {
    let source = untrusted_source(f)?;
    // "Allow always" on an earlier ask for this key and recipient.
    if rules.answered_always(t) {
        return None;
    }
    let e = &t.effects;
    let in_thread = |r: &String| {
        f.thread.is_some_and(|th| types::permissions::address_key(r).is_some_and(|k| th.people.contains(&k)))
    };
    let gated = t.operation.as_deref().is_some_and(tools::interface_catalog::is_gated);
    let outward = gated
        || e.money_cents.is_some_and(|c| c > 0)
        || !e.deletes.is_empty()
        || e.publishes == Knowable::Yes
        || e.recipients.iter().any(|r| !in_thread(r));
    // A gated operation whose only reach is the thread (a reply to the
    // person who wrote in) stays a reply.
    let thread_reply = !e.recipients.is_empty()
        && e.recipients.iter().all(in_thread)
        && e.money_cents.unwrap_or(0) == 0
        && e.deletes.is_empty()
        && e.publishes != Knowable::Yes;
    (outward && !thread_reply).then_some(AskCase::UntrustedInput { source })
}

/// The outside words the run carries, in plain words, or `None`.
fn untrusted_source(f: &Facts<'_>) -> Option<String> {
    if !f.taint.is_empty() {
        return Some(types::provenance::label_classes(f.taint));
    }
    f.outside_source.map(str::to_string)
}

/// The facts one call's cases read, loaded from the store.
pub struct Gathered {
    pub spend: SpendToday,
    pub counterparties: Counterparties,
    pub created: CreatedLedger,
    pub thread: Option<ThreadRef>,
    pub outside_source: Option<String>,
}

impl Gathered {
    /// Load what `t`'s cases need. A store that can't be read yields no
    /// history: every recipient is new and nothing was created, so the
    /// call asks rather than runs.
    pub fn load(cx: &CheckCx<'_>, rules: &RuleSet, t: &Target) -> Gathered {
        let agent = &cx.grant.agent_id;
        let spend = match (rules.money_limit(t), rules.decide(t)) {
            (Some(_), Some((rule, _))) => cx
                .store
                .permission_spend(agent, &super::today(), rule.key.value(), t.effects.counterparty.as_deref().unwrap_or(""))
                .unwrap_or_default(),
            _ => SpendToday::default(),
        };
        let counterparties = Counterparties(
            cx.store.known_counterparties(agent, &t.effects.recipients).unwrap_or_default(),
        );
        let mine: Vec<String> = t.effects.deletes.iter().chain(&t.effects.overwrites).cloned().collect();
        let created = CreatedLedger(cx.store.created_by(agent, &mine).unwrap_or_default());
        let thread = thread_of(cx.store, &cx.ctx.session_key);
        let outside_source = if cx.ctx.untrusted_input {
            Some("the run's input".to_string())
        } else if !cx.ctx.origin.is_trusted() {
            Some(super::limits::origin_label(cx.ctx.origin).to_string())
        } else {
            None
        };
        Gathered { spend, counterparties, created, thread, outside_source }
    }

    pub fn facts<'a>(&'a self, taint: &'a [ProvenanceClass], input: &'a serde_json::Value) -> Facts<'a> {
        Facts {
            spend: &self.spend,
            counterparties: &self.counterparties,
            created: &self.created,
            taint,
            thread: self.thread.as_ref(),
            outside_source: self.outside_source.as_deref(),
            input,
        }
    }
}

/// The people the run's thread is with: a case turn serves the person its
/// case is about (every alias the case knows them by).
fn thread_of(store: &db::Store, session_key: &str) -> Option<ThreadRef> {
    let run_id = tools::origin::workflow_run_id(session_key)?;
    let run = store.engine_get_run(run_id).ok()??;
    let inputs: serde_json::Value = serde_json::from_str(run.inputs.as_deref()?).ok()?;
    let people: Vec<String> = inputs["_case"]["aliases"]
        .as_array()?
        .iter()
        .filter_map(|a| a["value"].as_str())
        .filter_map(types::permissions::address_key)
        .collect();
    (!people.is_empty()).then_some(ThreadRef { people })
}

#[cfg(test)]
mod tests;
