//! The one end-of-turn hook. When the model answers without tool calls,
//! every registered check runs; one that says continue sends the loop into
//! another step with its reminder, one that says exit ends the turn.
//! Continuation is a loop transition, never a new run and never a stored
//! owner message. An app ends a turn from outside through `app_halt`,
//! asked before every step.

use std::collections::{HashMap, HashSet};

use super::TurnMode;
use super::events::TurnEvent;
use super::goal::GoalCheck;
use super::turn::TurnExit;

/// What an end check reads when the model has answered.
pub struct TurnEnd<'a> {
    /// The conversation the model just answered, as it was sent plus the
    /// answer.
    pub transcript: &'a [ai::Message],
    /// Model calls taken so far in this turn (1-based).
    pub step: u32,
    /// End checks that already continued this turn.
    pub checks_this_turn: u8,
}

/// A check the turn must pass before it ends.
#[async_trait::async_trait]
pub trait EndCheck: Send + Sync {
    fn name(&self) -> &'static str;
    async fn check(&self, end: &TurnEnd<'_>) -> EndVerdict;
}

/// What an end check decided.
#[derive(Debug, Clone)]
pub enum EndVerdict {
    /// Nothing to add; the turn may end.
    Stop,
    /// Take another step; the event becomes its reminder.
    Continue(TurnEvent),
    Exit(TurnExit),
}

/// The name the workflow contract's continue reminder carries.
pub const WORKFLOW_CONTRACT: &str = "workflow_contract";

/// A workflow activity's `LoopTurn` contract: the least number of steps and
/// the tools that must land a successful call before the turn may end.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowContract {
    pub min_iterations: u32,
    pub requires_tools: Vec<String>,
}

/// The workflow contract as an end check.
pub struct WorkflowContractCheck(pub WorkflowContract);

#[async_trait::async_trait]
impl EndCheck for WorkflowContractCheck {
    fn name(&self) -> &'static str {
        WORKFLOW_CONTRACT
    }

    async fn check(&self, end: &TurnEnd<'_>) -> EndVerdict {
        let contract = &self.0;
        if end.step < contract.min_iterations {
            return continue_with(format!(
                "This step asks for at least {} rounds of work and this was round {}. Take the next action.",
                contract.min_iterations, end.step
            ));
        }
        let landed = succeeded_tools(end.transcript);
        let missing: Vec<&str> = contract
            .requires_tools
            .iter()
            .filter(|t| !landed.contains(t.as_str()))
            .map(String::as_str)
            .collect();
        if missing.is_empty() {
            return EndVerdict::Stop;
        }
        continue_with(format!(
            "This step isn't done until a {} call succeeds, and none has yet. Make the call.",
            missing.join(" and ")
        ))
    }
}

/// Times the answer check sends a helper back before the turn ends as it
/// is (the caller reads it and reports what's wrong).
pub const ANSWER_SHAPE_RETRIES: u8 = 2;

/// A helper whose caller reads its final answer as data: the answer must be
/// one JSON object matching the schema (Claude Code's structured-output
/// Stop enforcement, `registerStructuredOutputEnforcement`, with the answer
/// as the turn's final text rather than a per-task tool, so the tool list
/// stays the one list).
pub struct AnswerShapeCheck(pub std::sync::Arc<serde_json::Value>);

/// The JSON object a final answer carries, if it carries one.
pub fn answer_object(text: &str) -> Option<serde_json::Value> {
    let json = crate::memory::extract_json_object_pub(text)?;
    serde_json::from_str::<serde_json::Value>(&json).ok().filter(|v| v.is_object())
}

/// What's wrong with `answer` against `schema`; empty when it matches.
pub fn answer_issues(schema: &serde_json::Value, answer: &str) -> Vec<String> {
    let Some(value) = answer_object(answer) else {
        return vec!["there is no JSON object in it".to_string()];
    };
    match a2ui_validation::validate(schema, &value, "answer") {
        Ok(()) => Vec::new(),
        Err(errors) => errors.iter().map(|e| format!("{}: {}", e.path, e.message)).collect(),
    }
}

