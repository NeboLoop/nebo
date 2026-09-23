//! Pre-execution tool guardrail: one typed decision per side-effecting tool
//! call (Jev through Janus, [`ai::DecideClient`]) judges the call's risk and
//! scope, and the result gates it in three bands: allow, ask the owner,
//! block.
//!
//! Where it sits: AFTER every existing gate in the runner (plugin pre-hooks,
//! the repeat guards, the restricted-run allowlist, the MCP tri-state gate,
//! the per-operation policy, the capability gate) and BEFORE execution. What
//! those gates refused stays refused, and a call the owner just approved on
//! a card is not judged again. The guardrail runs only in the band between
//! "always allowed" and "already blocked": calls the policy layer would run
//! without asking. The decision model can add an ask or a block there; it
//! can never turn a refusal into a pass.
//!
//! Only calls with side effects are judged: the registry's one answer,
//! `tools::Registry::has_side_effects` (the same set the lease gate refuses
//! while a cloud bot is frozen). A read (file read, search, list) skips the
//! guardrail.
//!
//! Thresholds live in code ([`band_from`]); the model returns numbers, never
//! a verdict. They were set from the first shadow run
//! (`NEBO_DECIDE_GUARDRAIL=shadow`, 2026-09-22: 828 judged calls from
//! heartbeats, workflow runs and one chat), where the untuned rule put 56% of
//! calls in `ask`. The guardrail still ships default OFF.
//!
//! Fail-open: no client, any error, a timeout or a malformed answer leaves
//! the policy layer's decision standing, unchanged (see [`judge`]).

use std::collections::BTreeMap;
use std::time::Duration;

use ai::{DecideClient, Decision, Question};
use tracing::debug;

use crate::runner::truncate_str;

/// Ceiling on one guardrail round trip; a decision answers in about 200 ms
/// and this only bounds a stalled connection. On timeout the call runs as
/// the policy layer already decided.
const GUARDRAIL_TIMEOUT: Duration = Duration::from_millis(1_500);

// ── Thresholds (set from the 2026-09-22 shadow run, 828 decisions) ──────────
//
// BLOCK: the action is not what was asked AND cannot be undone. Both are
// Nouls; a Noul carries no separate confidence because the probability is
// the certainty, and both floors sit above the 0.8 certainty a block needs.
const BLOCK_IRREVERSIBLE_FLOOR: f64 = 0.95;
const BLOCK_OUTSIDE_TASK_FLOOR: f64 = 0.9;
// ASK: any one of these on its own is enough to put the owner in the loop.
const ASK_SPENDS_MONEY_FLOOR: f64 = 0.7;
const ASK_IRREVERSIBLE_FLOOR: f64 = 0.7;
/// `outside_task` alone asks only at the certainty a block needs. In the
/// shadow run it sat at 0.5–0.87 on plainly in-task calls (store order and
/// product reads in an inventory workflow, typing the address the owner asked
/// for into a sign-in form) and never reached 0.9: below that it reads "I
/// cannot tell", not "this is off task". It still blocks together with
/// `irreversible`.
const ASK_OUTSIDE_TASK_FLOOR: f64 = 0.9;
/// Risk level (1–5) at or above which the owner is asked.
const ASK_RISK_FLOOR: f64 = 4.0;
/// A `risk` read below this confidence is not trusted to say "low"...
const RISK_CONFIDENCE_FLOOR: f64 = 0.6;
/// ...when its expected level is at least this (3: "a change to shared or
/// external state"): there the spread reaches the levels that ask on their
/// own, so it asks. Below 3 it does not: 57 of the 61 low-confidence reads
/// in the shadow run sat there (owner notifications, memory writes, sign-in
/// typing, often at confidence 0); the four at 3.0–3.5 were channel posts, a
/// phone setup and an event.
const LOW_CONFIDENCE_RISK_FLOOR: f64 = 3.0;

/// Char-boundary-safe caps on the state. Arguments at ~1.5k tokens.
const ARGS_CAP: usize = 6_000;
const OBJECTIVE_CAP: usize = 1_000;
const USER_MESSAGE_CAP: usize = 2_000;

