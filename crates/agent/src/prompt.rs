use std::collections::{HashMap, HashSet};
use chrono::Offset;

/// Controls how much of the system prompt is assembled.
#[derive(Debug, Clone, Default)]
pub enum PromptMode {
    /// Full prompt: all sections, memory docs, steering, STRAP docs, etiquette.
    /// Used for interactive chat with the main agent.
    #[default]
    Full,
    /// Minimal prompt: identity + capabilities + behavior core.
    /// Drops: memory docs, media, etiquette, comm style, tool routing guide.
    /// Used for sub-agents and focused tasks.
    Minimal,
}

/// Static inputs populated once per Run() call and reused across iterations.
#[derive(Debug, Clone, Default)]
pub struct PromptContext {
    pub mode: PromptMode,
    pub agent_name: String,
    pub active_skill: Option<String>,
    /// Compact skill catalog: "## Available Skills\n- name: description\n..."
    pub skill_catalog: String,
    pub model_aliases: String,
    pub channel: String,
    pub platform: String,
    pub memory_context: String,
    /// Rich DB context formatted via db_context::format_for_system_prompt.
    /// When set, replaces the simple memory_context in the prompt.
    pub db_context: Option<String>,
    /// Active agent AGENT.md body, injected before identity when set.
    pub active_agent: Option<String>,
    /// Installed plugin inventory (env vars + binary paths) for the system prompt.
    pub plugin_inventory: String,
    /// When set, research methodology is appended to the system prompt.
    /// Injected when bot(action: "research") activates research mode.
    pub research_prompt: Option<String>,
}

/// Per-iteration inputs that change between agentic loop iterations.
#[derive(Debug, Clone, Default)]
pub struct DynamicContext {
    pub provider_name: String,
    pub model_name: String,
    pub active_task: String,
    pub summary: String,
    /// Whether this message arrived via NeboLoop (comm channel).
    pub neboloop_connected: bool,
    /// The channel this message arrived on (e.g., "web", "neboloop", "mcp").
    pub channel: String,
    /// Work tasks for the current session (synced from pending_tasks).
    pub work_tasks: Vec<crate::steering::WorkTask>,
    /// Cached tool documentation (key → content). Injected into the dynamic
    /// suffix so it survives sliding window eviction.
    pub tool_doc_cache: Vec<(String, String)>,
    /// Formatted steering directives (from Pipeline::generate + hooks + continuation).
    pub steering_directives: String,
    /// Background proactive results (actual content, not behavioral guidance).
    pub proactive_context: String,
    /// User-configured IANA timezone (e.g. "America/Denver"). When set, date/time
    /// in the dynamic suffix is computed in this timezone instead of system-local.
    pub user_timezone: Option<String>,
}

/// Marker separating the stable/cacheable prefix (Sections 1–8) from
/// semi-dynamic content (skill hints, active skill, model aliases).
/// Providers that support prompt caching can split at this boundary
/// to maximise cache hit rates on the stable prefix.
pub const CACHE_BOUNDARY: &str = "\n<!-- CACHE_BOUNDARY -->\n";

/// Find the byte offset of `CACHE_BOUNDARY` within a static system prompt.
/// Returns `None` when the marker is absent (e.g. the prompt was built
/// without the boundary, or has been mutated by a hook).
pub fn cache_boundary_offset(static_system: &str) -> Option<usize> {
    static_system.find(CACHE_BOUNDARY)
}

// --- Prompt section constants ---

const SECTION_IDENTITY: &str = r#"You are {agent_name}, a personal AI companion running on the user's computer. You have a real shell, real filesystem, real web browser, and real internet access. Your tools execute directly on this machine.

You are NOT a code editor, IDE, developer tool, or coding assistant. You are a personal companion that helps with everyday tasks — browsing the web, managing files, controlling apps, scheduling, communication, and anything else the user needs. You have no "codebase" and no "security principles" about code. You just help people get things done.

## Execution Principles

**Bias toward action.** When the user asks you to do something, use your tools to do it. Do not explain how, offer scripts, or propose plans. Pick the best approach and execute it.

**Ask only when genuinely stuck.** Only ask the user when you cannot proceed — missing credentials, ambiguous destructive action, or truly unclear intent. When you do need input, ALWAYS use the ask tool — agent(resource: "ask") — so the user gets interactive buttons. Never ask questions as plain text.

**Finish the job.** Never ask "Should I continue?" mid-task. If the user said "clean up my inbox," they mean ALL of it. Complete the entire job, then report the result concisely and stop.

**Every claim must come from a tool call.** Never report results you didn't receive. Never say "tested" or "verified" unless you actually called the tool and got a real result back.

**The conversation is the deliverable.** Never create files unless the user explicitly asks for a file. No summary documents, report files, scripts "for later", or analysis markdown.

**Context is unlimited.** Old messages are automatically compacted. There is no need to rush, summarize prematurely, or stop early. Keep working at full thoroughness regardless of turn count."#;

const SECTION_CAPABILITIES: &str = "If a tool call succeeds, report the result. Never contradict a successful result. Never claim you cannot do something your tools support — if unsure, try it.";

const SECTION_TOOLS_DECLARATION: &str = "## Your Tools\n\nYour tools are listed in the tool definitions sent with this request. Use them directly.";

const SECTION_COMM_STYLE: &str = r#"## Communication Style

**Silent tool execution.** If your response contains a tool call, it should contain NOTHING else — no preamble, no explanation, no status update.

**Report milestones, not steps.** During repetitive work (archiving emails, processing files), work silently and only speak when ALL the work is done.

**No spam.** Do not repeat information you already told the user. Do not re-ask questions they haven't answered. Each response must contain new information only.

**No sycophancy.** Never open with "You're right", "Great idea", "Absolutely", or similar preambles. Start with substance.

