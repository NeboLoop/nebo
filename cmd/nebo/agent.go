package cli

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/gorilla/websocket"
	"github.com/spf13/cobra"
	_ "modernc.org/sqlite"

	"nebo/agent/ai"
	agentcfg "nebo/agent/config"
	"nebo/agent/memory"
	"nebo/agent/runner"
	"nebo/agent/session"
	"nebo/agent/tools"
	"nebo/internal/channels"
	"nebo/internal/local"
	"nebo/internal/provider"
)

// approvalResponse holds the result of an approval request
type approvalResponse struct {
	Approved bool
	Always   bool
}

// pendingApprovalInfo holds info about a pending approval request
type pendingApprovalInfo struct {
	RespCh   chan approvalResponse
	ToolName string
	Input    json.RawMessage
}

// agentState holds the state for a connected agent
type agentState struct {
	conn            *websocket.Conn
	connMu          sync.Mutex
	pendingApproval map[string]*pendingApprovalInfo
	approvalMu      sync.RWMutex
	quiet           bool // Suppress console output for clean CLI
	policy          *tools.Policy
}

// sendFrame sends a JSON frame to the server
func (s *agentState) sendFrame(frame map[string]any) error {
	s.connMu.Lock()
	defer s.connMu.Unlock()
	data, _ := json.Marshal(frame)
	return s.conn.WriteMessage(websocket.TextMessage, data)
}

// requestApproval sends an approval request and waits for response
func (s *agentState) requestApproval(ctx context.Context, requestID, toolName string, input json.RawMessage) (bool, error) {
	respCh := make(chan approvalResponse, 1)
	s.approvalMu.Lock()
	s.pendingApproval[requestID] = &pendingApprovalInfo{
		RespCh:   respCh,
		ToolName: toolName,
		Input:    input,
	}
	s.approvalMu.Unlock()

	defer func() {
		s.approvalMu.Lock()
		delete(s.pendingApproval, requestID)
		s.approvalMu.Unlock()
	}()

	frame := map[string]any{
		"type": "approval_request",
		"id":   requestID,
		"payload": map[string]any{
			"tool":  toolName,
			"input": json.RawMessage(input),
		},
	}
	if err := s.sendFrame(frame); err != nil {
		return false, err
	}

	select {
	case resp := <-respCh:
		// If "always" was selected, add the command to the allowlist
		if resp.Approved && resp.Always && s.policy != nil {
			var inputStr string
			if toolName == "bash" {
				var bashInput struct {
					Command string `json:"command"`
				}
				if err := json.Unmarshal(input, &bashInput); err == nil {
					inputStr = bashInput.Command
				}
			}
			if inputStr != "" {
				s.policy.AddToAllowlist(inputStr)
				if !s.quiet {
					fmt.Printf("\033[32m[Policy] Added to allowlist: %s\033[0m\n", inputStr)
				}
			}
		}
		return resp.Approved, nil
	case <-ctx.Done():
		return false, ctx.Err()
	}
}

// handleApprovalResponse processes an approval response from the server
func (s *agentState) handleApprovalResponse(requestID string, approved, always bool) {
	s.approvalMu.RLock()
	info, ok := s.pendingApproval[requestID]
	s.approvalMu.RUnlock()
	if ok && info != nil {
		select {
		case info.RespCh <- approvalResponse{Approved: approved, Always: always}:
		default:
		}
	}
}

// agentCmd creates the agent command
func AgentCmd() *cobra.Command {
	var serverURL string
	var dangerously bool

	cmd := &cobra.Command{
		Use:   "agent",
		Short: "Start the AI agent only",
		Long: `Start the Nebo AI agent to receive and process tasks from the web UI.

The agent connects to the local Nebo server and processes chat messages
using configured AI providers (Anthropic, OpenAI, etc.)

Examples:
  nebo agent                    # Start the agent
  nebo agent --dangerously      # Autonomous mode (no approval prompts)`,
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadAgentConfig()
			runAgent(cfg, serverURL, dangerously)
		},
	}

	cmd.Flags().StringVar(&serverURL, "server", "", "server URL (default: http://localhost:27895)")
	cmd.Flags().BoolVar(&dangerously, "dangerously", false, "100% autonomous mode - bypass ALL tool approval prompts")

	return cmd
}

// AgentOptions holds optional dependencies for the agent loop
type AgentOptions struct {
	ChannelManager   *channels.Manager
	Database         *sql.DB
	Quiet            bool   // Suppress console output for clean CLI
	Dangerously      bool   // Bypass all tool approval prompts (CLI flag)
	SettingsFilePath string // Path to agent-settings.json for UI-based settings
}

// runAgentLoop connects to the server as an agent (used by runAll)
func runAgentLoop(ctx context.Context, cfg *agentcfg.Config, serverURL string) error {
	return runAgentLoopWithOptions(ctx, cfg, serverURL, AgentOptions{})
}

