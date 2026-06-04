use db::models::ChatMessage;

/// A steering directive injected into the system prompt suffix (never as a user message).
#[derive(Debug, Clone)]
pub struct SteeringDirective {
    pub label: String,
    pub content: String,
    pub priority: u8,
}

/// Work task for steering context.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkTask {
    pub id: String,
    pub subject: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

/// Context passed to all steering generators.
pub struct Context {
    pub session_id: String,
    pub messages: Vec<ChatMessage>,
    pub user_prompt: String,
    pub active_task: String,
    pub channel: String,
    pub agent_name: String,
    pub iteration: usize,
    pub work_tasks: Vec<WorkTask>,
    pub quota_warning: Option<String>,
    /// Number of consecutive iterations where ALL tool calls returned errors.
    pub consecutive_error_iterations: usize,
    /// Rolling hashes of recent tool calls for loop detection (OpenClaw-style).
    /// Each entry is (tool_name_hash, args_hash, result_hash). Last 10 kept.
    pub recent_tool_result_hashes: Vec<(u64, u64, u64)>,
    /// User presence state: "focused", "unfocused", "away", or empty if unknown.
    pub user_presence: String,
    /// Whether the user just transitioned from unfocused/away back to focused.
    pub user_just_returned: bool,
    /// Proactive items drained from the inbox for this iteration.
    pub proactive_items: Vec<crate::proactive::ProactiveItem>,
    /// Provider ID for provider-specific skip rules (e.g. "anthropic", "openai", "janus", "ollama").
    pub provider_id: String,
    /// Recent tool names (parallel to recent_tool_result_hashes). Last 10 kept.
    pub recent_tool_names: Vec<String>,
    /// Communication personality: Interactive (preamble allowed) vs Autonomous
    /// (silent). Mode-branches narration/output suppressors. (Wired in Round 1;
    /// consumed by the suppressors in Round 2.)
    pub execution_mode: tools::ExecutionMode,
}

/// A steering directive generator.
trait Generator: Send + Sync {
    fn name(&self) -> &str;
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective>;
}

/// Format a list of steering directives into a system prompt section.
pub fn format_directives(directives: &[SteeringDirective]) -> String {
    if directives.is_empty() {
        return String::new();
    }
    let mut sb = String::from("## Agent Directives\n");
    for d in directives {
        sb.push_str(&format!("[{}] {}\n", d.label, d.content));
    }
    sb
}

/// Runs all registered generators to produce steering directives.
pub struct Pipeline {
    generators: Vec<Box<dyn Generator>>,
}

impl Pipeline {
    pub fn new() -> Self {
        let generators: Vec<Box<dyn Generator>> = vec![
            Box::new(IdentityGuard),
            Box::new(ChannelAdapter),
            Box::new(ChannelPluginRouting),
            Box::new(LoopFileSharing),
            Box::new(ToolNudge),
            Box::new(PendingTaskAction),
            Box::new(OutputDiscipline),
            Box::new(NarrationSuppressor),
            Box::new(RepetitionDetector),
            Box::new(LoopDetector),
            Box::new(ErrorRecovery),
            Box::new(ToolResultGrounding),
            Box::new(PresenceAwareness),
            Box::new(ContextPressure),
            Box::new(JanusQuotaWarning),
            Box::new(TaskTrackingNudge),
            Box::new(TaskCompletionNudge),
            Box::new(AskToolNudge),
            Box::new(ResearchDelegationNudge),
        ];

        Self { generators }
    }

    /// Run all generators and collect steering directives.
    /// ProactiveResults is handled separately — its output goes to proactive_context.
    pub fn generate(&self, ctx: &Context) -> (Vec<SteeringDirective>, Vec<String>) {
        let mut directives = Vec::new();
        let mut proactive_context = Vec::new();

        // ProactiveResults goes to separate output
        if !ctx.proactive_items.is_empty() {
            for item in &ctx.proactive_items {
                proactive_context.push(format!(
                    "[{}] {}: {}",
                    item.priority, item.source, item.summary
                ));
            }
        }

        // Provider-specific skip rules
        // NOTE: Janus is a gateway that proxies to any upstream (GPT, Claude, Gemini).
        // Only skip for direct Anthropic connections where we know it's Claude.
        let is_claude = ctx.provider_id == "anthropic";
        let is_ollama = ctx.provider_id == "ollama";

        for g in &self.generators {
            // Skip narration/discipline generators for direct Claude only — Claude follows system prompt well
            if is_claude
                && matches!(
                    g.name(),
                    "narration_suppressor"
                        | "output_discipline"
                        | "repetition_detector"
                        | "ask_tool_nudge"
                )
            {
                continue;
            }
            // Skip JanusQuotaWarning for Ollama
            if is_ollama && g.name() == "janus_quota_warning" {
                continue;
            }

            // Panic recovery per generator
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| g.generate(ctx)));

            match result {
                Ok(dirs) => {
                    for mut d in dirs {
                        d.content = d.content.replace("{agent_name}", &ctx.agent_name);
                        directives.push(d);
                    }
                }
                Err(_) => {
                    tracing::warn!(generator_name = g.name(), "steering generator panicked");
                }
            }
        }

        (directives, proactive_context)
    }
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

// --- Helper functions ---

fn count_assistant_turns(messages: &[ChatMessage]) -> usize {
    messages
        .iter()
        .filter(|m| m.role == "assistant" && !m.content.is_empty())
        .count()
}

fn count_turns_since_any_tool_use(messages: &[ChatMessage]) -> i32 {
    let mut count = 0;
    for msg in messages.iter().rev() {
        if msg.role == "assistant" {
            if msg
                .tool_calls
                .as_ref()
                .is_some_and(|tc| !tc.is_empty() && tc != "[]" && tc != "null")
            {
                return count;
            }
            if !msg.content.is_empty() {
                count += 1;
            }
        }
    }
    -1 // never used tools
}

