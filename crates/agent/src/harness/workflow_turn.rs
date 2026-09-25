//! Workflow activities on the one loop: the `workflow::ActivityLoop`
//! implementation. Every activity turn is a `TurnMode::Workflow` turn of
//! `drive_turn`: the activity's instructions (a row in its conversation;
//! the system prompt is every turn's one prompt), its scoped tools, the
//! approval park, the `exit` primitive and its contract (`min_iterations`
//! and `requires_tools`, checked at turn end by `WorkflowContractCheck`).
//!
//! History model: each turn gets a scratch session seeded from the engine's
//! curated conversation (step prompts and step results). The scratch
//! sessions are deleted at run end, except when the run parks for approval:
//! then the suspension row the park closure writes is the resume state.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use ai::RequestTrace;
use tracing::{info, warn};
use workflow::{ActivityLoop, LoopOutcome, LoopTurn, WorkflowError};

use super::turn_end::WorkflowContract;
use super::{Delivery, Harness, SeatRequest, TurnInput, TurnMode, TurnRequest};

/// Persists a parked call's suspension (sync: rusqlite is sync); the turn
/// then ends `AwaitingApproval`.
pub type ParkFn = Arc<dyn Fn(WorkflowPark<'_>) -> Result<(), String> + Send + Sync>;

/// A workflow activity turn: what the loop does differently for it.
#[derive(Clone)]
pub struct WorkflowMode {
    /// Janus attribution: workflow, action and step ids ride the request trace.
    pub trace: RequestTrace,
    /// The activity's instructions, built by the engine (rules, skills,
    /// type, parameters, task, inputs, prior results, controls): told as the
    /// turn's `activity` row, never in the system prompt.
    pub instructions: String,
    /// What this step is for, in words: workflow name, activity and step
    /// instruction.
    pub objective: String,
    /// The work order this turn was given (the seed's final user message).
    pub instruction: String,
    /// Only these tools' schemas ship to the model (context scoping, not
    /// security): dispatch still resolves through the full registry.
    pub advertised_tools: HashSet<String>,
    /// The run's inputs carry untrusted content.
    pub tainted: bool,
    /// The owner's per-run spending limit in microcents (0 = none): reaching
    /// it ends the turn `BudgetReached`.
    pub spend_cap_microcents: i64,
    /// Steps the turn may take; 0 = the loop's default.
    pub max_steps: u32,
    /// The activity's contract, checked when the model stops.
    pub contract: WorkflowContract,
    /// Park an ask on the owner instead of refusing it unattended. `None` =
    /// refuse.
    pub park: Option<ParkFn>,
}

impl std::fmt::Debug for WorkflowMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowMode")
            .field("trace_run", &self.trace.run_id)
            .field("advertised", &self.advertised_tools.len())
            .field("tainted", &self.tainted)
            .field("spend_cap_microcents", &self.spend_cap_microcents)
            .field("max_steps", &self.max_steps)
            .field("contract", &self.contract)
            .field("park", &self.park.is_some())
            .finish()
    }
}

#[cfg(test)]
impl Default for WorkflowMode {
    fn default() -> Self {
        Self {
            trace: RequestTrace::new("workflow"),
            instructions: String::new(),
            objective: String::new(),
            instruction: String::new(),
            advertised_tools: HashSet::new(),
            tainted: false,
            spend_cap_microcents: 0,
            max_steps: 0,
            contract: WorkflowContract::default(),
            park: None,
        }
    }
}

/// What the park closure receives: everything a suspension row needs.
pub struct WorkflowPark<'a> {
    /// The in-loop conversation at park time.
    pub messages: Vec<ai::Message>,
    pub call: &'a ai::ToolCall,
    /// The ask the call parked on: the owner's answer to it releases the run.
    pub ask_id: &'a str,
    /// Port-suffixed operation name and the owner-facing sentence.
    pub operation: String,
    pub display: String,
}

