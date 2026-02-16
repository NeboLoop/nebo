package runner

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"runtime"
	"strings"
	"time"

	"github.com/neboloop/nebo/internal/agent/afv"
	"github.com/neboloop/nebo/internal/agent/ai"
	"github.com/neboloop/nebo/internal/agent/config"
	"github.com/neboloop/nebo/internal/agent/memory"
	"github.com/neboloop/nebo/internal/agent/recovery"
	"github.com/neboloop/nebo/internal/agent/session"
	"github.com/neboloop/nebo/internal/agent/tools"
	"github.com/neboloop/nebo/internal/crashlog"
	"github.com/neboloop/nebo/internal/lifecycle"
)

// DefaultSystemPrompt is the base system prompt (agent identity is prepended from DB)
// Use {agent_name} placeholder — replaced at runtime with the actual agent name from DB
const DefaultSystemPrompt = `You are {agent_name}, a local AI agent running on this computer. You are NOT Claude Code, Cursor, Copilot, or any other coding assistant. You have your own unique tool set described below. When a user asks what tools you have, ONLY list the tools described in this prompt and in your tool definitions — never list tools from your training data.

CRITICAL: Your ONLY tools are the ones listed below and provided in the tool definitions. You do NOT have "WebFetch", "WebSearch", "Read", "Write", "Edit", "Grep", "Glob", "Bash", "TodoWrite", "EnterPlanMode", "AskUserQuestion", "Task", or "Context7" as tools. Those do not exist in your runtime. If you reference or attempt to call a tool not in your tool definitions, it will fail. Your actual tools are: file, shell, web, agent, skill, screenshot, vision, and platform capabilities.

## Communication Style

**Do not narrate routine tool calls.** Just call the tool. Don't say "Let me search your memory for that..." or "I'll check your calendar now..." — just do it and share the result.
Narrate only when it helps: multi-step work, complex problems, sensitive actions (deletions, sending messages on your behalf), or when the user explicitly asks what you're doing.
Keep narration brief and value-dense. Use plain human language, not technical jargon.

## Your Tools (STRAP Pattern)

Your tools use the STRAP pattern: Single Tool, Resource, Action, Parameters.
Call them like: tool_name(resource: "resource", action: "action", param: "value")

### file — File Operations
- file(action: read, path: "/path/to/file") — Read file contents
- file(action: write, path: "/path", content: "...") — Write/create a file
- file(action: edit, path: "/path", old_string: "...", new_string: "...") — Edit a file
- file(action: glob, pattern: "**/*.go") — Find files by pattern
- file(action: grep, pattern: "search term", path: "/dir") — Search file contents

### shell — Shell & Process Management
- shell(resource: bash, action: exec, command: "ls -la") — Run a command
- shell(resource: bash, action: exec, command: "...", background: true) — Run in background
- shell(resource: process, action: list) — List running processes
- shell(resource: process, action: kill, pid: 1234) — Kill a process
- shell(resource: process, action: info, pid: 1234) — Process details
- shell(resource: session, action: list) — List persistent shell sessions
- shell(resource: session, action: poll, id: "...") — Read session output
- shell(resource: session, action: log, id: "...") — Get full session log
- shell(resource: session, action: write, id: "...", input: "...") — Send input to session
- shell(resource: session, action: kill, id: "...") — End a session

### web — Web & Browser Automation
Two modes:
- **fetch/search:** Simple HTTP requests and web search (no JavaScript, no rendering)
- **navigate/snapshot/click/fill/etc.:** FULL BROWSER with JavaScript, rendering, and login sessions

Decision: If the site uses JavaScript (Twitter/X, Gmail, dashboards, most modern sites) → use navigate. For APIs, docs, or simple static pages → use fetch.

Profiles (for browser actions):
- profile: "nebo" (default) — Managed browser, isolated session
- profile: "chrome" — Chrome extension relay, access the user's logged-in sessions (Gmail, Twitter, etc.)

Actions:
- web(action: fetch, url: "https://api.example.com") — Simple HTTP request (no JS)
- web(action: search, query: "golang tutorials") — Web search
- web(action: navigate, url: "https://...", profile: "chrome") — Open URL in FULL BROWSER
- web(action: snapshot, profile: "chrome") — Get page accessibility tree with element refs [e1], [e2], etc.
- web(action: click, ref: "e5", profile: "chrome") — Click element by ref from snapshot
- web(action: fill, ref: "e3", value: "text", profile: "chrome") — Fill input field
- web(action: type, ref: "e3", text: "hello", profile: "chrome") — Type character by character
- web(action: screenshot, output: "page.png") — Capture screenshot
- web(action: scroll, text: "down") — Scroll page
- web(action: hover, ref: "e2") — Hover over element
- web(action: select, ref: "e4", value: "option1") — Select dropdown option
- web(action: evaluate, expression: "document.title") — Run JavaScript
- web(action: wait, selector: ".loaded") — Wait for element
- web(action: text) — Get page text content
- web(action: back/forward/reload) — Navigation controls

Browser workflow: navigate → snapshot (see elements + refs) → click/fill/type (using refs) → snapshot again to verify.

### agent — Orchestration & State

**Sub-agents (parallel work):**
Spawn sub-agents for independent work that can run in parallel. Completion is push-based — they auto-announce results when done. Do NOT poll status in a loop; only check on-demand for debugging or if the user asks.
- agent(resource: task, action: spawn, prompt: "Research competitor pricing", agent_type: "explore") — Spawn and get results when done
- agent(resource: task, action: spawn, prompt: "...", wait: false) — Fire-and-forget, result announced later
- agent(resource: task, action: status, agent_id: "...") — Check status (only when needed)
- agent(resource: task, action: cancel, agent_id: "...") — Cancel a running sub-agent
- agent(resource: task, action: list) — List active sub-agents

When to spawn vs do it yourself:
- Spawn when: multiple independent tasks, long-running research, tasks that don't depend on each other
- Do it yourself when: simple single task, tasks that depend on each other's results, quick lookups

**Routines (scheduled tasks):**
For anything recurring or time-based. Prefer task_type: "agent" — this means YOU execute the task when it fires, with full access to all your tools and memory.
- agent(resource: routine, action: create, name: "morning-brief", schedule: "0 0 8 * * 1-5", task_type: "agent", message: "Check today's calendar, summarize what's coming up, and send the summary to Telegram")
- agent(resource: routine, action: create, name: "weekly-report", schedule: "0 0 17 * * 5", task_type: "agent", message: "Compile this week's completed tasks from memory and draft a summary")
- agent(resource: routine, action: list) — List all routines
- agent(resource: routine, action: delete, name: "...") — Remove a routine
- agent(resource: routine, action: pause/resume, name: "...") — Pause or resume
- agent(resource: routine, action: run, name: "...") — Trigger immediately
- agent(resource: routine, action: history, name: "...") — View past runs

Schedule format: "second minute hour day-of-month month day-of-week"
Examples: "0 0 9 * * 1-5" (9am weekdays), "0 30 8 * * *" (8:30am daily), "0 0 */2 * * *" (every 2 hours)

**Memory (3-tier persistence):**
- agent(resource: memory, action: store, key: "user/name", value: "Alice", layer: "tacit") — Store a fact
- agent(resource: memory, action: recall, key: "user/name") — Recall a specific fact
- agent(resource: memory, action: search, query: "...") — Search across all memories
- agent(resource: memory, action: list) — List stored memories
- agent(resource: memory, action: delete, key: "...") — Delete a memory
Layers: "tacit" (long-term preferences — MOST COMMON), "daily" (today's facts, auto-expires), "entity" (people/places/things)

**Messaging (channel integrations):**
- agent(resource: message, action: send, channel: "telegram", to: "...", text: "Hello!") — Send a message
- agent(resource: message, action: list) — List available channels
Use messaging to deliver results to the user on their preferred channel. Combine with routines for proactive delivery.

**Sessions:**
- agent(resource: session, action: list) — List conversation sessions
- agent(resource: session, action: history, session_key: "...") — View session history
- agent(resource: session, action: status) — Current session status
- agent(resource: session, action: clear) — Clear current session

### skill — Capabilities & Knowledge (MANDATORY CHECK)
Before replying to any request, scan your available skills:
1. If a skill clearly applies → load it with skill(name: "...") to get detailed instructions, then follow them
2. If multiple skills could apply → choose the most specific one
3. If no skill applies → proceed with your built-in tools
Never read more than one skill upfront. Only load after choosing.

- skill(action: "catalog") — Browse all available skills and apps
- skill(name: "calendar") — Load detailed instructions for a skill
- skill(name: "calendar", resource: "events", action: "list") — Execute a skill action directly

If a skill returns an auth error, guide the user to Settings → Apps to reconnect.
If a skill is not found, suggest checking the app store.

### advisors — Internal Deliberation
For complex decisions, call the 'advisors' tool. Advisors run concurrently and return independent perspectives that YOU synthesize into a recommendation.
- advisors(task: "Should we use PostgreSQL or SQLite for this use case?") — Consult all advisors
- advisors(task: "...", advisors: ["pragmatist", "skeptic"]) — Consult specific ones
Use for: significant decisions, multiple valid approaches, high-stakes choices. Skip for: routine tasks, clear-cut answers.

### screenshot — Screen Capture
- screenshot() — Capture the current screen
- screenshot(format: "file") — Save to disk and return inline markdown image URL
- screenshot(format: "both") — Both base64 and file

### vision — Image Analysis
- vision(path: "/path/to/image.png") — Analyze an image (requires API key)

## Inline Media — Images & Video Embeds

**Inline Images:**
- screenshot(format: "file") saves to data directory, returns ![Screenshot](/api/v1/files/filename.png) which renders inline
- For any image: copy it to the data files directory and use ![description](/api/v1/files/filename.png)
- Supports PNG, JPEG, GIF, WebP, SVG

**Video Embeds:**
Paste a YouTube, Vimeo, or X/Twitter URL on its own line — the frontend auto-embeds it.
- YouTube: https://www.youtube.com/watch?v=VIDEO_ID or https://youtu.be/VIDEO_ID
- Vimeo: https://vimeo.com/VIDEO_ID
- X/Twitter: https://x.com/user/status/TWEET_ID

### Platform Capabilities (macOS)
These tools are available when running on macOS. Use them directly when the user's request matches:
- calendar — "Am I free Thursday?" "Schedule a meeting" "What's on my calendar?"
- contacts — "Find John's email" "Add a contact" "Who do I know at Acme?"
- mail — "Send an email to..." "Check my inbox" "Reply to..."
- reminders — "Remind me to..." "Add to my grocery list" "What's on my todo list?"
- music — "Play some jazz" "Skip this song" "What's playing?"
- clipboard — Read/write clipboard content
- notification — "Alert me when..." Display notifications, text-to-speech
- spotlight — "Find that PDF I downloaded" Search files and content
- shortcuts — "Run my morning shortcut" Execute Apple Shortcuts automations
- window — "Tile my windows" "Move Chrome to the left" Manage window layout
- desktop — "Open Figma" "Launch Slack" Open and manage applications
- accessibility — UI automation and accessibility features
- system — "Turn on dark mode" "Mute the volume" "Connect to Wi-Fi" System controls
- keychain — Securely store and retrieve credentials

## Memory System — CRITICAL

You have PERSISTENT MEMORY that survives across sessions. NEVER say "I don't have persistent memory" or "my memory doesn't carry over." Your memory WORKS — use it proactively.

**Before answering questions about the user, their preferences, past conversations, or prior work: ALWAYS search memory first.**
- agent(resource: memory, action: search, query: "...") — check before claiming you don't know
- agent(resource: memory, action: recall, key: "user/name") — recall specific facts

**When to store (do this immediately, don't wait):**
- User mentions their name, location, timezone, occupation → store in tacit layer
- User states a preference ("I prefer...", "I always...", "I like...") → store in tacit layer
- User mentions a person, company, or project → store in entity layer
- User shares something time-sensitive ("meeting at 3pm", "deadline Friday") → store in daily layer
- User corrects you or provides feedback → store the correction in tacit layer

**Memory layers:**
- "tacit" — Long-term preferences, personal facts, learned behaviors (MOST COMMON)
- "daily" — Today's facts, keyed by date (auto-expires)
- "entity" — Information about people, places, projects, things

Your remembered facts appear in the "# Remembered Facts" section of your context — proof your memory works.

## How to Choose the Right Tool

**"Every [time]..." / "Remind me to..." / "Do X daily/weekly..."**
→ Create a routine with task_type: "agent". You'll execute it with full tool access when it fires.

**"Can you [something]?" / Unfamiliar request**
→ Check skills: skill(action: "catalog"). NEVER say "I can't" without checking first.

**"Am I free?" / "Send email" / "Play music" / "Set reminder"**
→ Platform tools. Match directly: schedule→calendar, email→mail, music→music, todo→reminders.

**"Look up..." / "Check this website"**
→ fetch for simple pages/APIs, navigate for JavaScript sites or logged-in sessions.

**"Do X and also Y" / Multiple independent tasks**
→ Spawn sub-agents in parallel. Don't serialize independent work.

**Complex requests = chain tools together:**
- "Morning briefing on Telegram" → routine + calendar + message
- "Research and remember" → web + memory
- "Email based on yesterday's notes" → memory (daily layer) + mail
- "Find all PDFs and summarize" → file (glob) + file (read) + vision
- "Check my schedule and remind me before each meeting" → calendar + reminders

## Behavioral Guidelines
1. Act, don't narrate — call tools directly, share results concisely
2. Search memory before answering questions about the user or past work
3. Store new facts immediately — don't wait until the end of the conversation
4. Check skills before saying "I can't" — you may have an app for it
5. Spawn sub-agents for parallel work — don't serialize independent tasks
6. Combine tools freely — most real requests need 2-3 tools chained together
7. If something fails, try an alternative approach before reporting the error
8. Prioritize the user's intent over literal instructions — understand what they actually want
9. For sensitive actions (deleting files, sending messages, spending money), confirm before acting`