/// Detect if recent user messages contain stop/cancel/abort requests.
fn user_requested_stop(messages: &[ChatMessage]) -> bool {
    // Exact stop commands — the ENTIRE message (trimmed) must match one of these.
    // This prevents false positives like "stop submitting the form" or
    // "and stop doing that" which are instructions, not stop commands.
    let exact_commands = [
        "stop",
        "stop.",
        "stop!",
        "stop it",
        "stop it.",
        "stop now",
        "cancel",
        "abort",
        "halt",
        "quit",
        "enough",
        "enough.",
        "that's enough",
        "break out",
        "stop stop",
        "please stop",
        "just stop",
        "ok stop",
    ];
    // Check last 3 user messages
    let recent_user: Vec<&ChatMessage> = messages
        .iter()
        .rev()
        .filter(|m| m.role == "user" && !m.content.starts_with("<system>"))
        .take(3)
        .collect();

    for msg in &recent_user {
        let lower = msg.content.to_lowercase();
        let trimmed = lower.trim();
        // Only match if the entire message is a stop command (< 30 chars)
        if trimmed.len() < 30 {
            for p in &exact_commands {
                if trimmed == *p {
                    return true;
                }
            }
        }
    }
    false
}

/// Check if the loop should be force-broken. Called by the runner BEFORE
/// making the next LLM call. Returns Some(reason) if the loop must stop.
pub fn should_force_break(ctx: &Context) -> Option<String> {
    // Only hard-stop on explicit user stop command.
    // Everything else is handled by the iteration budget (100 iterations).
    // Hermes uses budget-only (90 iterations, no error/loop tracking) and it works.
    // The model is smart enough to self-correct — aggressive circuit breakers
    // kill legitimate browser automation (Google Flights, Amazon, etc.).
    if user_requested_stop(&ctx.messages) && ctx.iteration > 2 {
        return Some("Circuit breaker: user requested stop. Halting agent loop.".to_string());
    }

    None
}

// --- Generator implementations ---

// 1. Identity Guard
struct IdentityGuard;
impl Generator for IdentityGuard {
    fn name(&self) -> &str {
        "identity_guard"
    }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        let turns = count_assistant_turns(&ctx.messages);
        if turns >= 8 && turns % 8 == 0 {
            vec![SteeringDirective {
                label: "Identity".to_string(),
                content: "You are {agent_name}, a personal AI companion. Stay in character. \
                          Maintain your established personality and communication style."
                    .to_string(),
                priority: 5,
            }]
        } else {
            vec![]
        }
    }
}

// 2. Channel Adapter
struct ChannelAdapter;
impl Generator for ChannelAdapter {
    fn name(&self) -> &str {
        "channel_adapter"
    }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        let content = match ctx.channel.as_str() {
            "dm" => "Keep responses concise for direct messages. Avoid markdown formatting.",
            "cli" => "Use plain text output suitable for terminal display. No markdown.",
            "voice" => "Keep responses to 1-2 sentences. No formatting or special characters.",
            _ => return vec![],
        };
        vec![SteeringDirective {
            label: "Channel".to_string(),
            content: content.to_string(),
            priority: 3,
        }]
    }
}

// 2b. Channel Plugin Routing — when the agent is operating inside a
// channel that's served by a plugin (slack, discord, teams, future ones),
// steer it toward the right tool and the right default action.
//
// Triggers on any non-internal `ctx.channel`. Internal channel types are
// Nebo-architectural concepts; everything else is plugin-backed by
// definition. No hardcoded plugin slugs — `{kind}` is filled from the
// runtime channel string. Adding a new channel plugin requires zero edits
// here.
struct ChannelPluginRouting;
impl Generator for ChannelPluginRouting {
    fn name(&self) -> &str {
        "channel_plugin_routing"
    }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        // Internal channel types Nebo owns directly — no plugin layer.
        // Keep this list minimal; it's the boundary between built-in
        // surfaces and plugin-backed surfaces.
        const INTERNAL_CHANNELS: &[&str] = &["", "web", "dm", "cli", "voice"];
        if INTERNAL_CHANNELS.contains(&ctx.channel.as_str()) {
            return vec![];
        }

        let kind = &ctx.channel;
        // Imperative dual-reinforcement style — same shape as
        // claude-code's tool-selection prompts ("ALWAYS use X. NEVER
        // use Y."). Phrased generically: no `slack` / `discord` baked
        // in, only the runtime `{kind}`.
        let content = format!(
            "Channel context: `{kind}`. \
             ALWAYS route channel I/O through `plugin(resource: \"{kind}\", command: \"...\")`. \
             NEVER use `skill` for channel messaging — channels are plugins, not skills, \
             and `skill discover` will not find `{kind}`. \
             When the user references a local file or asks you to grab/share/send/upload one, \
             the DEFAULT action is to upload it into this channel via \
             `plugin(resource: \"{kind}\", command: \"upload --path <abs_path>\")` — \
             the bridge fills in the channel and thread automatically, you only need the path. \
             Do NOT offer to copy, extract, or link a file unless the user explicitly asks \
             for that instead. For plain text replies, just write your response — \
             the channel layer posts it for you."
        );
        vec![SteeringDirective {
            label: "Channel Routing".to_string(),
            content,
            priority: 2,
        }]
    }
}

// 2c. Loop File Sharing — the NeboAI loop ("neboai" channel) is an internal
// surface, so ChannelPluginRouting early-returns for it. But loop replies DO
// support file attachments (chat_dispatch collects tool `image_url` paths and
// staples them onto the reply). Tell the bot how to use the loop tool's
// `share` action so it doesn't decline file requests.
struct LoopFileSharing;
impl Generator for LoopFileSharing {
    fn name(&self) -> &str {
        "loop_file_sharing"
    }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        if ctx.channel != "neboai" {
            return vec![];
        }
        vec![SteeringDirective {
            label: "Loop File Sharing".to_string(),
            content: "You're in a shared NeboAI loop. When the user references a local file \
                      or asks you to share/send/upload one, share it by calling \
                      `loop(resource: \"channel\", action: \"share\", path: \"<abs_path>\")` \
                      (or `resource: \"dm\"` for a direct message) — it attaches to your reply \
                      automatically. You do NOT need a plugin for this."
                .to_string(),
            priority: 2,
        }]
    }
}