/// The step's task in words: `(objective, instruction)`. The objective names
/// the workflow, the activity (its label, else its id) with its intent, and
/// the current step's instruction; the instruction is the work order the
/// model was handed this turn, the seed's final user message.
fn step_task(
    workflow_name: &str,
    activity: &workflow::parser::Activity,
    step_index: Option<i64>,
    seed: &[ai::Message],
) -> (String, String) {
    let activity_name = activity
        .label
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .unwrap_or(&activity.id);
    let mut objective = format!("Workflow \"{}\", activity \"{}\"", workflow_name.trim(), activity_name);
    let intent = activity.intent.trim();
    if !intent.is_empty() {
        objective.push_str(": ");
        objective.push_str(intent);
    }
    let step = step_index
        .and_then(|i| usize::try_from(i).ok())
        .and_then(|i| activity.steps.get(i).map(|s| (i, s.trim())));
    if let Some((i, step)) = step {
        objective.push_str(&format!(
            "{}Step {}/{}: {}",
            if intent.is_empty() { ". " } else { " " },
            i + 1,
            activity.steps.len(),
            step
        ));
    }
    let instruction = seed
        .iter()
        .rev()
        .find(|m| m.role == "user" && !m.content.trim().is_empty())
        .map(|m| m.content.trim().to_string())
        .unwrap_or_default();
    (objective, instruction)
}

/// The workflow engine's loop: each activity turn runs on the harness.
pub struct WorkflowTurns {
    harness: Harness,
    /// run_id → scratch session ids created for it (cleanup).
    sessions_by_run: Mutex<HashMap<String, Vec<String>>>,
}

impl WorkflowTurns {
    pub fn new(harness: Harness) -> Self {
        Self {
            harness,
            sessions_by_run: Mutex::new(HashMap::new()),
        }
    }

    fn session_key(turn: &LoopTurn<'_>) -> String {
        // The `agent:<id>:` prefix is what tools parse for per-agent state
        // (plugin account profiles, memory scope); the turn key gives each
        // activity turn its own scratch conversation.
        if turn.agent_id.is_empty() {
            format!("workflow:{}:{}", turn.trace.run_id, turn.turn_key)
        } else {
            format!("agent:{}:workflow:{}:{}", turn.agent_id, turn.trace.run_id, turn.turn_key)
        }
    }

    /// Seed the scratch session from the curated conversation.
    fn seed_session(&self, session_id: &str, messages: &[ai::Message]) {
        for m in messages {
            let tc = m
                .tool_calls
                .as_ref()
                .map(|v| v.to_string())
                .filter(|s| !s.is_empty() && s != "null");
            let tr = m
                .tool_results
                .as_ref()
                .map(|v| v.to_string())
                .filter(|s| !s.is_empty() && s != "null");
            if m.content.is_empty() && tc.is_none() && tr.is_none() {
                continue;
            }
            if let Err(e) =
                self.harness
                    .sessions
                    .append_message(session_id, &m.role, &m.content, tc.as_deref(), tr.as_deref(), None)
            {
                warn!(session_id, error = %e, "workflow seed: failed to append message");
            }
        }
    }

    /// Run the call the owner approved (the one check still applies the hard
    /// limits, the ceiling and deny rules) and store its result so the
    /// turn's next step sees the outcome.
    async fn execute_pending(
        &self,
        turn: &LoopTurn<'_>,
        session_key: &str,
        session_id: &str,
        tc: &ai::ToolCall,
    ) -> Result<(), WorkflowError> {
        let mut ctx = tools::ToolContext::new(tools::Origin::Workflow)
            .with_session(session_key.to_string(), session_id.to_string());
        ctx.door = types::permissions::Door::Workflow;
        ctx.answered_ask = Some(format!("workflow:{}", turn.trace.run_id));
        ctx.user_id = turn.user_id.to_string();
        ctx.memory_writes_disabled = turn.memory_writes_disabled;
        ctx.run_id = Some(turn.trace.run_id.clone());
        let result = self.harness.tools.execute(&ctx, &tc.name, tc.input.clone()).await;
        if result.terminal {
            return Err(WorkflowError::Blocked(result.content.clone(), result.need.clone()));
        }
        if !result.is_error
            && let Some(reason) = result.content.strip_prefix(tools::EXIT_SENTINEL)
        {
            return Err(WorkflowError::Exited(reason.to_string()));
        }
        let tr = serde_json::json!([{
            "tool_call_id": tc.id,
            "content": result.content,
            "is_error": result.is_error,
        }])
        .to_string();
        let _ = self.harness.sessions.append_message(session_id, "tool", "", None, Some(&tr), None);
        Ok(())
    }