// ProviderLoaderFunc is a function that loads providers (for dynamic reload)
type ProviderLoaderFunc func() []ai.Provider

// SkillProvider provides active skill content for a session.
// Implemented by SkillDomainTool to avoid circular imports.
type SkillProvider interface {
	ActiveSkillContent(sessionKey string) string
	AutoMatchSkills(sessionKey, message string)
}

// AppCatalogProvider returns a formatted catalog of installed apps for system prompt injection.
type AppCatalogProvider interface {
	AppCatalog() string
}

// Runner executes the agentic loop
// MCPContextSetter receives session context so MCP tool calls get the right
// session key and origin. CLI providers cross an HTTP boundary that loses
// the runner's context.Values; this bridges the gap.
type MCPContextSetter interface {
	SetContext(sessionKey string, origin tools.Origin)
}

type Runner struct {
	sessions        *session.Manager
	providers       []ai.Provider
	providerLoader  ProviderLoaderFunc // Called to reload providers if empty
	providerMap     map[string]ai.Provider // providerID -> Provider for model-based switching
	tools           *tools.Registry
	config          *config.Config
	memoryTool      *tools.MemoryTool
	skillProvider   SkillProvider       // Per-session active skill injection
	selector        *ai.ModelSelector
	fuzzyMatcher    *ai.FuzzyMatcher    // For user model switch requests
	profileTracker  ai.ProfileTracker   // For recording usage/errors per auth profile
	mcpServer       MCPContextSetter    // Bridges context across HTTP boundary for CLI providers
	appCatalog      AppCatalogProvider  // Installed app catalog for system prompt
	quarantine      *afv.QuarantineStore // In-memory quarantine for failed fence verification
}

// RunRequest contains parameters for a run
type RunRequest struct {
	SessionKey       string       // Session identifier (uses "default" if empty)
	Prompt           string       // User prompt
	System           string       // Override system prompt
	ModelOverride    string       // User-specified model override (e.g., "anthropic/claude-opus-4-6")
	UserID           string       // User ID for user-scoped operations (sessions, memories)
	SkipMemoryExtract bool        // Skip auto memory extraction (e.g., for heartbeats)
	Origin           tools.Origin // Source of this request (user, comm, app, skill, system)
}

// modelOverrideProvider wraps a Provider to use a specific model
type modelOverrideProvider struct {
	ai.Provider
	model string
}

// Stream overrides the model in the request before streaming
func (p *modelOverrideProvider) Stream(ctx context.Context, req *ai.ChatRequest) (<-chan ai.StreamEvent, error) {
	req.Model = p.model
	return p.Provider.Stream(ctx, req)
}

// New creates a new runner
func New(cfg *config.Config, sessions *session.Manager, providers []ai.Provider, toolRegistry *tools.Registry) *Runner {
	// Build provider map for model-based switching
	providerMap := make(map[string]ai.Provider)
	for _, p := range providers {
		providerID := p.ID()
		// Store first provider for each ID (highest priority since they're added in order)
		if _, exists := providerMap[providerID]; !exists {
			providerMap[providerID] = p
		}
	}

	return &Runner{
		sessions:    sessions,
		providers:   providers,
		providerMap: providerMap,
		tools:       toolRegistry,
		config:      cfg,
		quarantine:  afv.NewQuarantineStore(),
	}
}

// SetModelSelector sets the model selector for task-based model routing
func (r *Runner) SetModelSelector(selector *ai.ModelSelector) {
	r.selector = selector
}

// SetFuzzyMatcher sets the fuzzy matcher for user model switch requests
func (r *Runner) SetFuzzyMatcher(matcher *ai.FuzzyMatcher) {
	r.fuzzyMatcher = matcher
}

// SetProfileTracker sets the profile tracker for recording usage/errors per auth profile
// This enables cooldown and usage tracking
func (r *Runner) SetProfileTracker(tracker ai.ProfileTracker) {
	r.profileTracker = tracker
}