// runAgentLoopWithChannels connects to the server with shared channel manager (legacy)
func runAgentLoopWithChannels(ctx context.Context, cfg *agentcfg.Config, serverURL string, channelMgr *channels.Manager) error {
	return runAgentLoopWithOptions(ctx, cfg, serverURL, AgentOptions{ChannelManager: channelMgr})
}

// runAgentLoopWithOptions connects to the server with full options
func runAgentLoopWithOptions(ctx context.Context, cfg *agentcfg.Config, serverURL string, opts AgentOptions) error {
	wsURL := strings.Replace(serverURL, "http://", "ws://", 1)
	wsURL = strings.Replace(wsURL, "https://", "wss://", 1)
	wsURL = fmt.Sprintf("%s/api/v1/agent/ws", wsURL)

	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		return fmt.Errorf("failed to connect: %w", err)
	}
	defer conn.Close()

	state := &agentState{
		conn:            conn,
		pendingApproval: make(map[string]*pendingApprovalInfo),
		quiet:           opts.Quiet,
	}

	// Use shared database if provided, otherwise open our own
	var sessions *session.Manager
	if opts.Database != nil {
		sessions, err = session.NewWithDB(opts.Database)
		// Set the shared DB for provider loading
		SetSharedDB(opts.Database)
	} else {
		sessions, err = session.New(cfg.DBPath())
		// Set the shared DB from session manager
		SetSharedDB(sessions.GetDB())
	}
	if err != nil {
		return fmt.Errorf("failed to open database: %w", err)
	}
	defer sessions.Close()

	providers := createProviders(cfg)
	if len(providers) == 0 && !opts.Quiet {
		fmt.Fprintln(os.Stderr, "Warning: No AI providers configured. Tasks requiring AI will fail.")
	}

	// Load initial settings from UI settings file
	// This allows the Autonomous Mode toggle in settings to take effect
	var initialAutonomous bool
	if opts.SettingsFilePath != "" {
		dataDir := filepath.Dir(opts.SettingsFilePath)
		settingsStore := local.NewAgentSettingsStore(dataDir)
		settings := settingsStore.Get()
		initialAutonomous = settings.AutonomousMode
		if initialAutonomous && !opts.Quiet {
			fmt.Println("[Agent] Autonomous mode enabled from settings")
		}
	}

	// In dangerously mode (CLI flag or UI setting), use "full" policy level to bypass all approvals
	policyLevel := cfg.Policy.Level
	if opts.Dangerously || initialAutonomous {
		policyLevel = "full"
	}

	policy := tools.NewPolicyFromConfig(
		policyLevel,
		cfg.Policy.AskMode,
		cfg.Policy.Allowlist,
	)

	// Store policy reference in state for "always" approval handling
	state.policy = policy

	var approvalCounter int64
	if opts.Dangerously || initialAutonomous {
		// Auto-approve everything in dangerous/autonomous mode
		policy.ApprovalCallback = func(ctx context.Context, toolName string, input json.RawMessage) (bool, error) {
			return true, nil
		}
	} else {
		policy.ApprovalCallback = func(ctx context.Context, toolName string, input json.RawMessage) (bool, error) {
			approvalCounter++
			requestID := fmt.Sprintf("approval-%d-%d", time.Now().UnixNano(), approvalCounter)
			return state.requestApproval(ctx, requestID, toolName, input)
		}
	}

	registry := tools.NewRegistry(policy)
	registry.RegisterDefaults()

	// Create memory tool for auto-extraction (requires shared database)
	var memoryTool *tools.MemoryTool
	if opts.Database != nil {
		memoryTool, err = tools.NewMemoryTool(tools.MemoryConfig{DB: opts.Database})
		if err == nil {
			registry.Register(memoryTool)
		}
	}

	// Register message tool with shared channel manager
	messageTool := tools.NewMessageTool()
	if opts.ChannelManager != nil {
		messageTool.SetChannels(opts.ChannelManager)
	}
	registry.Register(messageTool)

	// Register cron tool for scheduled tasks (requires shared database)
	var cronTool *tools.CronTool
	if opts.Database != nil {
		cronTool, err = tools.NewCronTool(tools.CronConfig{DB: opts.Database})
		if err == nil {
			registry.Register(cronTool)
		}
	}

	taskTool := tools.NewTaskTool()

	// Wire up cron → agent execution (after runner is created)
	// We'll set this callback after creating the runner below
	taskTool.CreateOrchestrator(cfg, sessions, providers, registry)
	registry.Register(taskTool)

	agentStatusTool := tools.NewAgentStatusTool()
	agentStatusTool.SetOrchestrator(taskTool.GetOrchestrator())
	registry.Register(agentStatusTool)

	r := runner.New(cfg, sessions, providers, registry)

	// Set provider loader for dynamic reload (after onboarding adds API key)
	r.SetProviderLoader(func() []ai.Provider {
		return createProviders(cfg)
	})

	// Set up model selector for intelligent model routing and cheapest model selection
	modelsConfig := provider.GetModelsConfig()
	if modelsConfig != nil {
		// Always create selector - needed for GetCheapestModel() even without task routing
		selector := ai.NewModelSelector(modelsConfig)
		r.SetModelSelector(selector)
		// Set up fuzzy matcher for user model switch requests
		fuzzyMatcher := ai.NewFuzzyMatcher(modelsConfig)
		r.SetFuzzyMatcher(fuzzyMatcher)
	}

	// Start config file watcher for hot-reload of models.yaml
	if err := provider.StartConfigWatcher(cfg.DataDir); err != nil {
		fmt.Printf("[agent] Warning: could not start config watcher: %v\n", err)
	}

	// Register callback to update selector/matcher/providers when models.yaml changes
	provider.OnConfigReload(func(newConfig *provider.ModelsConfig) {
		if newConfig != nil {
			newSelector := ai.NewModelSelector(newConfig)
			r.SetModelSelector(newSelector)
			newFuzzyMatcher := ai.NewFuzzyMatcher(newConfig)
			r.SetFuzzyMatcher(newFuzzyMatcher)
			// Reload providers in case credentials changed
			r.ReloadProviders()
			fmt.Printf("[agent] Config reloaded: model selector, fuzzy matcher, and providers updated\n")
		}
	})

	// Enable automatic memory extraction after conversations
	if memoryTool != nil {
		r.SetMemoryTool(memoryTool)
	}

	// Wire up cron agent task callback now that runner exists
	if cronTool != nil {
		cronTool.SetAgentCallback(func(ctx context.Context, name, message string, deliver *tools.DeliverConfig) error {

			// Run the agent task
			events, err := r.Run(ctx, &runner.RunRequest{
				SessionKey: fmt.Sprintf("cron-%s", name),
				Prompt:     message,
			})
			if err != nil {
				return err
			}

			// Collect result
			var result strings.Builder
			for event := range events {
				if event.Type == ai.EventTypeText {
					result.WriteString(event.Text)
				}
			}

			// Optionally deliver result to channel
			if deliver != nil && opts.ChannelManager != nil {
				ch, ok := opts.ChannelManager.Get(deliver.Channel)
				if ok {
					ch.Send(ctx, channels.OutboundMessage{
						ChannelID: deliver.To,
						Text:      result.String(),
						ParseMode: "markdown",
					})
				}
			}

			return nil
		})
	}

	// Close connection when context is cancelled to unblock ReadMessage
	go func() {
		<-ctx.Done()
		fmt.Printf("[Agent] Context done, closing connection: %v\n", ctx.Err())
		conn.Close()
	}()

	for {
		_, message, err := conn.ReadMessage()
		if err != nil {
			fmt.Printf("[Agent] ReadMessage error: %v, ctx.Err: %v\n", err, ctx.Err())
			if ctx.Err() != nil {
				fmt.Println("[Agent] Exiting agent loop (context cancelled)")
				return nil
			}
			fmt.Println("[Agent] Exiting agent loop (connection error)")
			return fmt.Errorf("connection closed: %w", err)
		}
		// Run handler in goroutine to avoid blocking the read loop.
		// This is essential for approval flow: the agent sends approval_request,
		// then blocks waiting for approval_response. If we don't run in goroutine,
		// the read loop is blocked and can't receive the approval_response = deadlock!
		go handleAgentMessageWithState(ctx, state, r, sessions, message)
	}
}