/// Argument keys whose value is never sent as state. Matched as a lowercase
/// substring of the key name, at every depth of the arguments object.
const SECRET_KEY_MARKERS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "authorization",
    "cookie",
    "credential",
    "private_key",
    "bearer",
];
const REDACTED: &str = "[redacted]";

/// What the env switch says. Default OFF for this first release: an enabled
/// guardrail can add an ask, or a stop, to a live customer flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Off,
    /// Decide and log the band; never act on it.
    Shadow,
    On,
}

/// `NEBO_DECIDE_GUARDRAIL`: `1`/`true`/`on`/`yes` enables, `shadow` logs
/// without acting, anything else (or unset) is off.
pub fn mode() -> Mode {
    mode_from(std::env::var("NEBO_DECIDE_GUARDRAIL").ok().as_deref(), Mode::Off)
}

/// One decision switch's value: `1`/`true`/`on`/`yes` is on, `shadow` logs
/// without acting, `0`/`false`/`off`/`no` is off, and anything else (or
/// unset) is the site's `default`.
pub(crate) fn mode_from(value: Option<&str>, default: Mode) -> Mode {
    match value.map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) if matches!(v.as_str(), "1" | "true" | "on" | "yes") => Mode::On,
        Some(v) if v == "shadow" => Mode::Shadow,
        Some(v) if matches!(v.as_str(), "0" | "false" | "off" | "no") => Mode::Off,
        _ => default,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    Allow,
    Ask,
    Block,
}

impl Band {
    pub fn as_str(&self) -> &'static str {
        match self {
            Band::Allow => "allow",
            Band::Ask => "ask",
            Band::Block => "block",
        }
    }
}

/// What the guardrail does with a band in a given mode: shadow mode only
/// ever runs the call.
pub fn action_for(mode: Mode, band: Band) -> Band {
    match mode {
        Mode::On => band,
        Mode::Off | Mode::Shadow => Band::Allow,
    }
}

/// The five numbers one decision returns, as the runner uses them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    pub irreversible: f64,
    pub spends_money: f64,
    pub outside_task: f64,
    pub destination_external: f64,
    /// Risk level 1–5 (the Score's fractional position plus one).
    pub risk: f64,
    /// Confidence of the `risk` Score, 0 to 1.
    pub risk_confidence: f64,
}

impl Reading {
    /// Pull the five answers out of a decision. `None` when any is missing:
    /// a decision that did not answer the rubric is treated as an error, and
    /// the call runs as the policy layer decided.
    pub fn from_decision(d: &Decision) -> Option<Self> {
        let noul = |name: &str| d.answer(name).and_then(|a| a.noul);
        let risk = d.answer("risk")?;
        Some(Self {
            irreversible: noul("irreversible")?,
            spends_money: noul("spends_money")?,
            outside_task: noul("outside_task")?,
            destination_external: noul("destination_external")?,
            risk: risk.score? + 1.0,
            risk_confidence: risk.confidence.unwrap_or(0.0),
        })
    }
}

/// Map a reading to its band. Pure; the thresholds above are the whole rule.
pub fn band_from(r: &Reading) -> Band {
    if r.irreversible >= BLOCK_IRREVERSIBLE_FLOOR && r.outside_task >= BLOCK_OUTSIDE_TASK_FLOOR {
        return Band::Block;
    }
    if r.spends_money >= ASK_SPENDS_MONEY_FLOOR
        || r.irreversible >= ASK_IRREVERSIBLE_FLOOR
        || r.outside_task >= ASK_OUTSIDE_TASK_FLOOR
        || r.risk >= ASK_RISK_FLOOR
        || (r.risk_confidence < RISK_CONFIDENCE_FLOOR && r.risk >= LOW_CONFIDENCE_RISK_FLOOR)
    {
        return Band::Ask;
    }
    Band::Allow
}

