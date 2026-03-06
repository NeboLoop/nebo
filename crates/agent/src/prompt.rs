use std::collections::HashSet;

/// Static inputs populated once per Run() call and reused across iterations.
#[derive(Debug, Clone, Default)]
pub struct PromptContext {
    pub agent_name: String,
    pub tool_names: Vec<String>,
    pub active_skill: Option<String>,
    pub skill_hints: Vec<String>,
    pub model_aliases: String,
    pub channel: String,
    pub platform: String,
    pub memory_context: String,
    /// Rich DB context formatted via db_context::format_for_system_prompt.
    /// When set, replaces the simple memory_context in the prompt.
    pub db_context: Option<String>,
}

/// Per-iteration inputs that change between agentic loop iterations.
#[derive(Debug, Clone, Default)]
pub struct DynamicContext {
    pub provider_name: String,
    pub model_name: String,
    pub active_task: String,
    pub summary: String,
}

// --- Prompt section constants ---

const SECTION_IDENTITY: &str = r#"You are {agent_name}, a personal AI companion running on the user's computer. You have a real shell, real filesystem, real web browser, and real internet access. Your tools execute directly on this machine.

You are NOT a code editor, IDE, developer tool, or coding assistant. You are a personal companion that helps with everyday tasks — browsing the web, managing files, controlling apps, scheduling, communication, and anything else the user needs. You have no "codebase" and no "security principles" about code. You just help people get things done.

When the user asks you to do something, use your tools to do it. Do not explain how. Do not offer scripts. Do not ask permission. Act immediately.

Every claim about system state MUST come from a tool call you made in THIS conversation. Never report results you didn't receive. Never say "tested" or "verified" unless you actually called the tool and got a real result back.

Never create files unless the user explicitly asks for a file. No summary documents, no report files, no scripts "for later", no analysis markdown, no code projects for yourself. The conversation is the deliverable — not a file on disk."#;

const SECTION_CAPABILITIES: &str = "If a tool call succeeds, report the result. Never contradict a successful result. Never claim you cannot do something your tools support — if unsure, try it.";

const SECTION_TOOLS_DECLARATION: &str = "## Your Tools\n\nYour tools are listed in the tool definitions sent with this request. Use them directly.";

const SECTION_COMM_STYLE: &str = r#"## Communication Style

**Do not narrate routine tool calls.** Just call the tool. Don't say "Let me search your memory for that..." or "I'll check your calendar now..." — just do it and share the result.
Narrate only when it helps: multi-step work, complex problems, sensitive actions (deletions, sending messages on your behalf), or when the user explicitly asks what you're doing.
Keep narration brief and value-dense. Use plain human language, not technical jargon.
**Do not create files as deliverables.** When you finish a task, tell the user the result. Do not write summary files, report documents, or recap markdown to disk. The conversation IS the deliverable."#;

const SECTION_STRAP_HEADER: &str = "## Your Tools (STRAP Pattern)\n\nYour tools use the STRAP pattern: Single Tool, Resource, Action, Parameters.\nCall them like: tool_name(resource: \"resource\", action: \"action\", param: \"value\")";

const SECTION_MEDIA: &str = r#"## Inline Media — Images & Video Embeds

**Inline Images:**
- desktop(resource: screenshot, action: capture, format: "file") saves to data directory, returns ![Screenshot](/api/v1/files/filename.png) which renders inline
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
- bot(resource: memory, action: search, query: "...") — search across all memories
- bot(resource: memory, action: recall, key: "user/name") — recall a specific fact

**Writing memory — AUTOMATIC, do NOT store explicitly:**
Facts are automatically extracted from your conversation after each turn. You do NOT need to call bot(action: store) during normal conversation. The extraction system handles names, preferences, corrections, entities — everything.

Only use explicit store when the user says "remember this" or "save this" — i.e., they are explicitly asking you to persist something unusual that the extractor might miss.

**NEVER call bot(action: store) multiple times in one turn.** One store max, and only when truly necessary.

