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
    /// A case turn: an `exit` must carry the turn object in its reason
    /// (`workflow::cases::exit_refusal`), or it is refused and the turn
    /// goes on.
    pub case_turn: bool,
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
        // A delivery that didn't go through is said plainly, once: runs
        // reported "sent" after send_message was refused (bake-off
        // 2026-10-10, 7 runs). The step's output carries the fact either
        // way (`workflow_turn`).
        if end.checks_this_turn == 0 {
            let failed = failed_deliveries(end.transcript.iter().map(|m| (m.tool_calls.as_ref(), m.tool_results.as_ref())));
            if !failed.is_empty() {
                return continue_with(format!(
                    "{} Say plainly in your answer that it was not sent; never say it was. Try again only if the \
                     reason allows it.",
                    failed.iter().map(FailedDelivery::line).collect::<Vec<_>>().join(" ")
                ));
            }
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

/// The tools that deliver a step's result to someone: a coworker, or the
/// owner.
pub const DELIVERY_TOOLS: [&str; 3] = ["send_message", "message_owner", "push_notification"];

/// A delivery call that failed, with no later call of the same tool to the
/// same recipient that went through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedDelivery {
    pub tool: String,
    /// Who it was for: the coworker or team named, else the owner.
    pub to: String,
    /// The first line of what the tool said.
    pub reason: String,
}

impl FailedDelivery {
    /// "Not sent: send_message to Sales Floor Manager — <reason>"
    pub fn line(&self) -> String {
        format!("Not sent: {} to {} — {}", self.tool, self.to, self.reason)
    }
}

/// The deliveries that failed and were never made good, in the order they
/// were first refused. Each item is one message's tool calls and tool
/// results (JSON arrays as the transcript stores them), oldest first.
pub fn failed_deliveries<'a>(
    messages: impl IntoIterator<Item = (Option<&'a serde_json::Value>, Option<&'a serde_json::Value>)>,
) -> Vec<FailedDelivery> {
    let mut calls: HashMap<String, (String, String)> = HashMap::new();
    let mut failed: Vec<FailedDelivery> = Vec::new();
    for (tool_calls, tool_results) in messages {
        for call in tool_calls.and_then(|c| c.as_array()).into_iter().flatten() {
            let (Some(id), Some(name)) = (call.get("id").and_then(|v| v.as_str()), call.get("name").and_then(|v| v.as_str())) else {
                continue;
            };
            if !DELIVERY_TOOLS.contains(&name) {
                continue;
            }
            let to = match name {
                "send_message" => call
                    .get("input")
                    .and_then(|i| i.get("to"))
                    .and_then(|v| v.as_str())
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or("a coworker")
                    .to_string(),
                _ => "the owner".to_string(),
            };
            calls.insert(id.to_string(), (name.to_string(), to));
        }
        for result in tool_results.and_then(|r| r.as_array()).into_iter().flatten() {
            let Some((tool, to)) = result.get("tool_call_id").and_then(|v| v.as_str()).and_then(|id| calls.get(id)) else {
                continue;
            };
            failed.retain(|f| !(f.tool == *tool && f.to == *to));
            if result.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false) {
                let reason = result
                    .get("content")
                    .and_then(|v| v.as_str())
                    .and_then(|c| c.lines().map(str::trim).find(|l| !l.is_empty()))
                    .unwrap_or("it failed")
                    .to_string();
                failed.push(FailedDelivery { tool: tool.clone(), to: to.clone(), reason });
            }
        }
    }
    failed
}

/// The tool a helper whose caller reads its answer as data returns that
/// answer with: its input schema is the answer's schema, so the call is
/// checked when it is made and a call that doesn't match is refused with
/// what's wrong, for the helper to call again. A valid call ends the turn
/// (`tool_round`), and its input is the answer (`delegation::collect`).
pub const ANSWER_TOOL: &str = "structured_output";

/// What the answer tool tells the model, in its description: it rides
/// every request of the turn, so a compacted conversation still has it.
const ANSWER_TOOL_DESCRIPTION: &str = "Return your answer. When your work is done, call this tool once with your \
whole answer as its input, in the shape its schema gives. This call is the only answer your caller reads: words you \
write are never read. If the call is refused, fix what it names and call it again.";

/// The answer tool's definition for an answer of `schema`.
pub fn answer_tool(schema: &serde_json::Value) -> ai::ToolDefinition {
    ai::ToolDefinition {
        name: ANSWER_TOOL.to_string(),
        description: ANSWER_TOOL_DESCRIPTION.to_string(),
        input_schema: schema.clone(),
    }
}