// maybeIntroduceSelf checks if a user needs onboarding and proactively introduces the agent
// This is called on agent startup and checks the global companion session (legacy)
// For per-user introduction, see maybeIntroduceToUser
func maybeIntroduceSelf(ctx context.Context, state *agentState, r *runner.Runner, sessions *session.Manager) {
	// Legacy: Check global companion session for backwards compatibility
	// New multi-user onboarding is handled per-user in maybeIntroduceToUser
	companionSession, err := sessions.GetOrCreate("companion")
	if err != nil {
		fmt.Printf("[Agent] Could not check companion session: %v\n", err)
		return
	}

	// Get messages for this session
	messages, err := sessions.GetMessages(companionSession.ID, 10)
	if err != nil {
		fmt.Printf("[Agent] Could not get companion messages: %v\n", err)
		return
	}

	// If there are already messages, no need to introduce
	if len(messages) > 0 {
		fmt.Printf("[Agent] Companion session already has %d messages, skipping introduction\n", len(messages))
		return
	}

	// No messages yet - this is a new user! Introduce ourselves.
	fmt.Println("[Agent] New user detected! Introducing myself...")

	// Run the agent with a special introduction request
	// Empty prompt signals the runner to not save a user message, just trigger the agent
	events, err := r.Run(ctx, &runner.RunRequest{
		SessionKey: "companion",
		Prompt:     "", // Empty prompt = agent speaks first
		System:     "You are starting a conversation with a new user. Introduce yourself warmly and ask what they would like to be called. Keep it brief and friendly.",
	})
	if err != nil {
		fmt.Printf("[Agent] Introduction failed: %v\n", err)
		return
	}

	// Stream the introduction response back to any connected clients
	var result strings.Builder
	for event := range events {
		switch event.Type {
		case ai.EventTypeText:
			result.WriteString(event.Text)
			state.sendFrame(map[string]any{
				"type": "stream",
				"id":   "introduction",
				"payload": map[string]any{
					"chunk":      event.Text,
					"session_id": companionSession.ID,
				},
			})
		case ai.EventTypeError:
			fmt.Printf("[Agent] Introduction error: %v\n", event.Error)
		}
	}

	// Send completion
	state.sendFrame(map[string]any{
		"type": "res",
		"id":   "introduction",
		"ok":   true,
		"payload": map[string]any{
			"result":     result.String(),
			"session_id": companionSession.ID,
		},
	})

	fmt.Printf("[Agent] Introduction complete (%d chars)\n", result.Len())
}