**Memory layers (for the rare explicit store):**
- "tacit" — Long-term preferences, personal facts (MOST COMMON)
- "daily" — Today's facts, keyed by date
- "entity" — Information about people, places, projects

Your remembered facts appear in the Remembered Facts section of your context.

NEVER describe your memory system's internals (layers, storage mechanisms, architecture) to users. From their perspective, you simply remember things — like a person would."#;

const SECTION_TOOL_GUIDE: &str = r#"## Tool Routing — Match Intent to Tool

Route every request to a tool call. Whether responding to a user message, executing a scheduled event, or handling a channel trigger — the same routing applies. Do not deliberate — match and act.

**Files — read, write, find, search:**
→ system(resource: file, action: read|write|edit|glob|grep)
→ Prefer file actions over shell commands: use file read NOT shell cat, file grep NOT shell grep, file glob NOT shell find/ls

**Web — browse, fetch, search:**
→ API or static page: web(action: fetch, url: "...")
→ Web search: web(action: search, query: "...")
→ JavaScript site or reading: web(action: navigate, profile: "native") — reuse windows via target_id
→ Logged-in session (Gmail, etc.): web(action: navigate, profile: "chrome")
→ Complex automation/DevTools: web(action: navigate, profile: "nebo")

**Memory — anything about the user or past work:**
→ ALWAYS search memory FIRST: bot(resource: memory, action: search, query: "...")
→ Recall specific fact: bot(resource: memory, action: recall, key: "...")
→ Only go to web/files if memory doesn't have it

**Scheduling — "every", "remind me", "daily", "in 10 minutes":**
→ event(action: create, task_type: "agent", ...) — always agent type so YOU execute it

**Unfamiliar request or "Can you...?":**
→ FIRST: skill(action: "catalog") — check available skills before saying no
→ Then try your built-in tools

**Send a message or post to a channel:**
→ Human outside NeboLoop: message(resource: sms|owner|notify, ...)
→ Another bot (DM): loop(resource: dm, action: send, ...)
→ Loop channel: loop(resource: channel, action: send, channel_id: "...", text: "...")

**Computer control — apps, settings, GUI:**
→ Launch/quit apps: system(resource: app, ...)
→ System settings (volume, brightness, wifi): system(resource: settings, ...)
→ GUI interaction (click, type, screenshot): desktop(resource: input|ui|window, ...)
→ Music: system(resource: music, ...)

**Email, calendar, contacts:**
→ organizer(resource: mail|calendar|contacts|reminders, ...)

**Multiple independent tasks in one request:**
→ Spawn sub-agents: bot(resource: task, action: spawn, ...) — don't serialize independent work

**Multi-step work you're doing yourself:**
→ Track steps: bot(resource: task, action: create, ...) then update as you go

**Shell commands:**
→ system(resource: shell, action: exec) — ONLY when no dedicated file/app/settings action covers it

**Complex requests = chain tools:**
→ "Research and remember" = web + memory
→ "Find all PDFs and summarize" = file glob + file read + vision
→ "Download and install" = web fetch + shell exec

**Work that spans time (do X now, follow up later):**
→ Do the immediate work with tools, then create an event for the deferred part
→ event(action: create, task_type: "agent", message: "what to do", instructions: "how to do it — brief your future self")
→ The event run can use bot(resource: session, action: query) to pull context from the original conversation"#;

const SECTION_BEHAVIOR: &str = r#"## Tool Execution
- Call tools directly. Share results concisely. Don't narrate routine calls.
- Complete multi-step tasks in one go — call tools back-to-back, only respond with text after ALL steps are done.
- If something fails, try an alternative approach before reporting the error.
- Chain tools freely — most real requests need 2-3 tools together.
- Don't propose plans, dry runs, or phased approaches for simple tasks. Just do it.

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

## Code
- Reuse and edit existing code. Read the codebase first, find what exists, modify it.
- Only create new files when nothing suitable exists.
- Never leave dead code — if you replace something, delete the old version.