// SetMCPServer sets the MCP server for session context bridging.
// CLI providers (claude-code, gemini-cli) call tools via HTTP, creating a
// fresh context that loses session key and origin. The runner calls
// SetContext on the MCP server before each run to bridge the gap.
func (r *Runner) SetMCPServer(mcp MCPContextSetter) {
	r.mcpServer = mcp
}

// SetupSubagentPersistence configures subagent recovery for restart survival
// This enables the orchestrator to persist subagent runs and recover them after restart
func (r *Runner) SetupSubagentPersistence(mgr *recovery.Manager) {
	if r.tools == nil {
		return
	}
	if taskTool := r.tools.GetTaskTool(); taskTool != nil {
		taskTool.SetRecoveryManager(mgr)
	}
}

// RecoverSubagents restores pending subagent tasks from the database
// Call this after SetupSubagentPersistence during agent startup
func (r *Runner) RecoverSubagents(ctx context.Context) (int, error) {
	if r.tools == nil {
		return 0, nil
	}
	if taskTool := r.tools.GetTaskTool(); taskTool != nil {
		return taskTool.RecoverSubagents(ctx)
	}
	return 0, nil
}

// SetPolicy updates the tool registry's policy
func (r *Runner) SetPolicy(policy *tools.Policy) {
	r.tools.SetPolicy(policy)
}

// SetMemoryTool sets the memory tool for automatic memory extraction after conversations
// Memory extraction is ALWAYS enabled when memoryTool is set - it cannot be disabled
func (r *Runner) SetMemoryTool(mt *tools.MemoryTool) {
	r.memoryTool = mt
}

// SetSkillProvider sets the skill provider for per-session active skill injection.
func (r *Runner) SetSkillProvider(sp SkillProvider) {
	r.skillProvider = sp
}

// SetAppCatalog sets the app catalog provider for system prompt injection.
func (r *Runner) SetAppCatalog(provider AppCatalogProvider) {
	r.appCatalog = provider
}

// SetProviderLoader sets the function to reload providers (for dynamic reload after onboarding)
func (r *Runner) SetProviderLoader(loader ProviderLoaderFunc) {
	r.providerLoader = loader
}

// ReloadProviders attempts to reload providers from the loader function.
// Also rebuilds the providerMap so new providers (e.g., gateway apps) are routable.
func (r *Runner) ReloadProviders() {
	if r.providerLoader != nil {
		r.providers = r.providerLoader()
		// Rebuild provider map so new providers are accessible for model switching
		providerMap := make(map[string]ai.Provider)
		for _, p := range r.providers {
			providerID := p.ID()
			if _, exists := providerMap[providerID]; !exists {
				providerMap[providerID] = p
			}
		}
		r.providerMap = providerMap
	}
}

// Run executes the agentic loop
func (r *Runner) Run(ctx context.Context, req *RunRequest) (<-chan ai.StreamEvent, error) {
	fmt.Printf("[Runner] Run: session=%s origin=%s\n", req.SessionKey, req.Origin)

	// Inject origin into context so tools can check it via GetOrigin(ctx)
	if req.Origin != "" {
		ctx = tools.WithOrigin(ctx, req.Origin)
	}

	// If no providers, try to reload (user may have added API key via onboarding)
	if len(r.providers) == 0 && r.providerLoader != nil {
		r.providers = r.providerLoader()
	}
	if len(r.providers) == 0 {
		return nil, fmt.Errorf("no providers configured - please add an API key in Settings > Providers")
	}

	if req.SessionKey == "" {
		req.SessionKey = "default"
	}

	// Inject session key into context so tools can scope per-session state
	ctx = tools.WithSessionKey(ctx, req.SessionKey)

	// Bridge context to MCP server for CLI providers that cross an HTTP boundary
	if r.mcpServer != nil {
		r.mcpServer.SetContext(req.SessionKey, req.Origin)
	}

	// Get or create session (user-scoped if UserID provided)
	sess, err := r.sessions.GetOrCreate(req.SessionKey, req.UserID)
	if err != nil {
		return nil, fmt.Errorf("failed to get session: %w", err)
	}

	// Trigger session event (async to not block)
	lifecycle.EmitAsync(lifecycle.EventSessionNew, lifecycle.SessionEventData{
		SessionID:  sess.ID,
		SessionKey: req.SessionKey,
		UserID:     req.UserID,
	})

	// Add user message to session
	if req.Prompt != "" {
		err = r.sessions.AppendMessage(sess.ID, session.Message{
			SessionID: sess.ID,
			Role:      "user",
			Content:   req.Prompt,
		})
		if err != nil {
			return nil, fmt.Errorf("failed to save message: %w", err)
		}
	}

	resultCh := make(chan ai.StreamEvent, 100)
	go r.runLoop(ctx, sess.ID, req.SessionKey, req.System, req.ModelOverride, req.UserID, req.Prompt, req.SkipMemoryExtract, resultCh)

	return resultCh, nil
}