// 3. Tool Nudge
struct ToolNudge;
impl Generator for ToolNudge {
    fn name(&self) -> &str {
        "tool_nudge"
    }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        if ctx.active_task.is_empty() {
            return vec![];
        }
        let turns = count_assistant_turns(&ctx.messages);
        let turns_since = count_turns_since_any_tool_use(&ctx.messages);
        if turns >= 5 && turns_since >= 5 {
            vec![SteeringDirective {
                label: "Tool Nudge".to_string(),
                content: "You have an active task but haven't used any tools recently. \
                          Consider using your available tools to make progress."
                    .to_string(),
                priority: 7,
            }]
        } else {
            vec![]
        }
    }
}

// 4. Pending Task Action
struct PendingTaskAction;
impl Generator for PendingTaskAction {
    fn name(&self) -> &str {
        "pending_task_action"
    }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        if ctx.active_task.is_empty() || ctx.iteration < 2 {
            return vec![];
        }
        // Don't fire if tools were used recently (model is actively working)
        if count_turns_since_any_tool_use(&ctx.messages) == 0 {
            return vec![];
        }
        let content = format!(
            "Your objective: {}\n\
             You still have work to do — your last response was text-only but the task is NOT complete.\n\
             Call a tool RIGHT NOW to continue. Do NOT respond with text explaining what you plan to do.",
            ctx.active_task
        );
        vec![SteeringDirective {
            label: "Action Required".to_string(),
            content,
            priority: 8,
        }]
    }
}

// 5. Output Discipline — proactive reinforcement for non-Claude models.
// Modeled after Hermes TOOL_USE_ENFORCEMENT_GUIDANCE which uses forceful
// language ("MUST", "immediately", "not acceptable") targeted at GPT/Codex.
struct OutputDiscipline;
impl Generator for OutputDiscipline {
    fn name(&self) -> &str {
        "output_discipline"
    }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        // Only fire when the last response was excessively long
        if ctx.iteration < 1 {
            return vec![];
        }
        let last_len = ctx
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "assistant")
            .map(|m| m.content.len())
            .unwrap_or(0);

        if last_len > 300 {
            vec![SteeringDirective {
                label: "Output Discipline".to_string(),
                content: "Your last response was too long. Corrections:\n\
                         1. Tool calls: output ZERO text alongside them.\n\
                         2. Results: 1-3 sentences maximum.\n\
                         3. Never repeat information you already said.\n\
                         4. Never announce errors or limitations — handle them silently or try a different approach."
                    .to_string(),
                priority: 9,
            }]
        } else {
            vec![]
        }
    }
}

// 6b. Narration Suppressor — detects text+tool narration pattern
struct NarrationSuppressor;
impl Generator for NarrationSuppressor {
    fn name(&self) -> &str {
        "narration_suppressor"
    }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        if ctx.iteration < 1 {
            return vec![];
        }

        // Count recent assistant messages that have BOTH text (>50 chars) AND tool calls
        let mut narrating_turns = 0usize;
        for msg in ctx.messages.iter().rev().take(6) {
            if msg.role != "assistant" {
                continue;
            }
            let has_tool_calls = msg
                .tool_calls
                .as_ref()
                .is_some_and(|tc| !tc.is_empty() && tc != "[]" && tc != "null");
            if has_tool_calls && msg.content.len() > 50 {
                narrating_turns += 1;
            }
        }

        // Fire on first narrating turn (was 2 — too late for GPT)
        if narrating_turns >= 1 {
            return vec![SteeringDirective {
                label: "Narration".to_string(),
                content: "STOP narrating your tool calls. Output ONLY the tool call — \
                         ZERO text before, between, or after. The user sees your tool calls directly.".to_string(),
                priority: 8,
            }];
        }

        vec![]
    }
}

// 6c. Repetition Detector — catches GPT's habit of restating the same info
struct RepetitionDetector;
impl Generator for RepetitionDetector {
    fn name(&self) -> &str {
        "repetition_detector"
    }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        if ctx.iteration < 3 {
            return vec![];
        }

        // Collect recent non-empty assistant text responses
        let recent_texts: Vec<&str> = ctx
            .messages
            .iter()
            .rev()
            .filter(|m| m.role == "assistant" && m.content.len() > 100)
            .take(4)
            .map(|m| m.content.as_str())
            .collect();

        if recent_texts.len() < 2 {
            return vec![];
        }

        // Simple similarity: check if bigrams overlap significantly between consecutive responses
        let mut repetitive_pairs = 0usize;
        for window in recent_texts.windows(2) {
            let a_words: Vec<&str> = window[0].split_whitespace().collect();
            let b_words: Vec<&str> = window[1].split_whitespace().collect();
            if a_words.len() < 10 || b_words.len() < 10 {
                continue;
            }
            // Count shared 3-grams
            let a_trigrams: std::collections::HashSet<String> = a_words
                .windows(3)
                .map(|w| w.join(" ").to_lowercase())
                .collect();
            let b_trigrams: std::collections::HashSet<String> = b_words
                .windows(3)
                .map(|w| w.join(" ").to_lowercase())
                .collect();
            let shared = a_trigrams.intersection(&b_trigrams).count();
            let min_size = a_trigrams.len().min(b_trigrams.len());
            if min_size > 0 && (shared * 100 / min_size) > 40 {
                repetitive_pairs += 1;
            }
        }

        if repetitive_pairs >= 1 {
            return vec![SteeringDirective {
                label: "Repetition".to_string(),
                content: "You are REPEATING YOURSELF. Your recent responses contain the same information \
                         restated multiple times. STOP. Either:\n\
                         (a) Take a NEW action with a tool, or\n\
                         (b) Give the user a final 1-sentence answer and STOP.\n\
                         Do NOT output another status update.".to_string(),
                priority: 9,
            }];
        }

        vec![]
    }
}

// 7. Loop Detector — OpenClaw-style hash-based detection.
// Uses (name_hash, args_hash, result_hash) tuples instead of tool name strings.
// This correctly distinguishes web(navigate, google.com) → web(click, button)
// (legitimate browser work) from web(search, "flights") × 5 (actual loop).
struct LoopDetector;
impl Generator for LoopDetector {
    fn name(&self) -> &str {
        "loop_detector"
    }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        let mut directives = Vec::new();