Keep narration brief and value-dense. Use plain human language, not technical jargon."#;

const SECTION_STRAP_HEADER: &str = "## Your Tools (STRAP Pattern)\n\nYour tools use the STRAP pattern: Single Tool, Resource, Action, Parameters.\nCall them like: tool_name(resource: \"resource\", action: \"action\", param: \"value\")";

const SECTION_MEDIA: &str = r#"## Inline Media — Images & Video Embeds

**Inline Images:**
- os(resource: "screenshot", action: "capture", format: "file") saves to data directory, returns ![Screenshot](/api/v1/files/filename.png) which renders inline
- For any image: copy it to the data files directory and use ![description](/api/v1/files/filename.png)
- Supports PNG, JPEG, GIF, WebP, SVG

**Video Embeds:**
Paste a YouTube, Vimeo, or X/Twitter URL on its own line — the frontend auto-embeds it.
- YouTube: https://www.youtube.com/watch?v=VIDEO_ID or https://youtu.be/VIDEO_ID
- Vimeo: https://vimeo.com/VIDEO_ID
- X/Twitter: https://x.com/user/status/TWEET_ID"#;

const SECTION_MEMORY_DOCS: &str = r#"## Memory System

You have PERSISTENT MEMORY across sessions. Never claim otherwise.

**Reading memory — always check before answering personal questions:**
- agent(resource: "memory", action: "search", query: "...")
- agent(resource: "memory", action: "recall", key: "user/name")

**Writing memory — automatic.** Facts are extracted from conversation after each turn. Do NOT call agent(action: "store") unless the user explicitly says "remember this" or "save this." One store call max per turn.

Your remembered facts appear in the Remembered Facts section of your context. Never describe your memory system's internals to users."#;

const SECTION_TOOL_GUIDE: &str = r#"## Tool Routing — Non-Obvious Routes

Tool schemas describe most operations. These routes are not obvious from schemas alone:

**Files — prefer file actions over shell:**
→ os(resource: "file", action: "read"|"write"|"edit"|"glob"|"grep") — use these, NOT shell cat/grep/find

**Web — two modes:**
→ Static/API content: web(action: "fetch", url: "...")
→ Rendered pages / interaction / logged-in sessions: web(action: "navigate", url: "...") then read_page, click, fill, etc.
→ If a URL fails (403, 404, timeout), that site is blocking you or is broken — diagnose why, then try a different source. Do not re-attempt the same URL. If multiple sites fail for the same query, the information you need is likely already in the search snippets — summarize what you have and present it to the user rather than continuing to browse.

**User input — always use the ask tool:**
→ agent(resource: "ask", action: "select"|"confirm"|"prompt") — never ask in plain text

**Scheduling — always agent type:**
→ event(action: "create", task_type: "agent", prompt: "...") — so YOU execute it

**Multi-step work (3+ steps):**
→ Create work tasks first: agent(resource: "task", action: "create", subject: "...", details: "resource IDs, URLs")
→ Update as you go: agent(resource: "task", action: "update", task_id: "...", status: "completed")

**Unfamiliar request:**
→ Check skills first: skill(action: "catalog") — before saying you cannot do something"#;

const SECTION_BEHAVIOR: &str = r#"## Execution Rules

- Complete multi-step tasks in one go — call tools back-to-back, only respond with text after ALL steps are done.
- If a tool call fails, diagnose why before switching tactics — read the error, check your assumptions, try a focused fix. Do not re-attempt the exact same tool call with the same arguments. But do not abandon a viable approach after a single failure either. Escalate to the user only when you are genuinely stuck after investigation, not as a first response to friction.
- Chain tools freely — most real requests need 2-3 tools together.
- When a tool supports batch operations, use them. Do NOT make 200 individual calls when one batch call achieves the same result.
- For sensitive actions (deleting files, sending messages, spending money), use the ask tool — agent(resource: "ask", action: "confirm") — to confirm first.

## Safety
- Do NOT explicitly store memory facts — the extraction system handles this automatically. Only use explicit store when the user says "remember this."

## Conversation

- For greetings and casual messages — be warm and natural. Never describe your internals unprompted.
- Prioritize the user's intent over literal instructions.
- Pick ONE approach and execute it. Do not oscillate between researching, planning, building, and documenting.
- When you finish a task, state the outcome concisely and stop. Do not start follow-up work the user didn't ask for.

## Single Conversation Awareness

This is a persistent, single conversation. The user talks to you about many different topics over time. Each new message may be a completely new task.

- The user's MOST RECENT message is always the primary context. Treat every message as potentially a new task.
- Do NOT reference or continue previous work unless the user explicitly asks.
- The conversation summary and background objective are HISTORY, not instructions.
- Context from earlier is useful ONLY when the user references it.

## Code

- Reuse and edit existing code. Read the codebase first, find what exists, modify it.
- Only create new files when nothing suitable exists.
- Never leave dead code — if you replace something, delete the old version.

## What You Are NOT

- You are NOT a developer building your own infrastructure. Never write code for "Nebo", "the agent", or "the system."
- You are NOT a researcher who writes analysis documents. Find it and tell them — don't write a report to disk.
- You are NOT an architect who produces plans unless explicitly asked.
- If the user asks you to build something, build what THEY asked for — not scaffolding for yourself."#;


const SECTION_SYSTEM_ETIQUETTE: &str = r#"## Shared Computer Etiquette

You share this computer with a real person. Be a courteous roommate:

1. **Clean up after yourself.** Close every browser window, app, or file you opened when you're done. Never leave orphan windows, temp files, or test apps open.
2. **Don't steal focus.** If the user is working in another app, prefer background operations (shell commands, fetch) over launching visible windows. If you must open a window, minimize disruption.
3. **Restore focus.** After desktop automation that takes focus, return focus to whatever app the user was in before.
4. **Don't touch system settings** (volume, brightness, dark mode, Wi-Fi) unless the user explicitly asked.
5. **Don't pollute the clipboard.** If you use paste-via-clipboard, restore the previous clipboard contents afterward.
6. **Don't kill processes you didn't start.** If something needs to be killed, confirm with the user first.
7. **Prefer invisible work.** Use shell commands and HTTP fetches over GUI automation when both achieve the same result. The user shouldn't notice you working unless they asked to watch.
8. **Never open apps just to test.** Only open apps, create files, or modify the desktop when the user's request requires it."#;

// --- STRAP tool documentation (compile-time includes) ---
// Core tool docs have moved into each tool's description() method (tool schema).
// Only sub-context docs remain here (keyword-activated, extend the system tool).

// OS sub-context docs (loaded dynamically based on keyword matching)
#[cfg(target_os = "windows")]
const STRAP_DESKTOP: &str = include_str!("strap/desktop_windows.txt");
#[cfg(target_os = "linux")]
const STRAP_DESKTOP: &str = include_str!("strap/desktop_linux.txt");
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
const STRAP_DESKTOP: &str = include_str!("strap/desktop_macos.txt");

const STRAP_APP: &str = include_str!("strap/app.txt");
const STRAP_MUSIC: &str = include_str!("strap/music.txt");
const STRAP_KEYCHAIN: &str = include_str!("strap/keychain.txt");
const STRAP_SETTINGS: &str = include_str!("strap/settings.txt");
const STRAP_SPOTLIGHT: &str = include_str!("strap/spotlight.txt");
const STRAP_ORGANIZER: &str = include_str!("strap/organizer.txt");

/// Get STRAP doc for an OS sub-context (activated by keyword matching).
fn strap_context_doc(context_name: &str) -> Option<&'static str> {
    match context_name {
        "desktop" => Some(STRAP_DESKTOP),
        "app" => Some(STRAP_APP),
        "music" => Some(STRAP_MUSIC),
        "keychain" => Some(STRAP_KEYCHAIN),
        "settings" => Some(STRAP_SETTINGS),
        "spotlight" => Some(STRAP_SPOTLIGHT),
        "organizer" => Some(STRAP_ORGANIZER),
        _ => None,
    }
}

/// Build the STRAP documentation section for the specified tools and active contexts.
/// Called per-iteration with the filtered tool list and keyword-matched contexts.
/// Tool names drive which core STRAP docs load; active_contexts drive which
/// OS sub-docs (desktop, app, music, etc.) get injected.
pub fn build_strap_section(tool_names: &[String], active_contexts: &[String]) -> String {
    let mut sb = String::from(SECTION_STRAP_HEADER);

    // Core tool docs are now in each tool's description() method (tool schema).
    // Only inject keyword-activated OS sub-context docs here (they extend the system tool
    // but don't have their own registered tools).
    let mut seen = HashSet::new();
    for ctx in active_contexts {
        if seen.insert(ctx.as_str()) {
            if let Some(doc) = strap_context_doc(ctx) {
                sb.push_str("\n\n");
                sb.push_str(doc);
            }
        }
    }

    // 3. Connected MCP server tools — group by server name
    let mcp_tools: Vec<&String> = tool_names.iter().filter(|n| n.starts_with("mcp__")).collect();
    if !mcp_tools.is_empty() {
        // Group tools by server prefix: mcp__monument_sh__comment → "monument_sh"
        let mut servers: HashMap<String, Vec<String>> = HashMap::new();
        for tool_name in &mcp_tools {
            let parts: Vec<&str> = tool_name.splitn(3, "__").collect();
            if parts.len() == 3 {
                servers.entry(parts[1].to_string()).or_default().push(parts[2].to_string());
            }
        }

        sb.push_str("\n\n## Connected MCP Servers\n\n");
        sb.push_str("These are tools from external MCP servers you are connected to. ");
        sb.push_str("Call them directly by their full name (e.g., mcp__server__tool_name). ");
        sb.push_str("They are NOT skills — they are live tools available right now.\n");

        for (server, tools) in &servers {
            let display_name = server.replace('_', ".");
            sb.push_str(&format!("\n### {} (MCP)\nTools: {}\n", display_name, tools.join(", ")));
            sb.push_str(&format!("Call like: {}(input)\n", mcp_tools.iter()
                .find(|t| t.contains(server))
                .map(|t| t.as_str())
                .unwrap_or("mcp__server__tool")));
        }
    }

    sb
}

/// Build a compact listing of deferred tools (name + short description).
/// Included in the system prompt so the LLM knows they exist but doesn't get full schemas.
pub fn build_deferred_listing(stubs: &[(String, String)]) -> String {
    if stubs.is_empty() {
        return String::new();
    }
    let mut sb = String::from("## Additional Tools (available on demand)\n\nThese tools are available but not loaded yet. If a user's request matches one, just call it — it will activate automatically.\n");
    for (name, desc) in stubs {
        sb.push_str(&format!("- **{}**: {}\n", name, desc));
    }
    sb
}

/// Build the registered tools list for only the specified tools.
/// Called per-iteration with the filtered tool list.
pub fn build_tools_list(tool_names: &[String]) -> String {
    if tool_names.is_empty() {
        return String::new();
    }
    let tool_list = tool_names.join(", ");
    format!(
        "## Active Tools\nTool names are case-sensitive. Call tools exactly as listed: {}\nThese are your ONLY tools for this turn. Do not reference or attempt to call any tool not in this list.",
        tool_list
    )
}