#[async_trait::async_trait]
impl EndCheck for AnswerShapeCheck {
    fn name(&self) -> &'static str {
        "answer_shape"
    }

    async fn check(&self, end: &TurnEnd<'_>) -> EndVerdict {
        let answer = end.transcript.iter().rev().find(|m| m.role == "assistant").map(|m| m.content.as_str()).unwrap_or("");
        let issues = answer_issues(&self.0, answer);
        if issues.is_empty() || end.checks_this_turn >= ANSWER_SHAPE_RETRIES {
            return EndVerdict::Stop;
        }
        EndVerdict::Continue(TurnEvent::AnswerShape(format!(
            "Your final answer must be one JSON object matching the schema you were given, and nothing else. It doesn't \
             yet:\n{}\nAnswer again with the corrected object.",
            issues.iter().map(|i| format!("- {i}")).collect::<Vec<_>>().join("\n")
        )))
    }
}

fn continue_with(text: String) -> EndVerdict {
    EndVerdict::Continue(TurnEvent::WorkflowContract(text))
}

/// Tools with at least one call whose result was not an error.
fn succeeded_tools(transcript: &[ai::Message]) -> HashSet<&str> {
    let mut names: HashMap<&str, &str> = HashMap::new();
    for call in transcript
        .iter()
        .filter_map(|m| m.tool_calls.as_ref()?.as_array())
        .flatten()
    {
        if let (Some(id), Some(name)) = (
            call.get("id").and_then(|v| v.as_str()),
            call.get("name").and_then(|v| v.as_str()),
        ) {
            names.insert(id, name);
        }
    }
    transcript
        .iter()
        .filter_map(|m| m.tool_results.as_ref()?.as_array())
        .flatten()
        .filter(|r| !r.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false))
        .filter_map(|r| names.get(r.get("tool_call_id")?.as_str()?).copied())
        .collect()
}

/// The apps subscribed to `agent.should_continue`, asked before every step
/// whether the employee may take it. An app that answers `false` halts the
/// turn there: it is how an app stops a running employee. Returns the
/// app's reason (empty when it gave none) when one said stop. An app never
/// keeps a turn going; a reply that does not say `false`, or no reply,
/// lets the step run.
pub async fn app_halt(
    hooks: &napp::HookDispatcher,
    session_id: &str,
    step: u32,
    called_tools: &[String],
    has_active_task: bool,
) -> Option<String> {
    if !hooks.has_subscribers("agent.should_continue") {
        return None;
    }
    let payload = serde_json::to_vec(&crate::hooks::ShouldContinuePayload {
        session_id: session_id.to_string(),
        turn: step as usize,
        total_tool_calls: called_tools.to_vec(),
        has_active_task,
    })
    .unwrap_or_default();
    let (result, _) = hooks.apply_filter("agent.should_continue", payload).await;
    match serde_json::from_slice::<crate::hooks::ShouldContinueResponse>(&result) {
        Ok(crate::hooks::ShouldContinueResponse { should_continue: false, reason }) => {
            Some(reason.unwrap_or_default().trim().to_string())
        }
        _ => None,
    }
}

/// What the checks of a turn are built from; the caller fills what it has.
#[derive(Default)]
pub struct EndChecks {
    /// The session's goal check (chat turns).
    pub goal: Option<GoalCheck>,
    /// The activity's contract (workflow turns).
    pub workflow_contract: Option<WorkflowContract>,
}