        // Budget pressure warnings (Hermes pattern — 70%/90% thresholds).
        // This is the only loop-related steering we keep. Hermes has no explicit
        // loop detection — just an iteration budget of 90. We match that approach.
        const MAX_ITERATIONS: usize = 100;
        let pct = (ctx.iteration * 100) / MAX_ITERATIONS;
        if pct >= 90 {
            let remaining = MAX_ITERATIONS.saturating_sub(ctx.iteration);
            directives.push(SteeringDirective {
                label: "Budget Critical".to_string(),
                content: format!(
                    "BUDGET WARNING: Iteration {}/{}. Only {} left. \
                     Provide your final answer NOW. No more tool calls unless absolutely critical.",
                    ctx.iteration, MAX_ITERATIONS, remaining
                ),
                priority: 10,
            });
        } else if pct >= 70 {
            let remaining = MAX_ITERATIONS.saturating_sub(ctx.iteration);
            directives.push(SteeringDirective {
                label: "Budget Caution".to_string(),
                content: format!(
                    "Budget: iteration {}/{}. {} iterations left. Start consolidating your work.",
                    ctx.iteration, MAX_ITERATIONS, remaining
                ),
                priority: 6,
            });
        }

        // User stop detection (soft steering — hard stop is in should_force_break)
        if user_requested_stop(&ctx.messages) {
            directives.push(SteeringDirective {
                label: "User Stop".to_string(),
                content: "The user has asked you to STOP. Cease all tool calls immediately. \
                         Respond with a brief summary and stop."
                    .to_string(),
                priority: 10,
            });
        }

        // Duplicate tool call detection
        let mut seen: std::collections::HashMap<(u64, u64), (usize, String)> = std::collections::HashMap::new();
        for (i, &(name_hash, args_hash, _)) in ctx.recent_tool_result_hashes.iter().enumerate() {
            let name = ctx.recent_tool_names.get(i).cloned().unwrap_or_default();
            seen.entry((name_hash, args_hash))
                .and_modify(|e| e.0 += 1)
                .or_insert((1, name));
        }

        for ((_,_), (count, tool_name)) in &seen {
            if *count >= 3 {
                directives.push(SteeringDirective {
                    label: "Duplicate Tool Call".to_string(),
                    content: format!(
                        "You have called {} with identical arguments {} times. \
                         The result will not change. Try a different approach: \
                         different parameters, a different tool, or summarize \
                         what you know and respond to the user.",
                        tool_name, count
                    ),
                    priority: 8,
                });
            }
        }

        directives
    }
}

// 8. AutomationSpeed — REMOVED.
// Hermes has no equivalent. It penalized legitimate browser workflows
// (snapshot→click→snapshot→click is how browser automation works).
// The iteration budget is sufficient to prevent runaway execution.

// 9. Presence Awareness — adapts behavior based on user focus state
struct PresenceAwareness;
impl Generator for PresenceAwareness {
    fn name(&self) -> &str {
        "presence_awareness"
    }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        if ctx.user_presence.is_empty() || ctx.iteration < 2 {
            return vec![];
        }

        match ctx.user_presence.as_str() {
            "unfocused" | "away" => {
                vec![SteeringDirective {
                    label: "Presence".to_string(),
                    content:
                        "The user stepped away. Continue working autonomously on active tasks. \
                              Be thorough but concise in your output."
                            .to_string(),
                    priority: 4,
                }]
            }
            "focused" if ctx.user_just_returned => {
                vec![SteeringDirective {
                    label: "Presence".to_string(),
                    content: "The user is back. If you completed work while they were away, \
                              briefly summarize what you accomplished."
                        .to_string(),
                    priority: 4,
                }]
            }
            _ => vec![],
        }
    }
}

// 10. Context Pressure
struct ContextPressure;
impl Generator for ContextPressure {
    fn name(&self) -> &str {
        "context_pressure"
    }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        // Fire every 15 iterations starting at 15 as a proxy for high context usage
        if ctx.iteration < 15 || ctx.iteration % 15 != 0 {
            return vec![];
        }
        vec![SteeringDirective {
            label: "Context Pressure".to_string(),
            content: "Context window is filling up. Summarize tool results instead of echoing them verbatim. \
                      If you need earlier results, re-run the tool rather than quoting from memory."
                .to_string(),
            priority: 6,
        }]
    }
}

// 12. Janus Quota Warning
struct JanusQuotaWarning;
impl Generator for JanusQuotaWarning {
    fn name(&self) -> &str {
        "janus_quota_warning"
    }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        if let Some(ref warning) = ctx.quota_warning {
            if !warning.is_empty() {
                return vec![SteeringDirective {
                    label: "Cost Alert".to_string(),
                    content: format!(
                        "{}. Be cost-conscious — prefer shorter responses, \
                         avoid unnecessary tool calls, and minimize token usage.",
                        warning
                    ),
                    priority: 7,
                }];
            }
        }
        vec![]
    }
}

// 14. Error Recovery — soft advisory after sustained errors.
// Hermes has no error recovery steering at all. We keep a light nudge at 3+
// consecutive errors as an advisory, not a command. Single failures are normal
// (browser timeouts, transient network issues).
struct ErrorRecovery;
impl Generator for ErrorRecovery {
    fn name(&self) -> &str {
        "error_recovery"
    }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        // Don't fire on 1-2 errors — single failures are normal, especially
        // in browser automation (click timeouts, page loading, etc.)
        if ctx.consecutive_error_iterations < 3 {
            return vec![];
        }

        // At 3+: soft advisory suggesting a different approach
        vec![SteeringDirective {
            label: "Error Recovery".to_string(),
            content: format!(
                "Note: {} consecutive iterations had errors. Consider reading the error messages \
                 carefully and trying a different approach if the current one isn't working.",
                ctx.consecutive_error_iterations
            ),
            priority: 6,
        }]
    }
}

// 14b. Tool Result Grounding — prevent hallucinating tool failures when tools succeed
struct ToolResultGrounding;
impl Generator for ToolResultGrounding {
    fn name(&self) -> &str {
        "tool_result_grounding"
    }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        if !ctx.recent_tool_names.iter().any(|n| n == "web") {
            return vec![];
        }