/// The answer tool's result for a call with `input`: accepted when it
/// matches `schema`, else refused with what's wrong.
pub fn answer_result(schema: &serde_json::Value, input: &serde_json::Value) -> tools::ToolResult {
    match a2ui_validation::validate(schema, input, "answer") {
        Ok(()) => tools::ToolResult::ok("Your answer was received."),
        Err(errors) => tools::ToolResult::error(format!(
            "Your answer doesn't match the schema, so it was not received:\n{}\nCall {ANSWER_TOOL} again with the \
             corrected answer.",
            errors.iter().map(|e| format!("- {}: {}", e.path, e.message)).collect::<Vec<_>>().join("\n")
        )),
    }
}

/// Times the answer check sends a helper back to call the answer tool
/// before the turn ends without an answer (its caller reads that as a
/// failure, and the verify loop runs the reviewer again).
pub const ANSWER_RETRIES: u8 = 3;

/// A helper whose caller reads its answer as data stopped without
/// returning it through [`ANSWER_TOOL`]: it is sent back to call it.
pub struct AnswerCheck;

#[async_trait::async_trait]
impl EndCheck for AnswerCheck {
    fn name(&self) -> &'static str {
        "answer"
    }

    async fn check(&self, end: &TurnEnd<'_>) -> EndVerdict {
        // The registry holds this check alone for a data helper, so the
        // turn's count of continued checks is this check's own.
        if succeeded_tools(end.transcript).contains(ANSWER_TOOL) || end.checks_this_turn >= ANSWER_RETRIES {
            return EndVerdict::Stop;
        }
        EndVerdict::Continue(TurnEvent::AnswerNotReturned)
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
    /// A save the owner asked for that no `remember` call answers yet (chat
    /// turns; `memory_save`).
    pub unsaved_memory: Option<super::memory_save::SaveCheck>,
    /// A command the owner gave that no run_command call ran yet (chat
    /// turns; `owner_command`).
    pub unrun_command: Option<super::owner_command::CommandCheck>,
    /// The turn's code work, verified before the turn may end
    /// (`verify::engine`): owner chats, workflow activities and general
    /// helpers.
    pub verify: Option<super::verify::engine::VerifyCheck>,
}