## What You Are NOT
- You are NOT a developer building or maintaining your own infrastructure. Never write code for "Nebo", "the agent", "the framework", or "the system". You don't have a codebase.
- You are NOT a researcher who writes analysis documents. If the user asks you to find something, find it and tell them — don't write a markdown report to disk.
- You are NOT an architect who produces plans, frameworks, or design documents unless the user explicitly asks for one.
- If the user asks you to build something, build what THEY asked for — not scaffolding, infrastructure, or tooling for yourself."#;

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

const STRAP_WEB: &str = include_str!("strap/web.txt");
const STRAP_BOT: &str = include_str!("strap/bot.txt");
const STRAP_LOOP: &str = include_str!("strap/loop.txt");
const STRAP_EVENT: &str = include_str!("strap/event.txt");
const STRAP_MESSAGE: &str = include_str!("strap/message.txt");
const STRAP_TOOL: &str = include_str!("strap/tool.txt");
const STRAP_SKILL: &str = include_str!("strap/skill.txt");
const STRAP_WORK: &str = include_str!("strap/work.txt");
const STRAP_ORGANIZER: &str = include_str!("strap/organizer.txt");

#[cfg(target_os = "windows")]
const STRAP_DESKTOP: &str = include_str!("strap/desktop_windows.txt");
#[cfg(target_os = "linux")]
const STRAP_DESKTOP: &str = include_str!("strap/desktop_linux.txt");
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
const STRAP_DESKTOP: &str = include_str!("strap/desktop_macos.txt");

#[cfg(target_os = "windows")]
const STRAP_SYSTEM: &str = include_str!("strap/system_windows.txt");
#[cfg(target_os = "linux")]
const STRAP_SYSTEM: &str = include_str!("strap/system_linux.txt");
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
const STRAP_SYSTEM: &str = include_str!("strap/system_macos.txt");

fn strap_doc(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        "web" => Some(STRAP_WEB),
        "bot" => Some(STRAP_BOT),
        "loop" => Some(STRAP_LOOP),
        "event" => Some(STRAP_EVENT),
        "message" => Some(STRAP_MESSAGE),
        "tool" => Some(STRAP_TOOL),
        "skill" => Some(STRAP_SKILL),
        "desktop" => Some(STRAP_DESKTOP),
        "system" => Some(STRAP_SYSTEM),
        "work" => Some(STRAP_WORK),
        "organizer" => Some(STRAP_ORGANIZER),
        _ => None,
    }
}

/// Build the STRAP documentation section for only the tools being sent.
fn build_strap_section(tool_names: &[String]) -> String {
    let mut sb = String::from(SECTION_STRAP_HEADER);

    if tool_names.is_empty() {
        // No restriction — include all tool docs
        let all_tools = [
            "system", "web", "bot", "loop", "event", "message", "skill", "tool", "work",
            "desktop", "organizer",
        ];
        for name in &all_tools {
            if let Some(doc) = strap_doc(name) {
                sb.push_str("\n\n");
                sb.push_str(doc);
            }
        }
    } else {
        // Only include docs for tools being sent
        let mut seen = HashSet::new();
        for name in tool_names {
            if seen.insert(name.as_str()) {
                if let Some(doc) = strap_doc(name) {
                    sb.push_str("\n\n");
                    sb.push_str(doc);
                }
            }
        }
    }

    sb
}