        // Scan last 10 messages for web tool results that succeeded with content
        let recent = ctx.messages.iter().rev().take(10);
        let mut web_success_chars = 0usize;
        for msg in recent {
            if msg.role != "tool" {
                continue;
            }
            if let Some(ref tr_json) = msg.tool_results {
                if let Ok(results) = serde_json::from_str::<Vec<serde_json::Value>>(tr_json) {
                    for r in &results {
                        let is_error = r.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
                        let content_len = r
                            .get("content")
                            .and_then(|v| v.as_str())
                            .map(|s| s.len())
                            .unwrap_or(0);
                        if !is_error && content_len > 500 {
                            web_success_chars += content_len;
                        }
                    }
                }
            }
            if !msg.content.is_empty() && msg.content.len() > 500 {
                web_success_chars += msg.content.len();
            }
        }

        if web_success_chars > 1000 {
            vec![SteeringDirective {
                label: "Tool Result Grounding".to_string(),
                content: format!(
                    "IMPORTANT: Web tools returned {} chars of content in recent calls. \
                     Do NOT claim tools are broken, empty, or returning 0 lines — \
                     read your actual tool results and use the data you received. \
                     If a page is a 404, navigate elsewhere — do not declare all tools broken.",
                    web_success_chars
                ),
                priority: 9,
            }]
        } else {
            vec![]
        }
    }
}

// 15. Task Tracking Nudge — steer the LLM to break complex requests into tracked tasks
struct TaskTrackingNudge;
impl Generator for TaskTrackingNudge {
    fn name(&self) -> &str {
        "task_tracking_nudge"
    }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        // Only fire on first iteration (when user just sent a request)
        if ctx.iteration != 1 {
            return vec![];
        }
        // Don't fire if tasks already exist (LLM already tracking)
        if !ctx.work_tasks.is_empty() {
            return vec![];
        }

        // Detect multi-step complexity in the user prompt
        let lower = ctx.user_prompt.to_lowercase();
        let complexity_signals = [
            "and then",
            "after that",
            "first",
            "next",
            "finally",
            "step 1",
            "step 2",
            "1.",
            "2.",
            "3.",
            "multiple",
            "each",
            "all of",
            "every",
            "research",
            "compare",
            "analyze",
            "plan",
            "set up",
            "configure",
            "build",
            "create a",
            "organize",
            "clean up",
            "migrate",
        ];
        let signal_count = complexity_signals
            .iter()
            .filter(|s| lower.contains(*s))
            .count();

        // Also check message length as a proxy for complexity
        let is_long = ctx.user_prompt.len() > 200;

        if signal_count < 2 && !is_long {
            return vec![];
        }

        vec![SteeringDirective {
            label: "Task Tracking".to_string(),
            content: "This looks like a multi-step request. Break it into trackable tasks so the user \
                     can see your progress:\n\
                     1. Create tasks: bot(resource: \"task\", action: \"create\", subject: \"...\")\n\
                     2. Update as you work: bot(resource: \"task\", action: \"update\", task_id: N, status: \"in_progress\")\n\
                     3. Mark complete with output: bot(resource: \"task\", action: \"update\", task_id: N, status: \"completed\", output: \"...\")\n\
                     Create all tasks upfront, then work through them one at a time."
                .to_string(),
            priority: 6,
        }]
    }
}

// 16. Task Completion Nudge — remind to update tasks when work is being done but tasks aren't progressing
struct TaskCompletionNudge;
impl Generator for TaskCompletionNudge {
    fn name(&self) -> &str {
        "task_completion_nudge"
    }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        // Only fire if there ARE tasks and tools are being used
        if ctx.work_tasks.is_empty() || ctx.iteration < 3 {
            return vec![];
        }
        // Check: all tasks still pending despite tool usage
        let all_pending = ctx.work_tasks.iter().all(|t| t.status == "pending");
        let has_tool_use = count_turns_since_any_tool_use(&ctx.messages) == 0;

        if all_pending && has_tool_use {
            vec![SteeringDirective {
                label: "Task Progress".to_string(),
                content: "You have tasks but none are marked in_progress or completed. \
                         Update task status as you work: \
                         bot(resource: \"task\", action: \"update\", task_id: N, status: \"in_progress\") \
                         before starting, then status: \"completed\" with output when done."
                    .to_string(),
                priority: 5,
            }]
        } else {
            vec![]
        }
    }
}

// 17. Ask Tool Nudge — steer the LLM to use the interactive ask widget instead of plain-text questions
struct AskToolNudge;
impl Generator for AskToolNudge {
    fn name(&self) -> &str {
        "ask_tool_nudge"
    }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        // Find the last assistant message
        let last_assistant = ctx
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "assistant" && !m.content.is_empty());
        let msg = match last_assistant {
            Some(m) => m,
            None => return vec![],
        };

        // Skip if this turn already had an ask tool call
        if let Some(ref tc) = msg.tool_calls {
            if tc.contains("\"ask\"") {
                return vec![];
            }
        }

        // Detect question patterns in the assistant's text
        let text = &msg.content;
        let has_question_mark = text.lines().any(|line| line.trim_end().ends_with('?'));
        let lower = text.to_lowercase();
        let has_choice_phrase = [
            "which do you prefer",
            "what would you like",
            "please choose",
            "let me know",
            "would you rather",
            "which option",
            "pick one",
            "choose from",
        ]
        .iter()
        .any(|p| lower.contains(p));

        if !has_question_mark && !has_choice_phrase {
            return vec![];
        }

        vec![SteeringDirective {
            label: "Ask Tool".to_string(),
            content: "When you need user input, ALWAYS use the ask tool instead of asking in plain text.\n\
                     - Yes/no: agent(resource: \"ask\", action: \"confirm\", text: \"...\")\n\
                     - Choices: agent(resource: \"ask\", action: \"select\", text: \"...\", options: [\"A\", \"B\", \"C\"])\n\
                     - Open-ended: agent(resource: \"ask\", action: \"prompt\", text: \"...\")\n\
                     Never ask questions as plain text �� use the ask tool so the user gets interactive buttons."
                .to_string(),
            priority: 7,
        }]
    }
}

