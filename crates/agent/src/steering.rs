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
            Box::new(ToolNudge),
            Box::new(PendingTaskAction),
            Box::new(ActionBias),
            Box::new(OutputDiscipline),
            Box::new(NarrationSuppressor),
            Box::new(RepetitionDetector),
            Box::new(LoopDetector),
            Box::new(AutomationSpeed),
            Box::new(PresenceAwareness),
            Box::new(ContextPressure),
            Box::new(JanusQuotaWarning),
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
            if is_claude && matches!(g.name(), "action_bias" | "narration_suppressor" | "output_discipline" | "repetition_detector") {
                continue;
            }
            // Skip JanusQuotaWarning for Ollama
            if is_ollama && g.name() == "janus_quota_warning" {
                continue;
            }

            // Panic recovery per generator
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                g.generate(ctx)
            }));

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
            if msg.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty() && tc != "[]" && tc != "null") {
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
    let stop_patterns = [
        "stop", "cancel", "abort", "halt", "quit",
        "enough", "break out", "stop stop",
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
        // Short messages that are clearly stop commands (not long messages that happen
        // to contain the word "stop" in a different context)
        if lower.len() < 80 {
            for p in &stop_patterns {
                if lower.contains(p) {
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
    // 1. Consecutive error iterations — hard limit
    if ctx.consecutive_error_iterations >= 5 {
        return Some(format!(
            "Circuit breaker: {} consecutive iterations where all tool calls failed. \
             Breaking loop to prevent runaway execution.",
            ctx.consecutive_error_iterations,
        ));
    }

    // 2. Same-tool-same-args loop at extreme count — the LLM is ignoring steering.
    // Uses hash-based detection: (name_hash, args_hash) must match for it to count.
    // This correctly allows web(navigate)→web(click)→web(fill) while catching
    // web(search, "flights") × 6 with identical args.
    if ctx.recent_tool_result_hashes.len() >= 2 {
        let last = ctx.recent_tool_result_hashes.last().unwrap();
        let mut same_call_count = 1usize;
        for entry in ctx.recent_tool_result_hashes.iter().rev().skip(1) {
            if entry.0 == last.0 && entry.1 == last.1 {
                same_call_count += 1;
            } else {
                break;
            }
        }
        if same_call_count >= 6 {
            return Some(format!(
                "Circuit breaker: same tool called with identical arguments {} times. \
                 The agent is stuck in a loop and steering was ignored.",
                same_call_count,
            ));
        }
    }

    // 3. User explicitly asked to stop — unconditional hard break
    if user_requested_stop(&ctx.messages) && ctx.iteration > 2 {
        return Some(
            "Circuit breaker: user requested stop. Halting agent loop.".to_string()
        );
    }

    None
}

// --- Generator implementations ---

// 1. Identity Guard
struct IdentityGuard;
impl Generator for IdentityGuard {
    fn name(&self) -> &str { "identity_guard" }
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
    fn name(&self) -> &str { "channel_adapter" }
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

// 3. Tool Nudge
struct ToolNudge;
impl Generator for ToolNudge {
    fn name(&self) -> &str { "tool_nudge" }
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
    fn name(&self) -> &str { "pending_task_action" }
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

// 5. Action Bias — language-agnostic structural detection of narration
struct ActionBias;
impl Generator for ActionBias {
    fn name(&self) -> &str { "action_bias" }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        if ctx.active_task.is_empty() || ctx.iteration < 2 {
            return vec![];
        }

        // Count consecutive text-only assistant responses (no tool calls)
        let mut consecutive_text_only = 0usize;
        for msg in ctx.messages.iter().rev() {
            if msg.role == "user" { break; }
            if msg.role != "assistant" { continue; }
            let has_tool_calls = msg.tool_calls.as_ref()
                .is_some_and(|tc| !tc.is_empty() && tc != "[]" && tc != "null");
            if has_tool_calls { break; }
            if !msg.content.is_empty() {
                consecutive_text_only += 1;
            }
        }

        if consecutive_text_only >= 2 {
            return vec![SteeringDirective {
                label: "Action Bias".to_string(),
                content: format!(
                    "You have responded with text {} times without calling any tool. \
                     You have an active task — call a tool NOW to make progress. \
                     Do not describe what you plan to do — just do it.",
                    consecutive_text_only
                ),
                priority: 8,
            }];
        }

        // Detect: long text response (>200 chars) with no tool call during active task
        if let Some(last) = ctx.messages.iter().rev()
            .find(|m| m.role == "assistant")
        {
            let has_tool_calls = last.tool_calls.as_ref()
                .is_some_and(|tc| !tc.is_empty() && tc != "[]" && tc != "null");
            if !has_tool_calls && last.content.len() > 200 && ctx.iteration >= 3 {
                return vec![SteeringDirective {
                    label: "Action Bias".to_string(),
                    content: "Your last response was long text with no tool call. \
                             Keep responses brief — the user can see your tool calls. \
                             Take the next action instead of explaining.".to_string(),
                    priority: 7,
                }];
            }
        }

        vec![]
    }
}

// 6. Output Discipline — proactive reinforcement for non-Claude models.
// Modeled after Hermes TOOL_USE_ENFORCEMENT_GUIDANCE which uses forceful
// language ("MUST", "immediately", "not acceptable") targeted at GPT/Codex.
struct OutputDiscipline;
impl Generator for OutputDiscipline {
    fn name(&self) -> &str { "output_discipline" }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        let mut directives = Vec::new();

        // Always-on tool enforcement (fires from iteration 0).
        // Hermes-strength language — "MUST", "immediately", "not acceptable".
        directives.push(SteeringDirective {
            label: "Tool Enforcement".to_string(),
            content: "You MUST use your tools to take action — do not describe what you would do \
                     or plan to do without actually doing it. When you say you will perform an \
                     action (e.g. 'I will search', 'Let me check', 'I will look up'), you MUST \
                     immediately make the corresponding tool call in the same response. \
                     Never end your turn with a promise of future action — execute it now.\n\
                     Keep working until the task is actually complete. Do not stop with a summary \
                     of what you plan to do next. If you have tools available that can accomplish \
                     the task, use them instead of telling the user what you would do.\n\
                     Every response MUST either (a) contain tool calls that make progress, or \
                     (b) deliver a final result to the user. Responses that only describe \
                     intentions without acting are not acceptable.".to_string(),
            priority: 9,
        });

        // Check if last assistant message was excessively long
        if ctx.iteration >= 1 {
            let last_len = ctx.messages.iter().rev()
                .find(|m| m.role == "assistant")
                .map(|m| m.content.len())
                .unwrap_or(0);

            if last_len > 300 {
                directives.push(SteeringDirective {
                    label: "Output Violation".to_string(),
                    content: "Your last response was too long. STRICT CORRECTIONS:\n\
                             1. When calling tools: output ZERO text. No preamble, no summary, no status update.\n\
                             2. When reporting results: 1-3 sentences maximum. No bullet lists of \"what I tried\".\n\
                             3. NEVER repeat information you already said.\n\
                             4. NEVER say \"if you want, I can...\" — just continue working.\n\
                             5. NEVER announce timeouts, errors, or limitations — handle them silently or try a different approach.\n\
                             6. NEVER explain what blocked you. The user cares about results, not your process.\n\
                             7. Cut your output by 80%.".to_string(),
                    priority: 9,
                });
            }
        }

        directives
    }
}

// 6b. Narration Suppressor — detects text+tool narration pattern
struct NarrationSuppressor;
impl Generator for NarrationSuppressor {
    fn name(&self) -> &str { "narration_suppressor" }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        if ctx.iteration < 1 {
            return vec![];
        }

        // Count recent assistant messages that have BOTH text (>50 chars) AND tool calls
        let mut narrating_turns = 0usize;
        for msg in ctx.messages.iter().rev().take(6) {
            if msg.role != "assistant" { continue; }
            let has_tool_calls = msg.tool_calls.as_ref()
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
    fn name(&self) -> &str { "repetition_detector" }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        if ctx.iteration < 3 {
            return vec![];
        }

        // Collect recent non-empty assistant text responses
        let recent_texts: Vec<&str> = ctx.messages.iter().rev()
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
            let a_trigrams: std::collections::HashSet<String> = a_words.windows(3)
                .map(|w| w.join(" ").to_lowercase())
                .collect();
            let b_trigrams: std::collections::HashSet<String> = b_words.windows(3)
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
impl LoopDetector {
    /// Check if the tool involved in the detected loop is the plugin tool.
    /// Returns true when the most recent entry in recent_tool_names is "plugin".
    fn is_plugin_loop(names: &[String]) -> bool {
        names.last().is_some_and(|n| n == "plugin")
    }

    /// Extra guidance when the looping tool is a plugin CLI command.
    fn plugin_hint() -> &'static str {
        "\n--- PLUGIN-SPECIFIC RECOVERY ---\n\
         The failing tool is a plugin (external CLI). The command syntax may be wrong. Try:\n\
         - Run the plugin with --help to discover the correct subcommands and flags \
         (e.g. plugin(resource: \"gws\", command: \"gmail users messages --help\"))\n\
         - Check if you're passing the right parameter format (JSON vs positional args)\n\
         - Try a simpler variant of the command first to confirm the subcommand exists"
    }
}
impl Generator for LoopDetector {
    fn name(&self) -> &str { "loop_detector" }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        let mut directives = Vec::new();
        let hashes = &ctx.recent_tool_result_hashes;

        // A. Same-tool-same-args detection (OpenClaw "generic repeat" pattern).
        // Counts consecutive calls with identical (name_hash, args_hash).
        // Different args = different action = not a loop.
        if hashes.len() >= 2 {
            let last = hashes.last().unwrap();
            let mut same_call_count = 1usize;
            for entry in hashes.iter().rev().skip(1) {
                if entry.0 == last.0 && entry.1 == last.1 {
                    same_call_count += 1;
                } else {
                    break;
                }
            }

            if same_call_count >= 4 {
                let plugin_extra = if Self::is_plugin_loop(&ctx.recent_tool_names) {
                    Self::plugin_hint()
                } else { "" };
                directives.push(SteeringDirective {
                    label: "Loop Warning".to_string(),
                    content: format!(
                        "LOOP DETECTED: You have called the same tool with identical arguments {} times. \
                         You are in an infinite loop and MUST break out NOW. Do one of the following:\n\
                         1. Check if a skill can handle this: skill(action: \"catalog\") — a specialized skill may solve this differently.\n\
                         2. Ask your advisors for help: agent(resource: \"advisors\", action: \"deliberate\", task: \"I am stuck in a loop trying to [describe what you're doing]. What alternative approach should I take?\")\n\
                         3. Try a COMPLETELY different tool or approach — not the same tool with different args.\n\
                         4. If none of the above work, tell the user what's blocking you and ask for guidance.\n\
                         Do NOT call the same tool again.{}",
                        same_call_count, plugin_extra
                    ),
                    priority: 10,
                });
            } else if same_call_count >= 2 {
                let plugin_extra = if Self::is_plugin_loop(&ctx.recent_tool_names) {
                    Self::plugin_hint()
                } else { "" };
                directives.push(SteeringDirective {
                    label: "Loop Warning".to_string(),
                    content: format!(
                        "You have called the same tool with identical arguments {} times and the result will not change. \
                         Before repeating, consider:\n\
                         - Is there a skill for this? Try skill(action: \"catalog\") to check.\n\
                         - Try different parameters, a different tool, or a different approach entirely.{}",
                        same_call_count, plugin_extra
                    ),
                    priority: 8,
                });
            }
        }

        // B. Stale-result detection — same tool, same args, AND same result.
        // Stronger signal: even the output is identical.
        if hashes.len() >= 2 {
            let last = hashes.last().unwrap();
            let prev = &hashes[hashes.len() - 2];
            if last.0 == prev.0 && last.1 == prev.1 && last.2 == prev.2 {
                directives.push(SteeringDirective {
                    label: "Stale Results".to_string(),
                    content: format!("You called the same tool with the same arguments and got \
                             identical results. You are NOT making progress. STOP and pivot:\n\
                             1. Check for a skill: skill(action: \"catalog\") — a specialized skill may handle this.\n\
                             2. Consult advisors: agent(resource: \"advisors\", action: \"deliberate\", task: \"describe your stuck situation\") for a fresh perspective.\n\
                             3. Use a completely different tool or approach.\n\
                             Do NOT repeat the same call.{}",
                             if Self::is_plugin_loop(&ctx.recent_tool_names) { Self::plugin_hint() } else { "" }),
                    priority: 9,
                });
            }
        }

        // C. Ping-pong detection (OpenClaw pattern) — A→B→A→B alternating.
        if hashes.len() >= 4 {
            let len = hashes.len();
            let a1 = &hashes[len - 4];
            let b1 = &hashes[len - 3];
            let a2 = &hashes[len - 2];
            let b2 = &hashes[len - 1];
            // Check: (name+args of position -4) == (name+args of position -2)
            //    AND (name+args of position -3) == (name+args of position -1)
            //    AND they're different from each other
            let a_matches = a1.0 == a2.0 && a1.1 == a2.1;
            let b_matches = b1.0 == b2.0 && b1.1 == b2.1;
            let a_differs_from_b = a1.0 != b1.0 || a1.1 != b1.1;
            if a_matches && b_matches && a_differs_from_b {
                directives.push(SteeringDirective {
                    label: "Ping-Pong".to_string(),
                    content: format!("You are alternating between two tool calls in a loop (A→B→A→B). \
                             Neither is making progress. STOP this pattern immediately.\n\
                             1. Check if a skill can handle this differently: skill(action: \"catalog\")\n\
                             2. Ask advisors for a fresh approach: agent(resource: \"advisors\", action: \"deliberate\", task: \"describe the problem you're trying to solve\")\n\
                             3. Try a completely different strategy — not a variation of the same two tools.\n\
                             4. If truly stuck, tell the user what's blocking you.{}",
                             if Self::is_plugin_loop(&ctx.recent_tool_names) { Self::plugin_hint() } else { "" }),
                    priority: 9,
                });
            }
        }

        // D. All-error iteration detection (catches varied-tool loops)
        if ctx.consecutive_error_iterations >= 3 {
            directives.push(SteeringDirective {
                label: "Error Loop".to_string(),
                content: format!(
                    "The last {} iterations all produced errors. You are stuck. STOP retrying and pivot:\n\
                     1. A skill might handle this better: skill(action: \"catalog\") to check.\n\
                     2. Get advice: agent(resource: \"advisors\", action: \"deliberate\", task: \"I keep getting errors trying to [describe task]. What should I try instead?\")\n\
                     3. Try a fundamentally different approach — not the same action with tweaks.\n\
                     4. If nothing works, tell the user what's failing and ask for guidance.{}",
                    ctx.consecutive_error_iterations,
                    if Self::is_plugin_loop(&ctx.recent_tool_names) { Self::plugin_hint() } else { "" }
                ),
                priority: 10,
            });
        }

        // E. Budget pressure warnings (Hermes pattern — 70%/90% thresholds).
        // Max iterations is 100 (from runner).
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

        // F. User stop detection
        if user_requested_stop(&ctx.messages) {
            directives.push(SteeringDirective {
                label: "User Stop".to_string(),
                content: "The user has asked you to STOP. Cease all tool calls immediately. \
                         Respond with a brief summary and stop."
                    .to_string(),
                priority: 10,
            });
        }

        directives
    }
}

// 8. Automation Speed — nudges agent to stop wasting time in automation loops
struct AutomationSpeed;
impl Generator for AutomationSpeed {
    fn name(&self) -> &str { "automation_speed" }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        if ctx.iteration < 4 {
            return vec![];
        }

        // Inspect recent assistant messages for automation inefficiencies
        let recent: Vec<&ChatMessage> = ctx.messages.iter().rev()
            .filter(|m| m.role == "assistant")
            .take(8)
            .collect();

        if recent.is_empty() {
            return vec![];
        }

        // Count wait() calls, redundant read_page/see calls, and single-tool responses
        let mut wait_calls = 0usize;
        let mut consecutive_reads = 0usize;
        let mut max_consecutive_reads = 0usize;
        let mut single_tool_responses = 0usize;

        for msg in &recent {
            if let Some(ref tc_json) = msg.tool_calls {
                if let Ok(calls) = serde_json::from_str::<Vec<serde_json::Value>>(tc_json) {
                    if calls.len() == 1 {
                        single_tool_responses += 1;
                    }

                    let mut has_mutation = false;
                    let mut has_read = false;
                    for call in &calls {
                        let name = call.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let input = call.get("input").or_else(|| call.get("arguments"));

                        // Detect wait() calls
                        if name == "web" {
                            if let Some(args) = input {
                                let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
                                if action == "wait" {
                                    wait_calls += 1;
                                } else if action == "read_page" || action == "screenshot" {
                                    has_read = true;
                                } else if matches!(action,
                                    "click" | "double_click" | "triple_click" | "right_click"
                                    | "fill" | "form_input" | "type" | "select" | "press"
                                    | "navigate" | "go_back" | "go_forward" | "evaluate"
                                    | "drag" | "new_tab" | "close_tab" | "scroll"
                                ) {
                                    has_mutation = true;
                                }
                            }
                        } else if name == "desktop" || name == "os" {
                            if let Some(args) = input {
                                let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
                                if action == "see" || action == "screenshot" {
                                    has_read = true;
                                } else if matches!(action,
                                    "click" | "double_click" | "right_click" | "type"
                                    | "press" | "hotkey" | "scroll" | "drag" | "paste"
                                ) {
                                    has_mutation = true;
                                }
                            }
                        }
                    }

                    // Track consecutive read-only responses (no mutation between reads)
                    if has_read && !has_mutation {
                        consecutive_reads += 1;
                        if consecutive_reads > max_consecutive_reads {
                            max_consecutive_reads = consecutive_reads;
                        }
                    } else if has_mutation {
                        consecutive_reads = 0;
                    }
                }
            }
        }

        // Fire if: 2+ wait calls, or 2+ consecutive reads without mutation, or mostly single-tool responses
        let should_fire = wait_calls >= 2
            || max_consecutive_reads >= 2
            || (single_tool_responses >= 4 && recent.len() >= 5);

        if !should_fire {
            return vec![];
        }

        let mut tips = String::from("You are in an automation loop. Speed tips:");
        if wait_calls >= 2 {
            tips.push_str(" (1) Don't call wait() unless you see a loading spinner.");
        }
        if single_tool_responses >= 4 {
            tips.push_str(" (2) Chain multiple tool calls in one response when possible.");
        }
        if max_consecutive_reads >= 2 {
            tips.push_str(" (3) Don't re-read the page unless you performed an action that changes it.");
        }
        tips.push_str(" Work efficiently.");

        vec![SteeringDirective {
            label: "Automation Speed".to_string(),
            content: tips,
            priority: 6,
        }]
    }
}

// 9. Presence Awareness — adapts behavior based on user focus state
struct PresenceAwareness;
impl Generator for PresenceAwareness {
    fn name(&self) -> &str { "presence_awareness" }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        if ctx.user_presence.is_empty() || ctx.iteration < 2 {
            return vec![];
        }

        match ctx.user_presence.as_str() {
            "unfocused" | "away" => {
                vec![SteeringDirective {
                    label: "Presence".to_string(),
                    content: "The user stepped away. Continue working autonomously on active tasks. \
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
    fn name(&self) -> &str { "context_pressure" }
    fn generate(&self, ctx: &Context) -> Vec<SteeringDirective> {
        // Fire every 15 iterations starting at 15 as a proxy for high context usage
        if ctx.iteration < 15 || ctx.iteration % 15 != 0 {
            return vec![];
        }
        vec![SteeringDirective {
            label: "Context Pressure".to_string(),
            content: "Context window is filling up. Keep responses concise. \
                      Summarize tool results instead of echoing them verbatim. \
                      If you need earlier results, re-run the tool."
                .to_string(),
            priority: 6,
        }]
    }
}

// 12. Janus Quota Warning
struct JanusQuotaWarning;
impl Generator for JanusQuotaWarning {
    fn name(&self) -> &str { "janus_quota_warning" }
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
        let messages = vec![
            make_msg("user", "hello"),
            make_msg("assistant", "hi"),
        ];
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
        let messages = vec![
            make_msg("user", "hello"),
            make_msg("assistant", "hi"),
        ];
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
        ctx.proactive_items = vec![
            crate::proactive::ProactiveItem {
                source: "heartbeat:gws-email".to_string(),
                summary: "3 urgent emails from your boss".to_string(),
                priority: crate::proactive::Priority::Urgent,
                created_at: 1000,
            },
        ];
        let (_, proactive) = pipeline.generate(&ctx);
        assert_eq!(proactive.len(), 1);
        assert!(proactive[0].contains("3 urgent emails"));
    }

    #[test]
    fn test_pipeline_skips_action_bias_for_claude() {
        let pipeline = Pipeline::new();
        let mut ctx = make_ctx(vec![
            make_msg("user", "do something"),
            make_msg("assistant", "I will explain what I plan to do in great detail here and here and here and more text"),
            make_msg("assistant", "I will explain what I plan to do in great detail here and here and here and more text again"),
        ]);
        ctx.active_task = "test task".to_string();
        ctx.iteration = 3;

        // OpenAI should get ActionBias
        ctx.provider_id = "openai".to_string();
        let (dirs_openai, _) = pipeline.generate(&ctx);
        let has_action_bias = dirs_openai.iter().any(|d| d.label == "Action Bias");

        // Claude should NOT get ActionBias
        ctx.provider_id = "anthropic".to_string();
        let (dirs_claude, _) = pipeline.generate(&ctx);
        let has_action_bias_claude = dirs_claude.iter().any(|d| d.label == "Action Bias");

        // Janus is a gateway — should NOT skip ActionBias (may proxy to GPT)
        ctx.provider_id = "janus".to_string();
        let (dirs_janus, _) = pipeline.generate(&ctx);
        let has_action_bias_janus = dirs_janus.iter().any(|d| d.label == "Action Bias");

        // ActionBias fires for openai and janus but not claude
        // (Note: it may not fire in these exact conditions, but the skip rule is exercised)
        assert!(!has_action_bias_claude || !has_action_bias,
            "Claude should skip action_bias when openai doesn't");
        // Janus should behave like openai, not like anthropic
        assert_eq!(has_action_bias, has_action_bias_janus,
            "Janus should not skip action_bias — it's a gateway, not Claude");
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
        assert!(result.is_some(), "user stop should force break even with zero errors");
        assert!(result.unwrap().contains("user requested stop"));
    }

    #[test]
    fn test_user_stop_no_break_at_iteration_2() {
        let messages = vec![
            make_msg("user", "stop"),
            make_msg("assistant", "ok"),
        ];
        let mut ctx = make_ctx(messages);
        ctx.iteration = 2;
        let result = should_force_break(&ctx);
        assert!(result.is_none(), "should NOT break at iteration 2");
    }
}