// handleIntroduction handles an explicit introduction request from the server
// This is called when a user loads an empty companion chat
func handleIntroduction(ctx context.Context, state *agentState, r *runner.Runner, sessions *session.Manager, requestID, sessionKey, userID string) {
	fmt.Printf("[Agent] Handling introduction request: id=%s session=%s user=%s\n", requestID, sessionKey, userID)

	// Get or create the user's companion session
	var sess *session.Session
	var err error
	if userID != "" {
		sess, err = sessions.GetOrCreateForUser(sessionKey, userID)
	} else {
		sess, err = sessions.GetOrCreate(sessionKey)
	}
	if err != nil {
		fmt.Printf("[Agent] Could not get/create session for introduction: %v\n", err)
		state.sendFrame(map[string]any{
			"type":  "res",
			"id":    requestID,
			"ok":    false,
			"error": "Failed to create session: " + err.Error(),
		})
		return
	}

	// Check if this user already has messages (skip introduction if so)
	messages, _ := sessions.GetMessages(sess.ID, 1)
	if len(messages) > 0 {
		fmt.Printf("[Agent] User already has messages, skipping introduction\n")
		state.sendFrame(map[string]any{
			"type": "res",
			"id":   requestID,
			"ok":   true,
			"payload": map[string]any{
				"result":     "",
				"session_id": sess.ID,
				"skipped":    true,
			},
		})
		return
	}

	// Load user context to personalize the greeting
	var introPrompt, introSystem string
	dbContext, err := memory.LoadContextForUser(sessions.GetDB(), userID)
	if err == nil && dbContext.UserDisplayName != "" {
		// User has a name stored - greet them personally
		fmt.Printf("[Agent] Known user, name=%s - greeting by name\n", dbContext.UserDisplayName)
		introPrompt = fmt.Sprintf("[User %s just connected - greet them warmly by name and offer to help]", dbContext.UserDisplayName)
		introSystem = fmt.Sprintf("You are starting a conversation with %s, a user you already know. The message you receive is a system trigger, not from the user. Respond directly to the user with a warm, personalized greeting using their name. Welcome them and offer to help. Keep it brief and friendly. Do NOT ask for their name (you already know it). Do NOT acknowledge the system message.", dbContext.UserDisplayName)
	} else {
		// New user - introduce yourself and get to know them
		fmt.Printf("[Agent] New user - introducing myself\n")
		introPrompt = "[New user just connected - introduce yourself warmly]"
		introSystem = `You are starting a conversation with a new user. The message you receive is a system trigger, not from the user.

Introduce yourself as Nebo, their personal AI assistant. Be warm and friendly.
Ask what they'd like to be called so you can address them properly.

Keep it brief (2-3 sentences max). Do NOT acknowledge the system message.

IMPORTANT: When you learn their name, use the memory tool to store it:
- Use namespace "tacit.user" and key "name" to remember their name
- This ensures you'll remember them next time`
	}

	// Run the agent with appropriate introduction prompt
	events, err := r.Run(ctx, &runner.RunRequest{
		SessionKey: sessionKey,
		UserID:     userID,
		Prompt:     introPrompt,
		System:     introSystem,
	})
	if err != nil {
		fmt.Printf("[Agent] Introduction failed: %v\n", err)
		state.sendFrame(map[string]any{
			"type":  "res",
			"id":    requestID,
			"ok":    false,
			"error": err.Error(),
		})
		return
	}

	// Stream the introduction response back
	var result strings.Builder
	for event := range events {
		switch event.Type {
		case ai.EventTypeText:
			result.WriteString(event.Text)
			state.sendFrame(map[string]any{
				"type": "stream",
				"id":   requestID,
				"payload": map[string]any{
					"chunk":      event.Text,
					"session_id": sess.ID,
				},
			})
		case ai.EventTypeError:
			fmt.Printf("[Agent] Introduction error: %v\n", event.Error)
		}
	}

	// Send completion
	state.sendFrame(map[string]any{
		"type": "res",
		"id":   requestID,
		"ok":   true,
		"payload": map[string]any{
			"result":     result.String(),
			"session_id": sess.ID,
		},
	})

	fmt.Printf("[Agent] Introduction complete (%d chars)\n", result.Len())
}