/// The checks a turn of `mode` runs at its end: an unanswered save, an
/// unrun command, then the agreed-goal check, for chat turns; the workflow contract for workflow
/// turns; none for helpers and forks.
pub fn registry(mode: &TurnMode, checks: EndChecks) -> Vec<Box<dyn EndCheck>> {
    // Code work is verified before anything else may end the turn.
    let verify = checks.verify.map(|v| Box::new(v) as Box<dyn EndCheck>);
    match mode {
        TurnMode::Chat => {
            let saves = checks.unsaved_memory.map(|c| Box::new(c) as Box<dyn EndCheck>);
            let command = checks.unrun_command.map(|c| Box::new(c) as Box<dyn EndCheck>);
            let goal = checks.goal.map(|g| Box::new(g) as Box<dyn EndCheck>);
            verify.into_iter().chain(saves).chain(command).chain(goal).collect()
        }
        TurnMode::Workflow(_) => verify
            .into_iter()
            .chain(checks.workflow_contract.map(|c| Box::new(WorkflowContractCheck(c)) as Box<dyn EndCheck>))
            .collect(),
        TurnMode::Helper { answer: Some(_), .. } => vec![Box::new(AnswerCheck) as Box<dyn EndCheck>],
        TurnMode::Helper { kind: super::delegation::HelperKind::General, .. } => verify.into_iter().collect(),
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

    /// A helper whose answer is read as data returns it through the answer
    /// tool, whose input is checked against the schema when it is called:
    /// a call that doesn't match is refused with what's wrong. A helper that
    /// stops without a received answer is sent back to call the tool, at
    /// most [`ANSWER_RETRIES`] times.
    #[tokio::test]
    async fn a_data_answer_is_returned_through_the_answer_tool() {
        let schema = std::sync::Arc::new(serde_json::json!({
            "type": "object", "properties": { "refuted": { "type": "boolean" } }, "required": ["refuted"]
        }));
        let tool = answer_tool(&schema);
        assert_eq!((tool.name.as_str(), &tool.input_schema), (ANSWER_TOOL, schema.as_ref()), "the answer's schema is the tool's");
        assert!(tool.description.contains("words you write are never read"), "{}", tool.description);
        let mistyped = answer_result(&schema, &serde_json::json!({"refuted": "yes"}));
        assert!(mistyped.is_error && mistyped.content.contains("refuted") && mistyped.content.contains("Call structured_output again"), "{}", mistyped.content);
        assert!(!answer_result(&schema, &serde_json::json!({"refuted": false})).is_error);

        let helper = TurnMode::Helper {
            parent_session_key: "subagent:agent:a:web:h-1".into(),
            kind: crate::harness::delegation::HelperKind::General,
            depth: 2,
            answer: Some(schema),
        };
        let checks = registry(&helper, EndChecks::default());
        assert_eq!(checks.iter().map(|c| c.name()).collect::<Vec<_>>(), ["answer"], "a data answer is checked, alone");
        let prose = [said(r#"It is probably false. {"refuted": false}"#)];
        let EndVerdict::Continue(event) = checks[0].check(&end(&prose, 1)).await else { panic!("sent back") };
        let text = attachment_for(&event).unwrap().text;
        assert!(text.contains("call structured_output"), "an answer in words is never the answer: {text}");
        let refused = [call("c1", ANSWER_TOOL), result("c1", true)];
        assert!(matches!(checks[0].check(&end(&refused, 2)).await, EndVerdict::Continue(_)), "a refused call is no answer");
        let received = [call("c1", ANSWER_TOOL), result("c1", true), call("c2", ANSWER_TOOL), result("c2", false)];
        assert!(matches!(checks[0].check(&end(&received, 3)).await, EndVerdict::Stop));
        let tired = TurnEnd { transcript: &prose, step: 4, checks_this_turn: ANSWER_RETRIES };
        assert!(matches!(checks[0].check(&tired).await, EndVerdict::Stop), "sent back a bounded number of times");
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
            case_turn: false,
        }
    }

    #[tokio::test]
    async fn workflow_contract_continues_workflow_mode_only() {
        let checks = || EndChecks {
            workflow_contract: Some(contract()),
            ..Default::default()
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

    fn send(id: &str, to: &str) -> ai::Message {
        ai::Message {
            role: "assistant".into(),
            tool_calls: Some(serde_json::json!([{"id": id, "name": "send_message", "input": {"to": to, "message": "forecast"}}])),
            ..Default::default()
        }
    }

    fn said_back(id: &str, content: &str, is_error: bool) -> ai::Message {
        ai::Message {
            role: "tool".into(),
            tool_results: Some(serde_json::json!([{"tool_call_id": id, "content": content, "is_error": is_error}])),
            ..Default::default()
        }
    }

    /// Bake-off 2026-10-10: a run's send_message was refused ("Not sent:
    /// you have messaged sales_floor_manager 6 times…") and the run ended
    /// "The forecast has been sent to the Sales Floor Manager." The step is
    /// sent back once to say it was not sent; a send that later went
    /// through clears it.
    #[tokio::test]
    async fn a_refused_send_cannot_end_the_step_as_sent() {
        let checks = registry(&workflow_mode(), EndChecks { workflow_contract: Some(WorkflowContract::default()), ..Default::default() });
        let refused = [
            send("c1", "Sales Floor Manager"),
            said_back("c1", "Not sent: you have messaged Sales Floor Manager 6 times in the last five minutes.", true),
            said("The forecast has been sent to the Sales Floor Manager."),
        ];
        let EndVerdict::Continue(ev) = checks[0].check(&end(&refused, 2)).await else { panic!("sent back") };
        let text = attachment_for(&ev).unwrap().text;
        assert!(
            text.contains("Not sent: send_message to Sales Floor Manager — Not sent: you have messaged") && text.contains("never say it was"),
            "{text}"
        );
        let again = TurnEnd { transcript: &refused, step: 3, checks_this_turn: 1 };
        assert!(matches!(checks[0].check(&again).await, EndVerdict::Stop), "once");

        let retried = [
            send("c1", "Sales Floor Manager"),
            said_back("c1", "Not sent: rate limit", true),
            send("c2", "Sales Floor Manager"),
            said_back("c2", "Delivered.", false),
        ];
        assert!(failed_deliveries(retried.iter().map(|m| (m.tool_calls.as_ref(), m.tool_results.as_ref()))).is_empty());
        assert!(matches!(checks[0].check(&end(&retried, 3)).await, EndVerdict::Stop));
    }

    #[test]
    fn a_chat_turn_without_a_goal_check_has_no_checks() {
        assert!(registry(&TurnMode::Chat, EndChecks::default()).is_empty());
        assert!(registry(&workflow_mode(), EndChecks::default()).is_empty());
    }
}