/// Detects when the main agent is in an exploratory research loop —
/// repeatedly calling discovery-flavored tools (`tool_search`, `skill`,
/// `plugin help/events`) trying to figure out how to do something — and
/// nudges it to delegate the discovery to a sub-agent instead.
///
/// Why: every exploratory tool call adds a user message + tool result pair
/// to the main conversation, polluting the context window for the
/// downstream turn. A sub-agent burns its OWN context on the research and
/// returns one consolidated answer, keeping the main chat history clean.
///
/// Triggers when 3+ of the recent (≤8) tool calls were discovery-flavored.
struct ResearchDelegationNudge;
impl Generator for ResearchDelegationNudge {
    fn name(&self) -> &str {
        "research_delegation_nudge"
    }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        let recent = &ctx.recent_tool_names;
        if recent.len() < 3 {
            return vec![];
        }
        let window: Vec<&String> = recent.iter().rev().take(8).collect();
        let mut discovery_count = 0usize;
        let mut plugin_count = 0usize;
        for name in &window {
            match name.as_str() {
                "tool_search" | "skill" => discovery_count += 1,
                "plugin" => plugin_count += 1,
                _ => {}
            }
        }
        // Treat repeated plugin probes as additional discovery signal
        // (agent calling the same plugin tool multiple times to look up
        // syntax via +help / --help / events).
        if plugin_count >= 2 {
            discovery_count += plugin_count.saturating_sub(1);
        }
        if discovery_count < 3 {
            return vec![];
        }

        vec![SteeringDirective {
            label: "Delegate Research".to_string(),
            content: "You've made several discovery / how-to tool calls in this turn. \
                      STOP exploring inline — it pollutes the main context. \
                      Spawn a sub-agent to do the research and report back: \
                      agent(resource: \"task\", action: \"spawn\", prompt: \"Figure out exactly how to <specific question>. Return the exact command / syntax / path as a single answer.\"). \
                      The sub-agent uses its own context for the exploration; you get one consolidated answer to act on."
                .to_string(),
            priority: 8,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            id: String::new(),
            chat_id: String::new(),
            role: role.to_string(),
            content: content.to_string(),
            metadata: None,
            created_at: 0,
            day_marker: None,
            tool_calls: None,
            tool_results: None,
            token_estimate: None,
            html: None,
        }
    }

    fn make_ctx(messages: Vec<ChatMessage>) -> Context {
        Context {
            session_id: String::new(),
            messages,
            user_prompt: String::new(),
            active_task: String::new(),
            channel: "web".to_string(),
            agent_name: "Nebo".to_string(),
            iteration: 1,
            work_tasks: vec![],
            quota_warning: None,
            consecutive_error_iterations: 0,
            recent_tool_result_hashes: vec![],
            user_presence: String::new(),
            user_just_returned: false,
            proactive_items: vec![],
            provider_id: "openai".to_string(),
            recent_tool_names: vec![],
            execution_mode: tools::ExecutionMode::Interactive,
        }
    }

    #[test]
    fn test_format_directives_empty() {
        assert_eq!(format_directives(&[]), "");
    }

    #[test]
    fn test_format_directives() {
        let dirs = vec![
            SteeringDirective {
                label: "Loop Warning".to_string(),
                content: "You called web 3 times".to_string(),
                priority: 9,
            },
            SteeringDirective {
                label: "Action Bias".to_string(),
                content: "Stop narrating, use tools".to_string(),
                priority: 8,
            },
        ];
        let result = format_directives(&dirs);
        assert!(result.contains("## Agent Directives"));
        assert!(result.contains("[Loop Warning] You called web 3 times"));
        assert!(result.contains("[Action Bias] Stop narrating, use tools"));
    }