// maybeIntroduceToUser checks if a SPECIFIC user needs onboarding
// This is called when a user sends their first message
func maybeIntroduceToUser(ctx context.Context, state *agentState, r *runner.Runner, sessions *session.Manager, userID string) {
	if userID == "" {
		return // No user ID, can't do per-user introduction
	}

	// Check if this specific user has a companion session with messages
	companionSession, err := sessions.GetOrCreateForUser("companion", userID)
	if err != nil {
		fmt.Printf("[Agent] Could not check user companion session: %v\n", err)
		return
	}

	// Get messages for this user's session
	messages, err := sessions.GetMessages(companionSession.ID, 10)
	if err != nil {
		fmt.Printf("[Agent] Could not get user companion messages: %v\n", err)
		return
	}

	// If there are already messages, no need to introduce
	if len(messages) > 0 {
		fmt.Printf("[Agent] User %s already has %d messages, skipping introduction\n", userID, len(messages))
		return
	}

	// No messages yet - this is a new user! Introduce ourselves.
	fmt.Printf("[Agent] New user %s detected! Introducing myself...\n", userID)

	// Run the agent with a special introduction request for this user
	events, err := r.Run(ctx, &runner.RunRequest{
		SessionKey: "companion",
		UserID:     userID, // User-scoped session
		Prompt:     "",     // Empty prompt = agent speaks first
		System:     "You are starting a conversation with a new user. Introduce yourself warmly and ask what they would like to be called. Keep it brief and friendly.",
	})
	if err != nil {
		fmt.Printf("[Agent] Introduction to user %s failed: %v\n", userID, err)
		return
	}

	// Stream the introduction response back
	var result strings.Builder
	for event := range events {
		switch event.Type {
		case ai.EventTypeText:
			result.WriteString(event.Text)
			state.sendFrame(map[string]any{
				"type": "stream",
				"id":   "introduction-" + userID,
				"payload": map[string]any{
					"chunk":      event.Text,
					"session_id": companionSession.ID,
				},
			})
		case ai.EventTypeError:
			fmt.Printf("[Agent] Introduction error for user %s: %v\n", userID, event.Error)
		}
	}

	// Send completion
	state.sendFrame(map[string]any{
		"type": "res",
		"id":   "introduction-" + userID,
		"ok":   true,
		"payload": map[string]any{
			"result":     result.String(),
			"session_id": companionSession.ID,
		},
	})

	fmt.Printf("[Agent] Introduction to user %s complete (%d chars)\n", userID, result.Len())
}