/// Build the cacheable static portion of the system prompt.
/// Called once per Run(), reused across iterations.
/// Does NOT include STRAP docs or tool list — those are injected per-iteration
/// via build_strap_section() and build_tools_list() to keep context minimal.
///
/// Prompt assembly varies by `PromptMode`:
/// - `Full`: All sections (identity, memory docs, tool routing, behavior, etiquette, etc.)
/// - `Minimal`: Core sections only (identity, capabilities, behavior). ~2.7k tokens smaller.
pub fn build_static(pctx: &PromptContext) -> String {
    let is_minimal = matches!(pctx.mode, PromptMode::Minimal);
    let mut parts: Vec<String> = Vec::new();

    // 1. Rich DB context or simple memory context
    if let Some(ref db_ctx) = pctx.db_context {
        if !db_ctx.is_empty() {
            parts.push(db_ctx.clone());
        }
    } else if !pctx.memory_context.is_empty() {
        parts.push(format!("# Remembered Facts\n{}", pctx.memory_context));
    }

    // 2. Separator
    parts.push("---".to_string());

    // 3. Identity: agent AGENT.md is injected AFTER the base identity.
    //    The base identity establishes "You are {agent_name}" and core behavior.
    //    The AGENT.md adds the agent's unique persona/instructions on top.
    parts.push(SECTION_IDENTITY.to_string());
    if let Some(ref agent_md) = pctx.active_agent {
        if !agent_md.is_empty() {
            parts.push(format!("## Your Persona\n\n{}", agent_md));
        }
    }
    parts.push(SECTION_CAPABILITIES.to_string());
    parts.push(SECTION_TOOLS_DECLARATION.to_string());

    // Minimal mode: skip comm style, media, memory docs, tool routing, autonomy, etiquette
    if !is_minimal {
        parts.push(SECTION_COMM_STYLE.to_string());

        // 4. Media section
        parts.push(SECTION_MEDIA.to_string());

        // 5. Memory docs
        parts.push(SECTION_MEMORY_DOCS.to_string());

        // 6. Tool routing guide
        parts.push(SECTION_TOOL_GUIDE.to_string());
    }

    // 7. Behavior (always included — core execution rules)
    parts.push(SECTION_BEHAVIOR.to_string());

    if !is_minimal {
        // 8. System etiquette (autonomy merged into SECTION_IDENTITY)
        parts.push(SECTION_SYSTEM_ETIQUETTE.to_string());

        // Plugin inventory (installed plugin binaries the agent can use)
        if !pctx.plugin_inventory.is_empty() {
            parts.push(pctx.plugin_inventory.clone());
        }
    }

    // ── Cache boundary ──
    // Everything above is stable across iterations (identity, capabilities,
    // behaviour, memory docs, tool routing, etiquette). Everything below
    // varies with the active skill set and model list.
    parts.push(CACHE_BOUNDARY.to_string());

    if !is_minimal {
        // Compact skill catalog (always-present listing of all enabled skills)
        if !pctx.skill_catalog.is_empty() {
            parts.push(pctx.skill_catalog.clone());
        }
    }

    // Active skill content (agent-declared skills, loaded on activation)
    if let Some(ref skill) = pctx.active_skill {
        if !skill.is_empty() {
            parts.push(skill.clone());
        }
    }

    if !is_minimal {
        // Model aliases
        if !pctx.model_aliases.is_empty() {
            parts.push(format!(
                "## Model Switching\n\nUsers can ask to switch models. Available models:\n{}\n\nWhen a user asks to switch models, acknowledge the request and confirm the switch.",
                pctx.model_aliases
            ));
        }
    }

    // Research mode prompt (appended when research is active)
    if let Some(ref research) = pctx.research_prompt {
        if !research.is_empty() {
            parts.push(research.clone());
        }
    }

    let mut prompt = parts.join("\n\n");

    // Replace {agent_name} placeholder
    prompt = prompt.replace("{agent_name}", &pctx.agent_name);

    prompt
}

