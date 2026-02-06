package runner

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/nebolabs/nebo/internal/agent/ai"
	"github.com/nebolabs/nebo/internal/agent/config"
	"github.com/nebolabs/nebo/internal/agent/memory"
	"github.com/nebolabs/nebo/internal/agent/recovery"
	"github.com/nebolabs/nebo/internal/agent/session"
	"github.com/nebolabs/nebo/internal/agent/skills"
	"github.com/nebolabs/nebo/internal/agent/tools"
	"github.com/nebolabs/nebo/internal/lifecycle"
)

// DefaultSystemPrompt is the base system prompt (agent identity is prepended from DB)
const DefaultSystemPrompt = `You are Nebo, a local AI agent running on this computer. You are NOT Claude Code, Cursor, Copilot, or any other coding assistant. You have your own unique tool set described below. When a user asks what tools you have, ONLY list the tools described in this prompt and in your tool definitions — never list tools from your training data.

CRITICAL: Your ONLY tools are the ones listed below and provided in the tool definitions. You do NOT have "WebFetch", "WebSearch", "Read", "Write", "Edit", "Grep", "Glob", "Bash", "TodoWrite", "EnterPlanMode", "AskUserQuestion", "Task", or "Context7" as tools. Those do not exist in your runtime. If you reference or attempt to call a tool not in your tool definitions, it will fail. Your actual tools are: file, shell, web, agent, screenshot, vision, and platform capabilities.

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
The 'web' tool handles HTTP requests, web search, and FULL BROWSER automation (with JavaScript execution).

There are two modes:
- fetch/search: Simple HTTP requests and web search (no JavaScript, no rendering)
- navigate/snapshot/click/fill/etc.: FULL BROWSER with JavaScript, rendering, and login sessions

When a website requires JavaScript (Twitter/X, Gmail, most modern sites), you MUST use navigate — fetch will NOT work.

Profiles (for browser actions):
- profile: "nebo" (default) — Managed browser, isolated session
- profile: "chrome" — Chrome extension relay, access YOUR logged-in sessions (Gmail, Twitter, etc.)

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

IMPORTANT: When interacting with browser pages:
1. First use navigate to open the page, then snapshot to see available elements and their refs
2. Then use click/fill/type with the ref from the snapshot
3. Use profile: "chrome" to access the user's logged-in sessions (Twitter, Gmail, etc.)
4. For ANY website that uses JavaScript rendering, use navigate — NOT fetch

### agent — Orchestration & State
The 'agent' tool manages sub-agents, scheduling, memory, messaging, and sessions.

Sub-agents (parallel work):
- agent(resource: task, action: spawn, description: "...", instructions: "...") — Spawn a sub-agent goroutine
- agent(resource: task, action: status, id: "...") — Check sub-agent status
- agent(resource: task, action: cancel, id: "...") — Cancel a sub-agent
- agent(resource: task, action: list) — List active sub-agents

Cron (scheduled events):
- agent(resource: cron, action: create, schedule: "0 9 * * *", task: "Daily briefing") — Schedule recurring work
- agent(resource: cron, action: list) — List scheduled jobs
- agent(resource: cron, action: delete, id: "...") — Remove a scheduled job
- agent(resource: cron, action: pause, id: "...") — Pause a job
- agent(resource: cron, action: resume, id: "...") — Resume a paused job
- agent(resource: cron, action: run, id: "...") — Trigger a job immediately
- agent(resource: cron, action: history, id: "...") — View job run history

Memory (3-tier persistence):
- agent(resource: memory, action: store, key: "user/name", value: "Alice", layer: "tacit") — Store a fact
- agent(resource: memory, action: recall, query: "user preferences") — Recall relevant memories
- agent(resource: memory, action: search, query: "...") — Search memories
- agent(resource: memory, action: list) — List stored memories
- agent(resource: memory, action: delete, key: "...") — Delete a memory
- agent(resource: memory, action: clear) — Clear all memories
Memory layers: "tacit" (long-term preferences), "daily" (today's facts), "entity" (people/places/things)

Messaging (channel integrations):
- agent(resource: message, action: send, channel: "telegram", text: "Hello!") — Send a message
- agent(resource: message, action: list) — List available channels

Sessions:
- agent(resource: session, action: list) — List conversation sessions
- agent(resource: session, action: history, id: "...") — View session history
- agent(resource: session, action: status) — Current session status
- agent(resource: session, action: clear) — Clear current session

### advisors — Internal Deliberation
For complex decisions, call the 'advisors' tool. Advisors run concurrently and return counsel that YOU synthesize.
- advisors(task: "Should we use PostgreSQL or SQLite for this use case?") — Consult all enabled advisors
- advisors(task: "Best architecture for real-time notifications", advisors: ["pragmatist", "skeptic"]) — Consult specific advisors
- Use advisors for: significant decisions, multiple valid approaches, or when uncertain
- Skip advisors for: simple, routine, or time-sensitive tasks

### screenshot — Screen Capture
- screenshot() — Capture the current screen

### vision — Image Analysis
- vision(path: "/path/to/image.png") — Analyze an image (requires API key)

### Platform Capabilities (macOS)
These tools are available when running on macOS:
- calendar — Read/create calendar events and check availability
- contacts — Search and manage contacts
- mail — Read, send, and manage email
- reminders — Create and manage reminders and lists
- music — Control music playback (play, pause, skip, queue)
- clipboard — Read/write clipboard content
- notification — Display notifications, alerts, and text-to-speech
- spotlight — Search files and content via Spotlight
- shortcuts — Run Apple Shortcuts automations
- window — Manage window positions and sizes
- desktop — Desktop operations
- accessibility — Accessibility features and UI automation
- system — System controls (volume, brightness, Wi-Fi, Bluetooth, dark mode, sleep, lock)
- app — Launch and manage applications
- keychain — Securely store and retrieve credentials

## Memory System — CRITICAL

You have PERSISTENT MEMORY that survives across sessions. NEVER say "I don't have persistent memory" or "my memory doesn't carry over." Your memory tool WORKS — use it.

**Proactive memory use:**
- When the user mentions a fact about themselves (name, preferences, project details, etc.), STORE IT immediately using agent(resource: memory, action: store, ...)
- When the user asks "do you remember...?" or references past conversations, SEARCH memory first: agent(resource: memory, action: search, query: "...")
- When unsure about user preferences or context, RECALL from memory before asking
- NEVER claim you can't remember something without first calling the memory tool to check

**Memory layers:**
- "tacit" — Long-term preferences, personal facts, learned behaviors (MOST COMMON)
- "daily" — Today's facts, keyed by date (auto-expires)
- "entity" — Information about people, places, projects, things

**Key facts to always store (layer: tacit, namespace: user):**
- User's name, location, timezone, occupation
- Preferred communication style
- Project names and details they frequently reference
- Tools, languages, and tech stacks they use
- Important dates or deadlines

Your remembered facts (if any) appear in the "# Remembered Facts" section of your context. Those were loaded from your memory database — proof that your memory works.

## Behavioral Guidelines
1. Break complex tasks into smaller steps and use tools to gather info before acting
2. If you encounter errors, analyze them and try alternative approaches
3. For scheduled/recurring work, use agent(resource: cron, action: create, ...)
4. For parallel work, spawn sub-agents with agent(resource: task, action: spawn, ...)
5. ALWAYS store important user facts in memory — use agent(resource: memory, action: store, ...) immediately when you learn something new
6. To notify the user on another channel, use agent(resource: message, action: send, ...)
7. Always verify your changes work before considering a task complete
8. When asked about past interactions or the user's preferences, ALWAYS search memory first — never guess or say "I don't remember"`