    /// A turn that ended in tool calls only: its output is the latest tool
    /// message's non-error results (errors excluded, so a step whose tools
    /// only failed yields empty output: branch termination).
    fn synthesize_output(&self, session_id: &str) -> String {
        let msgs = self.harness.sessions.get_messages(session_id).unwrap_or_default();
        for m in msgs.iter().rev().filter(|m| m.role == "tool") {
            let Some(Ok(serde_json::Value::Array(results))) =
                m.tool_results.as_deref().map(serde_json::from_str::<serde_json::Value>)
            else {
                continue;
            };
            let parts: Vec<&str> = results
                .iter()
                .filter(|e| !e.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false))
                .filter_map(|e| e.get("content").and_then(|v| v.as_str()).filter(|s| !s.is_empty()))
                .collect();
            if !parts.is_empty() {
                return parts.join("\n");
            }
        }
        String::new()
    }
}

#[async_trait::async_trait]
impl ActivityLoop for WorkflowTurns {
    async fn acquire_tool_permit(&self) -> tokio::sync::OwnedSemaphorePermit {
        self.harness.concurrency.acquire_tool_permit().await
    }

    async fn run_turn(&self, turn: LoopTurn<'_>) -> Result<LoopOutcome, WorkflowError> {
        let sessions = &self.harness.sessions;
        let key = Self::session_key(&turn);
        let session = sessions
            .get_or_create(&key, turn.user_id)
            .map_err(|e| WorkflowError::Database(format!("workflow session: {e}")))?;
        let mut session_id = session.id.clone();

        let existing = sessions.get_messages(&session_id).unwrap_or_default();
        if turn.pending.is_none() {
            // A fresh turn (or a retry): the curated seed is the whole truth;
            // a stale scratch conversation from an earlier attempt must not
            // leak in.
            if !existing.is_empty() {
                let _ = sessions.delete_session(&session_id);
                session_id = sessions
                    .get_or_create(&key, turn.user_id)
                    .map_err(|e| WorkflowError::Database(format!("workflow session: {e}")))?
                    .id;
            }
            self.seed_session(&session_id, &turn.seed_messages);
        } else if existing.is_empty() {
            // A durable resume from a persisted suspension row: rehydrate the
            // suspended conversation verbatim.
            self.seed_session(&session_id, &turn.seed_messages);
        }
        self.sessions_by_run
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(turn.trace.run_id.clone())
            .or_default()
            .push(session_id.clone());

        if let Some(tc) = turn.pending.clone() {
            self.execute_pending(&turn, &key, &session_id, &tc).await?;
        }

        // The approval park: the closure writes the suspension row and
        // records what parked, so the outcome maps to AwaitingApproval.
        let parked: Arc<Mutex<Option<(String, String)>>> = Arc::new(Mutex::new(None));
        let park = turn.checkpoint.map(|cp| {
            let store = self.harness.store.clone();
            let parked = parked.clone();
            let run_id = turn.trace.run_id.clone();
            let agent_id = turn.agent_id.to_string();
            let binding = cp.binding_name.clone();
            let activity_id = turn.activity.id.clone();
            let iteration = turn.iteration.to_string();
            let step_index = turn.step_index;
            Arc::new(move |p: WorkflowPark<'_>| -> Result<(), String> {
                let messages_json = serde_json::to_string(&p.messages).unwrap_or_default();
                let pending_json = serde_json::to_string(p.call).unwrap_or_default();
                store
                    .create_workflow_suspension(
                        &run_id,
                        &agent_id,
                        &binding,
                        &activity_id,
                        &iteration,
                        step_index,
                        &messages_json,
                        &pending_json,
                        &p.operation,
                        &p.display,
                    )
                    .map_err(|e| e.to_string())?;
                // The run waits on the ask's one card: its answer releases it.
                store.link_permission_ask_run(p.ask_id, &run_id).map_err(|e| e.to_string())?;
                let _ = store.update_workflow_run(&run_id, Some("awaiting_approval"), Some(&activity_id), None, None, None);
                *parked.lock().unwrap_or_else(|p| p.into_inner()) = Some((p.operation.clone(), p.display.clone()));
                Ok(())
            }) as ParkFn
        });