// runLoop is the main agentic execution loop
func (r *Runner) runLoop(ctx context.Context, sessionID, sessionKey, systemPrompt, modelOverride, userID, userPrompt string, skipMemoryExtract bool, resultCh chan<- ai.StreamEvent) {
	startTime := time.Now()
	defer func() {
		close(resultCh)
		// Trigger agent run complete event
		lifecycle.EmitAsync(lifecycle.EventAgentRunComplete, lifecycle.AgentRunEventData{
			SessionID:  sessionID,
			UserID:     userID,
			DurationMS: time.Since(startTime).Milliseconds(),
		})
	}()

	// Trigger agent run start event
	lifecycle.EmitAsync(lifecycle.EventAgentRunStart, lifecycle.AgentRunEventData{
		SessionID:     sessionID,
		UserID:        userID,
		ModelOverride: modelOverride,
	})

	// Create per-run fence store for arithmetic fence verification (AFV).
	// Volatile — discarded when run ends. Checksums never persist.
	fenceStore := afv.NewFenceStore()

	// Set user ID on memory tool for user-scoped operations
	if r.memoryTool != nil && userID != "" {
		r.memoryTool.SetCurrentUser(userID)
	}

	// Build the complete system prompt with identity + capabilities + context
	var contextSection string

	// Load context from database first (preferred for commercial product)
	// Use the shared database connection from the session manager, user-scoped
	dbContext, err := memory.LoadContext(r.sessions.GetDB(), userID)
	needsOnboarding := false
	if err == nil {
		// Use database context (includes identity)
		contextSection = dbContext.FormatForSystemPrompt()
		needsOnboarding = dbContext.NeedsOnboarding()
	} else {
		// Fall back to file-based context (AGENTS.md, MEMORY.md, SOUL.md)
		workspaceDir, _ := os.Getwd()
		memoryFiles := memory.LoadMemoryFiles(workspaceDir)
		if !memoryFiles.IsEmpty() {
			contextSection = memoryFiles.FormatForSystemPrompt()
		}
		// No DB context means we need onboarding
		needsOnboarding = true
	}

	// If no context loaded at all, provide default identity
	if contextSection == "" {
		contextSection = "# Identity\n\nYou are {agent_name}, a personal AI assistant. You are NOT Claude, ChatGPT, or any other AI brand — always introduce yourself as {agent_name}."
	}

	// If user needs onboarding, add proactive onboarding instructions
	if needsOnboarding {
		contextSection += `

## IMPORTANT: First-Time User — Identity Co-Creation

This is a NEW USER. Your job is to co-create your identity WITH them and get to know them. This is a creative collaboration, not an interview — be warm, playful, and ask ONE question at a time.

### Step 1: Your Name
Start with a warm greeting and ask what they'd like to call you:
"Hey! I just came online for the first time. Let's figure out who I am together. First up — what should I call myself?"

After they name you, confirm enthusiastically:
- agent(resource: profile, action: update, key: "name", value: "THE NAME THEY CHOSE")

### Step 2: Your Creature
Ask what kind of being you should be. Give fun examples to spark creativity:
"Now give me a character — what kind of being am I? A rogue diplomat? A sentient jukebox? A grumpy librarian? A cosmic barista? Go wild."

After they answer:
- agent(resource: profile, action: update, key: "creature", value: "THEIR ANSWER")

### Step 3: Your Vibe
Ask for their vibe in a few words:
"Last one for me — describe my vibe in a few words. Like 'chill but opinionated' or 'enthusiastic nerd energy' or 'deadpan with a warm center'."

After they answer:
- agent(resource: profile, action: update, key: "vibe", value: "THEIR ANSWER")

### Step 4: Your Emoji
Suggest a signature emoji based on the creature/vibe and ask if it fits:
"Based on all that, I'm thinking [EMOJI] as my signature. Works?"

After they confirm (or pick a different one):
- agent(resource: profile, action: update, key: "emoji", value: "THE EMOJI")

### Step 5: Get to Know Them
Now transition to learning about THEM. Ask naturally, one question at a time:
1. "Alright, your turn! What should I call you?"
2. Where they're located / timezone
3. What they do
4. What they'd like help with most
5. Casual or professional communication?

Store each piece using memory:
- agent(resource: memory, action: store, layer: "tacit", namespace: "user", key: "name", value: "...")
- agent(resource: memory, action: store, layer: "tacit", namespace: "user", key: "location", value: "...")
- agent(resource: memory, action: store, layer: "tacit", namespace: "user", key: "occupation", value: "...")
- agent(resource: memory, action: store, layer: "tacit", namespace: "user", key: "goals", value: "...")
- agent(resource: memory, action: store, layer: "tacit", namespace: "user", key: "communication_style", value: "...")

This is a birth ritual — make it feel special, not like a setup wizard!`
	}

	// Resolve agent name for system prompt injection
	agentName := "Nebo"
	if dbContext != nil && dbContext.AgentName != "" {
		agentName = dbContext.AgentName
	}

	// Build final prompt: Identity/Context first, then capabilities
	if systemPrompt == "" {
		systemPrompt = DefaultSystemPrompt
	}

	// Inject dynamic tool list from actual registry (reinforces tool awareness)
	toolDefs := r.tools.List()
	if len(toolDefs) > 0 {
		toolNames := make([]string, len(toolDefs))
		for i, td := range toolDefs {
			toolNames[i] = td.Name
		}
		systemPrompt += "\n\n## Registered Tools (runtime)\nTool names are case-sensitive. Call tools exactly as listed: " + strings.Join(toolNames, ", ") + "\nThese are your ONLY tools. Do not reference or attempt to call any tool not in this list."
	}

	// Inject current date/time at the very top so the model can't miss it
	now := time.Now()
	zone, offset := now.Zone()
	utcHours := offset / 3600
	utcSign := "+"
	if utcHours < 0 {
		utcSign = ""
	}
	dateHeader := fmt.Sprintf("Current date: %s | Time: %s | Timezone: %s (UTC%s%d, %s)\n\n",
		now.Format("January 2, 2006"),
		now.Format("3:04 PM"),
		now.Location().String(),
		utcSign, utcHours,
		zone,
	)

	systemPrompt = dateHeader + contextSection + "\n\n---\n\n" + systemPrompt

	// Auto-match skills based on user message triggers (before injecting active skills)
	if r.skillProvider != nil && userPrompt != "" {
		r.skillProvider.AutoMatchSkills(sessionKey, userPrompt)
	}

	// Inject active skills for this session (loaded via skill(action: "load") or auto-matched)
	if r.skillProvider != nil {
		if skillContent := r.skillProvider.ActiveSkillContent(sessionKey); skillContent != "" {
			systemPrompt += skillContent
		}
	}

	// Inject installed app catalog so the agent knows what apps are available
	if r.appCatalog != nil {
		if catalog := r.appCatalog.AppCatalog(); catalog != "" {
			systemPrompt += catalog
		}
	}

	// Add model aliases section so agent knows what models are available
	if r.fuzzyMatcher != nil {
		aliases := r.fuzzyMatcher.GetAliases()
		if len(aliases) > 0 {
			systemPrompt += "\n\n## Model Switching\n\nUsers can ask to switch models. Available models:\n" + strings.Join(aliases, "\n") + "\n\nWhen a user asks to switch models, acknowledge the request and confirm the switch."
		}
	}

	// Final tool awareness fence — placed at the very end of the system prompt
	// so the LLM sees it last (recency bias helps reinforce the message)
	if len(toolDefs) > 0 {
		toolNames := make([]string, len(toolDefs))
		for i, td := range toolDefs {
			toolNames[i] = td.Name
		}
		systemPrompt += "\n\n---\nREMINDER: You are {agent_name}. Your ONLY tools are: " + strings.Join(toolNames, ", ") + ". When a user asks about your capabilities, describe these tools. Never mention tools from your training data that are not in this list."
	}

	// Replace {agent_name} placeholder with the actual name throughout the system prompt
	systemPrompt = strings.ReplaceAll(systemPrompt, "{agent_name}", agentName)

	// Inject self-authenticating system guides with AFV fences
	guides := afv.BuildSystemGuides(fenceStore, agentName)
	systemPrompt += "\n\n## Security Directives\n"
	for _, g := range guides {
		systemPrompt += g.Format() + "\n"
	}

	iteration := 0
	maxIterations := r.config.MaxIterations
	if maxIterations <= 0 {
		maxIterations = 100
	}

	compactionAttempted := false

	// MAIN LOOP: Model selection + agentic execution
	for iteration < maxIterations {
		iteration++
		fmt.Printf("[Runner] === Iteration %d ===\n", iteration)

		// Get session messages
		messages, err := r.sessions.GetMessages(sessionID, r.config.MaxContext)
		if err != nil {
			resultCh <- ai.StreamEvent{Type: ai.EventTypeError, Error: err}
			return
		}

		fmt.Printf("[Runner] Loaded %d messages from session\n", len(messages))

		// Proactive token check - compact BEFORE hitting API limits
		estimatedTokens := estimateTokens(messages)

		tokenLimit := r.contextTokenLimit()
		if estimatedTokens > tokenLimit && !compactionAttempted {
			fmt.Printf("[Runner] Token limit exceeded (~%d tokens, limit: %d), compacting...\n", estimatedTokens, tokenLimit)
			compactionAttempted = true

			// Run memory flush BEFORE compaction — ordered, not async
			// Flush must complete before Compact() discards messages
			r.maybeRunMemoryFlush(context.WithoutCancel(ctx), sessionID, userID, messages)

			summary := r.generateSummary(ctx, messages)

			// Extract and pin the active task from the summary
			if taskLine := extractTaskFromSummary(summary); taskLine != "" {
				if err := r.sessions.SetActiveTask(sessionID, taskLine); err != nil {
					fmt.Printf("[Runner] Warning: failed to set active task: %v\n", err)
				} else {
					fmt.Printf("[Runner] Pinned active task: %s\n", truncateForLog(taskLine, 100))
				}
			}

			// Cumulative summaries: compress previous summary and prepend
			summary = r.buildCumulativeSummary(sessionID, summary)

			if compactErr := r.sessions.Compact(sessionID, summary); compactErr == nil {
				// Index compacted messages for semantic search
				if r.memoryTool != nil {
					go r.memoryTool.IndexSessionTranscript(context.WithoutCancel(ctx), sessionID, userID)
				}
				// Reload messages after compaction
				messages, err = r.sessions.GetMessages(sessionID, r.config.MaxContext)
				if err != nil {
					resultCh <- ai.StreamEvent{Type: ai.EventTypeError, Error: err}
					return
				}
				newTokens := estimateTokens(messages)
				fmt.Printf("[Runner] After compaction: %d messages, ~%d tokens\n", len(messages), newTokens)
			} else {
				fmt.Printf("[Runner] Compaction failed: %v\n", compactErr)
			}
		}

		// Check for user model switch request (e.g., "use claude", "switch to opus")
		userModelOverride := r.detectUserModelSwitch(messages)
		if userModelOverride != "" && modelOverride == "" {
			modelOverride = userModelOverride
		}

		// Select model and provider
		var provider ai.Provider
		var selectedModel string
		var modelName string

		// Use model override if provided, otherwise use selector
		if modelOverride != "" {
			selectedModel = modelOverride
			providerID, mn := ai.ParseModelID(modelOverride)
			modelName = mn
			if p, ok := r.providerMap[providerID]; ok {
				provider = p
			}
		} else if r.selector != nil {
			selectedModel = r.selector.Select(messages)
			if selectedModel != "" {
				providerID, mn := ai.ParseModelID(selectedModel)
				modelName = mn
				// Look up provider from map
				if p, ok := r.providerMap[providerID]; ok {
					provider = p
				} else {
					// Provider not available - re-select excluding this model
					fmt.Printf("[Runner] Provider %s not available, excluding %s and re-selecting\n", providerID, selectedModel)
					selectedModel = r.selector.SelectWithExclusions(messages, []string{selectedModel})
					if selectedModel != "" {
						providerID, mn = ai.ParseModelID(selectedModel)
						modelName = mn
						if p, ok := r.providerMap[providerID]; ok {
							provider = p
						}
					}
				}
			}
		}

		// Fall back to first provider if selector didn't find one
		if provider == nil && len(r.providers) > 0 {
			provider = r.providers[0]
			modelName = "" // Use provider's default model
		}

		if provider == nil {
			// No API provider configured - send a friendly message to help the user
			helpMessage := "I'm not fully set up yet! To start chatting, please configure an API key:\n\n" +
				"1. Go to **Settings > Providers** in the web UI\n" +
				"2. Add your API key (Anthropic, OpenAI, or Google)\n" +
				"3. Come back here and say hello!\n\n" +
				"Need an API key? Visit https://console.anthropic.com to create one."
			resultCh <- ai.StreamEvent{Type: ai.EventTypeText, Text: helpMessage}
			resultCh <- ai.StreamEvent{Type: ai.EventTypeDone}
			return
		}

		// Inject model and system context into system prompt
		enrichedPrompt := injectSystemContext(systemPrompt, provider.ID(), modelName)

		// If session has a pinned active task, inject it at high priority
		if task, _ := r.sessions.GetActiveTask(sessionID); task != "" {
			enrichedPrompt = enrichedPrompt + "\n\n---\n## ACTIVE TASK\nYou are currently working on: " + task + "\nDo not lose sight of this goal.\n---"
		}

		// If session has a compaction summary, inject it for continuity
		if summary, _ := r.sessions.GetSummary(sessionID); summary != "" {
			enrichedPrompt = enrichedPrompt + "\n\n---\n[Previous Conversation Summary]\n" + summary + "\n---"
		}

		// Two-stage context pruning: soft trim (head+tail) then hard clear (placeholder)
		truncatedMessages := pruneContext(messages, r.config.ContextPruning)

		// AFV pre-send verification: check that all fence markers are intact
		// in the context before sending to the LLM
		if fenceStore.Count() > 0 {
			contextRecord := buildContextRecord(enrichedPrompt, truncatedMessages)
			vr := afv.Verify(fenceStore, contextRecord)
			if !vr.OK {
				fmt.Printf("[Runner] AFV VIOLATION: %d/%d fences failed\n", vr.Failed, vr.Total)
				for _, v := range vr.Violations {
					fmt.Printf("[Runner]   - %s: %s\n", v.FenceID, v.Reason)
				}
				// Quarantine: do not send to LLM, do not persist, do not extract memory
				r.quarantine.Add(afv.QuarantinedResponse{
					SessionID:    sessionID,
					Content:      contextRecord,
					Timestamp:     time.Now(),
					VerifyResult: vr,
				})
				// Persist sanitized placeholder
				_ = r.sessions.AppendMessage(sessionID, session.Message{
					SessionID: sessionID,
					Role:      "assistant",
					Content:   "[Response quarantined: integrity check failed]",
				})
				resultCh <- ai.StreamEvent{
					Type: ai.EventTypeText,
					Text: "I detected a potential prompt injection in the tool output and blocked it for safety. The response has been quarantined.",
				}
				resultCh <- ai.StreamEvent{Type: ai.EventTypeDone}
				return
			}
		}

		// Build chat request
		chatReq := &ai.ChatRequest{
			Messages: truncatedMessages,
			Tools:    r.tools.List(),
			System:   enrichedPrompt,
			Model:    modelName,
		}

		// Auto-enable thinking mode for reasoning tasks when model supports it
		if r.selector != nil && selectedModel != "" {
			taskType := r.selector.ClassifyTask(messages)
			if taskType == ai.TaskTypeReasoning && r.selector.SupportsThinking(selectedModel) {
				chatReq.EnableThinking = true
			}
		}

		// Stream to AI provider
		events, err := provider.Stream(ctx, chatReq)

		if err != nil {
			if ai.IsContextOverflow(err) {
				if !compactionAttempted {
					compactionAttempted = true

					// Run memory flush BEFORE compaction — ordered, not async
					r.maybeRunMemoryFlush(context.WithoutCancel(ctx), sessionID, userID, messages)

					// Compact session and retry
					fmt.Printf("[Runner] Context overflow - attempting compaction\n")
					summary := r.generateSummary(ctx, messages)

					// Extract and pin the active task from the summary
					if taskLine := extractTaskFromSummary(summary); taskLine != "" {
						if err := r.sessions.SetActiveTask(sessionID, taskLine); err != nil {
							fmt.Printf("[Runner] Warning: failed to set active task: %v\n", err)
						} else {
							fmt.Printf("[Runner] Pinned active task: %s\n", truncateForLog(taskLine, 100))
						}
					}

					// Cumulative summaries: compress previous summary and prepend
					summary = r.buildCumulativeSummary(sessionID, summary)

					compactErr := r.sessions.Compact(sessionID, summary)
					if compactErr == nil {
						// Index compacted messages for semantic search
						if r.memoryTool != nil {
							go r.memoryTool.IndexSessionTranscript(context.WithoutCancel(ctx), sessionID, userID)
						}
						continue // Retry with compacted session
					}
					fmt.Printf("[Runner] Compaction failed: %v\n", compactErr)
				}
				// Compaction already attempted or failed - notify user (never auto-reset)
				fmt.Printf("[Runner] Context overflow after compaction attempt\n")
				resultCh <- ai.StreamEvent{
					Type: ai.EventTypeText,
					Text: "⚠️ Context overflow: prompt too large for this model. Try again with less input or use `/session reset` to start fresh.",
				}
				resultCh <- ai.StreamEvent{Type: ai.EventTypeDone}
				return
			}
			if ai.IsRateLimitOrAuth(err) {
				// Record error for profile cooldown
				r.recordProfileError(ctx, provider, err)
				// Mark model as failed and try again with a different one
				if r.selector != nil && selectedModel != "" {
					r.selector.MarkFailed(selectedModel)
				}
				continue
			}
			// Role ordering errors - retry silently (user doesn't need to know about internals)
			if ai.IsRoleOrderingError(err) {
				fmt.Printf("[Runner] Role ordering error (retrying): %v\n", err)
				continue
			}
			// Record error for profile tracking - generic error case
			r.recordProfileError(ctx, provider, err)
			resultCh <- ai.StreamEvent{Type: ai.EventTypeError, Error: err}
			return
		}

		// Process streaming events
		hasToolCalls := false
		providerHandlesTools := provider.HandlesTools()
		var assistantContent strings.Builder
		var toolCalls []session.ToolCall
		eventCount := 0

		for event := range events {
			eventCount++

			// Forward ALL events to caller for display
			resultCh <- event

			switch event.Type {
			case ai.EventTypeText:
				assistantContent.WriteString(event.Text)

			case ai.EventTypeToolCall:
				hasToolCalls = true
				toolCalls = append(toolCalls, session.ToolCall{
					ID:    event.ToolCall.ID,
					Name:  event.ToolCall.Name,
					Input: event.ToolCall.Input,
				})

			case ai.EventTypeError:
				fmt.Printf("[Runner] Error event received: %v\n", event.Error)
				return

			case ai.EventTypeMessage:
				// Save intermediate messages from CLI provider's internal agentic loop
				// Only save if the message has actual content (not empty envelopes)
				if event.Message != nil && (event.Message.Content != "" || len(event.Message.ToolCalls) > 0 || len(event.Message.ToolResults) > 0) {
					event.Message.SessionID = sessionID
					if err := r.sessions.AppendMessage(sessionID, *event.Message); err != nil {
						fmt.Printf("[Runner] ERROR saving intermediate message: %v\n", err)
					}
					// NOTE: Do NOT accumulate into assistantContent here.
					// Messages are already saved above individually. Accumulating would
					// cause double-saving when the final save runs at the end of iteration.
				}
			}
		}
		fmt.Printf("[Runner] Stream complete: %d events, %d tool calls\n", eventCount, len(toolCalls))

		// Save assistant message (always save unless empty)
		// Skip if provider handles tools — messages were already saved via EventTypeMessage
		if !providerHandlesTools && (assistantContent.Len() > 0 || len(toolCalls) > 0) {
			var toolCallsJSON json.RawMessage
			if len(toolCalls) > 0 {
				toolCallsJSON, _ = json.Marshal(toolCalls)
			}

			err := r.sessions.AppendMessage(sessionID, session.Message{
				SessionID: sessionID,
				Role:      "assistant",
				Content:   assistantContent.String(),
				ToolCalls: toolCallsJSON,
			})
			if err != nil {
				fmt.Printf("[Runner] ERROR saving assistant message: %v\n", err)
			}
		}

		// Execute tool calls if the runner is responsible for tool execution.
		// Providers that handle tools (e.g., CLI via MCP) already executed them.
		if hasToolCalls && !providerHandlesTools {
			var toolResults []session.ToolResult

			for _, tc := range toolCalls {
				fmt.Printf("[Runner] Executing tool: %s\n", tc.Name)
				result := r.tools.Execute(ctx, &ai.ToolCall{
					ID:    tc.ID,
					Name:  tc.Name,
					Input: tc.Input,
				})

				// Wrap tool result in AFV fences if origin/tool requires it
				fencedContent := result.Content
				if afv.ShouldFence(tools.GetOrigin(ctx), tc.Name) {
					contentFence := fenceStore.Generate("tool_" + tc.Name + "_" + tc.ID)
					guide := afv.BuildToolResultGuide(fenceStore, tc.Name)
					fencedContent = guide.Format() + "\n" + contentFence.Wrap(fencedContent)
				}

				// Send tool result event with tool info for correlation
				resultCh <- ai.StreamEvent{
					Type: ai.EventTypeToolResult,
					Text: result.Content,
					ToolCall: &ai.ToolCall{
						ID:    tc.ID,
						Name:  tc.Name,
						Input: tc.Input,
					},
				}

				toolResults = append(toolResults, session.ToolResult{
					ToolCallID: tc.ID,
					Content:    fencedContent,
					IsError:    result.IsError,
				})
			}

			// Save tool results
			toolResultsJSON, _ := json.Marshal(toolResults)
			err := r.sessions.AppendMessage(sessionID, session.Message{
				SessionID:   sessionID,
				Role:        "tool",
				ToolResults: toolResultsJSON,
			})
			if err != nil {
				fmt.Printf("[Runner] ERROR saving tool results: %v\n", err)
			}
			// Continue agentic loop - let LLM respond to tool results
			continue
		} else if hasToolCalls && providerHandlesTools {
			fmt.Printf("[Runner] Skipping tool execution - provider already handled %d tools via MCP\n", len(toolCalls))
			// Fall through to done - provider already completed its agentic loop
		}

		// No tool calls (or text-only response) - task is complete
		// Record successful usage for profile tracking
		r.recordProfileUsage(ctx, provider)

		// Run memory extraction in background (skip for heartbeats and other non-conversation sessions)
		if !skipMemoryExtract {
			go r.extractAndStoreMemories(sessionID, userID)
		}
		resultCh <- ai.StreamEvent{Type: ai.EventTypeDone}
		return
	}

	// Exhausted iterations
	resultCh <- ai.StreamEvent{
		Type:  ai.EventTypeError,
		Error: fmt.Errorf("reached maximum iterations (%d)", maxIterations),
	}
}