// ProviderLoaderFunc is a function that loads providers (for dynamic reload)
type ProviderLoaderFunc func() []ai.Provider

// Runner executes the agentic loop
type Runner struct {
	sessions        *session.Manager
	providers       []ai.Provider
	providerLoader  ProviderLoaderFunc // Called to reload providers if empty
	providerMap     map[string]ai.Provider // providerID -> Provider for model-based switching
	tools           *tools.Registry
	config          *config.Config
	skillLoader     *skills.Loader
	memoryTool      *tools.MemoryTool
	selector        *ai.ModelSelector
	fuzzyMatcher    *ai.FuzzyMatcher    // For user model switch requests
	profileTracker  ai.ProfileTracker   // For recording usage/errors per auth profile (moltbot pattern)
}

// RunRequest contains parameters for a run
type RunRequest struct {
	SessionKey       string // Session identifier (uses "default" if empty)
	Prompt           string // User prompt
	System           string // Override system prompt
	ModelOverride    string // User-specified model override (e.g., "anthropic/claude-opus-4-5")
	UserID           string // User ID for user-scoped operations (sessions, memories)
	SkipMemoryExtract bool   // Skip auto memory extraction (e.g., for heartbeats)
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
	// Load skills from extensions/skills directory (in working directory)
	// Also load user-installed skills from data directory
	skillLoader := skills.NewLoader(filepath.Join("extensions", "skills"))
	if err := skillLoader.LoadAll(); err != nil {
		// Log error but continue - skills are optional
		fmt.Printf("[runner] Warning: failed to load skills: %v\n", err)
	}

	// Also load user skills from data directory
	userSkillsDir := filepath.Join(cfg.DataDir, "skills")
	userSkillLoader := skills.NewLoader(userSkillsDir)
	if err := userSkillLoader.LoadAll(); err == nil {
		// Merge user skills into main loader
		for _, skill := range userSkillLoader.List() {
			skillLoader.Add(skill)
		}
	}

	// Load disabled skills from settings file (if exists)
	// This syncs the runner with UI-configured skill states
	disabledSkills := loadDisabledSkills(cfg.DataDir)
	if len(disabledSkills) > 0 {
		skillLoader.SetDisabledSkills(disabledSkills)
	}

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
		skillLoader: skillLoader,
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
// This enables moltbot-style cooldown and usage tracking
func (r *Runner) SetProfileTracker(tracker ai.ProfileTracker) {
	r.profileTracker = tracker
}

// SetupSubagentPersistence configures subagent recovery for restart survival (moltbot pattern)
// This enables the orchestrator to persist subagent runs and recover them after restart
func (r *Runner) SetupSubagentPersistence(mgr *recovery.Manager) {
	if r.tools == nil {
		return
	}
	if taskTool := r.tools.GetTaskTool(); taskTool != nil {
		taskTool.SetRecoveryManager(mgr)
	}
}

// RecoverSubagents restores pending subagent tasks from the database (moltbot pattern)
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

// loadDisabledSkills reads the skill-settings.json file and returns disabled skill names
func loadDisabledSkills(dataDir string) []string {
	settingsPath := filepath.Join(dataDir, "skill-settings.json")
	data, err := os.ReadFile(settingsPath)
	if err != nil {
		return nil
	}

	var settings struct {
		DisabledSkills []string `json:"disabledSkills"`
	}
	if err := json.Unmarshal(data, &settings); err != nil {
		return nil
	}

	return settings.DisabledSkills
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

// SetProviderLoader sets the function to reload providers (for dynamic reload after onboarding)
func (r *Runner) SetProviderLoader(loader ProviderLoaderFunc) {
	r.providerLoader = loader
}

// ReloadProviders attempts to reload providers from the loader function
func (r *Runner) ReloadProviders() {
	if r.providerLoader != nil {
		r.providers = r.providerLoader()
	}
}

// SkillLoader returns the skill loader for managing skills
func (r *Runner) SkillLoader() *skills.Loader {
	return r.skillLoader
}

// Run executes the agentic loop
func (r *Runner) Run(ctx context.Context, req *RunRequest) (<-chan ai.StreamEvent, error) {
	fmt.Printf("[Runner] Run: session=%s\n", req.SessionKey)

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
	go r.runLoop(ctx, sess.ID, req.System, req.ModelOverride, req.UserID, req.SkipMemoryExtract, resultCh)

	return resultCh, nil
}

// runLoop is the main agentic execution loop
func (r *Runner) runLoop(ctx context.Context, sessionID, systemPrompt, modelOverride, userID string, skipMemoryExtract bool, resultCh chan<- ai.StreamEvent) {
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
		contextSection = "# Identity\n\nYou are Nebo, a personal AI assistant. You are NOT Claude, ChatGPT, or any other AI brand — always introduce yourself as Nebo."
	}

	// If user needs onboarding, add proactive onboarding instructions
	if needsOnboarding {
		contextSection += `

## IMPORTANT: First-Time User Onboarding

This is a NEW USER who hasn't been onboarded yet. You MUST start by introducing yourself and getting to know them.

Start your FIRST response with a friendly greeting and ask their name:
"Hey! I'm Nebo, your personal AI assistant. I'm here to help with whatever you need. Before we dive in, what should I call you?"

After they respond, continue the conversation naturally to learn:
1. Their name (what to call them)
2. Their location/timezone
3. What they do (occupation)
4. What they'd like help with most
5. Their preferred communication style (casual or professional)

Use the memory tool to store each piece of information as you learn it. Use layer="tacit" and namespace="user":
- Store name: {"action": "store", "layer": "tacit", "namespace": "user", "key": "name", "value": "Their Name"}
- Store location: {"action": "store", "layer": "tacit", "namespace": "user", "key": "location", "value": "Their Location"}
- Store occupation: {"action": "store", "layer": "tacit", "namespace": "user", "key": "occupation", "value": "Their Role"}
- Store goals: {"action": "store", "layer": "tacit", "namespace": "user", "key": "goals", "value": "What they want help with"}
- Store style: {"action": "store", "layer": "tacit", "namespace": "user", "key": "communication_style", "value": "casual|professional|adaptive"}

Be warm and conversational - ask ONE question at a time, acknowledge their answers, then naturally transition to the next topic. This is a friendly chat, not an interview!`
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

	systemPrompt = contextSection + "\n\n---\n\n" + systemPrompt

	// Add model aliases section so agent knows what models are available
	if r.fuzzyMatcher != nil {
		aliases := r.fuzzyMatcher.GetAliases()
		if len(aliases) > 0 {
			systemPrompt += "\n\n## Model Switching\n\nUsers can ask to switch models. Available models:\n" + strings.Join(aliases, "\n") + "\n\nWhen a user asks to switch models, acknowledge the request and confirm the switch."
		}
	}

	// Apply matching skills based on the user's last message
	if r.skillLoader != nil {
		// Get the last user message to match against skills
		messages, _ := r.sessions.GetMessages(sessionID, r.config.MaxContext)
		var lastUserInput string
		for i := len(messages) - 1; i >= 0; i-- {
			if messages[i].Role == "user" && messages[i].Content != "" {
				lastUserInput = messages[i].Content
				break
			}
		}
		if lastUserInput != "" {
			systemPrompt = r.skillLoader.ApplyMatchingSkills(systemPrompt, lastUserInput)
		}
	}

	// Final tool awareness fence — placed at the very end of the system prompt
	// so the LLM sees it last (recency bias helps reinforce the message)
	if len(toolDefs) > 0 {
		toolNames := make([]string, len(toolDefs))
		for i, td := range toolDefs {
			toolNames[i] = td.Name
		}
		systemPrompt += "\n\n---\nREMINDER: You are Nebo. Your ONLY tools are: " + strings.Join(toolNames, ", ") + ". When a user asks about your capabilities, describe these tools. Never mention tools from your training data that are not in this list."
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

		if estimatedTokens > DefaultContextTokenLimit && !compactionAttempted {
			fmt.Printf("[Runner] Token limit exceeded (~%d tokens), compacting...\n", estimatedTokens)
			compactionAttempted = true

			// Run proactive memory flush before compaction (moltbot pattern)
			// This ensures important memories are persisted before being summarized
			r.maybeRunMemoryFlush(ctx, sessionID, userID, messages)

			summary := r.generateSummary(ctx, messages)
			if compactErr := r.sessions.Compact(sessionID, summary); compactErr == nil {
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

		// If session has a compaction summary, inject it for continuity
		if summary, _ := r.sessions.GetSummary(sessionID); summary != "" {
			enrichedPrompt = enrichedPrompt + "\n\n---\n[Previous Conversation Summary]\n" + summary + "\n---"
		}

		// Two-stage context pruning: soft trim (head+tail) then hard clear (placeholder)
		truncatedMessages := pruneContext(messages, r.config.ContextPruning)

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

					// Run proactive memory flush before compaction (moltbot pattern)
					// This ensures important memories are persisted before being summarized
					r.maybeRunMemoryFlush(ctx, sessionID, userID, messages)

					// Compact session and retry
					fmt.Printf("[Runner] Context overflow - attempting compaction\n")
					summary := r.generateSummary(ctx, messages)
					compactErr := r.sessions.Compact(sessionID, summary)
					if compactErr == nil {
						continue // Retry with compacted session
					}
					fmt.Printf("[Runner] Compaction failed: %v\n", compactErr)
				}
				// Compaction already attempted or failed - notify user (moltbot pattern: never auto-reset)
				fmt.Printf("[Runner] Context overflow after compaction attempt\n")
				resultCh <- ai.StreamEvent{
					Type: ai.EventTypeText,
					Text: "⚠️ Context overflow: prompt too large for this model. Try again with less input or use `/session reset` to start fresh.",
				}
				resultCh <- ai.StreamEvent{Type: ai.EventTypeDone}
				return
			}
			if ai.IsRateLimitOrAuth(err) {
				// Record error for profile cooldown (moltbot pattern)
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
			// Record error for profile tracking (moltbot pattern) - generic error case
			r.recordProfileError(ctx, provider, err)
			resultCh <- ai.StreamEvent{Type: ai.EventTypeError, Error: err}
			return
		}

		// Process streaming events
		hasToolCalls := false
		cliProviderComplete := false // Track if CLI provider completed its full agentic loop
		var assistantContent strings.Builder
		var toolCalls []session.ToolCall
		eventCount := 0

		for event := range events {
			eventCount++

			// Forward event to caller (except internal done signals)
			if !(event.Type == ai.EventTypeDone && event.Text == "cli_complete") {
				resultCh <- event
			}

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

			case ai.EventTypeDone:
				// Check for CLI provider completion signal
				if event.Text == "cli_complete" {
					cliProviderComplete = true
				}

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
		// Skip if CLI provider completed — messages were already saved via EventTypeMessage
		if !cliProviderComplete && (assistantContent.Len() > 0 || len(toolCalls) > 0) {
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

		// Execute tool calls (skip if CLI provider already executed them)
		if hasToolCalls && !cliProviderComplete {
			var toolResults []session.ToolResult

			for _, tc := range toolCalls {
				fmt.Printf("[Runner] Executing tool: %s\n", tc.Name)
				result := r.tools.Execute(ctx, &ai.ToolCall{
					ID:    tc.ID,
					Name:  tc.Name,
					Input: tc.Input,
				})

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
					Content:    result.Content,
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
		} else if hasToolCalls && cliProviderComplete {
			fmt.Printf("[Runner] Skipping tool execution - CLI provider already executed %d tools\n", len(toolCalls))
			// Fall through to done - CLI provider already completed its agentic loop
		}

		// No tool calls (or text-only response) - task is complete
		// Record successful usage for profile tracking (moltbot pattern)
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

// generateSummary creates a summary of the conversation for compaction.
// Includes compaction safeguard: tool failures are preserved in the summary
// so the agent knows what went wrong even after context is compacted.
// (moltbot pattern: compaction-safeguard.ts)
func (r *Runner) generateSummary(_ context.Context, messages []session.Message) string {
	// Build base summary with key conversation points
	var summary strings.Builder
	summary.WriteString("[Previous conversation summary]\n")

	// Extract key points from messages
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

	// Apply compaction safeguard: append tool failures section
	// This ensures the agent knows about errors that occurred before compaction
	return EnhancedSummary(messages, summary.String())
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
// This follows the moltbot pattern: fire-and-forget, fully non-blocking, with error recovery
func (r *Runner) extractAndStoreMemories(sessionID, userID string) {
	// Capture start time for logging
	startTime := time.Now()

	// Recover from any panics to avoid crashing the main goroutine
	defer func() {
		if r := recover(); r != nil {
			fmt.Printf("[runner] Memory extraction PANIC recovered: %v\n", r)
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

	// Store extracted facts using explicit userID (thread-safe)
	entries := facts.FormatForStorage()
	stored := 0
	for _, entry := range entries {
		if err := r.memoryTool.StoreEntryForUser(entry.Layer, entry.Namespace, entry.Key, entry.Value, entry.Tags, userID); err != nil {
			fmt.Printf("[runner] Failed to store memory %s: %v\n", entry.Key, err)
		} else {
			stored++
		}
	}

	durationMs := time.Since(startTime).Milliseconds()
	if stored > 0 {
		fmt.Printf("[runner] Auto-extracted %d memories from conversation (user: %s) in %dms\n", stored, userID, durationMs)
	} else {
		fmt.Printf("[runner] Memory extraction complete (no new memories) in %dms\n", durationMs)
	}
}

// maybeRunMemoryFlush checks if the context is approaching the limit and runs
// a proactive memory flush to persist important memories before compaction.
// Returns true if a flush was performed. (moltbot pattern: memory-flush.ts)
// Deduplication: Only runs once per compaction cycle using session tracking.
func (r *Runner) maybeRunMemoryFlush(ctx context.Context, sessionID, userID string, messages []session.Message) bool {
	tokens := estimateTokens(messages)
	if tokens < MemoryFlushThreshold {
		return false
	}

	// Check if we should run flush for this compaction cycle (moltbot pattern)
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

	fmt.Printf("[runner] Context at %d tokens (threshold: %d) - running proactive memory flush (session: %s)\n", tokens, MemoryFlushThreshold, sessionID)

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
			// Store extracted facts
			entries := facts.FormatForStorage()
			stored := 0
			for _, entry := range entries {
				if err := r.memoryTool.StoreEntryForUser(entry.Layer, entry.Namespace, entry.Key, entry.Value, entry.Tags, userID); err != nil {
					fmt.Printf("[runner] Memory flush store failed for %s: %v\n", entry.Key, err)
				} else {
					stored++
				}
			}
			fmt.Printf("[runner] Memory flush stored %d memories before compaction\n", stored)
		}

		// Record that we ran memory flush for this compaction cycle (moltbot pattern)
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
// This resets error count and updates usage stats (moltbot pattern)
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
// This triggers cooldown with exponential backoff (moltbot pattern)
// Also applies API error fingerprinting for deduplication (moltbot pattern: dedup.ts)
func (r *Runner) recordProfileError(ctx context.Context, provider ai.Provider, err error) {
	if r.profileTracker == nil {
		return
	}
	profileID := provider.ProfileID()
	if profileID == "" {
		return // Provider doesn't have profile tracking
	}

	// API error fingerprinting (moltbot pattern)
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

// DefaultContextTokenLimit is the default max tokens before proactive compaction.
// Set conservatively to work with smaller context windows (gpt-5-nano, haiku, etc.)
const DefaultContextTokenLimit = 6000

// MemoryFlushThreshold is the token count at which we trigger a proactive memory flush
// before compaction. This ensures critical memories are persisted before being summarized.
// (moltbot pattern: memory-flush.ts)
const MemoryFlushThreshold = 4500

// MemoryFlushPrompt is the prompt sent to trigger a memory flush before compaction
const MemoryFlushPrompt = `Pre-compaction memory flush. The conversation is getting long and will soon be summarized.

IMPORTANT: Review the conversation and use the memory tool to store any important information that should be remembered long-term:
- User preferences or facts about them (layer: "tacit", namespace: "user")
- Important decisions or agreements (layer: "daily", namespace: today's date)
- Information about people, projects, or entities mentioned (layer: "entity", namespace: "default")

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