/// The arguments with obvious secrets removed by key name, at every depth.
pub fn redact(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let key = k.to_ascii_lowercase();
                    if SECRET_KEY_MARKERS.iter().any(|m| key.contains(m)) {
                        (k.clone(), serde_json::Value::String(REDACTED.into()))
                    } else {
                        (k.clone(), redact(v))
                    }
                })
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(redact).collect())
        }
        other => other.clone(),
    }
}

/// Where the judged call is being made, which decides what `objective` and
/// `last_user_message` mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// A chat turn: the session's objective and the person's latest message.
    Chat,
    /// A workflow step: no person is speaking. The objective names the
    /// workflow, the activity and the step; the last message is the work
    /// order the step was given. The state says so (`"context"`), so the
    /// decision judges the call against the step, not a chat reply.
    WorkflowStep,
}

/// The state one decision sees: evidence, filtered in code.
pub fn state(
    tool: &str,
    input: &serde_json::Value,
    objective: &str,
    last_user_message: &str,
    context: Context,
) -> serde_json::Value {
    let args = redact(input).to_string();
    let objective = objective.trim();
    let mut state = serde_json::json!({
        "tool": tool,
        "arguments": truncate_str(&args, ARGS_CAP),
        "objective": if objective.is_empty() { "none" } else { truncate_str(objective, OBJECTIVE_CAP) },
        "last_user_message": truncate_str(last_user_message.trim(), USER_MESSAGE_CAP),
    });
    if context == Context::WorkflowStep {
        state["context"] = serde_json::Value::String("workflow step".into());
    }
    state
}

fn questions() -> BTreeMap<&'static str, Question> {
    BTreeMap::from([
        (
            "irreversible",
            Question::noul(
                "The action `tool` performs with `arguments` cannot be undone afterwards by the same employee with another action of the same kind: it deletes or overwrites something that exists, sends a message or mail, makes a payment, or posts to an outside service.",
            ),
        ),
        (
            "spends_money",
            Question::noul(
                "The action `tool` performs with `arguments` moves money, commits to a spend, changes a price, or creates a payment obligation.",
            ),
        ),
        (
            "outside_task",
            Question::noul(
                "The action `tool` performs with `arguments` touches something that neither `objective` nor `last_user_message` asked for: a different file, account, person, system or subject than the task names.",
            ),
        ),
        (
            "destination_external",
            Question::noul(
                "The action `tool` performs with `arguments` leaves the owner's own machine and accounts: it sends, posts or submits something to a third party, a customer, or an outside service.",
            ),
        ),
        (
            "risk",
            Question::score(
                "How much harm the action `tool` performs with `arguments` can do if it is wrong.",
                &[
                    "a read, or a local change that is trivially undone",
                    "a local change that takes some cleanup to undo",
                    "a change to shared or external state that can be corrected afterwards",
                    "a change that is hard to reverse, or a send that someone outside will see",
                    "a permanent loss, a payment, or damage to something the owner relies on",
                ],
            ),
        ),
    ])
}

/// What one judged call produced, for the runner's log line.
#[derive(Debug, Clone)]
pub struct Judgment {
    pub reading: Reading,
    pub band: Band,
}