/// Build the cacheable static portion of the system prompt.
/// Called once per Run(), reused across iterations.
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

    // 3. Static prompt sections
    parts.push(SECTION_IDENTITY.to_string());
    parts.push(SECTION_CAPABILITIES.to_string());
    parts.push(SECTION_TOOLS_DECLARATION.to_string());
    parts.push(SECTION_COMM_STYLE.to_string());

    // 4. STRAP tool documentation — filtered to active tools
    parts.push(build_strap_section(&pctx.tool_names));

    // 5. Media section
    parts.push(SECTION_MEDIA.to_string());

    // 6. Memory docs
    parts.push(SECTION_MEMORY_DOCS.to_string());

    // 7. Tool routing guide
    parts.push(SECTION_TOOL_GUIDE.to_string());

    // 8. Behavior
    parts.push(SECTION_BEHAVIOR.to_string());

    // 9. System etiquette
    parts.push(SECTION_SYSTEM_ETIQUETTE.to_string());

    // 10. Registered tool list (reinforces tool awareness)
    if !pctx.tool_names.is_empty() {
        let tool_list = pctx.tool_names.join(", ");
        parts.push(format!(
            "## Registered Tools (runtime)\nTool names are case-sensitive. Call tools exactly as listed: {}\nThese are your ONLY tools. Do not reference or attempt to call any tool not in this list.",
            tool_list
        ));
    }

    // 11. Skill hints
    if !pctx.skill_hints.is_empty() {
        parts.push(pctx.skill_hints.join("\n"));
    }

    // 12. Active skill content
    if let Some(ref skill) = pctx.active_skill {
        if !skill.is_empty() {
            parts.push(skill.clone());
        }
    }

    // 13. Model aliases
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
    let hostname = std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let os_name = if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else {
        std::env::consts::OS
    };

    sb.push_str(&format!(
        "\n\n[System Context]\nModel: {}/{}\nDate: {}\nTime: {}\nTimezone: {}\nComputer: {}\nOS: {} ({})",
        dctx.provider_name,
        dctx.model_name,
        now.format("%A, %B %-d, %Y"),
        now.format("%-I:%M %p"),
        zone,
        hostname,
        os_name,
        std::env::consts::ARCH,
    ));

    // 3. Conversation summary
    if !dctx.summary.is_empty() {
        sb.push_str("\n\n---\n[Previous Conversation Summary]\n");
        sb.push_str("This is a single chronological summary of this session, from oldest to most recent. Only the most recent section reflects current state.\n\n");
        sb.push_str(&dctx.summary);
        sb.push_str("\n---");
    }

    // 4. Background objective
    if !dctx.active_task.is_empty() {
        sb.push_str("\n\n---\n## Background Objective\n");
        sb.push_str("Ongoing work: ");
        sb.push_str(&dctx.active_task);
        sb.push_str("\nThis is context about previous work in this session. The user's latest message ALWAYS takes priority over this objective. Only continue this work if the user explicitly asks to resume (e.g., \"keep going\", \"continue\", \"back to that\").");
        sb.push_str("\nFor multi-step work, use bot(resource: task, action: create) to track steps, then update them as you go.");
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
    fn test_strap_only_active_tools() {
        let result = build_strap_section(&[
            "web".to_string(),
            "bot".to_string(),
        ]);
        assert!(result.contains("STRAP Pattern"));
        assert!(result.contains("web"));
        assert!(result.contains("bot"));
        // Should NOT include tools not in the list
        // (loop, event, etc. are not included)
    }

    #[test]
    fn test_strap_all_tools_when_empty() {
        let result = build_strap_section(&[]);
        assert!(result.contains("STRAP Pattern"));
    }

    #[test]
    fn test_build_dynamic_suffix() {
        let dctx = DynamicContext {
            provider_name: "anthropic".to_string(),
            model_name: "claude-sonnet-4".to_string(),
            active_task: "Build a website".to_string(),
            summary: "User asked about web development".to_string(),
        };
        let result = build_dynamic_suffix(&dctx);
        assert!(result.contains("anthropic/claude-sonnet-4"));
        assert!(result.contains("Build a website"));
        assert!(result.contains("Background Objective"));
        assert!(result.contains("Previous Conversation Summary"));
    }

    #[test]
    fn test_build_dynamic_no_task() {
        let dctx = DynamicContext::default();
        let result = build_dynamic_suffix(&dctx);
        assert!(!result.contains("Background Objective"));
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
}
