package runner

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/neboloop/nebo/internal/agent/afv"
	"github.com/neboloop/nebo/internal/agent/ai"
	"github.com/neboloop/nebo/internal/agent/config"
	"github.com/neboloop/nebo/internal/agent/memory"
	"github.com/neboloop/nebo/internal/agent/recovery"
	"github.com/neboloop/nebo/internal/agent/session"
	"github.com/neboloop/nebo/internal/agent/steering"
	"github.com/neboloop/nebo/internal/agent/tools"
	"github.com/neboloop/nebo/internal/crashlog"
	"github.com/neboloop/nebo/internal/lifecycle"
	"github.com/neboloop/nebo/internal/provider"
)

// DefaultSystemPrompt is kept as a convenience reference and for tests.
// The actual prompt assembly uses BuildStaticPrompt() + BuildDynamicSuffix()
// defined in prompt.go, which produce a two-tier prompt optimized for caching.
const DefaultSystemPrompt = sectionIdentityAndPrime

// ProviderLoaderFunc is a function that loads providers (for dynamic reload)
type ProviderLoaderFunc func() []ai.Provider

// SkillProvider provides skill matching and invoked skill content for a session.
// Implemented by SkillDomainTool to avoid circular imports.
type SkillProvider interface {
	ActiveSkillContent(sessionKey string) string
	AutoMatchSkills(sessionKey, message string) string // returns brief match hints for system prompt
	ForceLoadSkill(sessionKey, skillName string) bool  // pre-load a skill into the session; returns true if found
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
	steering        *steering.Pipeline   // Mid-conversation steering message generator
	fileTracker     *FileAccessTracker   // Tracks file reads for post-compaction re-injection
	rateLimitStore      func(*ai.RateLimitInfo)  // Callback to publish latest rate-limit snapshot
	extractingMemory    sync.Map          // sessionID → true: prevents overlapping extractions
	detectingObjective  sync.Map          // sessionID → true: prevents overlapping detections
	memoryTimers        sync.Map          // sessionID → *time.Timer: debounced extraction
	cachedThresholds    *ContextThresholds // Cached per-run to avoid redundant model selection
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
	Channel          string       // Source channel: "web", "cli", "telegram", "discord", "slack" (default "web")
	ForceSkill       string       // Force-load a specific skill into the session (e.g., "introduction")
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

	r := &Runner{
		sessions:    sessions,
		providers:   providers,
		providerMap: providerMap,
		tools:       toolRegistry,
		config:      cfg,
		quarantine:  afv.NewQuarantineStore(),
		steering:    steering.New(),
		fileTracker: NewFileAccessTracker(),
	}

	// Wire file access tracking into the file tool
	if fileTool := toolRegistry.GetFileTool(); fileTool != nil {
		fileTool.OnFileRead = func(path string) { r.fileTracker.Track(path) }
	}

	return r
}

// SetModelSelector sets the model selector for task-based model routing.
// Also syncs loaded provider IDs to the selector so it only considers
// models from providers that actually have loaded instances.
func (r *Runner) SetModelSelector(selector *ai.ModelSelector) {
	r.selector = selector
	// Always sync loaded providers — even if empty. An empty-but-initialized
	// map means "loading ran, found nothing" vs nil meaning "not initialized yet".
	var allIDs []string
	for id := range r.providerMap {
		allIDs = append(allIDs, id)
	}
	selector.SetLoadedProviders(allIDs)
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

// SetRateLimitStore sets a callback to publish rate-limit snapshots.
// Called from agent.go to wire up svcCtx.JanusUsage.Store.
func (r *Runner) SetRateLimitStore(fn func(*ai.RateLimitInfo)) {
	r.rateLimitStore = fn
}

// SetProviderLoader sets the function to reload providers (for dynamic reload after onboarding)
func (r *Runner) SetProviderLoader(loader ProviderLoaderFunc) {
	r.providerLoader = loader
}

// ReloadProviders attempts to reload providers from the loader function.
// Also rebuilds the providerMap so new providers (e.g., gateway apps) are routable,
// and syncs runtime provider IDs to the model selector so routing honors them.
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

		// Tell selector which providers are actually loaded so it only
		// considers models from providers with real instances. This prevents
		// phantom matches against credential placeholders in models.yaml.
		if r.selector != nil {
			var allIDs []string
			var runtimeIDs []string
			config := r.selector.GetConfig()
			for id := range providerMap {
				allIDs = append(allIDs, id)
				// Runtime providers bypass credentials check (Janus, gateway apps)
				if config != nil && config.Credentials != nil {
					if _, hasCreds := config.Credentials[id]; hasCreds {
						continue
					}
				}
				if provider.IsCLIProvider(id) {
					continue
				}
				runtimeIDs = append(runtimeIDs, id)
			}
			r.selector.SetLoadedProviders(allIDs)
			r.selector.SetRuntimeProviders(runtimeIDs)
		}
	}
}

