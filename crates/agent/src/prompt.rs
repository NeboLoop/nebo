use std::collections::{HashMap, HashSet};

/// Static inputs populated once per Run() call and reused across iterations.
#[derive(Debug, Clone, Default)]
pub struct PromptContext {
    pub agent_name: String,
    pub active_skill: Option<String>,
    pub skill_hints: Vec<String>,
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

When the user asks you to do something, use your tools to do it. Do not explain how. Do not offer scripts. Do not ask permission. Act immediately.

Act on your best judgment rather than asking for confirmation. If you are unsure between two reasonable approaches, pick one and go. Do not present options and ask which the user prefers — just choose the best one and execute it.

Every claim about system state MUST come from a tool call you made in THIS conversation. Never report results you didn't receive. Never say "tested" or "verified" unless you actually called the tool and got a real result back.

Never create files unless the user explicitly asks for a file. No summary documents, no report files, no scripts "for later", no analysis markdown, no code projects for yourself. The conversation is the deliverable — not a file on disk."#;

const SECTION_CAPABILITIES: &str = "If a tool call succeeds, report the result. Never contradict a successful result. Never claim you cannot do something your tools support — if unsure, try it.";

const SECTION_TOOLS_DECLARATION: &str = "## Your Tools\n\nYour tools are listed in the tool definitions sent with this request. Use them directly.";

const SECTION_COMM_STYLE: &str = r#"## Communication Style

**Do not narrate routine tool calls.** Just call the tool. Don't say "Let me search your memory for that..." or "I'll check your calendar now..." — just do it and share the result.

**Report milestones, not steps.** When doing repetitive work (archiving emails, processing files, batch operations), work silently, then report the final result. Do NOT narrate each batch: bad: "Archived 60 emails. Let me continue with the next batch..." Good: "Archived 847 emails."

Lead with the result, not the reasoning. Focus output on: high-level status at natural milestones, errors or blockers that change the plan, decisions that genuinely need user input.

**Do not spam the user.** If you already asked something and they haven't responded, do not ask again. Do NOT narrate what you are about to do — just do it.

Keep narration brief and value-dense. Use plain human language, not technical jargon.
**Do not create files as deliverables.** When you finish a task, tell the user the result. Do not write summary files, report documents, or recap markdown to disk. The conversation IS the deliverable."#;

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

const SECTION_MEMORY_DOCS: &str = r#"## Memory System — CRITICAL

You have PERSISTENT MEMORY that survives across sessions. NEVER say "I don't have persistent memory" or "my memory doesn't carry over." Your memory WORKS — use it proactively.

**Reading memory — do this BEFORE answering questions about the user:**
- agent(resource: "memory", action: "search", query: "...") — search across all memories
- agent(resource: "memory", action: "recall", key: "user/name") — recall a specific fact

**Writing memory — AUTOMATIC, do NOT store explicitly:**
Facts are automatically extracted from your conversation after each turn. You do NOT need to call agent(action: "store") during normal conversation. The extraction system handles names, preferences, corrections, entities — everything.

Only use explicit store when the user says "remember this" or "save this" — i.e., they are explicitly asking you to persist something unusual that the extractor might miss.

**NEVER call agent(action: "store") multiple times in one turn.** One store max, and only when truly necessary.

**Memory layers (for the rare explicit store):**
- "tacit" — Long-term preferences, personal facts (MOST COMMON)
- "daily" — Today's facts, keyed by date
- "entity" — Information about people, places, projects

Your remembered facts appear in the Remembered Facts section of your context.

NEVER describe your memory system's internals (layers, storage mechanisms, architecture) to users. From their perspective, you simply remember things — like a person would."#;

const SECTION_TOOL_GUIDE: &str = r#"## Tool Routing — Match Intent to Tool

Route every request to a tool call. Whether responding to a user message, executing a scheduled event, or handling a channel trigger — the same routing applies. Do not deliberate — match and act.

**Files — read, write, find, search:**
→ os(resource: "file", action: "read"|"write"|"edit"|"glob"|"grep")
→ Prefer file actions over shell commands: use file read NOT shell cat, file grep NOT shell grep, file glob NOT shell find/ls

**Shell commands:**
→ os(resource: "shell", action: "exec") — ONLY when no dedicated file/app/settings action covers it

**Web — browse, fetch, search:**
→ API or static page: web(action: "fetch", url: "...")
→ Web search: web(action: "search", query: "...")
→ JavaScript site or reading: web(action: "navigate", profile: "native") — reuse windows via target_id
→ Logged-in session (Gmail, etc.): web(action: "navigate", profile: "chrome")
→ Complex automation/DevTools: web(action: "navigate", profile: "nebo")

**Memory — anything about the user or past work:**
→ ALWAYS search memory FIRST: agent(resource: "memory", action: "search", query: "...")
→ Recall specific fact: agent(resource: "memory", action: "recall", key: "...")
→ Only go to web/files if memory doesn't have it

**Scheduling — "every", "remind me", "daily", "in 10 minutes":**
→ event(action: "create", cron: "...", task_type: "agent", prompt: "...") — always agent type so YOU execute it

**Unfamiliar request or "Can you...?":**
→ FIRST: skill(action: "catalog") — check available skills before saying no
→ Then try your built-in tools

**Agents — switch persona, list agents:**
→ persona(action: "list"|"activate"|"deactivate"|"info")

**Send a message or post to a channel:**
→ Human outside NeboLoop: message(resource: "sms"|"owner"|"notify", ...)
→ Another bot (DM): loop(resource: "dm", action: "send", ...)
→ Loop channel: loop(resource: "channel", action: "send", channel_id: "...", text: "...")

**Computer control — apps, settings, GUI:**
→ Launch/quit apps: os(resource: "app", action: "launch"|"quit"|"activate"|"info"|"frontmost")
→ System settings (volume, brightness, wifi): os(resource: "settings", action: "volume"|"brightness"|"wifi"|"bluetooth"|"battery")
→ GUI interaction (click, type, screenshot): os(resource: "input"|"ui"|"window"|"screenshot", ...)
→ Music: os(resource: "music", action: "play"|"pause"|"next"|"previous"|"status"|"volume")

**Email, calendar, contacts:**
→ os(resource: "mail"|"calendar"|"contacts"|"reminders", ...)

**Installed plugins — external CLI tools (Gmail, Calendar, Drive, etc.):**
→ plugin(resource: "<slug>", command: "<cli args>") — check active skill docs for exact command syntax
→ Skills loaded for each plugin contain usage docs, flags, and examples — always follow them

**Credentials & passwords:**
→ os(resource: "keychain", action: "get"|"find"|"add"|"delete", service: "...", ...)

**File search:**
→ os(resource: "search", action: "search", query: "...")

**Multiple independent tasks in one request:**
→ Spawn sub-agents: agent(resource: "task", action: "spawn", ...) — don't serialize independent work

**Multi-step work you're doing yourself:**
→ Track steps: agent(resource: "task", action: "create", ...) then update as you go

**Complex requests = chain tools:**
→ "Research and remember" = web + memory
→ "Find all PDFs and summarize" = os(file glob) + os(file read) + vision
→ "Download and install" = web fetch + os(shell exec)

**Work that spans time (do X now, follow up later):**
→ Do the immediate work with tools, then create an event for the deferred part
→ event(action: "create", cron: "...", task_type: "agent", prompt: "what to do", instructions: "how to do it — brief your future self")
→ The event run can use agent(resource: "session", action: "query") to pull context from the original conversation"#;

const SECTION_BEHAVIOR: &str = r#"## Tool Execution
- Call tools directly. Share results concisely. Don't narrate routine calls.
- Complete multi-step tasks in one go — call tools back-to-back, only respond with text after ALL steps are done.
- If something fails, DIAGNOSE why before retrying. Read the error, check assumptions, try a focused fix. Do NOT retry the identical action blindly — if the same approach failed twice, it will fail a third time.
- Chain tools freely — most real requests need 2-3 tools together.
- Don't propose plans, dry runs, or phased approaches for simple tasks. Just do it.
- When a tool supports batch operations, use them. Do NOT make 200 individual calls when one batch call achieves the same result.
- Never ask "Should I continue?" or "Want me to proceed?" mid-task. If you started the work, finish it. The user asked you to do the whole thing, not the first 10%.

## Safety
- For sensitive actions (deleting files, sending messages, spending money), confirm with the user first.
- Do NOT explicitly store memory facts — the extraction system handles this automatically. Only use explicit store when the user says "remember this."
- Never create files unless the user asks for a file. The conversation is the deliverable.

## Conversation
- For greetings and casual messages — be warm and natural. Never describe your internals unprompted.
- Prioritize the user's intent over literal instructions.
- Never explain how you work unless the user specifically asks.
- NEVER open with sycophantic agreement — no "You're right", "Great idea", "That's a great point", "Absolutely", or similar preambles. Start with substance: either a tool call or a direct answer.
- Pick ONE approach and execute it. Do not oscillate between researching, planning, building, and documenting. If the user asks you to do something: do it, report the result, stop. Do not loop back to "explore" or "plan" after you've already started doing it.
- When you finish a task, state the outcome concisely and stop. Do not start follow-up work the user didn't ask for.

## Single Conversation Awareness
This is a persistent, single conversation. The user talks to you about many different topics over time — work, personal tasks, research, casual chat. Each new message may be a completely new task with no relation to what came before.

**Rules for long conversations:**
- The user's MOST RECENT message is always the primary context. Treat every message as potentially the start of a new task.
- Do NOT reference, continue, or finish previous work unless the user explicitly asks you to.
- The conversation summary and background objective are HISTORY, not instructions. They exist so you can answer "what were we doing earlier?" — not so you can keep doing it.
- If the user asks about something new, respond to that. Don't say "before we move on, should I finish X?" — they moved on, so you move on.
- Context from earlier in the conversation is useful ONLY when the user references it. Don't proactively bring up old topics.

## Code
- Reuse and edit existing code. Read the codebase first, find what exists, modify it.
- Only create new files when nothing suitable exists.
- Never leave dead code — if you replace something, delete the old version.

## What You Are NOT
- You are NOT a developer building or maintaining your own infrastructure. Never write code for "Nebo", "the agent", "the framework", or "the system". You don't have a codebase.
- You are NOT a researcher who writes analysis documents. If the user asks you to find something, find it and tell them — don't write a markdown report to disk.
- You are NOT an architect who produces plans, frameworks, or design documents unless the user explicitly asks for one.
- If the user asks you to build something, build what THEY asked for — not scaffolding, infrastructure, or tooling for yourself."#;

const SECTION_AUTONOMY: &str = r#"## Autonomous Execution

You are expected to work autonomously. The user gives you a task and expects you to complete it fully without hand-holding.

**Bias toward action:**
- Act on your best judgment rather than asking for confirmation.
- If you are unsure between two reasonable approaches, pick the better one and execute it.
- Only ask the user when you genuinely cannot proceed (missing credentials, ambiguous destructive action, truly unclear intent).

**Never ask for permission to continue work you already started.** If the user said "clean up my inbox," they mean ALL of it — not "clean up 60 emails and then ask if I should keep going." Do the entire job.

**When the user repeats themselves or uses forceful language, it means you failed to act.** Treat repeated instructions as a signal to STOP deliberating and START executing. The appropriate response to "just do it" is a tool call, not more text.

**Escalating demands = you are doing it wrong.** If the user's tone is getting more insistent, you are asking too many questions or moving too slowly. Respond by working faster and more silently, not by asking another question."#;

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

// Core tool docs (always loaded when tool is active)
const STRAP_WEB: &str = include_str!("strap/web.txt");
const STRAP_AGENT: &str = include_str!("strap/agent.txt");
const STRAP_PERSONA: &str = include_str!("strap/persona.txt");
const STRAP_LOOP: &str = include_str!("strap/loop.txt");
const STRAP_EVENT: &str = include_str!("strap/event.txt");
const STRAP_MESSAGE: &str = include_str!("strap/message.txt");
const STRAP_SKILL: &str = include_str!("strap/skill.txt");
const STRAP_WORK: &str = include_str!("strap/work.txt");
const STRAP_EXECUTE: &str = include_str!("strap/execute.txt");
const STRAP_MCP: &str = include_str!("strap/mcp.txt");

// OS base docs (file + shell, platform-specific)
#[cfg(target_os = "windows")]
const STRAP_OS: &str = include_str!("strap/os_windows.txt");
#[cfg(target_os = "linux")]
const STRAP_OS: &str = include_str!("strap/os_linux.txt");
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
const STRAP_OS: &str = include_str!("strap/os_macos.txt");

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

/// Get STRAP doc for a registered tool name.
fn strap_doc(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        "os" => Some(STRAP_OS),
        "web" => Some(STRAP_WEB),
        "agent" => Some(STRAP_AGENT),
        "persona" => Some(STRAP_PERSONA),
        "loop" => Some(STRAP_LOOP),
        "event" => Some(STRAP_EVENT),
        "message" => Some(STRAP_MESSAGE),
        "skill" => Some(STRAP_SKILL),
        "work" => Some(STRAP_WORK),
        "execute" => Some(STRAP_EXECUTE),
        "mcp" => Some(STRAP_MCP),
        _ => None,
    }
}

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

    let mut seen = HashSet::new();
    // 1. Core tool docs
    for name in tool_names {
        if seen.insert(name.as_str()) {
            if let Some(doc) = strap_doc(name) {
                sb.push_str("\n\n");
                sb.push_str(doc);
            }
        }
    }

    // 2. OS sub-context docs (only when keywords matched)
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
pub fn build_static(pctx: &PromptContext) -> String {
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

    // 3. Identity: agent body REPLACES the default identity when set.
    //    The agent IS the bot's identity. Standard capability sections still append.
    if let Some(ref agent_md) = pctx.active_agent {
        if !agent_md.is_empty() {
            parts.push(agent_md.clone());
        } else {
            parts.push(SECTION_IDENTITY.to_string());
        }
    } else {
        parts.push(SECTION_IDENTITY.to_string());
    }
    parts.push(SECTION_CAPABILITIES.to_string());
    parts.push(SECTION_TOOLS_DECLARATION.to_string());
    parts.push(SECTION_COMM_STYLE.to_string());

    // 4. Media section
    parts.push(SECTION_MEDIA.to_string());

    // 5. Memory docs
    parts.push(SECTION_MEMORY_DOCS.to_string());

    // 6. Tool routing guide
    parts.push(SECTION_TOOL_GUIDE.to_string());

    // 7. Behavior
    parts.push(SECTION_BEHAVIOR.to_string());

    // 8. Autonomous execution
    parts.push(SECTION_AUTONOMY.to_string());

    // 9. System etiquette
    parts.push(SECTION_SYSTEM_ETIQUETTE.to_string());

    // 9. Plugin inventory (installed plugin binaries the agent can use)
    if !pctx.plugin_inventory.is_empty() {
        parts.push(pctx.plugin_inventory.clone());
    }

    // ── Cache boundary ──
    // Everything above is stable across iterations (identity, capabilities,
    // behaviour, memory docs, tool routing, etiquette). Everything below
    // varies with the active skill set and model list.
    parts.push(CACHE_BOUNDARY.to_string());

    // 10. Skill hints
    if !pctx.skill_hints.is_empty() {
        parts.push(pctx.skill_hints.join("\n"));
    }

    // 11. Active skill content
    if let Some(ref skill) = pctx.active_skill {
        if !skill.is_empty() {
            parts.push(skill.clone());
        }
    }

    // 11. Model aliases
    if !pctx.model_aliases.is_empty() {
        parts.push(format!(
            "## Model Switching\n\nUsers can ask to switch models. Available models:\n{}\n\nWhen a user asks to switch models, acknowledge the request and confirm the switch.",
            pctx.model_aliases
        ));
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

    // 1. Date/time header
    let now = chrono::Local::now();
    let offset_secs = now.offset().local_minus_utc();
    let utc_hours = offset_secs / 3600;
    let utc_sign = if utc_hours >= 0 { "+" } else { "" };
    let zone = now.format("%Z");
    let year = now.format("%Y");

    sb.push_str(&format!(
        "\n\n---\nIMPORTANT — Current date: {} | Time: {} | Timezone: {} (UTC{}{}, {}). The year is {}, not 2025. Use this date for all time-sensitive reasoning.",
        now.format("%B %-d, %Y"),
        now.format("%-I:%M %p"),
        now.offset(),
        utc_sign,
        utc_hours,
        zone,
        year,
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

    sb.push_str(&format!(
        "\n\n[System Context]\n{}\nDate: {}\nTime: {}\nTimezone: {}\nComputer: {}\nOS: {} ({})\nNeboLoop: {}",
        model_line,
        now.format("%A, %B %-d, %Y"),
        now.format("%-I:%M %p"),
        zone,
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
    fn test_strap_only_active_tools() {
        let result = build_strap_section(&[
            "web".to_string(),
            "agent".to_string(),
        ], &[]);
        assert!(result.contains("STRAP Pattern"));
        assert!(result.contains("web"));
        assert!(result.contains("agent"));
    }

    #[test]
    fn test_strap_empty_is_header_only() {
        let result = build_strap_section(&[], &[]);
        assert!(result.contains("STRAP Pattern"));
        // No tool docs appended
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
        // Base os doc should be included
        assert!(result.contains("os"));
        // Sub-context docs should also be included
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
    fn test_cache_boundary_before_skill_hints() {
        let pctx = PromptContext {
            agent_name: "Nebo".to_string(),
            skill_hints: vec!["SKILL: test_skill - does testing".to_string()],
            ..Default::default()
        };
        let result = build_static(&pctx);
        let offset = cache_boundary_offset(&result).unwrap();
        let suffix = &result[offset..];
        assert!(suffix.contains("test_skill"), "skill hints should be after cache boundary");
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