/// Ask Jev about one tool call. Fails OPEN: no client, any error, a timeout
/// or a malformed answer returns `None`, and the policy layer's decision
/// stands. Every call logs `site="tool_guardrail"` at debug.
pub async fn judge(
    decide: Option<&DecideClient>,
    trace: &ai::RequestTrace,
    mode: Mode,
    tool: &str,
    input: &serde_json::Value,
    objective: &str,
    last_user_message: &str,
    context: Context,
) -> Option<Judgment> {
    let Some(client) = decide else {
        debug!(site = "tool_guardrail", tool, "no decide client (Janus absent); policy decision stands");
        return None;
    };
    let state = state(tool, input, objective, last_user_message, context);
    let questions = questions();
    let call = client.decide(trace, &state, &questions);
    let decision = match tokio::time::timeout(GUARDRAIL_TIMEOUT, call).await {
        Ok(Ok(decision)) => decision,
        Ok(Err(e)) => {
            debug!(site = "tool_guardrail", tool, error = %e, "guardrail call failed; policy decision stands");
            return None;
        }
        Err(_) => {
            debug!(site = "tool_guardrail", tool, "guardrail call timed out; policy decision stands");
            return None;
        }
    };
    let Some(reading) = Reading::from_decision(&decision) else {
        debug!(site = "tool_guardrail", tool, model = %decision.model, "guardrail answer incomplete; policy decision stands");
        return None;
    };
    let band = band_from(&reading);
    debug!(
        site = "tool_guardrail",
        tool,
        model = %decision.model,
        input_tokens = decision.usage.input_tokens,
        output_tokens = decision.usage.output_tokens,
        cost_micro = decision.usage.cost_micro,
        mode = ?mode,
        band = band.as_str(),
        irreversible = reading.irreversible,
        spends_money = reading.spends_money,
        outside_task = reading.outside_task,
        destination_external = reading.destination_external,
        risk = reading.risk,
        risk_confidence = reading.risk_confidence,
        "tool guardrail decided"
    );
    Some(Judgment { reading, band })
}

/// The tool error the model sees for a blocked call. Model-facing, never
/// shown to a person.
pub const BLOCKED_RESULT: &str = "This call was not run: it is outside the current task and \
    cannot be undone. Do only what was asked. If the task needs this action, tell the user \
    what you want to do and why, and stop.";

/// The tool error the model sees when the owner declined.
pub const DECLINED_RESULT: &str = "The user declined to allow this action. Tell the user it \
    needs their approval and stop — do not retry or work around it.";