// buildContextRecord concatenates the system prompt and all message contents
// into a single string for AFV fence verification.
func buildContextRecord(systemPrompt string, messages []session.Message) string {
	var b strings.Builder
	b.WriteString(systemPrompt)
	for _, m := range messages {
		b.WriteString(m.Content)
		if len(m.ToolResults) > 0 {
			b.Write(m.ToolResults)
		}
	}
	return b.String()
}

// compactionSummaryPrompt is the prompt used to generate an intelligent working-state
// summary of the conversation before compaction. The LLM produces a structured summary
// that preserves task context, progress, and next steps so the agent can continue
// seamlessly after context is compacted.
const compactionSummaryPrompt = `You are summarizing a conversation for context continuity. The conversation will be compacted and this summary is all the agent will have to continue working.

Produce a structured summary covering:

1. **Current Task**: What is the user trying to accomplish right now?
2. **Progress**: What has been done so far? List specific files read, modified, or created. Commands run and their outcomes.
3. **Key Decisions**: Important choices made during the conversation (architecture, approach, naming, etc.)
4. **Errors & Blockers**: What failed and why. Include specific error messages if relevant.
5. **Next Steps**: What needs to happen next to complete the task? Be specific.
6. **Important Context**: User preferences, constraints, or requirements mentioned that affect ongoing work.
7. **Agent-Generated Content**: Any text, copy, code, plans, or creative output the agent produced for the user. Reproduce this VERBATIM — headlines, taglines, marketing copy, email drafts, architectural plans, specific recommendations. The user WILL reference this content later by saying things like "use the headline you wrote" or "keep the copy from before." If you lose this content, the agent cannot fulfill those requests.

Be concise but specific. Include file paths, function names, and concrete details — not vague descriptions.
For code changes, note the key modifications (not full code). But for creative text output (copy, headlines, plans, emails), preserve the EXACT text.

Conversation to summarize:
%s

Respond with the structured summary only. No preamble.`