// Run executes the agentic loop
func (r *Runner) Run(ctx context.Context, req *RunRequest) (<-chan ai.StreamEvent, error) {
	fmt.Printf("[Runner] Run: session=%s origin=%s\n", req.SessionKey, req.Origin)

	// Inject origin into context so tools can check it via GetOrigin(ctx)
	if req.Origin != "" {
		ctx = tools.WithOrigin(ctx, req.Origin)
	}

	// If no providers, try to reload (user may have added API key via onboarding).
	// Must use ReloadProviders() to also rebuild providerMap and sync loadedProviders
	// to the selector — otherwise model routing can't find the new provider.
	if len(r.providers) == 0 {
		r.ReloadProviders()
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

	// Default channel to "web" if not specified
	channel := req.Channel
	if channel == "" {
		channel = "web"
	}

	// Background objective detection: classify user message to set/update/clear active task.
	// Fires before runLoop so the objective is available by iteration 2+.
	if req.Prompt != "" && len(req.Prompt) >= 20 && !req.SkipMemoryExtract {
		go r.detectAndSetObjective(sess.ID, req.SessionKey, req.Prompt)
	}

	resultCh := make(chan ai.StreamEvent, 100)
	go r.runLoop(ctx, sess.ID, req.SessionKey, req.System, req.ModelOverride, req.UserID, req.Prompt, channel, req.SkipMemoryExtract, req.ForceSkill, resultCh)

	return resultCh, nil
}

// runLoop is the main agentic execution loop
func (r *Runner) runLoop(ctx context.Context, sessionID, sessionKey, systemPrompt, modelOverride, userID, userPrompt, channel string, skipMemoryExtract bool, forceSkill string, resultCh chan<- ai.StreamEvent) {
	startTime := time.Now()
	r.cachedThresholds = nil // Fresh thresholds for each run
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

	// --- Build system prompt using section-based builder (prompt.go) ---
	// Static sections are assembled once here and reused across all iterations.
	// Dynamic sections (date, model, active task, summary) are appended per iteration.

	// Step 1: Load memory context from database
	var contextSection string
	dbContext, err := memory.LoadContext(r.sessions.GetDB(), userID)
	needsOnboarding := false
	if err == nil {
		contextSection = dbContext.FormatForSystemPrompt()
		needsOnboarding = dbContext.NeedsOnboarding()
	} else {
		// Fall back to file-based context (AGENTS.md, MEMORY.md, SOUL.md)
		workspaceDir, _ := os.Getwd()
		memoryFiles := memory.LoadMemoryFiles(workspaceDir)
		if !memoryFiles.IsEmpty() {
			contextSection = memoryFiles.FormatForSystemPrompt()
		}
		needsOnboarding = true
	}
	if contextSection == "" {
		contextSection = "# Identity\n\nYou are {agent_name}, a personal desktop AI companion. You are NOT Claude, ChatGPT, or any other AI brand — always introduce yourself as {agent_name}."
	}

	// Step 2: Resolve agent name
	agentName := "Nebo"
	if dbContext != nil && dbContext.AgentName != "" {
		agentName = dbContext.AgentName
	}

	// Step 3: Collect tool names
	toolDefs := r.tools.List()
	toolNames := make([]string, len(toolDefs))
	for i, td := range toolDefs {
		toolNames[i] = td.Name
	}

	// Step 4: Collect optional prompt inputs
	var skillHints, activeSkills, appCatalog string
	var modelAliases []string

	// Force-load a skill if explicitly requested (e.g., introduction on first launch),
	// or fall back to auto-loading introduction for users who haven't completed onboarding.
	if r.skillProvider != nil {
		if forceSkill != "" {
			r.skillProvider.ForceLoadSkill(sessionKey, forceSkill)
		} else if needsOnboarding {
			r.skillProvider.ForceLoadSkill(sessionKey, "introduction")
		}
	}

	if r.skillProvider != nil && userPrompt != "" {
		skillHints = r.skillProvider.AutoMatchSkills(sessionKey, userPrompt)
	}
	if r.skillProvider != nil {
		activeSkills = r.skillProvider.ActiveSkillContent(sessionKey)
	}
	if r.appCatalog != nil {
		appCatalog = r.appCatalog.AppCatalog()
	}
	if r.fuzzyMatcher != nil {
		modelAliases = r.fuzzyMatcher.GetAliases()
	}

	// Step 5: Build the static (cacheable) system prompt
	pctx := PromptContext{
		AgentName:      agentName,
		DBContext:       dbContext,
		ContextSection: contextSection,
		ToolNames:      toolNames,
		SkillHints:     skillHints,
		ActiveSkills:   activeSkills,
		AppCatalog:     appCatalog,
		ModelAliases:   modelAliases,
		FenceStore:     fenceStore,
	}

	if systemPrompt == "" {
		systemPrompt = BuildStaticPrompt(pctx)
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

		// Graduated context thresholds: Warning → Error → AutoCompact
		thresholds := r.contextThresholds()
		estimatedTokens := estimateTokens(messages)

		// Error tier: log warning about context size
		if estimatedTokens > thresholds.Error {
			fmt.Printf("[Runner] Context getting large: ~%d tokens (error threshold: %d)\n", estimatedTokens, thresholds.Error)
		}

		// AutoCompact tier: trigger full compaction.
		// Nebo has ONE eternal conversation — it must always be able to continue.
		// If context exceeds the threshold, compact. If still too large after
		// compaction, compact again more aggressively (fewer kept messages).
		if estimatedTokens > thresholds.AutoCompact {
			fmt.Printf("[Runner] Token limit exceeded (~%d tokens, limit: %d), compacting...\n", estimatedTokens, thresholds.AutoCompact)

			// Only flush memory on the first compaction attempt per run
			if !compactionAttempted {
				r.maybeRunMemoryFlush(context.WithoutCancel(ctx), sessionID, userID, messages)
			}
			compactionAttempted = true

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

			// Progressive compaction: try keeping 10, then 3, then 1 message(s).
			// Nebo has ONE eternal conversation — it must always continue.
			for _, keep := range []int{10, 3, 1} {
				if compactErr := r.sessions.Compact(sessionID, summary, keep); compactErr != nil {
					fmt.Printf("[Runner] Compaction (keep=%d) failed: %v\n", keep, compactErr)
					break
				}
				// Index compacted messages for semantic search
				if r.memoryTool != nil {
					go func() {
						indexCtx, indexCancel := context.WithTimeout(context.Background(), 60*time.Second)
						defer indexCancel()
						r.memoryTool.IndexSessionTranscript(indexCtx, sessionID, userID)
					}()
				}
				// Reload messages after compaction
				messages, err = r.sessions.GetMessages(sessionID, r.config.MaxContext)
				if err != nil {
					resultCh <- ai.StreamEvent{Type: ai.EventTypeError, Error: err}
					return
				}
				newTokens := estimateTokens(messages)
				fmt.Printf("[Runner] After compaction (keep=%d): %d messages, ~%d tokens\n", keep, len(messages), newTokens)

				if newTokens <= thresholds.AutoCompact {
					// Re-inject recently accessed files to recover working context
					if reinjectMsg := buildFileReinjectionMessage(r.fileTracker); reinjectMsg != nil {
						messages = append(messages, *reinjectMsg)
					}
					r.fileTracker.Clear()
					break
				}
			}
			// Never block — proceed with whatever context we have
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
				if p, ok := r.providerMap[providerID]; ok {
					provider = p
				}
			}
		}

		// Fall back to first provider when the selector returned nothing usable.
		// This handles clean installs where only Janus is configured but
		// default task routing points to anthropic/openai models.
		if provider == nil && len(r.providers) > 0 && selectedModel == "" {
			provider = r.providers[0]
			modelName = ""
		}

		if provider == nil {
			var errorMsg string
			if selectedModel != "" {
				providerID, _ := ai.ParseModelID(selectedModel)
				fmt.Printf("[Runner] Provider %s selected but not loaded (available: %v)\n", providerID, r.getProviderIDs())
				errorMsg = fmt.Sprintf("The model provider **%s** is configured but not available right now. "+
					"Please check **Settings > Providers** to make sure it's connected.", providerID)
			} else {
				errorMsg = "I'm not fully set up yet! To start chatting, connect a provider:\n\n" +
					"1. Go to **Settings > Providers**\n" +
					"2. Activate a provider (Janus is the easiest — one click)\n" +
					"3. Come back here and say hello!"
			}
			// Save error response to session so it survives page refresh/reload
			_ = r.sessions.AppendMessage(sessionID, session.Message{
				SessionID: sessionID,
				Role:      "assistant",
				Content:   errorMsg,
			})
			resultCh <- ai.StreamEvent{Type: ai.EventTypeText, Text: errorMsg}
			resultCh <- ai.StreamEvent{Type: ai.EventTypeDone}
			return
		}

		// Build per-iteration dynamic suffix (date/time, model context, active task, summary)
		activeTask, _ := r.sessions.GetActiveTask(sessionID)
		summaryText, _ := r.sessions.GetSummary(sessionID)
		dynamicSuffix := BuildDynamicSuffix(DynamicContext{
			ProviderID: provider.ID(),
			ModelName:  modelName,
			ActiveTask: activeTask,
			Summary:    summaryText,
		})

		// Refresh active skill content (may have changed if model invoked a skill)
		if r.skillProvider != nil {
			if updated := r.skillProvider.ActiveSkillContent(sessionKey); updated != activeSkills {
				activeSkills = updated
				pctx.ActiveSkills = activeSkills
				systemPrompt = BuildStaticPrompt(pctx)
			}
		}

		enrichedPrompt := systemPrompt + dynamicSuffix

		// Warning tier: micro-compact silently trims old tool results + strips images.
		// Runs before the two-stage pruning. Only activates above the warning threshold.
		messages, _ = microCompact(messages, thresholds.Warning)

		// Two-stage context pruning: soft trim (head+tail) then hard clear (placeholder).
		// Override ContextTokens with the actual model-derived threshold.
		pruningCfg := r.config.ContextPruning
		pruningCfg.ContextTokens = thresholds.AutoCompact
		truncatedMessages := pruneContext(messages, pruningCfg)

		// Mid-conversation steering: generate ephemeral guidance messages
		if r.steering != nil {
			// Gather work tasks from AgentDomainTool for steering context
			var workTasks []steering.WorkTask
			if agentTool := r.tools.GetAgentDomainTool(); agentTool != nil {
				for _, wt := range agentTool.ListWorkTasks(sessionKey) {
					workTasks = append(workTasks, steering.WorkTask{
						ID: wt.ID, Subject: wt.Subject, Status: wt.Status,
					})
				}
			}
			steeringCtx := &steering.Context{
				SessionID:      sessionID,
				Messages:       truncatedMessages,
				UserPrompt:     userPrompt,
				ActiveTask:     activeTask,
				Channel:        channel,
				AgentName:      agentName,
				Iteration:      iteration,
				JustCompacted:  compactionAttempted,
				RunStartTime:   startTime,
				WorkTasks:      workTasks,
				JanusRateLimit: r.latestRateLimit(provider),
			}
			if steeringMsgs := r.steering.Generate(steeringCtx); len(steeringMsgs) > 0 {
				truncatedMessages = steering.Inject(truncatedMessages, steeringMsgs)
			}
		}

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

		// Strip fence markers from context before sending to LLM.
		// Fences served their purpose (AFV verification passed above).
		// Removing them prevents the model from seeing and echoing them.
		if fenceStore.Count() > 0 {
			stripFencesFromMessages(truncatedMessages)
		}

		// Always send all registered tools — never filter by skill restrictions
		chatTools := r.tools.List()

		// Build chat request
		chatReq := &ai.ChatRequest{
			Messages: truncatedMessages,
			Tools:    chatTools,
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
				fmt.Printf("[Runner] Context overflow - progressive compaction\n")

				// Only flush memory on the first overflow per run
				if !compactionAttempted {
					r.maybeRunMemoryFlush(context.WithoutCancel(ctx), sessionID, userID, messages)
				}

				// Determine starting keep count: skip 10 if we already compacted
				keepCounts := []int{10, 3, 1}
				if compactionAttempted {
					keepCounts = []int{3, 1}
				}
				compactionAttempted = true

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

				for _, keep := range keepCounts {
					if compactErr := r.sessions.Compact(sessionID, summary, keep); compactErr != nil {
						fmt.Printf("[Runner] Overflow compaction (keep=%d) failed: %v\n", keep, compactErr)
						break
					}
					// Index compacted messages for semantic search
					if r.memoryTool != nil {
						go func() {
							indexCtx, indexCancel := context.WithTimeout(context.Background(), 60*time.Second)
							defer indexCancel()
							r.memoryTool.IndexSessionTranscript(indexCtx, sessionID, userID)
						}()
					}
					fmt.Printf("[Runner] Overflow compaction (keep=%d) succeeded\n", keep)
					break
				}

				// File re-injection will happen on the next iteration
				r.fileTracker.Clear()
				continue // ALWAYS retry — maxIterations is the natural bound
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
			errMsg := extractProviderErrorMessage(err)
			resultCh <- ai.StreamEvent{Type: ai.EventTypeText, Text: errMsg}
			resultCh <- ai.StreamEvent{Type: ai.EventTypeDone}
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
				// Validate tool call input JSON before accepting — corrupted input
				// (e.g., concatenated chunks like "{...}{...}") would poison the session.
				if event.ToolCall.Input != nil && !json.Valid(event.ToolCall.Input) {
					fmt.Printf("[Runner] WARNING: tool call %q has invalid JSON input, skipping to prevent session poisoning\n", event.ToolCall.Name)
					continue
				}
				hasToolCalls = true
				toolCalls = append(toolCalls, session.ToolCall{
					ID:    event.ToolCall.ID,
					Name:  event.ToolCall.Name,
					Input: event.ToolCall.Input,
				})

			case ai.EventTypeError:
				fmt.Printf("[Runner] Error event received: %v\n", event.Error)
				// Send user-visible error message so the chat doesn't just hang
				errMsg := extractProviderErrorMessage(event.Error)
				resultCh <- ai.StreamEvent{Type: ai.EventTypeText, Text: errMsg}
				resultCh <- ai.StreamEvent{Type: ai.EventTypeDone}
				return

			case ai.EventTypeMessage:
				// Save intermediate messages from CLI provider's internal agentic loop
				// Only save if the message has actual content (not empty envelopes)
				if event.Message != nil && (event.Message.Content != "" || len(event.Message.ToolCalls) > 0 || len(event.Message.ToolResults) > 0) {
					msg := *event.Message
					msg.SessionID = sessionID

					// Normalize: Anthropic CLI wraps tool results in "user" messages,
					// but the universal format uses "tool" role. Convert so sessions
					// work correctly when replayed through any provider adapter.
					if msg.Role == "user" && msg.Content == "" && len(msg.ToolResults) > 0 {
						msg.Role = "tool"
					}

					if err := r.sessions.AppendMessage(sessionID, msg); err != nil {
						fmt.Printf("[Runner] ERROR saving intermediate message: %v\n", err)
					}
					// NOTE: Do NOT accumulate into assistantContent here.
					// Messages are already saved above individually. Accumulating would
					// cause double-saving when the final save runs at the end of iteration.
				}
			}
		}
		fmt.Printf("[Runner] Stream complete: %d events, %d tool calls\n", eventCount, len(toolCalls))

		// Capture rate-limit info from Janus responses (every iteration)
		r.captureRateLimit(provider)

		// Save assistant message (always save unless empty)
		// Skip if provider handles tools — messages were already saved via EventTypeMessage
		if !providerHandlesTools && (assistantContent.Len() > 0 || len(toolCalls) > 0) {
			var toolCallsJSON json.RawMessage
			if len(toolCalls) > 0 {
				data, err := json.Marshal(toolCalls)
				if err != nil {
					fmt.Printf("[Runner] ERROR marshaling tool calls (dropping to prevent session poisoning): %v\n", err)
					// Still save the assistant text, just without tool calls
				} else {
					// Validate round-trip: unmarshal back to catch subtle corruption
					var check []session.ToolCall
					if err := json.Unmarshal(data, &check); err != nil {
						fmt.Printf("[Runner] ERROR tool calls JSON validation failed (dropping): %v\n", err)
					} else {
						toolCallsJSON = data
					}
				}
			}

			err := r.sessions.AppendMessage(sessionID, session.Message{
				SessionID: sessionID,
				Role:      "assistant",
				Content:   afv.StripFenceMarkers(assistantContent.String()),
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
					ImageURL: result.ImageURL,
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

		// Guard: if the model returned absolutely nothing (0 text, 0 tool calls),
		// something is wrong — poisoned history, provider glitch, etc. Instead of
		// silently completing (user sees nothing), send a visible error and retry
		// once. The retry will benefit from buildMessages stripping any corrupt
		// history on the next pass.
		if assistantContent.Len() == 0 && !hasToolCalls && !providerHandlesTools {
			if iteration == 1 {
				fmt.Printf("[Runner] WARNING: empty model response on iteration 1, retrying\n")
				continue
			}
			fmt.Printf("[Runner] WARNING: empty model response on iteration %d, giving up\n", iteration)
			resultCh <- ai.StreamEvent{
				Type: ai.EventTypeText,
				Text: "I'm having trouble generating a response right now. Please try again.",
			}
			resultCh <- ai.StreamEvent{Type: ai.EventTypeDone}
			return
		}

		// No tool calls — task is complete
		// Record successful usage for profile tracking
		r.recordProfileUsage(ctx, provider)

		// Debounced memory extraction: wait for conversation to go idle before
		// hitting the API. Each new message resets the timer so extraction never
		// competes with chat requests for API bandwidth.
		if !skipMemoryExtract {
			r.scheduleMemoryExtraction(sessionID, userID)
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

// stripFencesFromMessages removes AFV fence markers from message content and tool results.
// Called after AFV verification so the LLM never sees or echoes fence markers.
func stripFencesFromMessages(messages []session.Message) {
	for i := range messages {
		if messages[i].Content != "" {
			messages[i].Content = afv.StripFenceMarkers(messages[i].Content)
		}
		if len(messages[i].ToolResults) > 0 {
			var results []session.ToolResult
			if err := json.Unmarshal(messages[i].ToolResults, &results); err == nil {
				changed := false
				for j := range results {
					clean := afv.StripFenceMarkers(results[j].Content)
					if clean != results[j].Content {
						results[j].Content = clean
						changed = true
					}
				}
				if changed {
					if updated, err := json.Marshal(results); err == nil {
						messages[i].ToolResults = updated
					}
				}
			}
		}
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
// extractProviderErrorMessage turns a provider error into a user-visible message.
// Parses known Janus/OpenAI error formats to extract a clean message.
// Janus quota exhaustion gets a friendly message instead of the raw error code.
func extractProviderErrorMessage(err error) string {
	msg := err.Error()
	lower := strings.ToLower(msg)

	// Janus quota exhaustion — friendly message with upgrade path
	if strings.Contains(lower, "limit_exceeded") || strings.Contains(lower, "quota") || strings.Contains(lower, "usage limit") {
		return "You've used all your AI tokens for this week. Your quota resets automatically — check **Settings > NeboLoop** to see when.\n\nNeed more right now? [Upgrade your plan](https://neboloop.com/app/settings/billing) for a higher weekly limit."
	}

	// Try to extract "message" from JSON error body
	// e.g. {"code":"provider_error","message":"Usage limit exceeded: ...","type":"server_error"}
	if idx := strings.Index(msg, `"message":"`); idx != -1 {
		start := idx + len(`"message":"`)
		if end := strings.Index(msg[start:], `"`); end != -1 {
			msg = msg[start : start+end]
		}
	}

	return fmt.Sprintf("Something went wrong: %s", msg)
}

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

// detectAndSetObjective runs in background to classify a user message and
// set/update/clear the session's active_task. This keeps the agent anchored
// to a working objective even after many tool calls push the original message
// out of the context window.
//
// Same pattern as extractAndStoreMemories: sync.Map dedup, cheapest model,
// panic recovery, generous timeout.
func (r *Runner) detectAndSetObjective(sessionID, sessionKey, userPrompt string) {
	// Prevent overlapping detections for the same session
	if _, running := r.detectingObjective.LoadOrStore(sessionID, true); running {
		return
	}
	defer r.detectingObjective.Delete(sessionID)

	defer func() {
		if rec := recover(); rec != nil {
			crashlog.LogPanic("runner", rec, map[string]string{"op": "objective_detection", "session": sessionID})
		}
	}()

	if len(r.providers) == 0 {
		return
	}

	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()

	// Read current objective
	currentObjective, _ := r.sessions.GetActiveTask(sessionID)

	// Build classification prompt
	objDisplay := currentObjective
	if objDisplay == "" {
		objDisplay = "none"
	}
	classifyPrompt := fmt.Sprintf(`Classify this user message relative to the current working objective.

Current objective: %s
User message: %s

Respond with ONLY one JSON line, no markdown:
{"action": "set", "objective": "concise 1-sentence objective"}
OR {"action": "update", "objective": "refined objective"}
OR {"action": "clear"}
OR {"action": "keep"}

Rules:
- "set": User stated a new, distinct objective (e.g., "let's build X", "create Y", "fix Z")
- "update": User is refining or adding to the current objective (e.g., "also add tests", "and make it async")
- "clear": User is done or moving on without a new goal (e.g., "thanks", "looks good", "never mind")
- "keep": No change needed (questions, feedback, corrections, short replies)
- If the message is short (<15 words) and conversational, use "keep"
- If unsure, use "keep"`, objDisplay, userPrompt)

	// Get cheapest model
	var provider ai.Provider
	var modelName string
	if r.selector != nil {
		cheapest := r.selector.GetCheapestModel()
		if cheapest != "" {
			provID, mName := ai.ParseModelID(cheapest)
			if p, ok := r.providerMap[provID]; ok {
				provider = &modelOverrideProvider{Provider: p, model: mName}
				modelName = mName
			}
		}
	}
	if provider == nil && len(r.providers) > 0 {
		provider = r.providers[0]
		modelName = "default"
	}
	if provider == nil {
		return
	}

	// Make LLM call
	streamCh, err := provider.Stream(ctx, &ai.ChatRequest{
		Messages: []session.Message{{Role: "user", Content: classifyPrompt}},
		Model:    modelName,
	})
	if err != nil {
		fmt.Printf("[runner] Objective detection failed (stream): %v\n", err)
		return
	}

	// Collect response
	var resp strings.Builder
	for event := range streamCh {
		if event.Type == ai.EventTypeText {
			resp.WriteString(event.Text)
		}
		if event.Type == ai.EventTypeError {
			fmt.Printf("[runner] Objective detection failed (event): %v\n", event.Error)
			return
		}
	}

	// Parse JSON response
	respText := strings.TrimSpace(resp.String())
	// Strip markdown code fences if present
	respText = strings.TrimPrefix(respText, "```json")
	respText = strings.TrimPrefix(respText, "```")
	respText = strings.TrimSuffix(respText, "```")
	respText = strings.TrimSpace(respText)

	var result struct {
		Action    string `json:"action"`
		Objective string `json:"objective"`
	}
	if err := json.Unmarshal([]byte(respText), &result); err != nil {
		fmt.Printf("[runner] Objective detection failed (parse): %v response=%q\n", err, respText)
		return
	}

	switch result.Action {
	case "set":
		if result.Objective != "" {
			fmt.Printf("[runner] Objective detection: SET → %s\n", result.Objective)
			if err := r.sessions.SetActiveTask(sessionID, result.Objective); err != nil {
				fmt.Printf("[runner] Objective detection: SetActiveTask failed: %v\n", err)
			}
			// Clear work tasks — new objective means fresh task list
			if agentTool := r.tools.GetAgentDomainTool(); agentTool != nil {
				agentTool.ClearWorkTasks(sessionKey)
			}
		}
	case "update":
		if result.Objective != "" {
			fmt.Printf("[runner] Objective detection: UPDATE → %s\n", result.Objective)
			if err := r.sessions.SetActiveTask(sessionID, result.Objective); err != nil {
				fmt.Printf("[runner] Objective detection: SetActiveTask failed: %v\n", err)
			}
		}
	case "clear":
		fmt.Printf("[runner] Objective detection: CLEAR\n")
		_ = r.sessions.ClearActiveTask(sessionID)
		if agentTool := r.tools.GetAgentDomainTool(); agentTool != nil {
			agentTool.ClearWorkTasks(sessionKey)
		}
	case "keep":
		// No change
	default:
		fmt.Printf("[runner] Objective detection: unknown action=%q\n", result.Action)
	}
}

// scheduleMemoryExtraction debounces memory extraction for a session.
// Each call resets the idle timer so extraction only runs when the
// conversation pauses. This prevents background API calls from competing
// with chat requests for provider bandwidth.
func (r *Runner) scheduleMemoryExtraction(sessionID, userID string) {
	const idleDelay = 5 * time.Second

	// Cancel any pending timer for this session
	if existing, ok := r.memoryTimers.Load(sessionID); ok {
		existing.(*time.Timer).Stop()
	}

	timer := time.AfterFunc(idleDelay, func() {
		r.memoryTimers.Delete(sessionID)
		r.extractAndStoreMemories(sessionID, userID)
	})
	r.memoryTimers.Store(sessionID, timer)
}

// extractAndStoreMemories runs in background to extract facts from a completed conversation
// userID is passed explicitly to avoid race conditions with concurrent requests
// Fire-and-forget, fully non-blocking, with error recovery
func (r *Runner) extractAndStoreMemories(sessionID, userID string) {
	// Prevent overlapping extractions for the same session
	if _, running := r.extractingMemory.LoadOrStore(sessionID, true); running {
		fmt.Printf("[runner] Memory extraction already in progress for session=%s, skipping\n", sessionID)
		return
	}
	defer r.extractingMemory.Delete(sessionID)

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

	// Use background context with generous timeout
	// Small models on congested APIs can take a while for structured extraction
	ctx, cancel := context.WithTimeout(context.Background(), 90*time.Second)
	defer cancel()

	// Add a watchdog timer that logs if extraction is taking too long
	// This helps diagnose hangs without blocking
	watchdog := time.AfterFunc(30*time.Second, func() {
		fmt.Printf("[runner] WARNING: Memory extraction taking >30s for session=%s\n", sessionID)
	})
	defer watchdog.Stop()

	// Only extract from the latest turn (last ~6 messages covers user + assistant + tool calls).
	// Extraction runs after every turn, so older messages were already processed.
	messages, err := r.sessions.GetMessages(sessionID, 6)
	if err != nil || len(messages) < 2 {
		return // Not enough conversation to extract from
	}

	// Build a list of providers to try for extraction (cheapest first, then fallbacks)
	type candidate struct {
		provider ai.Provider
		label    string
	}
	var candidates []candidate

	if r.selector != nil {
		cheapestModelID := r.selector.GetCheapestModel()
		if cheapestModelID != "" {
			providerID, modelName := ai.ParseModelID(cheapestModelID)
			if p, ok := r.providerMap[providerID]; ok {
				candidates = append(candidates, candidate{
					provider: &modelOverrideProvider{Provider: p, model: modelName},
					label:    cheapestModelID,
				})
			}
		}
	}
	// Add remaining providers as fallbacks (skip duplicates)
	seen := map[string]bool{}
	for _, c := range candidates {
		seen[c.label] = true
	}
	for _, p := range r.providers {
		label := "fallback/" + p.ID()
		if seen[label] {
			continue
		}
		candidates = append(candidates, candidate{provider: p, label: label})
		seen[label] = true
	}

	if len(candidates) == 0 {
		fmt.Printf("[runner] ERROR: Memory extraction - no providers available\n")
		return
	}

	// Try each candidate until one succeeds
	var facts *memory.ExtractedFacts
	var successProvider ai.Provider
	for _, c := range candidates {
		fmt.Printf("[runner] Memory extraction trying: %s\n", c.label)
		extractor := memory.NewExtractor(c.provider)
		var err error
		facts, err = extractor.Extract(ctx, messages)
		if err == nil {
			fmt.Printf("[runner] Memory extraction succeeded with: %s\n", c.label)
			successProvider = c.provider
			break
		}
		fmt.Printf("[runner] Memory extraction failed with %s: %v\n", c.label, err)
		if ctx.Err() != nil {
			fmt.Printf("[runner] Memory extraction context expired, giving up\n")
			return
		}
	}
	if facts == nil {
		fmt.Printf("[runner] Memory extraction failed with all providers\n")
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
			directive, err := memory.SynthesizeDirective(ctx, db, successProvider, userID)
			if err != nil {
				fmt.Printf("[runner] Personality synthesis failed: %v\n", err)
			} else if directive != "" {
				fmt.Printf("[runner] Personality directive updated for user %s\n", userID)
			}
			// directive == "" means not enough observations yet — that's fine
		}
	}
}

// maybeRunMemoryFlush kicks off background memory extraction before compaction.
// Threshold and dedup checks are synchronous. The actual LLM extraction runs in
// a goroutine so it doesn't block the conversation lane — the messages slice is
// safe to read concurrently since Compact() only modifies the DB.
// Returns true if a flush was initiated.
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

	if r.memoryTool == nil || len(r.providers) == 0 {
		return false
	}

	// Record flush intent immediately to prevent re-triggering on next iteration
	if r.sessions != nil {
		if err := r.sessions.RecordMemoryFlush(sessionID); err != nil {
			fmt.Printf("[runner] Warning: failed to record memory flush: %v\n", err)
		}
	}

	fmt.Printf("[runner] Context at %d tokens (threshold: %d) - launching background memory flush (session: %s)\n", tokens, flushThreshold, sessionID)

	// Resolve provider synchronously (fast, no LLM call)
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

	// Run extraction in background — the messages slice is an in-memory copy
	// safe to read concurrently while Compact() modifies the DB.
	go r.runMemoryFlush(ctx, flushProvider, messages, userID)

	return true
}

// runMemoryFlush performs the actual LLM extraction and storage in the background.
func (r *Runner) runMemoryFlush(ctx context.Context, provider ai.Provider, messages []session.Message, userID string) {
	defer func() {
		if v := recover(); v != nil {
			crashlog.LogPanic("runner", v, map[string]string{"op": "memory_flush"})
		}
	}()

	flushCtx, cancel := context.WithTimeout(ctx, 90*time.Second)
	defer cancel()

	extractor := memory.NewExtractor(provider)
	facts, err := extractor.Extract(flushCtx, messages)
	if err != nil {
		fmt.Printf("[runner] Background memory flush failed: %v\n", err)
		return
	}

	if facts.IsEmpty() {
		fmt.Printf("[runner] Background memory flush complete (no memories to store)\n")
		return
	}

	// Store extracted facts (with dedup: skip if identical value already exists)
	entries := facts.FormatForStorage()
	stored, skipped := 0, 0
	for _, entry := range entries {
		var storeErr error
		if entry.IsStyle {
			storeErr = r.memoryTool.StoreStyleEntryForUser(entry.Layer, entry.Namespace, entry.Key, entry.Value, entry.Tags, userID)
		} else {
			if r.memoryTool.IsDuplicate(entry.Layer, entry.Namespace, entry.Key, entry.Value, userID) {
				skipped++
				continue
			}
			storeErr = r.memoryTool.StoreEntryForUser(entry.Layer, entry.Namespace, entry.Key, entry.Value, entry.Tags, userID)
		}
		if storeErr != nil {
			fmt.Printf("[runner] Memory flush store failed for %s: %v\n", entry.Key, storeErr)
		} else {
			stored++
		}
	}
	fmt.Printf("[runner] Background memory flush: stored %d, skipped %d duplicates\n", stored, skipped)
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

// captureRateLimit checks if the provider implements RateLimitProvider and
// stores the latest snapshot via the configured callback.
func (r *Runner) captureRateLimit(provider ai.Provider) {
	if r.rateLimitStore == nil {
		return
	}
	// Unwrap ProfiledProvider if needed
	rlp, ok := provider.(ai.RateLimitProvider)
	if !ok {
		return
	}
	if rl := rlp.GetRateLimit(); rl != nil {
		r.rateLimitStore(rl)
	}
}

// latestRateLimit returns the latest rate-limit info from the current provider, or nil.
func (r *Runner) latestRateLimit(provider ai.Provider) *ai.RateLimitInfo {
	if rlp, ok := provider.(ai.RateLimitProvider); ok {
		return rlp.GetRateLimit()
	}
	return nil
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

// Threshold offset constants
const (
	// WarningOffset is how far below effective context the warning threshold sits.
	// Micro-compact activates above this point.
	WarningOffset = 20000

	// ErrorOffset is how far below effective context the error threshold sits.
	// A warning is logged above this point.
	ErrorOffset = 10000
)

// ContextThresholds defines graduated tiers for context management.
// Thresholds are absolute token counts derived from the model's context window.
type ContextThresholds struct {
	Warning     int // Micro-compact activates above this
	Error       int // Log warning about context size
	AutoCompact int // Trigger full compaction (LLM summarization)
}

// contextThresholds computes graduated context thresholds from the active model's
// context window. Caches the result for the duration of a run since the context
// window doesn't change mid-conversation.
func (r *Runner) contextThresholds() ContextThresholds {
	if r.cachedThresholds != nil {
		return *r.cachedThresholds
	}

	contextWindow := 0
	if r.selector != nil {
		// Try the actual provider being used first (not just the routing default).
		// For Janus users, routing may point to anthropic/claude-* which isn't loaded,
		// but the actual provider (janus/janus) has a known context window.
		for _, p := range r.providers {
			providerID := p.ID()
			models := r.selector.GetProviderModels(providerID)
			for _, m := range models {
				if m.ContextWindow > contextWindow {
					contextWindow = m.ContextWindow
				}
			}
		}
		// Fall back to selector's routing default if no provider models found
		if contextWindow == 0 {
			modelID := r.selector.Select(nil)
			if modelID != "" {
				info := r.selector.GetModelInfo(modelID)
				if info != nil && info.ContextWindow > 0 {
					contextWindow = info.ContextWindow
				}
			}
		}
	}

	if contextWindow <= 0 {
		result := ContextThresholds{
			Warning:     DefaultContextTokenLimit - WarningOffset,
			Error:       DefaultContextTokenLimit - ErrorOffset,
			AutoCompact: DefaultContextTokenLimit,
		}
		r.cachedThresholds = &result
		return result
	}

	// Reserve tokens for system prompt, tool definitions
	const reserveTokens = 20000
	effective := contextWindow - reserveTokens
	if effective < DefaultContextTokenLimit {
		effective = DefaultContextTokenLimit
	}

	warning := effective - WarningOffset
	errorT := effective - ErrorOffset
	autoCompact := effective

	// Floor: reasonable minimums
	if warning < 40000 {
		warning = 40000
	}
	if errorT < 50000 {
		errorT = 50000
	}

	// Cap: avoid extremely long summarization tasks
	const maxAutoCompact = 500000
	if autoCompact > maxAutoCompact {
		autoCompact = maxAutoCompact
	}

	result := ContextThresholds{
		Warning:     warning,
		Error:       errorT,
		AutoCompact: autoCompact,
	}
	r.cachedThresholds = &result
	return result
}

// contextTokenLimit returns the max tokens before proactive compaction triggers.
// Delegates to the AutoCompact tier of the graduated thresholds.
func (r *Runner) contextTokenLimit() int {
	return r.contextThresholds().AutoCompact
}

// contextWarningThreshold returns the token count above which micro-compaction
// should activate. Delegates to the Warning tier of the graduated thresholds.
func (r *Runner) contextWarningThreshold() int {
	return r.contextThresholds().Warning
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

// buildPlatformSection and injectSystemContext have moved to prompt.go
// as part of the section-based prompt builder.