/// The checks a turn of `mode` runs at its end: the agreed-goal check for
/// chat turns, the workflow contract for workflow turns; none for helpers
/// and forks.
pub fn registry(mode: &TurnMode, checks: EndChecks) -> Vec<Box<dyn EndCheck>> {
    match mode {
        TurnMode::Chat => checks.goal.map(|g| Box::new(g) as Box<dyn EndCheck>).into_iter().collect(),
        TurnMode::Workflow(_) => checks
            .workflow_contract
            .map(|c| Box::new(WorkflowContractCheck(c)) as Box<dyn EndCheck>)
            .into_iter()
            .collect(),
        TurnMode::Helper { answer: Some(schema), .. } => vec![Box::new(AnswerShapeCheck(schema.clone())) as Box<dyn EndCheck>],
        TurnMode::Helper { .. } | TurnMode::Fork(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::events::attachment_for;

    fn end(transcript: &[ai::Message], step: u32) -> TurnEnd<'_> {
        TurnEnd {
            transcript,
            step,
            checks_this_turn: 0,
        }
    }

    fn said(text: &str) -> ai::Message {
        ai::Message { role: "assistant".into(), content: text.into(), ..Default::default() }
    }

    /// D13: a helper whose answer is read as data can't end on an answer in
    /// the wrong shape: it is sent back with what's wrong, twice at most,
    /// then ends as it is for its caller to report.
    #[tokio::test]
    async fn a_data_answer_must_match_its_schema() {
        let schema = std::sync::Arc::new(serde_json::json!({
            "type": "object", "properties": { "refuted": { "type": "boolean" } }, "required": ["refuted"]
        }));
        let helper = TurnMode::Helper {
            parent_session_key: "subagent:agent:a:web:h-1".into(),
            kind: crate::harness::delegation::HelperKind::General,
            depth: 2,
            answer: Some(schema),
        };
        let checks = registry(&helper, EndChecks::default());
        assert_eq!(checks.len(), 1, "a data answer is checked");
        let wrong = [said("It is probably false.")];
        let EndVerdict::Continue(event) = checks[0].check(&end(&wrong, 1)).await else { panic!("sent back") };
        let text = attachment_for(&event).unwrap().text;
        assert!(text.contains("one JSON object") && text.contains("there is no JSON object in it"), "{text}");
        let mistyped = [said(r#"{"refuted": "yes"}"#)];
        assert!(matches!(checks[0].check(&end(&mistyped, 2)).await, EndVerdict::Continue(_)), "a wrong type is sent back too");
        let right = [said(r#"Done. {"refuted": false}"#)];
        assert!(matches!(checks[0].check(&end(&right, 3)).await, EndVerdict::Stop));
        let tired = TurnEnd { transcript: &wrong, step: 4, checks_this_turn: ANSWER_SHAPE_RETRIES };
        assert!(matches!(checks[0].check(&tired).await, EndVerdict::Stop), "at most twice");
    }

    fn call(id: &str, name: &str) -> ai::Message {
        ai::Message {
            role: "assistant".into(),
            tool_calls: Some(serde_json::json!([{"id": id, "name": name, "input": {}}])),
            ..Default::default()
        }
    }

    fn result(id: &str, is_error: bool) -> ai::Message {
        ai::Message {
            role: "tool".into(),
            tool_results: Some(
                serde_json::json!([{"tool_call_id": id, "content": "ok", "is_error": is_error}]),
            ),
            ..Default::default()
        }
    }

    fn workflow_mode() -> TurnMode {
        TurnMode::Workflow(Box::new(crate::harness::WorkflowMode {
            trace: ai::RequestTrace::new("workflow"),
            ..Default::default()
        }))
    }

    fn contract() -> WorkflowContract {
        WorkflowContract {
            min_iterations: 2,
            requires_tools: vec!["send_mail".into()],
        }
    }

    #[tokio::test]
    async fn workflow_contract_continues_workflow_mode_only() {
        let checks = || EndChecks {
            goal: None,
            workflow_contract: Some(contract()),
        };
        // Chat, helpers and forks never run the workflow contract.
        assert!(registry(&TurnMode::Chat, checks()).is_empty());
        assert!(registry(&TurnMode::Fork(crate::harness::ForkKind::Review { staged: false }), checks()).is_empty());
        let helper = TurnMode::Helper {
            parent_session_key: "agent:a:web".into(),
            kind: crate::harness::delegation::HelperKind::General,
            depth: 1,
            answer: None,
        };
        assert!(registry(&helper, checks()).is_empty());

        let wf = registry(&workflow_mode(), checks());
        assert_eq!(
            wf.iter().map(|c| c.name()).collect::<Vec<_>>(),
            [WORKFLOW_CONTRACT]
        );
        let check = &wf[0];

        // Round 1 of at least 2: continue.
        let EndVerdict::Continue(ev) = check.check(&end(&[], 1)).await else {
            panic!("under min_iterations continues");
        };
        let row = attachment_for(&ev).unwrap();
        let text = row.text;
        assert_eq!(row.kind, "workflow_contract");
        assert!(text.contains("at least 2 rounds"), "{text}");

        // Enough rounds, but the required call failed: continue.
        let failed = [call("c1", "send_mail"), result("c1", true)];
        let EndVerdict::Continue(ev) = check.check(&end(&failed, 2)).await else {
            panic!("a failed required call is not the effect");
        };
        assert!(attachment_for(&ev).unwrap().text.contains("send_mail"));

        // The required call landed: the turn may end.
        let landed = [
            call("c1", "send_mail"),
            result("c1", true),
            call("c2", "send_mail"),
            result("c2", false),
        ];
        assert!(matches!(
            check.check(&end(&landed, 3)).await,
            EndVerdict::Stop
        ));
    }

    #[test]
    fn a_chat_turn_without_a_goal_check_has_no_checks() {
        assert!(registry(&TurnMode::Chat, EndChecks::default()).is_empty());
        assert!(registry(&workflow_mode(), EndChecks::default()).is_empty());
    }
}