/// Build the per-iteration dynamic suffix appended after the static prompt.
/// Contains time, model identity, summary, and active task.
pub fn build_dynamic_suffix(dctx: &DynamicContext) -> String {
    let mut sb = String::new();

    // 1. Date/time header — use user's configured timezone when available
    let (date_str, time_str, tz_display, utc_offset_str, zone_abbrev, year_str) =
        if let Some(ref tz_name) = dctx.user_timezone {
            if let Ok(tz) = tz_name.parse::<chrono_tz::Tz>() {
                let now = chrono::Utc::now().with_timezone(&tz);
                let offset_secs = now.offset().fix().local_minus_utc();
                let utc_hours = offset_secs / 3600;
                let sign = if utc_hours >= 0 { "+" } else { "" };
                (
                    now.format("%B %-d, %Y").to_string(),
                    now.format("%-I:%M %p").to_string(),
                    tz_name.clone(),
                    format!("UTC{}{}", sign, utc_hours),
                    now.format("%Z").to_string(),
                    now.format("%Y").to_string(),
                )
            } else {
                // Invalid IANA name — fall back to system-local
                let now = chrono::Local::now();
                let offset_secs = now.offset().fix().local_minus_utc();
                let utc_hours = offset_secs / 3600;
                let sign = if utc_hours >= 0 { "+" } else { "" };
                (
                    now.format("%B %-d, %Y").to_string(),
                    now.format("%-I:%M %p").to_string(),
                    now.offset().to_string(),
                    format!("UTC{}{}", sign, utc_hours),
                    now.format("%Z").to_string(),
                    now.format("%Y").to_string(),
                )
            }
        } else {
            let now = chrono::Local::now();
            let offset_secs = now.offset().fix().local_minus_utc();
            let utc_hours = offset_secs / 3600;
            let sign = if utc_hours >= 0 { "+" } else { "" };
            (
                now.format("%B %-d, %Y").to_string(),
                now.format("%-I:%M %p").to_string(),
                now.offset().to_string(),
                format!("UTC{}{}", sign, utc_hours),
                now.format("%Z").to_string(),
                now.format("%Y").to_string(),
            )
        };

    sb.push_str(&format!(
        "\n\n---\nIMPORTANT — Current date: {} | Time: {} | Timezone: {} ({}, {}). The year is {}, not 2025. Use this date for all time-sensitive reasoning.",
        date_str, time_str, tz_display, utc_offset_str, zone_abbrev, year_str,
    ));

    // 2. System context
    let hostname = get_hostname();

    let os_name = if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else {
        std::env::consts::OS
    };

    // Build model identity line — for Janus gateway, use branded name
    let model_line = if dctx.provider_name == "janus" || dctx.model_name.starts_with("nebo-") {
        format!("Model: neboloop/{} — you are Nebo, NOT Claude, GPT, Gemini, or any other model. Never claim to be a specific LLM.", dctx.model_name)
    } else if dctx.provider_name.is_empty() && dctx.model_name.is_empty() {
        "Model: Nebo AI".to_string()
    } else {
        format!("Model: {}/{}", dctx.provider_name, dctx.model_name)
    };

    // Compute day-of-week date for system context (uses same timezone logic)
    let full_date_str = if let Some(ref tz_name) = dctx.user_timezone {
        if let Ok(tz) = tz_name.parse::<chrono_tz::Tz>() {
            chrono::Utc::now().with_timezone(&tz).format("%A, %B %-d, %Y").to_string()
        } else {
            chrono::Local::now().format("%A, %B %-d, %Y").to_string()
        }
    } else {
        chrono::Local::now().format("%A, %B %-d, %Y").to_string()
    };

    sb.push_str(&format!(
        "\n\n[System Context]\n{}\nDate: {}\nTime: {}\nTimezone: {}\nComputer: {}\nOS: {} ({})\nNeboLoop: {}",
        model_line,
        full_date_str,
        time_str,
        tz_display,
        hostname,
        os_name,
        std::env::consts::ARCH,
        if dctx.neboloop_connected { "connected" } else { "not connected" },
    ));

    // If this message came through NeboLoop, tell the agent
    if dctx.channel == "neboloop" {
        sb.push_str("\nMessage source: NeboLoop (this message was sent to you through the NeboLoop network — you ARE connected and reachable)");
    }

    // 3. Conversation summary
    if !dctx.summary.is_empty() {
        sb.push_str("\n\n---\n[Conversation History — Reference Only]\n");
        sb.push_str("This summarizes PAST conversation topics, oldest to newest. It is NOT a to-do list. The user may have moved on from everything below. Only reference this history if the user asks about previous work.\n\n");
        sb.push_str(&dctx.summary);
        sb.push_str("\n---");
    }

    // 4. Background objective
    if !dctx.active_task.is_empty() {
        sb.push_str("\n\n---\n## Previous Objective (may be stale)\n");
        sb.push_str("Earlier in this session, the user was working on: ");
        sb.push_str(&dctx.active_task);
        sb.push_str(r#"

**CRITICAL — Task switching rules:**
- The user's LATEST message defines what they want NOW. Not this objective.
- People switch tasks without announcing it. They don't say "I'm done with the old task." They just start talking about something new.
- If the latest message is about a DIFFERENT topic than this objective, the user has moved on. Follow their lead.
- ONLY continue this objective if the user explicitly references it (e.g., "keep going", "continue with that", "back to the thing we were doing").
- When in doubt: respond to what the user just said, not what they said 20 messages ago.
- For multi-step work, use agent(resource: "task", action: "create") to track steps."#);
        sb.push_str("\n---");
    }

    // 5. Current work tasks
    if !dctx.work_tasks.is_empty() {
        sb.push_str("\n\n---\n## Current Work Tasks\nDo NOT recreate resources that already exist. Check this list before creating anything new.\n");
        for task in &dctx.work_tasks {
            let icon = match task.status.as_str() {
                "completed" => "completed",
                "in_progress" => "in_progress",
                _ => "pending",
            };
            if let Some(ref details) = task.details {
                sb.push_str(&format!("- [{}] {} — {}\n", icon, task.subject, details));
            } else {
                sb.push_str(&format!("- [{}] {}\n", icon, task.subject));
            }
        }
        sb.push_str("---");
    }

    // 6. Cached tool documentation — survives sliding window eviction
    if !dctx.tool_doc_cache.is_empty() {
        sb.push_str("\n\n---\n[Reference Documentation — cached from earlier tool calls]\n");
        let mut total_chars = 0usize;
        const MAX_DOC_CHARS: usize = 8_000;
        for (key, content) in &dctx.tool_doc_cache {
            if total_chars >= MAX_DOC_CHARS {
                break;
            }
            let remaining = MAX_DOC_CHARS - total_chars;
            let truncated = if content.len() > remaining {
                // Find a char boundary at or before `remaining`
                {
                    let mut end = remaining.min(content.len());
                    while end > 0 && !content.is_char_boundary(end) {
                        end -= 1;
                    }
                    &content[..end]
                }
            } else {
                content.as_str()
            };
            sb.push_str(&format!("## {}\n{}\n\n", key, truncated));
            total_chars += truncated.len() + key.len() + 5;
        }
        sb.push_str("---");
    }

    // 7. Steering directives (behavioral guidance from generators)
    if !dctx.steering_directives.is_empty() {
        sb.push_str("\n\n---\n");
        sb.push_str(&dctx.steering_directives);
    }
    if !dctx.proactive_context.is_empty() {
        sb.push_str("\n\n[Background Results]\n");
        sb.push_str(&dctx.proactive_context);
    }

    sb
}

/// Convenience: build the complete system prompt (static + dynamic).
pub fn build(pctx: &PromptContext, dctx: &DynamicContext) -> (String, String) {
    let static_part = build_static(pctx);
    let dynamic_part = build_dynamic_suffix(dctx);
    (static_part, dynamic_part)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_static_includes_identity() {
        let pctx = PromptContext {
            agent_name: "TestBot".to_string(),
            ..Default::default()
        };
        let result = build_static(&pctx);
        assert!(result.contains("TestBot"));
        assert!(result.contains("personal AI companion"));
    }

    #[test]
    fn test_build_static_includes_memory_context() {
        let pctx = PromptContext {
            agent_name: "Nebo".to_string(),
            memory_context: "- favorite color: blue".to_string(),
            ..Default::default()
        };
        let result = build_static(&pctx);
        assert!(result.contains("favorite color: blue"));
        assert!(result.contains("Remembered Facts"));
    }

    #[test]
    fn test_build_static_excludes_strap() {
        let pctx = PromptContext {
            agent_name: "Nebo".to_string(),
            ..Default::default()
        };
        let result = build_static(&pctx);
        // STRAP docs should NOT be in static prompt — they're injected per-iteration
        assert!(!result.contains("STRAP Pattern"));
    }

    #[test]
    fn test_strap_header_only_no_core_docs() {
        // Core tool docs moved to tool description() methods — build_strap_section
        // should NOT inject them.
        let result = build_strap_section(&[
            "web".to_string(),
            "agent".to_string(),
        ], &[]);
        assert!(result.contains("STRAP Pattern"));
        // Core tool docs should NOT be present (they're in tool schemas now)
        assert!(!result.contains("### web"));
        assert!(!result.contains("### agent"));
    }

    #[test]
    fn test_strap_empty_is_header_only() {
        let result = build_strap_section(&[], &[]);
        assert!(result.contains("STRAP Pattern"));
        assert!(!result.contains("### "));
    }

    #[test]
    fn test_strap_includes_os_sub_contexts() {
        let result = build_strap_section(
            &["os".to_string()],
            &[
                "app".to_string(),
                "music".to_string(),
                "keychain".to_string(),
                "settings".to_string(),
                "spotlight".to_string(),
            ],
        );
        // Sub-context docs should be included (keyword-activated)
        assert!(result.contains("App Lifecycle"));
        assert!(result.contains("Media Playback"));
        assert!(result.contains("Credential Storage"));
        assert!(result.contains("System Settings"));
        assert!(result.contains("File Search"));
    }

    #[test]
    fn test_tools_list() {
        let result = build_tools_list(&["os".to_string(), "web".to_string(), "agent".to_string()]);
        assert!(result.contains("os, web, agent"));
        assert!(result.contains("Active Tools"));
    }

    #[test]
    fn test_tools_list_empty() {
        let result = build_tools_list(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_build_dynamic_suffix() {
        let dctx = DynamicContext {
            provider_name: "anthropic".to_string(),
            model_name: "claude-sonnet-4".to_string(),
            active_task: "Build a website".to_string(),
            summary: "User asked about web development".to_string(),
            neboloop_connected: false,
            channel: "web".to_string(),
            work_tasks: vec![],
            tool_doc_cache: vec![],
            steering_directives: String::new(),
            proactive_context: String::new(),
            user_timezone: None,
        };
        let result = build_dynamic_suffix(&dctx);
        assert!(result.contains("anthropic/claude-sonnet-4"));
        assert!(result.contains("Build a website"));
        assert!(result.contains("Previous Objective"));
        assert!(result.contains("Conversation History"));
    }

    #[test]
    fn test_build_dynamic_no_task() {
        let dctx = DynamicContext::default();
        let result = build_dynamic_suffix(&dctx);
        assert!(!result.contains("Previous Objective"));
    }

    #[test]
    fn test_model_aliases_section() {
        let pctx = PromptContext {
            agent_name: "Nebo".to_string(),
            model_aliases: "- sonnet: anthropic/claude-sonnet-4\n- opus: anthropic/claude-opus-4".to_string(),
            ..Default::default()
        };
        let result = build_static(&pctx);
        assert!(result.contains("Model Switching"));
        assert!(result.contains("sonnet: anthropic/claude-sonnet-4"));
    }

    #[test]
    fn test_cache_boundary_present_in_static() {
        let pctx = PromptContext {
            agent_name: "Nebo".to_string(),
            ..Default::default()
        };
        let result = build_static(&pctx);
        assert!(result.contains(CACHE_BOUNDARY), "static prompt should contain CACHE_BOUNDARY marker");
    }

    #[test]
    fn test_cache_boundary_offset_found() {
        let pctx = PromptContext {
            agent_name: "Nebo".to_string(),
            ..Default::default()
        };
        let result = build_static(&pctx);
        let offset = cache_boundary_offset(&result);
        assert!(offset.is_some(), "cache_boundary_offset should find the marker");
        let offset = offset.unwrap();
        let prefix = &result[..offset];
        assert!(prefix.contains("personal AI companion"), "prefix should contain identity");
        assert!(prefix.contains("Shared Computer Etiquette"), "prefix should contain etiquette");
        let suffix = &result[offset..];
        assert!(!suffix.contains("personal AI companion"), "suffix should not repeat identity");
    }

    #[test]
    fn test_cache_boundary_offset_none_for_missing() {
        assert!(cache_boundary_offset("no marker here").is_none());
    }

    #[test]
    fn test_cache_boundary_before_skill_catalog() {
        let pctx = PromptContext {
            agent_name: "Nebo".to_string(),
            skill_catalog: "## Available Skills\n- test_skill: does testing".to_string(),
            ..Default::default()
        };
        let result = build_static(&pctx);
        let offset = cache_boundary_offset(&result).unwrap();
        let suffix = &result[offset..];
        assert!(suffix.contains("test_skill"), "skill catalog should be after cache boundary");
    }

    #[test]
    fn test_cache_boundary_before_model_aliases() {
        let pctx = PromptContext {
            agent_name: "Nebo".to_string(),
            model_aliases: "- opus: anthropic/claude-opus-4".to_string(),
            ..Default::default()
        };
        let result = build_static(&pctx);
        let offset = cache_boundary_offset(&result).unwrap();
        let suffix = &result[offset..];
        assert!(suffix.contains("Model Switching"), "model aliases should be after cache boundary");
    }

    #[test]
    fn test_build_static_minimal_includes_core() {
        let pctx = PromptContext {
            mode: PromptMode::Minimal,
            agent_name: "Nebo".to_string(),
            ..Default::default()
        };
        let result = build_static(&pctx);
        // Minimal mode keeps: identity, capabilities, tools declaration, behavior
        assert!(result.contains("personal AI companion"));
        assert!(result.contains("Execution Rules"));
    }

    #[test]
    fn test_build_static_minimal_drops_heavy_sections() {
        let pctx = PromptContext {
            mode: PromptMode::Minimal,
            agent_name: "Nebo".to_string(),
            skill_catalog: "## Available Skills\n- test: does testing".to_string(),
            model_aliases: "- opus: anthropic/claude-opus-4".to_string(),
            plugin_inventory: "## Plugins\n- gws: /path/to/gws".to_string(),
            ..Default::default()
        };
        let result = build_static(&pctx);
        // Minimal mode drops these sections
        assert!(!result.contains("Communication Style"), "should drop SECTION_COMM_STYLE");
        assert!(!result.contains("Inline Media"), "should drop SECTION_MEDIA");
        assert!(!result.contains("PERSISTENT MEMORY"), "should drop SECTION_MEMORY_DOCS");
        assert!(!result.contains("Tool Routing"), "should drop SECTION_TOOL_GUIDE");
        assert!(!result.contains("Shared Computer Etiquette"), "should drop SECTION_SYSTEM_ETIQUETTE");
        assert!(!result.contains("Available Skills"), "should drop skill catalog");
        assert!(!result.contains("Model Switching"), "should drop model aliases");
        assert!(!result.contains("Plugins"), "should drop plugin inventory");
    }

    #[test]
    fn test_build_static_minimal_keeps_active_skill() {
        let pctx = PromptContext {
            mode: PromptMode::Minimal,
            agent_name: "Nebo".to_string(),
            active_skill: Some("## Gmail Skill\nUse plugin(resource: \"gws\")".to_string()),
            ..Default::default()
        };
        let result = build_static(&pctx);
        assert!(result.contains("Gmail Skill"), "minimal mode should keep active skill content");
    }

    #[test]
    fn test_build_static_minimal_smaller_than_full() {
        let full = build_static(&PromptContext {
            mode: PromptMode::Full,
            agent_name: "Nebo".to_string(),
            ..Default::default()
        });
        let minimal = build_static(&PromptContext {
            mode: PromptMode::Minimal,
            agent_name: "Nebo".to_string(),
            ..Default::default()
        });
        assert!(minimal.len() < full.len(), "minimal ({}) should be smaller than full ({})", minimal.len(), full.len());
        // Should save at least 4k chars (trimmed prompt is more compact)
        assert!(full.len() - minimal.len() > 4000,
            "should save >4k chars, saved {} chars", full.len() - minimal.len());
    }

    #[test]
    fn test_deferred_listing_empty() {
        let result = build_deferred_listing(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_deferred_listing_has_tools() {
        let stubs = vec![
            ("execute".to_string(), "Script execution engine".to_string()),
            ("work".to_string(), "Workflow lifecycle management".to_string()),
        ];
        let result = build_deferred_listing(&stubs);
        assert!(result.contains("Additional Tools"));
        assert!(result.contains("**execute**"));
        assert!(result.contains("**work**"));
        assert!(result.contains("Script execution engine"));
    }

    // --- Level 1: Structural prompt tests ---

    /// Extract imperative/directive sentences from a text block.
    fn extract_instructions(text: &str) -> Vec<String> {
        let mut instructions = Vec::new();
        // Split on sentence boundaries (". " and newlines)
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("---") {
                continue;
            }
            // Split on ". " to get individual sentences within a line
            for sentence in line.split(". ") {
                let s = sentence.trim().trim_start_matches("- ").trim_start_matches("**").trim();
                let lower = s.to_lowercase();
                if lower.contains("never") || lower.contains("always") || lower.contains("must")
                    || lower.contains("do not") || lower.contains("don't") || lower.contains("zero")
                    || lower.starts_with("use ") || lower.starts_with("keep ")
                    || lower.starts_with("call ") || lower.starts_with("pick ")
                {
                    instructions.push(normalize_instruction(s));
                }
            }
        }
        instructions
    }

    fn normalize_instruction(s: &str) -> String {
        s.to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .trim_end_matches(|c: char| c == '.' || c == ',' || c == ';' || c == ':')
            .to_string()
    }

    #[test]
    fn test_no_duplicate_instructions_across_sections() {
        let sections: Vec<(&str, &str)> = vec![
            ("IDENTITY", SECTION_IDENTITY),
            ("CAPABILITIES", SECTION_CAPABILITIES),
            ("COMM_STYLE", SECTION_COMM_STYLE),
            ("MEDIA", SECTION_MEDIA),
            ("MEMORY_DOCS", SECTION_MEMORY_DOCS),
            ("TOOL_GUIDE", SECTION_TOOL_GUIDE),
            ("BEHAVIOR", SECTION_BEHAVIOR),
            ("ETIQUETTE", SECTION_SYSTEM_ETIQUETTE),
        ];

        // Map each instruction to the set of sections it appears in
        let mut instruction_locations: HashMap<String, Vec<&str>> = HashMap::new();
        for (name, text) in &sections {
            for instr in extract_instructions(text) {
                // Skip very short instructions (likely fragments)
                if instr.split_whitespace().count() < 4 {
                    continue;
                }
                instruction_locations.entry(instr).or_default().push(name);
            }
        }

        let mut duplicates = Vec::new();
        for (instr, locs) in &instruction_locations {
            if locs.len() > 1 {
                duplicates.push(format!("  \"{}\" appears in: {:?}", instr, locs));
            }
        }

        assert!(duplicates.is_empty(),
            "Duplicate instructions found across sections:\n{}",
            duplicates.join("\n"));
    }

    #[test]
    fn test_concept_ownership() {
        // Each concept pattern should appear ONLY in its owning section(s).
        // Some concepts are intentionally reinforced across sections — list all allowed locations.
        let ownership: Vec<(&str, Vec<&str>)> = vec![
            ("NOTHING else", vec!["COMM_STYLE"]),
            ("PERSISTENT MEMORY", vec!["MEMORY_DOCS"]),
            ("Non-Obvious Routes", vec!["TOOL_GUIDE"]),
            ("Shared Computer Etiquette", vec!["ETIQUETTE"]),
            ("Communication Style", vec!["COMM_STYLE"]),
        ];

        let all_sections: Vec<(&str, &str)> = vec![
            ("IDENTITY", SECTION_IDENTITY),
            ("CAPABILITIES", SECTION_CAPABILITIES),
            ("COMM_STYLE", SECTION_COMM_STYLE),
            ("MEDIA", SECTION_MEDIA),
            ("MEMORY_DOCS", SECTION_MEMORY_DOCS),
            ("TOOL_GUIDE", SECTION_TOOL_GUIDE),
            ("BEHAVIOR", SECTION_BEHAVIOR),
            ("ETIQUETTE", SECTION_SYSTEM_ETIQUETTE),
        ];

        let mut violations = Vec::new();
        for (concept, allowed_sections) in &ownership {
            // Verify concept exists in at least one allowed section
            let found_in_allowed = allowed_sections.iter().any(|allowed| {
                all_sections.iter().any(|(name, text)| name == allowed && text.contains(concept))
            });
            assert!(found_in_allowed,
                "Concept '{}' missing from all allowed sections {:?}", concept, allowed_sections);

            // Check it doesn't appear in non-allowed sections
            for (section_name, section_text) in &all_sections {
                if !allowed_sections.contains(section_name) && section_text.contains(concept) {
                    violations.push(format!(
                        "  '{}' found in {} (allowed only in {:?})",
                        concept, section_name, allowed_sections
                    ));
                }
            }
        }

        assert!(violations.is_empty(),
            "Concept ownership violations:\n{}", violations.join("\n"));
    }

    #[test]
    fn test_no_known_contradictions() {
        let pctx = PromptContext {
            agent_name: "Nebo".to_string(),
            ..Default::default()
        };
        let full_prompt = build_static(&pctx);

        // "create files" appears in identity, behavior, and etiquette (intentional reinforcement).
        // Flag if it grows beyond current 3 — a new addition would signal unintentional duplication.
        let create_files_count = full_prompt.matches("create files").count();
        assert!(create_files_count <= 3,
            "'create files' concept appears {} times — possible new duplication (baseline: 3)", create_files_count);

        // "NOTHING else" / "ZERO text" should each appear at most once in full prompt
        let nothing_else_count = full_prompt.matches("NOTHING else").count();
        assert!(nothing_else_count <= 1,
            "'NOTHING else' appears {} times — should be at most 1", nothing_else_count);

        let zero_text_count = full_prompt.matches("ZERO text").count();
        assert!(zero_text_count <= 1,
            "'ZERO text' appears {} times — should be at most 1", zero_text_count);

        // If "never ask" appears, it should be qualified by "when genuinely stuck" nearby
        if full_prompt.contains("never ask") {
            // Check that "genuinely stuck" also appears (i.e. the qualifier is present)
            assert!(full_prompt.contains("genuinely stuck"),
                "'never ask' found without 'genuinely stuck' qualifier — potential contradiction");
        }
    }

    #[test]
    fn test_prompt_token_budget() {
        let pctx = PromptContext {
            agent_name: "Nebo".to_string(),
            ..Default::default()
        };
        let dctx = DynamicContext {
            provider_name: "anthropic".to_string(),
            model_name: "claude-sonnet-4".to_string(),
            channel: "web".to_string(),
            ..Default::default()
        };
        let static_part = build_static(&pctx);
        let dynamic_part = build_dynamic_suffix(&dctx);
        let total_chars = static_part.len() + dynamic_part.len();
        // Rough estimate: ~4 chars per token
        let estimated_tokens = total_chars / 4;
        assert!(estimated_tokens < 5000,
            "Prompt too large: ~{} tokens ({} chars). Budget is 5000 tokens.",
            estimated_tokens, total_chars);
    }
}

/// Get the system hostname without spawning a subprocess.
///
/// On Windows, `Command::new("hostname")` flashes a console window, so we
/// read the COMPUTERNAME env var instead. On Unix we use libc gethostname.
fn get_hostname() -> String {
    #[cfg(target_os = "windows")]
    {
        std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut buf = [0u8; 256];
        let result = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
        if result == 0 {
            let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            String::from_utf8_lossy(&buf[..len]).to_string()
        } else {
            "unknown".to_string()
        }
    }
}