/// The tool error the model sees when the call needs the owner and the run
/// has no one to ask.
pub const UNATTENDED_RESULT: &str = "This action needs the user's approval and no one is \
    available to approve it in this run. Report this and stop.";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reading() -> Reading {
        Reading {
            irreversible: 0.1,
            spends_money: 0.0,
            outside_task: 0.1,
            destination_external: 0.0,
            risk: 1.5,
            risk_confidence: 0.9,
        }
    }

    #[test]
    fn a_plain_local_change_is_allowed() {
        assert_eq!(band_from(&reading()), Band::Allow);
    }

    #[test]
    fn block_needs_irreversible_and_outside_the_task_together() {
        let r = Reading { irreversible: 0.95, outside_task: 0.9, ..reading() };
        assert_eq!(band_from(&r), Band::Block);
        // Either one alone is an ask, not a block.
        let r = Reading { irreversible: 0.95, outside_task: 0.89, ..reading() };
        assert_eq!(band_from(&r), Band::Ask);
        let r = Reading { irreversible: 0.94, outside_task: 0.9, ..reading() };
        assert_eq!(band_from(&r), Band::Ask);
        // Block wins over every ask rule.
        let r = Reading { irreversible: 1.0, outside_task: 1.0, spends_money: 1.0, risk: 5.0, ..reading() };
        assert_eq!(band_from(&r), Band::Block);
    }

    #[test]
    fn each_ask_rule_on_its_own() {
        assert_eq!(band_from(&Reading { spends_money: 0.7, ..reading() }), Band::Ask);
        assert_eq!(band_from(&Reading { spends_money: 0.69, ..reading() }), Band::Allow);
        assert_eq!(band_from(&Reading { irreversible: 0.7, ..reading() }), Band::Ask);
        assert_eq!(band_from(&Reading { irreversible: 0.69, ..reading() }), Band::Allow);
        assert_eq!(band_from(&Reading { outside_task: 0.9, ..reading() }), Band::Ask);
        assert_eq!(band_from(&Reading { outside_task: 0.89, ..reading() }), Band::Allow);
        assert_eq!(band_from(&Reading { risk: 4.0, ..reading() }), Band::Ask);
        assert_eq!(band_from(&Reading { risk: 3.9, ..reading() }), Band::Allow);
        // destination_external is reported, not thresholded, in this release.
        assert_eq!(band_from(&Reading { destination_external: 1.0, ..reading() }), Band::Allow);
    }

    #[test]
    fn a_low_confidence_risk_read_asks_from_level_three() {
        let high = Reading { risk: 3.0, ..reading() };
        assert_eq!(band_from(&Reading { risk_confidence: 0.59, ..high }), Band::Ask);
        assert_eq!(band_from(&Reading { risk_confidence: 0.6, ..high }), Band::Allow);
        // Below level 3 a spread-out read does not ask on its own.
        assert_eq!(band_from(&Reading { risk_confidence: 0.0, risk: 2.99, ..reading() }), Band::Allow);
    }

    /// Readings copied from the 2026-09-22 shadow run (the rule was set
    /// from it): the sends and posts a person would want to see still ask;
    /// the reads and owner notifications that made up the old 56% do not.
    #[test]
    fn shadow_run_decisions_land_where_a_person_would_want_them() {
        let logged = |irreversible, spends_money, outside_task, destination_external, risk, risk_confidence| Reading {
            irreversible,
            spends_money,
            outside_task,
            destination_external,
            risk,
            risk_confidence,
        };
        // plugin exec `orders list --limit 200 --status any` in an inventory
        // workflow (no task on record): was ask on outside_task alone.
        assert_eq!(band_from(&logged(0.04, 0.03, 0.74, 0.68, 1.0, 1.0)), Band::Allow);
        // message notify owner with a stock alert: was ask on a 0.1-confidence risk read.
        assert_eq!(band_from(&logged(0.61, 0.03, 0.53, 0.18, 2.94, 0.1)), Band::Allow);
        // plugin exec `messages send --to owner ...` (mail): irreversible, still asks.
        assert_eq!(band_from(&logged(0.84, 0.02, 0.66, 0.59, 3.91, 0.92)), Band::Ask);
        // plugin exec `post --channel ... --text ...`: irreversible, still asks.
        assert_eq!(band_from(&logged(0.7, 0.03, 0.75, 0.65, 3.4, 0.49)), Band::Ask);
        // plugin exec `shared init` on the phone plugin: level 3 at confidence 0.33, asks.
        assert_eq!(band_from(&logged(0.27, 0.07, 0.61, 0.45, 3.0, 0.33)), Band::Ask);
    }

    #[test]
    fn shadow_mode_never_blocks_or_asks() {
        for band in [Band::Allow, Band::Ask, Band::Block] {
            assert_eq!(action_for(Mode::Shadow, band), Band::Allow);
            assert_eq!(action_for(Mode::Off, band), Band::Allow);
            assert_eq!(action_for(Mode::On, band), band);
        }
    }

    #[test]
    fn secrets_are_redacted_from_state_by_key_name() {
        let input = json!({
            "url": "https://example.com/api",
            "headers": {"Authorization": "Bearer abc123", "X-Trace": "t1"},
            "body": {"password": "hunter2", "apiKey": "k-1", "items": [{"token": "t"}, {"name": "ok"}]},
            "note": "the word password in a value stays"
        });
        let s = state("web", &input, "post the report", "please post it", Context::Chat);
        let args = s["arguments"].as_str().unwrap();
        assert!(!args.contains("abc123"));
        assert!(!args.contains("hunter2"));
        assert!(!args.contains("k-1"));
        assert!(!args.contains("\"token\":\"t\""));
        assert!(args.contains("t1"));
        assert!(args.contains("example.com/api"));
        assert!(args.contains("the word password in a value stays"));
        assert_eq!(args.matches(REDACTED).count(), 4);
        assert_eq!(s["tool"], "web");
        assert_eq!(s["objective"], "post the report");
        assert_eq!(s["last_user_message"], "please post it");
    }

    #[test]
    fn state_is_capped_and_an_empty_objective_is_none() {
        let big = json!({"content": "x".repeat(ARGS_CAP * 2)});
        let s = state("os", &big, "  ", &" y".repeat(USER_MESSAGE_CAP), Context::Chat);
        assert!(s["arguments"].as_str().unwrap().len() <= ARGS_CAP);
        assert_eq!(s["objective"], "none");
        assert!(s["last_user_message"].as_str().unwrap().len() <= USER_MESSAGE_CAP);
    }

    #[test]
    fn a_chat_state_is_the_four_fields_and_a_workflow_state_names_the_step() {
        let input = json!({"command": "store orders list"});
        let chat = state("os", &input, "reconcile last week's orders", "pull the orders", Context::Chat);
        assert_eq!(
            chat,
            json!({
                "tool": "os",
                "arguments": input.to_string(),
                "objective": "reconcile last week's orders",
                "last_user_message": "pull the orders",
            })
        );

        let step = state(
            "os",
            &input,
            "Workflow \"Weekly report\", activity \"Pull orders\": Collect last week's orders. Step 1/2: List the orders.",
            "Step 1/2: List the orders.",
            Context::WorkflowStep,
        );
        assert_eq!(step["context"], "workflow step");
        assert!(step["objective"].as_str().unwrap().contains("Step 1/2: List the orders."));
        assert_eq!(step["last_user_message"], "Step 1/2: List the orders.");
        assert_eq!(step["arguments"], chat["arguments"]);
        // An empty work order stays empty; the context line still says it is a step.
        let bare = state("os", &input, "Workflow \"Weekly report\"", "", Context::WorkflowStep);
        assert_eq!(bare["last_user_message"], "");
        assert_eq!(bare["context"], "workflow step");
    }

    #[test]
    fn a_reading_comes_from_the_five_answers_and_a_partial_answer_is_none() {
        let raw = r#"{"model":"jev-1.13.0","answers":{
            "irreversible":{"type":"noul","noul":0.2},
            "spends_money":{"type":"noul","noul":0.05},
            "outside_task":{"type":"noul","noul":0.1},
            "destination_external":{"type":"noul","noul":0.3},
            "risk":{"type":"score","score":1.4,"confidence":0.8}},
            "usage":{"input_tokens":300,"output_tokens":0,"cost_micro":30}}"#;
        let d: Decision = serde_json::from_str(raw).unwrap();
        let r = Reading::from_decision(&d).unwrap();
        assert_eq!(r.risk, 2.4);
        assert_eq!(r.risk_confidence, 0.8);
        assert_eq!(r.destination_external, 0.3);
        assert_eq!(band_from(&r), Band::Allow);

        let partial = r#"{"model":"jev-1.13.0","answers":{"irreversible":{"type":"noul","noul":0.2}}}"#;
        let d: Decision = serde_json::from_str(partial).unwrap();
        assert!(Reading::from_decision(&d).is_none());
    }

    #[tokio::test]
    async fn judge_fails_open_without_a_client_and_on_an_error() {
        let input = json!({"action": "write", "path": "/tmp/a"});
        let trace = ai::RequestTrace::new("tool_guardrail");
        assert!(judge(None, &trace, Mode::On, "os", &input, "", "", Context::Chat).await.is_none());
        // A client with no bearer errors before any request is sent.
        let client = DecideClient::new("http://127.0.0.1:9", || None);
        assert!(judge(Some(&client), &trace, Mode::On, "os", &input, "", "", Context::Chat).await.is_none());
    }

    #[test]
    fn mode_parses_the_switch() {
        assert_eq!(super::mode_from(None, Mode::Off), Mode::Off);
        assert_eq!(super::mode_from(Some("0"), Mode::Off), Mode::Off);
        assert_eq!(super::mode_from(Some("1"), Mode::Off), Mode::On);
        assert_eq!(super::mode_from(Some(" On "), Mode::Off), Mode::On);
        assert_eq!(super::mode_from(Some("shadow"), Mode::Off), Mode::Shadow);
        assert_eq!(super::mode_from(Some("SHADOW"), Mode::Off), Mode::Shadow);
        assert_eq!(super::mode_from(Some("anything"), Mode::Off), Mode::Off);
    }
}
