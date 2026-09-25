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