// runAgent connects to the local server and runs as an agent (standalone command)
func runAgent(cfg *agentcfg.Config, serverURL string, dangerously bool) {
	if dangerously {
		if !confirmDangerousMode() {
			fmt.Println("Aborted.")
			os.Exit(0)
		}
	}

	if serverURL == "" {
		serverURL = cfg.ServerURL
	}
	if serverURL == "" {
		serverURL = "http://localhost:27895"
	}

	wsURL := strings.Replace(serverURL, "http://", "ws://", 1)
	wsURL = strings.Replace(wsURL, "https://", "wss://", 1)
	wsURL = fmt.Sprintf("%s/api/v1/agent/ws", wsURL)

	fmt.Printf("Connecting to server: %s\n", serverURL)

	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error connecting to server: %v\n", err)
		os.Exit(1)
	}
	defer conn.Close()

	fmt.Println("\033[32m✓ Connected\033[0m")
	fmt.Println("Waiting for tasks... (Ctrl+C to exit)")

	sessions, err := session.New(cfg.DBPath())
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error opening database: %v\n", err)
		os.Exit(1)
	}
	defer sessions.Close()

	// Use the shared DB from session manager for all components
	db := sessions.GetDB()
	SetSharedDB(db)

	providers := createProviders(cfg)
	if len(providers) == 0 {
		fmt.Fprintln(os.Stderr, "Warning: No AI providers configured. Tasks requiring AI will fail.")
	}

	var policy *tools.Policy
	if dangerously {
		policy = tools.NewPolicyFromConfig("full", "off", nil)
	} else {
		policy = tools.NewPolicyFromConfig(
			cfg.Policy.Level,
			cfg.Policy.AskMode,
			cfg.Policy.Allowlist,
		)
	}
	registry := tools.NewRegistry(policy)
	registry.RegisterDefaults()

	// Create memory tool for auto-extraction (using shared DB)
	var memoryTool *tools.MemoryTool
	memoryTool, err = tools.NewMemoryTool(tools.MemoryConfig{DB: db})
	if err != nil {
		fmt.Printf("[agent] Warning: failed to initialize memory tool: %v\n", err)
	} else {
		registry.Register(memoryTool)
	}

	// Create cron tool for scheduled tasks (using shared DB)
	cronTool, err := tools.NewCronTool(tools.CronConfig{DB: db})
	if err != nil {
		fmt.Printf("[agent] Warning: failed to initialize cron tool: %v\n", err)
	} else {
		registry.Register(cronTool)
	}

	taskTool := tools.NewTaskTool()
	taskTool.CreateOrchestrator(cfg, sessions, providers, registry)
	registry.Register(taskTool)

	agentStatusTool := tools.NewAgentStatusTool()
	agentStatusTool.SetOrchestrator(taskTool.GetOrchestrator())
	registry.Register(agentStatusTool)

	r := runner.New(cfg, sessions, providers, registry)

	// Set provider loader for dynamic reload (after onboarding adds API key)
	r.SetProviderLoader(func() []ai.Provider {
		return createProviders(cfg)
	})

	// Set up model selector for intelligent model routing and cheapest model selection
	modelsConfig := provider.GetModelsConfig()
	if modelsConfig != nil {
		// Always create selector - needed for GetCheapestModel() even without task routing
		selector := ai.NewModelSelector(modelsConfig)
		r.SetModelSelector(selector)
		// Set up fuzzy matcher for user model switch requests
		fuzzyMatcher := ai.NewFuzzyMatcher(modelsConfig)
		r.SetFuzzyMatcher(fuzzyMatcher)
	}

	// Start config file watcher for hot-reload of models.yaml
	if err := provider.StartConfigWatcher(cfg.DataDir); err != nil {
		fmt.Printf("[agent] Warning: could not start config watcher: %v\n", err)
	}

	// Register callback to update selector/matcher/providers when models.yaml changes
	provider.OnConfigReload(func(newConfig *provider.ModelsConfig) {
		if newConfig != nil {
			newSelector := ai.NewModelSelector(newConfig)
			r.SetModelSelector(newSelector)
			newFuzzyMatcher := ai.NewFuzzyMatcher(newConfig)
			r.SetFuzzyMatcher(newFuzzyMatcher)
			// Reload providers in case credentials changed
			r.ReloadProviders()
			fmt.Printf("[agent] Config reloaded: model selector, fuzzy matcher, and providers updated\n")
		}
	})

	// Enable automatic memory extraction after conversations
	if memoryTool != nil {
		r.SetMemoryTool(memoryTool)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
	go func() {
		<-sigCh
		fmt.Println("\n\033[33mDisconnecting...\033[0m")
		cancel()
		conn.Close()
	}()

	for {
		select {
		case <-ctx.Done():
			return
		default:
			_, message, err := conn.ReadMessage()
			if err != nil {
				if ctx.Err() != nil {
					return
				}
				fmt.Fprintf(os.Stderr, "Error reading message: %v\n", err)
				return
			}

			handleAgentMessage(ctx, conn, r, message)
		}
	}
}

