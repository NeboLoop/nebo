//! The workflow adapter for the ONE loop (Phase 4: "second loop deleted").
//!
//! Implements `workflow::ActivityLoop` on top of the chat `Runner`: every
//! workflow activity turn runs through the same agentic loop as chat —
//! compaction, read ledger, identical-call kernel, terminal classification,
//! spiral guards — with workflow semantics carried by `WorkflowMode` on the
//! request (deterministic sampling, advertised-tools scoping, approval
//! parking, the `exit` primitive, output budgets).
//!
//! History model: each turn gets a scratch session seeded from the engine's
//! curated conversation (step prompts + step results), exactly the message
//! set the old engine loop started from. The scratch sessions are deleted at
//! run end — EXCEPT when the run parks for approval, where the suspension
//! row (written by the park closure, same shape as before) is the resume
//! state.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tracing::{info, warn};

use crate::runner::{RunRequest, Runner, WorkflowMode, WorkflowPark};
use workflow::{ActivityLoop, LoopOutcome, LoopTurn, WorkflowError};

pub struct RunnerActivityLoop {
    runner: Arc<Runner>,
    store: Arc<db::Store>,
    /// run_id → scratch session ids created for it (cleanup).
    sessions_by_run: Mutex<HashMap<String, Vec<String>>>,
}

impl RunnerActivityLoop {
    pub fn new(runner: Arc<Runner>, store: Arc<db::Store>) -> Self {
        Self {
            runner,
            store,
            sessions_by_run: Mutex::new(HashMap::new()),
        }
    }

    fn session_key(turn: &LoopTurn<'_>) -> String {
        // The `agent:<id>:` prefix is what tools parse for per-agent state
        // (plugin account profiles, memory scope) — see
        // tools::workflow_session_key, extended with the turn key so each
        // activity turn gets its own scratch conversation.
        if turn.agent_id.is_empty() {
            format!("workflow:{}:{}", turn.trace.run_id, turn.turn_key)
        } else {
            format!(
                "agent:{}:workflow:{}:{}",
                turn.agent_id, turn.trace.run_id, turn.turn_key
            )
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
            if let Err(e) = self.runner.sessions().append_message(
                session_id,
                &m.role,
                &m.content,
                tc.as_deref(),
                tr.as_deref(),
                None,
            ) {
                warn!(session_id, error = %e, "workflow seed: failed to append message");
            }
        }
    }

    /// Execute the owner-approved pending call directly (checkpoint bypassed
    /// — it IS what the owner saw), appending its result to the session so
    /// the loop's next model turn sees the outcome. Mirrors the old engine's
    /// durable-resume entry, including its minimal tool context.
    async fn execute_pending(
        &self,
        turn: &LoopTurn<'_>,
        session_key: &str,
        session_id: &str,
        tc: &ai::ToolCall,
    ) -> Result<(), WorkflowError> {
        let mut ctx = tools::ToolContext::new(tools::Origin::Workflow)
            .with_session(session_key.to_string(), session_id.to_string());
        ctx.user_id = turn.user_id.to_string();
        ctx.memory_writes_disabled = turn.memory_writes_disabled;
        ctx.run_id = Some(turn.trace.run_id.clone());
        let result = self
            .runner
            .tool_registry()
            .execute(&ctx, &tc.name, tc.input.clone())
            .await;
        if result.terminal {
            return Err(WorkflowError::Blocked(result.content.clone()));
        }
        if !result.is_error {
            if let Some(reason) = result.content.strip_prefix(tools::EXIT_SENTINEL) {
                return Err(WorkflowError::Exited(reason.to_string()));
            }
        }
        let tr = serde_json::json!([{
            "tool_call_id": tc.id,
            "content": result.content,
            "is_error": result.is_error,
        }])
        .to_string();
        let _ = self
            .runner
            .sessions()
            .append_message(session_id, "tool", "", None, Some(&tr), None);
        Ok(())
    }