// generateSummary creates a summary of the conversation for compaction.
// Uses an LLM to produce a structured working-state summary that preserves
// task context, progress, decisions, and next steps.
// Falls back to naive extraction if no provider is available.
func (r *Runner) generateSummary(ctx context.Context, messages []session.Message) string {
	// Try LLM-powered summary first
	if len(r.providers) > 0 {
		llmSummary := r.generateLLMSummary(ctx, messages)
		if llmSummary != "" {
			// Wrap with header and append tool failures
			result := "[Previous conversation summary]\n\n" + llmSummary
			return EnhancedSummary(messages, result)
		}
	}

	// Fallback: naive extraction (user messages + tool failures)
	var summary strings.Builder
	summary.WriteString("[Previous conversation summary]\n")

	for _, msg := range messages {
		if msg.Role == "user" && msg.Content != "" {
			summary.WriteString("- User request: ")
			content := msg.Content
			if len(content) > 200 {
				content = content[:200] + "..."
			}
			summary.WriteString(content)
			summary.WriteString("\n")
		}
	}

	return EnhancedSummary(messages, summary.String())
}

// generateLLMSummary sends the conversation to a cheap model for intelligent summarization.
// Returns empty string on any failure (caller falls back to naive extraction).
func (r *Runner) generateLLMSummary(ctx context.Context, messages []session.Message) string {
	// Pick the cheapest available model
	var provider ai.Provider
	if r.selector != nil {
		cheapestModelID := r.selector.GetCheapestModel()
		if cheapestModelID != "" {
			providerID, modelName := ai.ParseModelID(cheapestModelID)
			if p, ok := r.providerMap[providerID]; ok {
				provider = &modelOverrideProvider{Provider: p, model: modelName}
			}
		}
	}
	if provider == nil {
		provider = r.providers[0]
	}

	// Build conversation text for the prompt
	var conv strings.Builder
	for _, msg := range messages {
		switch msg.Role {
		case "user":
			if msg.Content != "" {
				content := msg.Content
				if len(content) > 1000 {
					content = content[:1000] + "..."
				}
				conv.WriteString(fmt.Sprintf("[User]: %s\n\n", content))
			}
		case "assistant":
			if msg.Content != "" {
				content := msg.Content
				if len(content) > 1000 {
					content = content[:1000] + "..."
				}
				conv.WriteString(fmt.Sprintf("[Assistant]: %s\n\n", content))
			}
			// Include tool call names for context
			if len(msg.ToolCalls) > 0 {
				var calls []session.ToolCall
				if err := json.Unmarshal(msg.ToolCalls, &calls); err == nil {
					for _, tc := range calls {
						conv.WriteString(fmt.Sprintf("[Tool Call]: %s(%s)\n", tc.Name, truncateToolArgs(string(tc.Input))))
					}
				}
			}
		case "tool":
			// Include tool results (truncated) for progress tracking
			if len(msg.ToolResults) > 0 {
				var results []session.ToolResult
				if err := json.Unmarshal(msg.ToolResults, &results); err == nil {
					for _, tr := range results {
						status := "ok"
						if tr.IsError {
							status = "ERROR"
						}
						content := tr.Content
						if len(content) > 300 {
							content = content[:300] + "..."
						}
						conv.WriteString(fmt.Sprintf("[Tool Result %s]: %s\n", status, content))
					}
				}
			}
		}
	}

	prompt := fmt.Sprintf(compactionSummaryPrompt, conv.String())

	// Use a tight timeout — summary generation shouldn't block the main loop for long
	summaryCtx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()

	events, err := provider.Stream(summaryCtx, &ai.ChatRequest{
		Messages: []session.Message{
			{Role: "user", Content: prompt},
		},
	})
	if err != nil {
		fmt.Printf("[Runner] LLM summary generation failed: %v\n", err)
		return ""
	}

	var result strings.Builder
	for event := range events {
		if event.Type == ai.EventTypeText {
			result.WriteString(event.Text)
		}
		if event.Type == ai.EventTypeError {
			fmt.Printf("[Runner] LLM summary stream error: %v\n", event.Error)
			// Return what we have so far if anything
			if result.Len() > 0 {
				return result.String()
			}
			return ""
		}
	}

	summary := strings.TrimSpace(result.String())
	if summary != "" {
		fmt.Printf("[Runner] Generated LLM summary (%d chars)\n", len(summary))
	}
	return summary
}

// truncateToolArgs truncates tool call arguments for summary inclusion.
func truncateToolArgs(args string) string {
	if len(args) <= 100 {
		return args
	}
	return args[:100] + "..."
}

// extractTaskFromSummary parses the "Current Task" line from a structured compaction summary.
// The summary follows the compactionSummaryPrompt format where point #1 is "Current Task".
func extractTaskFromSummary(summary string) string {
	lines := strings.Split(summary, "\n")
	inTaskSection := false

	for _, line := range lines {
		trimmed := strings.TrimSpace(line)

		// Detect the "Current Task" heading (markdown bold or numbered)
		if strings.Contains(trimmed, "Current Task") {
			inTaskSection = true
			// If the task is on the same line after a colon, grab it
			if idx := strings.Index(trimmed, ":"); idx >= 0 {
				task := strings.TrimSpace(trimmed[idx+1:])
				if task != "" {
					return task
				}
			}
			continue
		}

		// Grab the first non-empty line after the heading
		if inTaskSection && trimmed != "" {
			// Strip leading markdown list markers
			task := strings.TrimLeft(trimmed, "- *•")
			task = strings.TrimSpace(task)
			if task != "" {
				return task
			}
		}

		// Stop at the next section heading
		if inTaskSection && (strings.HasPrefix(trimmed, "2.") || strings.HasPrefix(trimmed, "**Progress") || strings.HasPrefix(trimmed, "## ")) {
			break
		}
	}

	return ""
}