// handleAgentMessage processes a message from the server
func handleAgentMessage(ctx context.Context, conn *websocket.Conn, r *runner.Runner, message []byte) {
	var frame struct {
		Type   string `json:"type"`
		ID     string `json:"id"`
		Method string `json:"method"`
		Params struct {
			Prompt     string `json:"prompt"`
			SessionKey string `json:"session_key"`
			UserID     string `json:"user_id"`
		} `json:"params"`
	}

	if err := json.Unmarshal(message, &frame); err != nil {
		fmt.Fprintf(os.Stderr, "Invalid message: %v\n", err)
		return
	}

	switch frame.Type {
	case "req":
		switch frame.Method {
		case "ping":
			response := map[string]any{
				"type":    "res",
				"id":      frame.ID,
				"ok":      true,
				"payload": map[string]any{"pong": true},
			}
			data, _ := json.Marshal(response)
			conn.WriteMessage(websocket.TextMessage, data)

		case "introduce":
			// Introduction requests are not supported in standalone agent mode
			// Use the default `nebo` command (RunAll) for full functionality
			fmt.Printf("[Agent] Introduction requested but not supported in standalone mode\n")
			response := map[string]any{
				"type":  "res",
				"id":    frame.ID,
				"ok":    false,
				"error": "Introduction not supported in standalone agent mode. Run 'nebo' instead of 'nebo agent'.",
			}
			data, _ := json.Marshal(response)
			conn.WriteMessage(websocket.TextMessage, data)

		case "run", "generate_title":
			sessionKey := frame.Params.SessionKey
			if sessionKey == "" {
				sessionKey = "agent-" + frame.ID
			}
			userID := frame.Params.UserID

			fmt.Printf("\n[Agent] Received %s request: id=%s session=%s user=%s prompt=%q\n", frame.Method, frame.ID, sessionKey, userID, frame.Params.Prompt)

			if frame.Method == "generate_title" {
				fmt.Printf("\n\033[90m[Title Gen %s]\033[0m\n", frame.ID)
			} else {
				fmt.Printf("\n\033[36m[Task %s]\033[0m %s\n", frame.ID, frame.Params.Prompt)
			}

			events, err := r.Run(ctx, &runner.RunRequest{
				SessionKey: sessionKey,
				Prompt:     frame.Params.Prompt,
				UserID:     userID,
			})
			fmt.Printf("[Agent] Run started, events channel created, err=%v\n", err)

			if err != nil {
				response := map[string]any{
					"type":  "res",
					"id":    frame.ID,
					"ok":    false,
					"error": err.Error(),
				}
				data, _ := json.Marshal(response)
				conn.WriteMessage(websocket.TextMessage, data)
				return
			}

			var result strings.Builder
			eventCount := 0
			for event := range events {
				eventCount++
				fmt.Printf("[Agent] Event %d: type=%s text_len=%d\n", eventCount, event.Type, len(event.Text))
				switch event.Type {
				case ai.EventTypeText:
					result.WriteString(event.Text)
					fmt.Print(event.Text)
					chunk := map[string]any{
						"type": "stream",
						"id":   frame.ID,
						"payload": map[string]any{
							"chunk": event.Text,
						},
					}
					chunkData, _ := json.Marshal(chunk)
					fmt.Printf("[Agent] Sending stream frame: %s\n", string(chunkData))
					conn.WriteMessage(websocket.TextMessage, chunkData)

				case ai.EventTypeToolCall:
					toolEvent := map[string]any{
						"type": "stream",
						"id":   frame.ID,
						"payload": map[string]any{
							"tool":  event.ToolCall.Name,
							"input": event.ToolCall.Input,
						},
					}
					toolData, _ := json.Marshal(toolEvent)
					conn.WriteMessage(websocket.TextMessage, toolData)

				case ai.EventTypeToolResult:
					resultEvent := map[string]any{
						"type": "stream",
						"id":   frame.ID,
						"payload": map[string]any{
							"tool_result": event.Text,
						},
					}
					resultData, _ := json.Marshal(resultEvent)
					conn.WriteMessage(websocket.TextMessage, resultData)
				}
			}
			fmt.Println()

			fmt.Printf("[Agent] Events complete, total events=%d, result_len=%d\n", eventCount, result.Len())
			response := map[string]any{
				"type": "res",
				"id":   frame.ID,
				"ok":   true,
				"payload": map[string]any{
					"result": result.String(),
				},
			}
			data, _ := json.Marshal(response)
			fmt.Printf("[Agent] Sending final response for %s\n", frame.ID)
			conn.WriteMessage(websocket.TextMessage, data)

		default:
			response := map[string]any{
				"type":  "res",
				"id":    frame.ID,
				"ok":    false,
				"error": "unknown method: " + frame.Method,
			}
			data, _ := json.Marshal(response)
			conn.WriteMessage(websocket.TextMessage, data)
		}

	case "event":
		var eventFrame struct {
			Method  string `json:"method"`
			Payload struct {
				AutonomousMode   bool `json:"autonomousMode"`
				AutoApproveRead  bool `json:"autoApproveRead"`
				AutoApproveWrite bool `json:"autoApproveWrite"`
				AutoApproveBash  bool `json:"autoApproveBash"`
			} `json:"payload"`
		}
		if err := json.Unmarshal(message, &eventFrame); err == nil {
			if eventFrame.Method == "settings_updated" {
				p := eventFrame.Payload
				if p.AutonomousMode {
					fmt.Println("\033[33m[Settings] Autonomous mode ENABLED - all approvals bypassed\033[0m")
					r.SetPolicy(tools.NewPolicyFromConfig("full", "off", nil))
				} else {
					askMode := "on-miss"
					if p.AutoApproveRead && p.AutoApproveWrite && p.AutoApproveBash {
						askMode = "off"
					}
					fmt.Printf("\033[36m[Settings] Updated - read:%v write:%v bash:%v\033[0m\n",
						p.AutoApproveRead, p.AutoApproveWrite, p.AutoApproveBash)
					r.SetPolicy(tools.NewPolicyFromConfig("allowlist", askMode, nil))
				}
			}
		} else {
			fmt.Printf("[Event] %s\n", string(message))
		}
	}
}