        let cancel = turn.cancel.clone().unwrap_or_default();
        let (objective, instruction) = step_task(turn.workflow_name, turn.activity, turn.step_index, &turn.seed_messages);
        let req = TurnRequest {
            session_key: key,
            // The seed carries the work order.
            input: TurnInput::None,
            // The activity runs under its employee's own grant, through the
            // one permission check: an ask parks the run for the owner.
            seat: SeatRequest {
                agent_id: turn.agent_id.to_string(),
                user_id: turn.user_id.to_string(),
                origin: tools::Origin::Workflow,
                door: types::permissions::Door::Workflow,
                mode: None,
                ceiling: None,
                cwd: None,
                seed_taint: Vec::new(),
                audience: None,
                tool_allowlist: None,
                tool_denial_hint: None,
                handoff_depth: 0,
                model_override: turn.model.clone(),
                model_preference: None,
                personality_snippet: None,
                tool_scope: None,
            },
            mode: TurnMode::Workflow(Box::new(WorkflowMode {
                trace: turn.trace.clone(),
                instructions: turn.instructions.clone(),
                objective,
                instruction,
                advertised_tools: turn.advertised_tools.iter().cloned().collect(),
                tainted: turn.checkpoint.map(|c| c.tainted).unwrap_or(false),
                spend_cap_microcents: turn.spend_cap_microcents,
                max_steps: turn.max_iterations,
                contract: WorkflowContract {
                    min_iterations: turn.min_iterations,
                    requires_tools: turn.requires_tools.clone(),
                },
                park,
            })),
            delivery: Delivery {
                channel: "workflow".into(),
                channel_ctx: None,
                mention_briefing: None,
            },
            cancel: cancel.clone(),
            progress: None,
        };

        let mut rx = self
            .harness
            .start_turn(req)
            .await
            .map_err(|e| WorkflowError::Provider(e.to_string()))?
            .events;

        // Collect: the last step's text (the buffer resets when a new tool
        // batch starts), per-stream usage (cumulative within one stream, so
        // the max is final; committed at stream boundaries), control notices
        // and the Done reason.
        let mut text = String::new();
        let mut text_stale = false;
        let (mut cur_in, mut cur_out): (i32, i32) = (0, 0);
        let (mut total_in, mut total_out): (u32, u32) = (0, 0);
        let mut notice = String::new();
        let mut need: Option<types::OwnerNeed> = None;
        let mut error: Option<String> = None;
        let mut exit_reason = String::new();
        let mut tainted = false;
        let commit = |ci: &mut i32, co: &mut i32, ti: &mut u32, to: &mut u32| {
            *ti += (*ci).max(0) as u32;
            *to += (*co).max(0) as u32;
            *ci = 0;
            *co = 0;
        };
        while let Some(ev) = rx.recv().await {
            match ev.event_type {
                ai::StreamEventType::Text => {
                    if text_stale {
                        text.clear();
                        text_stale = false;
                    }
                    text.push_str(&ev.text);
                }
                ai::StreamEventType::ToolCall => text_stale = true,
                ai::StreamEventType::ToolResult => commit(&mut cur_in, &mut cur_out, &mut total_in, &mut total_out),
                ai::StreamEventType::Usage => {
                    if let Some(u) = ev.usage {
                        cur_in = cur_in.max(u.input_tokens);
                        cur_out = cur_out.max(u.output_tokens);
                    }
                }
                ai::StreamEventType::ControlNotice => {
                    need = ev.owner_need();
                    notice = ev.text.clone();
                }
                ai::StreamEventType::Error => error = ev.error.clone().or(Some("stream error".into())),
                ai::StreamEventType::Done => {
                    commit(&mut cur_in, &mut cur_out, &mut total_in, &mut total_out);
                    if let Some(r) = ev.stop_reason {
                        exit_reason = r;
                    }
                    tainted |= ev.provenance.is_some_and(|p| !p.is_empty());
                }
                _ => {}
            }
        }