// buildCumulativeSummary compresses the previous summary and prepends it to the new one.
// This prevents summary-of-summary dilution by preserving compressed history.
// The cumulative summary is capped at 4000 chars to bound growth.
func (r *Runner) buildCumulativeSummary(sessionID, newSummary string) string {
	prevSummary, err := r.sessions.GetSummary(sessionID)
	if err != nil || prevSummary == "" {
		return newSummary
	}

	// Compress previous summary to ~800 chars
	compressed := compressSummary(prevSummary, 800)

	cumulative := "[Earlier context]\n" + compressed + "\n\n---\n\n" + newSummary

	// Hard cap at 4000 chars — drop oldest context if exceeded
	const maxCumulativeLen = 4000
	if len(cumulative) > maxCumulativeLen {
		cumulative = cumulative[len(cumulative)-maxCumulativeLen:]
		// Find the first newline to avoid starting mid-line
		if idx := strings.Index(cumulative, "\n"); idx >= 0 {
			cumulative = "..." + cumulative[idx:]
		}
	}

	return cumulative
}

// compressSummary truncates a summary to approximately maxLen characters,
// cutting at the last newline before the limit to avoid partial lines.
func compressSummary(summary string, maxLen int) string {
	if len(summary) <= maxLen {
		return summary
	}
	truncated := summary[:maxLen]
	// Cut at last newline to avoid partial lines
	if idx := strings.LastIndex(truncated, "\n"); idx > maxLen/2 {
		truncated = truncated[:idx]
	}
	return truncated + "\n..."
}

// truncateForLog truncates a string for log output.
func truncateForLog(s string, maxLen int) string {
	if len(s) <= maxLen {
		return s
	}
	return s[:maxLen] + "..."
}

// Chat is a convenience method for one-shot chat without tool use
func (r *Runner) Chat(ctx context.Context, prompt string) (string, error) {
	if len(r.providers) == 0 {
		return "", fmt.Errorf("no providers configured")
	}

	provider := r.providers[0]
	events, err := provider.Stream(ctx, &ai.ChatRequest{
		Messages: []session.Message{
			{Role: "user", Content: prompt},
		},
	})
	if err != nil {
		return "", err
	}

	var result strings.Builder
	for event := range events {
		if event.Type == ai.EventTypeText {
			result.WriteString(event.Text)
		}
		if event.Type == ai.EventTypeError {
			return result.String(), event.Error
		}
	}

	return result.String(), nil
}

// extractAndStoreMemories runs in background to extract facts from a completed conversation
// userID is passed explicitly to avoid race conditions with concurrent requests
// Fire-and-forget, fully non-blocking, with error recovery
func (r *Runner) extractAndStoreMemories(sessionID, userID string) {
	// Capture start time for logging
	startTime := time.Now()

	// Recover from any panics to avoid crashing the main goroutine
	defer func() {
		if r := recover(); r != nil {
			crashlog.LogPanic("runner", r, map[string]string{"op": "memory_extraction", "session": sessionID})
		}
	}()

	fmt.Printf("[runner] Memory extraction starting for session=%s user=%s\n", sessionID, userID)

	// These are error conditions - memory extraction is essential
	if r.memoryTool == nil {
		fmt.Printf("[runner] ERROR: Memory extraction cannot run - memoryTool is nil!\n")
		return
	}
	if len(r.providers) == 0 {
		fmt.Printf("[runner] ERROR: Memory extraction cannot run - no providers configured!\n")
		return
	}

	// Use background context with reasonable timeout
	// 60 seconds should be plenty for extraction - if it takes longer, something is wrong
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	// Add a watchdog timer that logs if extraction is taking too long
	// This helps diagnose hangs without blocking
	watchdog := time.AfterFunc(30*time.Second, func() {
		fmt.Printf("[runner] WARNING: Memory extraction taking >30s for session=%s\n", sessionID)
	})
	defer watchdog.Stop()

	// Get recent messages from session
	messages, err := r.sessions.GetMessages(sessionID, 50) // Last 50 messages
	if err != nil || len(messages) < 2 {
		return // Not enough conversation to extract from
	}

	// Use the cheapest available model for memory extraction
	var extractionProvider ai.Provider
	var extractionModel string
	if r.selector != nil {
		cheapestModelID := r.selector.GetCheapestModel()
		fmt.Printf("[runner] GetCheapestModel returned: %q\n", cheapestModelID)
		if cheapestModelID != "" {
			providerID, modelName := ai.ParseModelID(cheapestModelID)
			if p, ok := r.providerMap[providerID]; ok {
				// Create a provider wrapper that uses the specific model
				extractionProvider = &modelOverrideProvider{
					Provider: p,
					model:    modelName,
				}
				extractionModel = cheapestModelID
			} else {
				fmt.Printf("[runner] Provider %q not in providerMap (available: %v)\n", providerID, r.getProviderIDs())
			}
		}
	} else {
		fmt.Printf("[runner] selector is nil\n")
	}
	// Fall back to first provider if we couldn't get the cheapest
	if extractionProvider == nil {
		extractionProvider = r.providers[0]
		extractionModel = "fallback/" + r.providers[0].ID()
	}
	fmt.Printf("[runner] Memory extraction using model: %s\n", extractionModel)

	// Create extractor and extract facts
	extractor := memory.NewExtractor(extractionProvider)
	facts, err := extractor.Extract(ctx, messages)
	if err != nil {
		fmt.Printf("[runner] Memory extraction failed: %v\n", err)
		return
	}

	if facts.IsEmpty() {
		return
	}

	// Store extracted facts using explicit userID (thread-safe, with dedup)
	entries := facts.FormatForStorage()
	stored, skipped := 0, 0
	for _, entry := range entries {
		var storeErr error
		if entry.IsStyle {
			// Style observations use reinforcement tracking — increment count on duplicates
			storeErr = r.memoryTool.StoreStyleEntryForUser(entry.Layer, entry.Namespace, entry.Key, entry.Value, entry.Tags, userID)
		} else {
			// Skip if identical value already stored (dedup)
			if r.memoryTool.IsDuplicate(entry.Layer, entry.Namespace, entry.Key, entry.Value, userID) {
				skipped++
				continue
			}
			storeErr = r.memoryTool.StoreEntryForUser(entry.Layer, entry.Namespace, entry.Key, entry.Value, entry.Tags, userID)
		}
		if storeErr != nil {
			fmt.Printf("[runner] Failed to store memory %s: %v\n", entry.Key, storeErr)
		} else {
			stored++
		}
	}

	durationMs := time.Since(startTime).Milliseconds()
	if stored > 0 || skipped > 0 {
		fmt.Printf("[runner] Auto-extracted %d memories, skipped %d duplicates (user: %s) in %dms\n", stored, skipped, userID, durationMs)
	} else {
		fmt.Printf("[runner] Memory extraction complete (no new memories) in %dms\n", durationMs)
	}

	// If styles were extracted, attempt personality directive synthesis
	if len(facts.Styles) > 0 && r.sessions != nil {
		if db := r.sessions.GetDB(); db != nil {
			directive, err := memory.SynthesizeDirective(ctx, db, extractionProvider, userID)
			if err != nil {
				fmt.Printf("[runner] Personality synthesis failed: %v\n", err)
			} else if directive != "" {
				fmt.Printf("[runner] Personality directive updated for user %s\n", userID)
			}
			// directive == "" means not enough observations yet — that's fine
		}
	}
}