    /// Which required tools landed a successful call, read back from the
    /// scratch session (assistant tool_calls map ids→names; tool results
    /// carry is_error per id).
    fn missing_required(&self, session_id: &str, required: &[String]) -> Vec<String> {
        if required.is_empty() {
            return vec![];
        }
        let msgs = self
            .runner
            .sessions()
            .get_messages(session_id)
            .unwrap_or_default();
        let mut id_to_name: HashMap<String, String> = HashMap::new();
        let mut succeeded: std::collections::HashSet<String> = Default::default();
        for m in &msgs {
            if let Some(tc) = m.tool_calls.as_deref() {
                if let Ok(calls) = serde_json::from_str::<serde_json::Value>(tc) {
                    if let Some(arr) = calls.as_array() {
                        for c in arr {
                            if let (Some(id), Some(name)) = (
                                c.get("id").and_then(|v| v.as_str()),
                                c.get("name").and_then(|v| v.as_str()),
                            ) {
                                id_to_name.insert(id.to_string(), name.to_string());
                            }
                        }
                    }
                }
            }
            if let Some(tr) = m.tool_results.as_deref() {
                if let Ok(results) = serde_json::from_str::<serde_json::Value>(tr) {
                    if let Some(arr) = results.as_array() {
                        for r in arr {
                            let is_err = r
                                .get("is_error")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            if is_err {
                                continue;
                            }
                            if let Some(id) = r.get("tool_call_id").and_then(|v| v.as_str()) {
                                if let Some(name) = id_to_name.get(id) {
                                    succeeded.insert(name.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
        let mut missing: Vec<String> = required
            .iter()
            .filter(|t| !succeeded.contains(*t))
            .cloned()
            .collect();
        missing.sort();
        missing
    }

    /// Tool-only completions synthesize output from the LATEST tool
    /// message's non-error results (old-engine parity: errors excluded, so a
    /// step whose tools only failed yields empty output — branch
    /// termination — instead of an error string masquerading as data).
    fn synthesize_output(&self, session_id: &str) -> String {
        let msgs = self
            .runner
            .sessions()
            .get_messages(session_id)
            .unwrap_or_default();
        for m in msgs.iter().rev() {
            if m.role != "tool" {
                continue;
            }
            if let Some(tr) = m.tool_results.as_deref() {
                if let Ok(serde_json::Value::Array(results)) =
                    serde_json::from_str::<serde_json::Value>(tr)
                {
                    let parts: Vec<&str> = results
                        .iter()
                        .filter_map(|e| {
                            if e.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false) {
                                return None;
                            }
                            e.get("content").and_then(|v| v.as_str()).filter(|s| !s.is_empty())
                        })
                        .collect();
                    if !parts.is_empty() {
                        return parts.join("\n");
                    }
                }
            }
        }
        String::new()
    }
}

#[async_trait::async_trait]
impl ActivityLoop for RunnerActivityLoop {
    async fn run_turn(&self, turn: LoopTurn<'_>) -> Result<LoopOutcome, WorkflowError> {
        let key = Self::session_key(&turn);
        let session = self
            .runner
            .sessions()
            .get_or_create(&key, turn.user_id)
            .map_err(|e| WorkflowError::Database(format!("workflow session: {e}")))?;
        let mut session_id = session.id.clone();

        let existing = self
            .runner
            .sessions()
            .get_messages(&session_id)
            .unwrap_or_default();
        if turn.pending.is_none() {
            // Fresh turn (or a retry attempt): the curated seed is the whole
            // truth — a stale scratch conversation from a previous attempt
            // must not leak in.
            if !existing.is_empty() {
                let _ = self.runner.sessions().delete_session(&session_id);
                let session = self
                    .runner
                    .sessions()
                    .get_or_create(&key, turn.user_id)
                    .map_err(|e| WorkflowError::Database(format!("workflow session: {e}")))?;
                session_id = session.id.clone();
            }
            self.seed_session(&session_id, &turn.seed_messages);
        } else if existing.is_empty() {
            // Durable resume from a persisted suspension row (or a run
            // parked before this adapter existed): rehydrate the suspended
            // conversation verbatim.
            self.seed_session(&session_id, &turn.seed_messages);
        }
        self.sessions_by_run
            .lock()
            .unwrap()
            .entry(turn.trace.run_id.clone())
            .or_default()
            .push(session_id.clone());

        if let Some(tc) = turn.pending.clone() {
            self.execute_pending(&turn, &key, &session_id, &tc).await?;
        }

        // Approval park plumbing: the closure persists the suspension row —
        // exactly the row the old engine wrote — and records what parked so
        // the outcome maps to AwaitingApproval.
        let parked: Arc<Mutex<Option<(String, String)>>> = Arc::new(Mutex::new(None));
        let park = turn.checkpoint.map(|cp| {
            let store = self.store.clone();
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
                let _ = store.update_workflow_run(
                    &run_id,
                    Some("awaiting_approval"),
                    Some(&activity_id),
                    None,
                    None,
                    None,
                );
                *parked.lock().unwrap() = Some((p.operation.clone(), p.display.clone()));
                Ok(())
            }) as Arc<dyn Fn(WorkflowPark<'_>) -> Result<(), String> + Send + Sync>
        });

        let cancel = turn.cancel.clone().unwrap_or_default();
        let req = RunRequest {
            session_key: key,
            prompt: String::new(), // the seed carries the work order
            system: turn.system.clone(),
            user_id: turn.user_id.to_string(),
            agent_id: turn.agent_id.to_string(),
            origin: tools::Origin::Workflow,
            channel: "workflow".into(),
            skip_memory_extract: true,
            prompt_mode: crate::prompt::PromptMode::Minimal,
            max_iterations: turn.max_iterations as usize,
            min_iterations: turn.min_iterations as usize,
            model_override: turn.model.clone(),
            cancel_token: cancel.clone(),
            operation_policy: turn.checkpoint.and_then(|c| c.operation_policy.clone()),
            // Old-engine parity: the engine executed tools directly with no
            // capability gate — the operation policy (above) and the roster
            // scoping are the workflow's controls. Full Access keeps the
            // chat-only capability prompts out of an unattended run; the
            // operation gate deliberately ignores it (money ops still park).
            full_access: true,
            workflow: Some(WorkflowMode {
                trace: turn.trace.clone(),
                advertised_tools: turn.advertised_tools.iter().cloned().collect(),
                tainted: turn.checkpoint.map(|c| c.tainted).unwrap_or(false),
                output_budget_max: turn.output_budget_max,
                spent_output_before: turn.spent_output_before,
                park,
            }),
            ..Default::default()
        };

        let mut rx = self
            .runner
            .run(req)
            .await
            .map_err(|e| WorkflowError::Provider(e.to_string()))?;

        // Collect: last turn's text (buffer resets when a new tool batch
        // starts), per-stream usage (cumulative running totals within one
        // stream — max is final; committed at stream boundaries), control
        // notices, the Done exit reason.
        let mut text = String::new();
        let mut text_stale = false;
        let (mut cur_in, mut cur_out): (i32, i32) = (0, 0);
        let (mut total_in, mut total_out): (u32, u32) = (0, 0);
        let mut notice = String::new();
        let mut error: Option<String> = None;
        let mut exit_reason = String::new();
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
                ai::StreamEventType::ToolCall => {
                    text_stale = true;
                }
                ai::StreamEventType::ToolResult => {
                    commit(&mut cur_in, &mut cur_out, &mut total_in, &mut total_out);
                }
                ai::StreamEventType::Usage => {
                    if let Some(u) = ev.usage {
                        cur_in = cur_in.max(u.input_tokens);
                        cur_out = cur_out.max(u.output_tokens);
                    }
                }
                ai::StreamEventType::ControlNotice => {
                    notice = ev.text.clone();
                }
                ai::StreamEventType::Error => {
                    error = ev.error.clone().or(Some("stream error".into()));
                }
                ai::StreamEventType::Done => {
                    commit(&mut cur_in, &mut cur_out, &mut total_in, &mut total_out);
                    if let Some(r) = ev.stop_reason {
                        exit_reason = r;
                    }
                }
                _ => {}
            }
        }

        // Outcome mapping, most specific first.
        if let Some((operation, display)) = parked.lock().unwrap().take() {
            return Err(WorkflowError::AwaitingApproval { operation, display });
        }
        if let Some(rest) = exit_reason.strip_prefix("workflow_exit:") {
            return Err(WorkflowError::Exited(rest.to_string()));
        }
        if exit_reason == "terminal_tool_error" {
            return Err(WorkflowError::Blocked(if notice.is_empty() {
                "terminal tool error".into()
            } else {
                notice
            }));
        }
        if exit_reason == "runaway_tool_loop" {
            return Err(WorkflowError::RunawayLoop(if notice.is_empty() {
                format!("runaway loop in activity '{}'", turn.activity.id)
            } else {
                notice
            }));
        }
        if let Some(rest) = exit_reason.strip_prefix("suspension_failed:") {
            return Err(WorkflowError::Database(format!(
                "failed to persist approval suspension: {rest}"
            )));
        }
        if exit_reason == "output_budget_exceeded" {
            return Err(WorkflowError::BudgetExceeded {
                activity_id: turn.activity.id.clone(),
                used: turn.spent_output_before + total_out,
                limit: turn.output_budget_max,
            });
        }
        if exit_reason.starts_with("max_iterations") {
            return Err(WorkflowError::MaxIterations(turn.activity.id.clone()));
        }
        if cancel.is_cancelled() || exit_reason == "cancelled" || exit_reason == "user_requested_stop"
        {
            return Err(WorkflowError::Cancelled);
        }
        if let Some(e) = error {
            return Err(WorkflowError::ActivityFailed(turn.activity.id.clone(), e));
        }

        if text.trim().is_empty() {
            text = self.synthesize_output(&session_id);
        }
        let missing = self.missing_required(&session_id, &turn.requires_tools);
        if !missing.is_empty() {
            return Err(WorkflowError::ActivityFailed(
                turn.activity.id.clone(),
                format!(
                    "stopped without a successful {} call — the activity's required effect \
                     never happened. The model's own summary said: {}",
                    missing.join(", "),
                    text.chars().take(300).collect::<String>()
                ),
            ));
        }

        info!(
            activity = %turn.activity.id,
            run_id = %turn.trace.run_id,
            tokens = total_in + total_out,
            "workflow turn complete (runner loop)"
        );
        Ok(LoopOutcome {
            text,
            total_tokens: total_in + total_out,
            output_tokens: total_out,
        })
    }

    fn cleanup(&self, run_id: &str) {
        let ids = self
            .sessions_by_run
            .lock()
            .unwrap()
            .remove(run_id)
            .unwrap_or_default();
        for id in ids {
            if let Err(e) = self.runner.sessions().delete_session(&id) {
                warn!(run_id, session = %id, error = %e, "workflow cleanup: delete failed");
            }
        }
    }
}