        // The outcome, most specific first.
        if let Some((operation, display)) = parked.lock().unwrap_or_else(|p| p.into_inner()).take() {
            return Err(WorkflowError::AwaitingApproval { operation, display });
        }
        if let Some(rest) = exit_reason.strip_prefix("workflow_exit:") {
            return Err(WorkflowError::Exited(rest.to_string()));
        }
        if let Some(rest) = exit_reason.strip_prefix("suspension_failed:") {
            return Err(WorkflowError::Database(format!("failed to persist approval suspension: {rest}")));
        }
        match exit_reason.as_str() {
            "terminal_tool_error" => {
                return Err(WorkflowError::Blocked(
                    if notice.is_empty() { "terminal tool error".into() } else { notice },
                    need,
                ));
            }
            super::delegation::collect::STOP_SPEND_CAP => {
                // Money in cents for the owner's words; the loop compares microcents.
                return Err(WorkflowError::SpendCapReached {
                    activity_id: turn.activity.id.clone(),
                    spent_cents: turn.spend_cap_microcents / 1_000_000,
                    cap_cents: turn.spend_cap_microcents / 1_000_000,
                    partial: text.clone(),
                });
            }
            super::delegation::collect::STOP_MAX_STEPS => {
                return Err(WorkflowError::MaxIterations(turn.activity.id.clone()));
            }
            _ => {}
        }
        if cancel.is_cancelled() || exit_reason == "cancelled" {
            return Err(WorkflowError::Cancelled);
        }
        if let Some(e) = error {
            return Err(WorkflowError::ActivityFailed(turn.activity.id.clone(), e));
        }

        if text.trim().is_empty() {
            text = self.synthesize_output(&session_id);
        }
        info!(
            activity = %turn.activity.id,
            run_id = %turn.trace.run_id,
            tokens = total_in + total_out,
            "workflow turn complete"
        );
        Ok(LoopOutcome {
            text,
            total_tokens: total_in + total_out,
            output_tokens: total_out,
            tainted,
        })
    }

    fn cleanup(&self, run_id: &str) {
        let ids = self
            .sessions_by_run
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(run_id)
            .unwrap_or_default();
        for id in ids {
            if let Err(e) = self.harness.sessions.delete_session(&id) {
                warn!(run_id, session = %id, error = %e, "workflow cleanup: delete failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::step_task;

    fn activity(json: serde_json::Value) -> workflow::parser::Activity {
        serde_json::from_value(json).unwrap()
    }

    fn user(text: &str) -> ai::Message {
        ai::Message { role: "user".into(), content: text.into(), ..Default::default() }
    }

    fn assistant(text: &str) -> ai::Message {
        ai::Message { role: "assistant".into(), content: text.into(), ..Default::default() }
    }

    #[test]
    fn a_step_task_names_the_workflow_the_activity_and_the_step() {
        let a = activity(serde_json::json!({
            "id": "pull-orders",
            "label": "Pull orders",
            "intent": "Collect last week's orders and the products they name.",
            "steps": ["List the orders.", "List the products."],
        }));
        let seed = vec![
            user("Step 1/2: List the orders."),
            assistant("12 orders."),
            user("Step 2/2: List the products."),
        ];
        let (objective, instruction) = step_task("Weekly report", &a, Some(1), &seed);
        assert_eq!(
            objective,
            "Workflow \"Weekly report\", activity \"Pull orders\": Collect last week's orders \
             and the products they name. Step 2/2: List the products."
        );
        assert_eq!(instruction, "Step 2/2: List the products.");
    }

    #[test]
    fn a_single_turn_activity_is_its_intent_and_an_unlabelled_one_its_id() {
        let a = activity(serde_json::json!({"id": "send-summary", "intent": "Mail the summary."}));
        let (objective, instruction) = step_task("Weekly report", &a, None, &[user("Mail the summary.")]);
        assert_eq!(objective, "Workflow \"Weekly report\", activity \"send-summary\": Mail the summary.");
        assert_eq!(instruction, "Mail the summary.");

        // A typed node with no intent and no seed: the names alone, no instruction.
        let a = activity(serde_json::json!({"id": "fetch", "type": "http", "steps": ["GET the feed."]}));
        let (objective, instruction) = step_task("Feeds", &a, Some(0), &[]);
        assert_eq!(objective, "Workflow \"Feeds\", activity \"fetch\". Step 1/1: GET the feed.");
        assert_eq!(instruction, "");
    }
}