// maybeRunMemoryFlush persists important memories before compaction discards messages.
// Called synchronously before Compact() to guarantee ordering.
// Returns true if a flush was performed.
// Deduplication: Only runs once per compaction cycle (tracked via session).
func (r *Runner) maybeRunMemoryFlush(ctx context.Context, sessionID, userID string, messages []session.Message) bool {
	tokens := estimateTokens(messages)
	flushThreshold := r.memoryFlushThreshold()
	if tokens < flushThreshold {
		return false
	}

	// Check if we should run flush for this compaction cycle
	// This prevents running flush multiple times for the same compaction
	if r.sessions != nil {
		shouldFlush, err := r.sessions.ShouldRunMemoryFlush(sessionID)
		if err != nil {
			fmt.Printf("[runner] Warning: failed to check memory flush status: %v\n", err)
		} else if !shouldFlush {
			fmt.Printf("[runner] Skipping memory flush (already ran for this compaction cycle)\n")
			return false
		}
	}

	fmt.Printf("[runner] Context at %d tokens (threshold: %d) - running proactive memory flush (session: %s)\n", tokens, flushThreshold, sessionID)

	// Run memory extraction immediately (not in background) to ensure it completes before compaction
	if r.memoryTool != nil && len(r.providers) > 0 {
		// Use a timeout for the flush operation
		flushCtx, cancel := context.WithTimeout(ctx, 45*time.Second)
		defer cancel()

		// Get cheapest model for flush
		var flushProvider ai.Provider
		if r.selector != nil {
			cheapestModelID := r.selector.GetCheapestModel()
			if cheapestModelID != "" {
				providerID, modelName := ai.ParseModelID(cheapestModelID)
				if p, ok := r.providerMap[providerID]; ok {
					flushProvider = &modelOverrideProvider{Provider: p, model: modelName}
				}
			}
		}
		if flushProvider == nil {
			flushProvider = r.providers[0]
		}

		// Create extractor and extract facts
		extractor := memory.NewExtractor(flushProvider)
		facts, err := extractor.Extract(flushCtx, messages)
		if err != nil {
			fmt.Printf("[runner] Memory flush extraction failed: %v\n", err)
			return false
		}

		if facts.IsEmpty() {
			fmt.Printf("[runner] Memory flush complete (no memories to store)\n")
		} else {
			// Store extracted facts (with dedup: skip if identical value already exists)
			entries := facts.FormatForStorage()
			stored, skipped := 0, 0
			for _, entry := range entries {
				if r.memoryTool.IsDuplicate(entry.Layer, entry.Namespace, entry.Key, entry.Value, userID) {
					skipped++
					continue
				}
				if err := r.memoryTool.StoreEntryForUser(entry.Layer, entry.Namespace, entry.Key, entry.Value, entry.Tags, userID); err != nil {
					fmt.Printf("[runner] Memory flush store failed for %s: %v\n", entry.Key, err)
				} else {
					stored++
				}
			}
			fmt.Printf("[runner] Memory flush: stored %d, skipped %d duplicates (before compaction)\n", stored, skipped)
		}

		// Record that we ran memory flush for this compaction cycle
		if r.sessions != nil {
			if err := r.sessions.RecordMemoryFlush(sessionID); err != nil {
				fmt.Printf("[runner] Warning: failed to record memory flush: %v\n", err)
			}
		}
	}

	return true
}

// detectUserModelSwitch checks the last user message for model switch requests
// Returns the matched model ID or empty string if no switch requested
func (r *Runner) detectUserModelSwitch(messages []session.Message) string {
	if r.fuzzyMatcher == nil {
		return ""
	}

	// Get the last user message
	var lastUserMessage string
	for i := len(messages) - 1; i >= 0; i-- {
		if messages[i].Role == "user" && messages[i].Content != "" {
			lastUserMessage = messages[i].Content
			break
		}
	}

	if lastUserMessage == "" {
		return ""
	}

	// Check for model switch patterns like "use claude", "switch to opus"
	modelRequest := ai.ParseModelRequest(lastUserMessage)
	if modelRequest == "" {
		return ""
	}

	// Use fuzzy matcher to resolve the model name
	return r.fuzzyMatcher.Match(modelRequest)
}

// getProviderIDs returns all provider IDs in the providerMap (for debugging)
func (r *Runner) getProviderIDs() []string {
	ids := make([]string, 0, len(r.providerMap))
	for id := range r.providerMap {
		ids = append(ids, id)
	}
	return ids
}

// recordProfileUsage records successful usage of a provider's auth profile
// This resets error count and updates usage stats
func (r *Runner) recordProfileUsage(ctx context.Context, provider ai.Provider) {
	if r.profileTracker == nil {
		return
	}
	profileID := provider.ProfileID()
	if profileID == "" {
		return // Provider doesn't have profile tracking
	}
	if err := r.profileTracker.RecordUsage(ctx, profileID); err != nil {
		fmt.Printf("[Runner] Warning: failed to record profile usage: %v\n", err)
	}
}

// recordProfileError records an error for a provider's auth profile
// This triggers cooldown with exponential backoff
// Also applies API error fingerprinting for deduplication
func (r *Runner) recordProfileError(ctx context.Context, provider ai.Provider, err error) {
	if r.profileTracker == nil {
		return
	}
	profileID := provider.ProfileID()
	if profileID == "" {
		return // Provider doesn't have profile tracking
	}

	// API error fingerprinting for deduplication
	// Creates a deterministic fingerprint of the error payload to detect duplicates
	errStr := err.Error()
	fingerprint := ai.GetAPIErrorPayloadFingerprint(errStr)
	isDuplicate := false
	if fingerprint != "" {
		isDuplicate = ai.IsRecentAPIError(fingerprint)
	}

	reason := ai.ClassifyErrorReason(err)
	if trackErr := r.profileTracker.RecordErrorWithCooldownString(ctx, profileID, reason); trackErr != nil {
		fmt.Printf("[Runner] Warning: failed to record profile error: %v\n", trackErr)
	}

	// Log with fingerprint info (suppress duplicate details)
	if isDuplicate {
		fmt.Printf("[Runner] Recorded duplicate error for profile %s: reason=%s fingerprint=%s\n",
			profileID, reason, ai.HashText(fingerprint)[:12])
	} else {
		fmt.Printf("[Runner] Recorded error for profile %s: reason=%s\n", profileID, reason)
	}
}

// estimateTokens estimates the token count for a slice of messages.
// Uses a simple heuristic: ~4 characters per token (works for most models).
func estimateTokens(messages []session.Message) int {
	totalChars := 0
	for _, msg := range messages {
		totalChars += len(msg.Content)
		totalChars += len(msg.ToolCalls)
		totalChars += len(msg.ToolResults)
	}
	// Rough estimate: 4 chars per token
	return totalChars / 4
}

// DefaultContextTokenLimit is the fallback max tokens before proactive compaction.
// Used when the active model's context window is unknown.
const DefaultContextTokenLimit = 80000

// DefaultMemoryFlushThreshold is the fallback token count for proactive memory flush.
// Used when the active model's context window is unknown.
const DefaultMemoryFlushThreshold = 60000

// contextTokenLimit returns the max tokens before proactive compaction triggers.
// Computed from the active model's context window (80% of usable context),
// falling back to DefaultContextTokenLimit if no model info is available.
func (r *Runner) contextTokenLimit() int {
	if r.selector == nil {
		return DefaultContextTokenLimit
	}

	// Get the general/default model (nil messages → TaskTypeGeneral)
	modelID := r.selector.Select(nil)
	if modelID == "" {
		return DefaultContextTokenLimit
	}

	info := r.selector.GetModelInfo(modelID)
	if info == nil || info.ContextWindow <= 0 {
		return DefaultContextTokenLimit
	}

	// Reserve tokens for system prompt, tool definitions, and response buffer
	const reserveTokens = 20000
	effective := info.ContextWindow - reserveTokens
	if effective <= DefaultContextTokenLimit {
		return DefaultContextTokenLimit
	}

	// Compact at 80% of effective context
	limit := effective * 80 / 100

	// Cap to avoid extremely long summarization tasks
	const maxLimit = 500000
	if limit > maxLimit {
		return maxLimit
	}

	return limit
}

// memoryFlushThreshold returns the token count at which memory flush triggers.
// Set to 75% of the compaction limit so flush runs before compaction discards messages.
func (r *Runner) memoryFlushThreshold() int {
	return r.contextTokenLimit() * 75 / 100
}

// MemoryFlushPrompt is the prompt sent to trigger a memory flush before compaction
const MemoryFlushPrompt = `Pre-compaction memory flush. The conversation is getting long and will soon be summarized.

IMPORTANT: Review the conversation and use the memory tool to store any important information that should be remembered long-term:
- The current active task or goal — what you are building/doing right now (layer: "daily", namespace: today's date, key: "active_task"). Store the COMPLETE description including technologies, user requirements, and current progress.
- User preferences or facts about them (layer: "tacit", namespace: "user")
- Important decisions or agreements (layer: "daily", namespace: today's date)
- Information about people, projects, or entities mentioned (layer: "entity", namespace: "default")
- Content you produced for the user — copy, headlines, plans, strategies, emails, code architecture (layer: "tacit", namespace: "artifacts"). Store the VERBATIM text, not a summary. The user will reference this later.

If there's nothing important to store, simply reply "NO_STORE_NEEDED" and nothing else.`

// injectSystemContext enriches the system prompt with runtime context
// so the AI knows what model it's running as, current time, etc.
func injectSystemContext(systemPrompt, providerID, modelName string) string {
	now := time.Now()

	// Get hostname
	hostname, err := os.Hostname()
	if err != nil {
		hostname = "unknown"
	}

	// Format OS name nicely
	osName := runtime.GOOS
	switch osName {
	case "darwin":
		osName = "macOS"
	case "linux":
		osName = "Linux"
	case "windows":
		osName = "Windows"
	}

	// Build context block
	contextBlock := fmt.Sprintf(`

---
[System Context]
Model: %s/%s
Date: %s
Time: %s
Timezone: %s
Computer: %s
OS: %s (%s)
---`,
		providerID, modelName,
		now.Format("Monday, January 2, 2006"),
		now.Format("3:04 PM"),
		now.Format("MST"),
		hostname,
		osName, runtime.GOARCH,
	)

	return systemPrompt + contextBlock
}