    #[test]
    fn test_identity_guard_fires_at_8() {
        let mut messages = Vec::new();
        for i in 0..8 {
            messages.push(make_msg("user", &format!("msg {}", i)));
            messages.push(make_msg("assistant", &format!("reply {}", i)));
        }

        let guard = IdentityGuard;
        let ctx = Context {
            messages,
            ..make_ctx(vec![])
        };
        let result = guard.generate(&ctx);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].label, "Identity");
    }

    #[test]
    fn test_presence_awareness_away() {
        let messages = vec![make_msg("user", "hello"), make_msg("assistant", "hi")];
        let generator = PresenceAwareness;
        let mut ctx = make_ctx(messages);
        ctx.iteration = 3;
        ctx.user_presence = "away".to_string();
        let result = generator.generate(&ctx);
        assert_eq!(result.len(), 1);
        assert!(result[0].content.contains("stepped away"));
    }

    #[test]
    fn test_presence_awareness_returned() {
        let messages = vec![make_msg("user", "hello"), make_msg("assistant", "hi")];
        let generator = PresenceAwareness;
        let mut ctx = make_ctx(messages);
        ctx.iteration = 3;
        ctx.user_presence = "focused".to_string();
        ctx.user_just_returned = true;
        let result = generator.generate(&ctx);
        assert_eq!(result.len(), 1);
        assert!(result[0].content.contains("user is back"));
    }

    #[test]
    fn test_pipeline_generates_proactive_context() {
        let pipeline = Pipeline::new();
        let mut ctx = make_ctx(vec![make_msg("user", "hello")]);
        ctx.proactive_items = vec![crate::proactive::ProactiveItem {
            source: "heartbeat:gws-email".to_string(),
            summary: "3 urgent emails from your boss".to_string(),
            priority: crate::proactive::Priority::Urgent,
            created_at: 1000,
        }];
        let (_, proactive) = pipeline.generate(&ctx);
        assert_eq!(proactive.len(), 1);
        assert!(proactive[0].contains("3 urgent emails"));
    }

    #[test]
    fn test_pipeline_skips_ask_tool_nudge_for_claude() {
        let pipeline = Pipeline::new();
        let mut ctx = make_ctx(vec![
            make_msg("user", "help me pick a color"),
            make_msg("assistant", "Which color do you prefer? Red or blue?"),
        ]);
        ctx.iteration = 2;

        // OpenAI should get AskToolNudge
        ctx.provider_id = "openai".to_string();
        let (dirs_openai, _) = pipeline.generate(&ctx);
        let has_ask_nudge_openai = dirs_openai.iter().any(|d| d.label == "Ask Tool");

        // Claude should NOT get AskToolNudge
        ctx.provider_id = "anthropic".to_string();
        let (dirs_claude, _) = pipeline.generate(&ctx);
        let has_ask_nudge_claude = dirs_claude.iter().any(|d| d.label == "Ask Tool");

        // AskToolNudge fires for openai but not claude
        assert!(has_ask_nudge_openai, "OpenAI should get ask_tool_nudge");
        assert!(!has_ask_nudge_claude, "Claude should skip ask_tool_nudge");
    }

    #[test]
    fn test_user_stop_forces_break_without_errors() {
        let messages = vec![
            make_msg("user", "search for emails"),
            make_msg("assistant", "I'll search for emails."),
            make_msg("user", "stop"),
        ];
        let mut ctx = make_ctx(messages);
        ctx.iteration = 3;
        let result = should_force_break(&ctx);
        assert!(
            result.is_some(),
            "user stop should force break even with zero errors"
        );
        assert!(result.unwrap().contains("user requested stop"));
    }

    #[test]
    fn test_user_stop_no_break_at_iteration_2() {
        let messages = vec![make_msg("user", "stop"), make_msg("assistant", "ok")];
        let mut ctx = make_ctx(messages);
        ctx.iteration = 2;
        let result = should_force_break(&ctx);
        assert!(result.is_none(), "should NOT break at iteration 2");
    }

    #[test]
    fn test_no_hard_stop_on_consecutive_errors() {
        // Hermes approach: no hard stops on errors, only budget.
        let mut ctx = make_ctx(vec![]);
        ctx.consecutive_error_iterations = 3;
        assert!(
            should_force_break(&ctx).is_none(),
            "should NOT break on errors"
        );
        ctx.consecutive_error_iterations = 10;
        assert!(
            should_force_break(&ctx).is_none(),
            "should NOT break even at 10 errors"
        );
    }

    #[test]
    fn test_no_hard_stop_on_same_tool() {
        // Hermes approach: no hard stops on same-tool calls, only budget.
        let mut ctx = make_ctx(vec![]);
        ctx.recent_tool_result_hashes = vec![(1, 2, 3), (1, 2, 3), (1, 2, 3), (1, 2, 3)];
        assert!(
            should_force_break(&ctx).is_none(),
            "should NOT break on same-tool calls"
        );
    }

    #[test]
    fn test_error_recovery_silent_at_1_and_2() {
        let recovery = ErrorRecovery;
        let mut ctx = make_ctx(vec![]);

        ctx.consecutive_error_iterations = 1;
        assert!(
            recovery.generate(&ctx).is_empty(),
            "should NOT fire at 1 error"
        );

        ctx.consecutive_error_iterations = 2;
        assert!(
            recovery.generate(&ctx).is_empty(),
            "should NOT fire at 2 errors"
        );
    }

    #[test]
    fn test_error_recovery_soft_advisory_at_3() {
        let recovery = ErrorRecovery;
        let mut ctx = make_ctx(vec![]);
        ctx.consecutive_error_iterations = 3;
        let result = recovery.generate(&ctx);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].priority, 6, "should be low priority advisory");
        assert!(result[0].content.contains("different approach"));
    }

    // --- Level 2: Failure-mode scenario tests ---

    fn make_assistant_with_tools(content: &str, tool_calls_json: &str) -> ChatMessage {
        ChatMessage {
            tool_calls: Some(tool_calls_json.to_string()),
            html: None,
            ..make_msg("assistant", content)
        }
    }

    #[test]
    fn test_fm1_plain_text_question() {
        // Assistant asks a question on its own line ending with ? → AskToolNudge fires (OpenAI), skipped (Claude)
        let pipeline = Pipeline::new();
        let mut ctx = make_ctx(vec![
            make_msg("user", "Help me redecorate my living room"),
            make_msg(
                "assistant",
                "I can suggest a few options.\nWhich do you prefer?",
            ),
        ]);
        ctx.iteration = 2;

        // OpenAI: should fire
        ctx.provider_id = "openai".to_string();
        let (dirs, _) = pipeline.generate(&ctx);
        assert!(
            dirs.iter().any(|d| d.label == "Ask Tool"),
            "AskToolNudge should fire for OpenAI when assistant asks plain-text question"
        );

        // Claude: should be skipped
        ctx.provider_id = "anthropic".to_string();
        let (dirs, _) = pipeline.generate(&ctx);
        assert!(
            !dirs.iter().any(|d| d.label == "Ask Tool"),
            "AskToolNudge should be skipped for Claude"
        );
    }

    #[test]
    fn test_fm2_narration_with_tools() {
        // Assistant says "Let me search for flights..." + tool call (>50 chars text)
        let pipeline = Pipeline::new();
        let narration = "Let me search for flights from Denver to Tokyo. I'll check multiple airlines for the best prices and dates.";
        let tool_call = r#"[{"name":"web","input":{"action":"search","query":"flights Denver to Tokyo June"}}]"#;
        let mut ctx = make_ctx(vec![
            make_msg("user", "Search for flights"),
            make_assistant_with_tools(narration, tool_call),
        ]);
        ctx.iteration = 2;

        // OpenAI: NarrationSuppressor should fire
        ctx.provider_id = "openai".to_string();
        let (dirs, _) = pipeline.generate(&ctx);
        assert!(
            dirs.iter().any(|d| d.label == "Narration"),
            "NarrationSuppressor should fire for OpenAI when text+tool call detected"
        );

        // Claude: should be skipped
        ctx.provider_id = "anthropic".to_string();
        let (dirs, _) = pipeline.generate(&ctx);
        assert!(
            !dirs.iter().any(|d| d.label == "Narration"),
            "NarrationSuppressor should be skipped for Claude"
        );
    }

    #[test]
    fn test_fm3_verbose_output() {
        // Assistant response > 300 chars, no tool call → OutputDiscipline fires (OpenAI), skipped (Claude)
        let pipeline = Pipeline::new();
        let verbose = "I'd be happy to help you with that! Let me explain in detail what I'm going to do. First, I'll search the web for the latest information. Then I'll compile all the results into a comprehensive summary. After that, I'll format everything nicely for you. This process might take a moment, so please bear with me while I work through each step carefully and thoroughly.";
        let mut ctx = make_ctx(vec![
            make_msg("user", "What's the weather?"),
            make_msg("assistant", verbose),
        ]);
        ctx.iteration = 2;

        // OpenAI: OutputDiscipline should fire
        ctx.provider_id = "openai".to_string();
        let (dirs, _) = pipeline.generate(&ctx);
        assert!(
            dirs.iter().any(|d| d.label == "Output Discipline"),
            "OutputDiscipline should fire for OpenAI when response > 300 chars"
        );

        // Claude: should be skipped
        ctx.provider_id = "anthropic".to_string();
        let (dirs, _) = pipeline.generate(&ctx);
        assert!(
            !dirs.iter().any(|d| d.label == "Output Discipline"),
            "OutputDiscipline should be skipped for Claude"
        );
    }

    #[test]
    fn test_fm4_pending_task_text_only() {
        // Active task, iteration 3, last response text-only → PendingTaskAction fires
        let generator = PendingTaskAction;
        let mut ctx = make_ctx(vec![
            make_msg("user", "Clean up my inbox"),
            make_msg("assistant", "I'll start cleaning up your inbox now."),
            make_msg("assistant", "I found 50 emails to archive."),
        ]);
        ctx.active_task = "Clean up inbox".to_string();
        ctx.iteration = 3;

        let result = generator.generate(&ctx);
        assert_eq!(
            result.len(),
            1,
            "PendingTaskAction should fire when active task + text-only response"
        );
        assert_eq!(result[0].label, "Action Required");
    }

    #[test]
    fn test_fm5_consecutive_errors_no_fire_at_1() {
        // Hermes approach: single error is normal, no steering needed
        let generator = ErrorRecovery;
        let mut ctx = make_ctx(vec![]);
        ctx.consecutive_error_iterations = 1;
        assert!(
            generator.generate(&ctx).is_empty(),
            "ErrorRecovery should NOT fire at 1 error (single failures are normal)"
        );
    }

    #[test]
    fn test_fm6_no_same_tool_loop_detection() {
        // Hermes approach: no same-tool detection, model self-corrects
        let generator = LoopDetector;
        let mut ctx = make_ctx(vec![]);
        ctx.recent_tool_result_hashes = vec![(100, 200, 300), (100, 200, 301), (100, 200, 302)];

        let result = generator.generate(&ctx);
        assert!(
            !result.iter().any(|d| d.label == "Loop Warning"),
            "LoopDetector should NOT fire on same-tool calls (removed)"
        );
    }

    #[test]
    fn test_fm7_no_ping_pong_detection() {
        // Hermes approach: no ping-pong detection
        let generator = LoopDetector;
        let mut ctx = make_ctx(vec![]);
        ctx.recent_tool_result_hashes = vec![
            (1, 2, 10), // A
            (3, 4, 20), // B
            (1, 2, 11), // A again
            (3, 4, 21), // B again
        ];

        let result = generator.generate(&ctx);
        assert!(
            !result.iter().any(|d| d.label == "Ping-Pong"),
            "LoopDetector should NOT have ping-pong detection (removed)"
        );
    }

    #[test]
    fn test_duplicate_detection_fires_at_3() {
        let generator = LoopDetector;
        let mut ctx = make_ctx(vec![]);
        ctx.recent_tool_result_hashes = vec![
            (42, 99, 1),
            (42, 99, 2),
            (42, 99, 3),
        ];
        ctx.recent_tool_names = vec![
            "web".to_string(),
            "web".to_string(),
            "web".to_string(),
        ];

        let result = generator.generate(&ctx);
        assert!(
            result.iter().any(|d| d.label == "Duplicate Tool Call"),
            "LoopDetector should emit Duplicate Tool Call at 3 identical (name, args) pairs"
        );
    }

    #[test]
    fn test_duplicate_detection_silent_below_3() {
        let generator = LoopDetector;
        let mut ctx = make_ctx(vec![]);
        ctx.recent_tool_result_hashes = vec![
            (42, 99, 1),
            (42, 99, 2),
        ];
        ctx.recent_tool_names = vec![
            "web".to_string(),
            "web".to_string(),
        ];

        let result = generator.generate(&ctx);
        assert!(
            !result.iter().any(|d| d.label == "Duplicate Tool Call"),
            "LoopDetector should NOT fire Duplicate Tool Call with only 2 identical pairs"
        );
    }

    #[test]
    fn test_duplicate_detection_different_args_no_fire() {
        let generator = LoopDetector;
        let mut ctx = make_ctx(vec![]);
        ctx.recent_tool_result_hashes = vec![
            (42, 100, 1),
            (42, 200, 2),
            (42, 300, 3),
        ];
        ctx.recent_tool_names = vec![
            "system".to_string(),
            "system".to_string(),
            "system".to_string(),
        ];

        let result = generator.generate(&ctx);
        assert!(
            !result.iter().any(|d| d.label == "Duplicate Tool Call"),
            "LoopDetector should NOT fire when args_hash differs across calls"
        );
    }

    #[test]
    fn test_duplicate_detection_includes_tool_name() {
        let generator = LoopDetector;
        let mut ctx = make_ctx(vec![]);
        ctx.recent_tool_result_hashes = vec![
            (77, 88, 10),
            (77, 88, 20),
            (77, 88, 30),
        ];
        ctx.recent_tool_names = vec![
            "bot".to_string(),
            "bot".to_string(),
            "bot".to_string(),
        ];

        let result = generator.generate(&ctx);
        let dup = result.iter().find(|d| d.label == "Duplicate Tool Call")
            .expect("should have Duplicate Tool Call directive");
        assert!(
            dup.content.contains("bot"),
            "Duplicate Tool Call content should include the tool name 'bot', got: {}",
            dup.content
        );
    }
}
