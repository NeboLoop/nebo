//! The ONE loop contract (Phase 4 of the coding harness: "second loop
//! deleted"). Workflow activities execute through a single injected agentic
//! loop implementation — the chat Runner, adapted in `nebo-agent` — instead
//! of a second hand-rolled loop in this crate.
//!
//! `workflow` cannot depend on `agent` (the dependency runs the other way),
//! so the engine names the contract here and the server injects the
//! implementation at construction time.

use crate::WorkflowError;
use crate::engine::CheckpointCtx;
use crate::parser::Activity;

/// One turn-loop execution for an activity — the whole activity, or one step
/// of it: stream the model, execute tool calls, repeat until it answers in
/// text, with workflow semantics honored by the implementation:
/// deterministic sampling, scoped tool advertising, approval parking,
/// the `exit` tool, iteration/token budgets, `requires_tools`.
pub struct LoopTurn<'a> {
    pub activity: &'a Activity,
    /// Fully built activity system prompt (context, inputs, skills, agent
    /// identity) — used as-is, never merged with a chat persona.
    pub system: String,
    /// The curated conversation so far — step prompts and step results, plus
    /// (on durable resume) the suspended in-loop transcript restored
    /// verbatim. The implementation seeds its own scratch history from this;
    /// the final user message is the work order for this turn.
    pub seed_messages: Vec<ai::Message>,
    /// Names the model may see schemas for: the activity's scoped set,
    /// including `exit`, and `emit` when granted. Dispatch still falls back
    /// to the full roster exactly as before — advertising is context
    /// scoping, not a security boundary.
    pub advertised_tools: Vec<String>,
    pub agent_id: &'a str,
    /// Caller-resolved memory scope user id (rides ToolContext.user_id).
    pub user_id: &'a str,
    pub memory_writes_disabled: bool,
    /// Janus attribution: run/workflow/action/step ids.
    pub trace: ai::RequestTrace,
    /// Per-employee approval policy + input-taint flag for the operation
    /// checkpoint. `None` = standalone run, no gating.
    pub checkpoint: Option<&'a CheckpointCtx>,
    /// Durable-resume entry: execute exactly this call FIRST, checkpoint
    /// bypassed — it IS the call the owner approved.
    pub pending: Option<ai::ToolCall>,
    /// Loop scope path ("" outside a loop) and step index, for suspension
    /// rows written at the approval park.
    pub iteration: &'a str,
    pub step_index: Option<i64>,
    pub max_iterations: u32,
    pub min_iterations: u32,
    /// Tools that must land a successful (non-error) call before this turn
    /// may finish — the activity's declared outward effect.
    pub requires_tools: Vec<String>,
    /// Activity output-token ceiling (0 = none) and what earlier turns of
    /// the same activity already spent against it.
    pub output_budget_max: u32,
    pub spent_output_before: u32,
    /// Per-activity model override ("" = session default).
    pub model: String,
    pub cancel: Option<tokio_util::sync::CancellationToken>,
    /// Distinct per (activity, iteration, step): names the implementation's
    /// scratch conversation so `cleanup` can find and remove it.
    pub turn_key: String,
}

/// What one turn-loop produced.
pub struct LoopOutcome {
    /// The final text response (the activity/step result).
    pub text: String,
    /// Input+output tokens this turn-loop consumed (run totals).
    pub total_tokens: u32,
    /// Output tokens only — the unit activity budgets are enforced in.
    pub output_tokens: u32,
}

#[async_trait::async_trait]
pub trait ActivityLoop: Send + Sync {
    async fn run_turn(&self, turn: LoopTurn<'_>) -> Result<LoopOutcome, WorkflowError>;

    /// Drop the scratch conversations this run created. The engine calls it
    /// on every run exit EXCEPT AwaitingApproval — the parked transcript is
    /// part of the resume state.
    fn cleanup(&self, run_id: &str);
}