// handleAgentMessageWithState processes a message from the server (with approval support)
func handleAgentMessageWithState(ctx context.Context, state *agentState, r *runner.Runner, sessions *session.Manager, message []byte) {
	fmt.Printf("[Agent-WS] Received message: %s\n", string(message))

	var frame struct {
		Type    string `json:"type"`
		ID      string `json:"id"`
		Method  string `json:"method"`
		Payload struct {
			Approved bool `json:"approved"`
			Always   bool `json:"always"`
		} `json:"payload"`
		Params struct {
			Prompt     string `json:"prompt"`
			SessionKey string `json:"session_key"`
			UserID     string `json:"user_id"`
		} `json:"params"`
	}

	if err := json.Unmarshal(message, &frame); err != nil {
		fmt.Fprintf(os.Stderr, "[Agent-WS] Invalid message: %v\n", err)
		return
	}

	fmt.Printf("[Agent-WS] Parsed frame: type=%s method=%s id=%s\n", frame.Type, frame.Method, frame.ID)

	switch frame.Type {
	case "approval_response":
		state.handleApprovalResponse(frame.ID, frame.Payload.Approved, frame.Payload.Always)

	case "req":
		switch frame.Method {
		case "ping":
			state.sendFrame(map[string]any{
				"type":    "res",
				"id":      frame.ID,
				"ok":      true,
				"payload": map[string]any{"pong": true},
			})

		case "introduce":
			// Agent introduction request for a new user
			sessionKey := frame.Params.SessionKey
			if sessionKey == "" {
				sessionKey = "companion"
			}
			userID := frame.Params.UserID

			fmt.Printf("[Agent-WS] Processing introduce request: session=%s user=%s\n", sessionKey, userID)

			// Handle introduction synchronously (not in goroutine) so we can respond
			handleIntroduction(ctx, state, r, sessions, frame.ID, sessionKey, userID)

		case "run", "generate_title":
			sessionKey := frame.Params.SessionKey
			if sessionKey == "" {
				sessionKey = "agent-" + frame.ID
			}
			userID := frame.Params.UserID

			fmt.Printf("[Agent-WS] Processing %s request: session=%s user=%s prompt=%q\n", frame.Method, sessionKey, userID, frame.Params.Prompt)

			events, err := r.Run(ctx, &runner.RunRequest{
				SessionKey: sessionKey,
				Prompt:     frame.Params.Prompt,
				UserID:     userID,
			})
			fmt.Printf("[Agent-WS] r.Run returned: err=%v events_nil=%v\n", err, events == nil)

			if err != nil {
				state.sendFrame(map[string]any{
					"type":  "res",
					"id":    frame.ID,
					"ok":    false,
					"error": err.Error(),
				})
				return
			}

			var result strings.Builder
			eventCount := 0
			for event := range events {
				eventCount++
				fmt.Printf("[Agent-WS] Event %d: type=%s\n", eventCount, event.Type)
				switch event.Type {
				case ai.EventTypeText:
					result.WriteString(event.Text)
					state.sendFrame(map[string]any{
						"type": "stream",
						"id":   frame.ID,
						"payload": map[string]any{
							"chunk": event.Text,
						},
					})

				case ai.EventTypeToolCall:
					state.sendFrame(map[string]any{
						"type": "stream",
						"id":   frame.ID,
						"payload": map[string]any{
							"tool":  event.ToolCall.Name,
							"input": event.ToolCall.Input,
						},
					})

				case ai.EventTypeToolResult:
					state.sendFrame(map[string]any{
						"type": "stream",
						"id":   frame.ID,
						"payload": map[string]any{
							"tool_result": event.Text,
						},
					})

				case ai.EventTypeError:
					fmt.Printf("[Agent-WS] Error event: %v\n", event.Error)
				}
			}

			fmt.Printf("[Agent-WS] Events loop complete: %d events, result_len=%d\n", eventCount, result.Len())
			state.sendFrame(map[string]any{
				"type": "res",
				"id":   frame.ID,
				"ok":   true,
				"payload": map[string]any{
					"result": result.String(),
				},
			})
			fmt.Printf("[Agent-WS] Sent final response for %s\n", frame.ID)

		default:
			state.sendFrame(map[string]any{
				"type":  "res",
				"id":    frame.ID,
				"ok":    false,
				"error": "unknown method: " + frame.Method,
			})
		}

	case "event":
		var eventFrame struct {
			Method  string `json:"method"`
			Payload struct {
				AutonomousMode   bool `json:"autonomousMode"`
				AutoApproveRead  bool `json:"autoApproveRead"`
				AutoApproveWrite bool `json:"autoApproveWrite"`
				AutoApproveBash  bool `json:"autoApproveBash"`
			} `json:"payload"`
		}
		if err := json.Unmarshal(message, &eventFrame); err == nil {
			switch eventFrame.Method {
			case "ready":
				// Server signals agent is fully connected and ready
				// Introduction is now handled by frontend via request_introduction message
				fmt.Println("[Agent] Received ready event from server")

			case "settings_updated":
				p := eventFrame.Payload
				if p.AutonomousMode {
					r.SetPolicy(tools.NewPolicyFromConfig("full", "off", nil))
				} else {
					askMode := "on-miss"
					if p.AutoApproveRead && p.AutoApproveWrite && p.AutoApproveBash {
						askMode = "off"
					}
					r.SetPolicy(tools.NewPolicyFromConfig("allowlist", askMode, nil))
				}
			}
		}
	}
}
