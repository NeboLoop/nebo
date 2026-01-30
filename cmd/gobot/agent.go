package cli

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"os"
	"os/signal"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/gorilla/websocket"
	"github.com/spf13/cobra"

	"gobot/agent/ai"
	agentcfg "gobot/agent/config"
	"gobot/agent/runner"
	"gobot/agent/session"
	"gobot/agent/tools"
	"gobot/internal/channels"
	"gobot/internal/provider"
)

// agentState holds the state for a connected agent
type agentState struct {
	conn            *websocket.Conn
	connMu          sync.Mutex
	pendingApproval map[string]chan bool
	approvalMu      sync.RWMutex
	quiet           bool // Suppress console output for clean CLI
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
	respCh := make(chan bool, 1)
	s.approvalMu.Lock()
	s.pendingApproval[requestID] = respCh
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
	case approved := <-respCh:
		return approved, nil
	case <-ctx.Done():
		return false, ctx.Err()
	}
}

// handleApprovalResponse processes an approval response from the server
func (s *agentState) handleApprovalResponse(requestID string, approved bool) {
	s.approvalMu.RLock()
	ch, ok := s.pendingApproval[requestID]
	s.approvalMu.RUnlock()
	if ok {
		select {
		case ch <- approved:
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
		Long: `Start the GoBot AI agent to receive and process tasks from the web UI.

The agent connects to the local GoBot server and processes chat messages
using configured AI providers (Anthropic, OpenAI, etc.)

Examples:
  gobot agent                    # Start the agent
  gobot agent --dangerously      # Autonomous mode (no approval prompts)`,
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
	ChannelManager *channels.Manager
	Database       *sql.DB
	Quiet          bool // Suppress console output for clean CLI
	Dangerously    bool // Bypass all tool approval prompts
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
		pendingApproval: make(map[string]chan bool),
		quiet:           opts.Quiet,
	}

	sessions, err := session.New(cfg.DBPath())
	if err != nil {
		return fmt.Errorf("failed to open database: %w", err)
	}
	defer sessions.Close()

	providers := createProviders(cfg)
	if len(providers) == 0 && !opts.Quiet {
		fmt.Fprintln(os.Stderr, "Warning: No AI providers configured. Tasks requiring AI will fail.")
	}

	// In dangerously mode, use "full" policy level to bypass all approvals
	policyLevel := cfg.Policy.Level
	if opts.Dangerously {
		policyLevel = "full"
	}

	policy := tools.NewPolicyFromConfig(
		policyLevel,
		cfg.Policy.AskMode,
		cfg.Policy.Allowlist,
	)

	var approvalCounter int64
	if opts.Dangerously {
		// Auto-approve everything in dangerous mode
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

	// Set up task-based model selector for intelligent model routing
	modelsConfig := provider.GetModelsConfig()
	if modelsConfig != nil {
		if modelsConfig.TaskRouting != nil {
			selector := ai.NewModelSelector(modelsConfig)
			r.SetModelSelector(selector)
		}
		// Set up fuzzy matcher for user model switch requests
		fuzzyMatcher := ai.NewFuzzyMatcher(modelsConfig)
		r.SetFuzzyMatcher(fuzzyMatcher)
	}

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
		handleAgentMessageWithState(ctx, state, r, message)
	}
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

	// Create memory tool for auto-extraction
	memoryTool, memErr := tools.NewMemoryTool(tools.MemoryConfig{})
	if memErr != nil {
		fmt.Printf("[agent] Warning: failed to initialize memory tool: %v\n", memErr)
	} else {
		registry.Register(memoryTool)
	}

	taskTool := tools.NewTaskTool()
	taskTool.CreateOrchestrator(cfg, sessions, providers, registry)
	registry.Register(taskTool)

	agentStatusTool := tools.NewAgentStatusTool()
	agentStatusTool.SetOrchestrator(taskTool.GetOrchestrator())
	registry.Register(agentStatusTool)

	r := runner.New(cfg, sessions, providers, registry)

	// Set up task-based model selector for intelligent model routing
	modelsConfig := provider.GetModelsConfig()
	if modelsConfig != nil {
		if modelsConfig.TaskRouting != nil {
			selector := ai.NewModelSelector(modelsConfig)
			r.SetModelSelector(selector)
		}
		// Set up fuzzy matcher for user model switch requests
		fuzzyMatcher := ai.NewFuzzyMatcher(modelsConfig)
		r.SetFuzzyMatcher(fuzzyMatcher)
	}

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

		case "run", "generate_title":
			sessionKey := frame.Params.SessionKey
			if sessionKey == "" {
				sessionKey = "agent-" + frame.ID
			}

			fmt.Printf("\n[Agent] Received %s request: id=%s session=%s prompt=%q\n", frame.Method, frame.ID, sessionKey, frame.Params.Prompt)

			if frame.Method == "generate_title" {
				fmt.Printf("\n\033[90m[Title Gen %s]\033[0m\n", frame.ID)
			} else {
				fmt.Printf("\n\033[36m[Task %s]\033[0m %s\n", frame.ID, frame.Params.Prompt)
			}

			events, err := r.Run(ctx, &runner.RunRequest{
				SessionKey: sessionKey,
				Prompt:     frame.Params.Prompt,
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
func handleAgentMessageWithState(ctx context.Context, state *agentState, r *runner.Runner, message []byte) {
	var frame struct {
		Type    string `json:"type"`
		ID      string `json:"id"`
		Method  string `json:"method"`
		Payload struct {
			Approved bool `json:"approved"`
		} `json:"payload"`
		Params struct {
			Prompt     string `json:"prompt"`
			SessionKey string `json:"session_key"`
		} `json:"params"`
	}

	if err := json.Unmarshal(message, &frame); err != nil {
		fmt.Fprintf(os.Stderr, "Invalid message: %v\n", err)
		return
	}

	switch frame.Type {
	case "approval_response":
		state.handleApprovalResponse(frame.ID, frame.Payload.Approved)

	case "req":
		switch frame.Method {
		case "ping":
			state.sendFrame(map[string]any{
				"type":    "res",
				"id":      frame.ID,
				"ok":      true,
				"payload": map[string]any{"pong": true},
			})

		case "run", "generate_title":
			sessionKey := frame.Params.SessionKey
			if sessionKey == "" {
				sessionKey = "agent-" + frame.ID
			}

			events, err := r.Run(ctx, &runner.RunRequest{
				SessionKey: sessionKey,
				Prompt:     frame.Params.Prompt,
			})

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
			for event := range events {
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
				}
			}

			state.sendFrame(map[string]any{
				"type": "res",
				"id":   frame.ID,
				"ok":   true,
				"payload": map[string]any{
					"result": result.String(),
				},
			})

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
			if eventFrame.Method == "settings_updated" {
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
