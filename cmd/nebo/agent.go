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

	"github.com/neboloop/nebo/internal/agent/advisors"
	"github.com/neboloop/nebo/internal/agent/ai"
	"github.com/neboloop/nebo/internal/agent/comm"
	"github.com/neboloop/nebo/internal/agent/comm/neboloop"
	neboloopapi "github.com/neboloop/nebo/internal/neboloop"
	agentcfg "github.com/neboloop/nebo/internal/agent/config"
	"github.com/neboloop/nebo/internal/agent/embeddings"
	agentmcp "github.com/neboloop/nebo/internal/agent/mcp"
	mcpbridge "github.com/neboloop/nebo/internal/mcp/bridge"
	"github.com/neboloop/nebo/internal/agent/memory"
	"github.com/neboloop/nebo/internal/agent/recovery"
	"github.com/neboloop/nebo/internal/agent/runner"
	"github.com/neboloop/nebo/internal/agent/session"
	"github.com/neboloop/nebo/internal/agent/skills"
	"github.com/neboloop/nebo/internal/agent/tools"
	"github.com/neboloop/nebo/internal/agenthub"
	"github.com/neboloop/nebo/internal/apps"
	"github.com/neboloop/nebo/internal/neboloop/sdk"
	"github.com/neboloop/nebo/internal/crashlog"
	"github.com/neboloop/nebo/internal/daemon"
	"github.com/neboloop/nebo/internal/browser"
	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/local"
	"github.com/neboloop/nebo/internal/apps/settings"
	"github.com/neboloop/nebo/internal/provider"
	"github.com/neboloop/nebo/internal/server"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/updater"
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

	// Lane-based work queue - implements supervisor pattern
	lanes *agenthub.LaneManager

	// Task recovery manager for persistence across restarts
	recovery *recovery.Manager

	// Comm system references for dynamic settings updates
	commManager  *comm.CommPluginManager
	commAgentID  string
	registry     *tools.Registry
	skillLoader  *skills.Loader

	// App registry for install listener activation on NeboLoop connect
	appRegistry *apps.AppRegistry

	// MCP bridge for external tool integrations
	mcpBridge *mcpbridge.Bridge

	// Heartbeat daemon for cron wake/enqueue (pointer-to-pointer: filled in by root.go/desktop.go after creation)
	heartbeat **daemon.Heartbeat
}

// sendFrame sends a JSON frame to the server
func (s *agentState) sendFrame(frame map[string]any) error {
	s.connMu.Lock()
	defer s.connMu.Unlock()
	data, _ := json.Marshal(frame)
	err := s.conn.WriteMessage(websocket.TextMessage, data)
	if err != nil {
		fmt.Printf("[Agent-WS] ERROR sending frame: %v\n", err)
	}
	return err
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
			if dangerously {
				if !confirmDangerousMode() {
					fmt.Println("Aborted.")
					os.Exit(0)
				}
			}

			cfg := loadAgentConfig()

			if serverURL == "" {
				serverURL = cfg.ServerURL
			}
			if serverURL == "" {
				serverURL = "http://localhost:27895"
			}

			fmt.Printf("Connecting to server: %s\n", serverURL)

			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()

			sigCh := make(chan os.Signal, 1)
			signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
			go func() {
				<-sigCh
				fmt.Println("\n\033[33mDisconnecting...\033[0m")
				cancel()
			}()

			opts := AgentOptions{
				Dangerously: dangerously,
			}

			if err := runAgent(ctx, cfg, serverURL, opts); err != nil {
				if ctx.Err() != nil {
					return // Clean shutdown
				}
				fmt.Fprintf(os.Stderr, "Agent error: %v\n", err)
				os.Exit(1)
			}
		},
	}

	cmd.Flags().StringVar(&serverURL, "server", "", "server URL (default: http://localhost:27895)")
	cmd.Flags().BoolVar(&dangerously, "dangerously", false, "100% autonomous mode - bypass ALL tool approval prompts")

	return cmd
}

// AgentOptions holds optional dependencies for the agent loop
type AgentOptions struct {
	Database         *sql.DB
	PluginStore      *settings.Store
	SvcCtx           *svc.ServiceContext   // For registering app capabilities with the HTTP layer
	Quiet            bool                // Suppress console output for clean CLI
	Dangerously      bool                // Bypass all tool approval prompts (CLI flag)
	AgentMCPProxy    *server.AgentMCPProxy // Lazy handler for CLI provider MCP loopback
	Heartbeat        **daemon.Heartbeat    // Pointer-to-pointer: set by root.go/desktop.go after creation
}

// isNeboLoopCode checks if a prompt is a NeboLoop connection code.
func isNeboLoopCode(prompt string) bool {
	prompt = strings.TrimSpace(prompt)
	if len(prompt) != 19 {
		return false
	}
	// Pattern: NEBO-XXXX-XXXX-XXXX (uppercase alphanumeric)
	if prompt[:5] != "NEBO-" || prompt[9] != '-' || prompt[14] != '-' {
		return false
	}
	for _, c := range prompt[5:9] + prompt[10:14] + prompt[15:] {
		if !((c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9')) {
			return false
		}
	}
	return true
}

// isLoopCode checks if a prompt is a NeboLoop loop invite code.
func isLoopCode(prompt string) bool {
	prompt = strings.TrimSpace(prompt)
	if len(prompt) != 19 {
		return false
	}
	// Pattern: LOOP-XXXX-XXXX-XXXX (uppercase alphanumeric)
	if prompt[:5] != "LOOP-" || prompt[9] != '-' || prompt[14] != '-' {
		return false
	}
	for _, c := range prompt[5:9] + prompt[10:14] + prompt[15:] {
		if !((c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9')) {
			return false
		}
	}
	return true
}

// handleNeboLoopCode processes a connection code and emits tool-use-style events.
// Returns true if the prompt was a connection code (handled), false otherwise.
// When state is provided, a successful connection also activates the neboloop comm plugin.
func handleNeboLoopCode(ctx context.Context, prompt, requestID string, pluginStore *settings.Store, state *agentState, send func(map[string]any)) bool {
	if !isNeboLoopCode(prompt) {
		return false
	}

	code := strings.TrimSpace(prompt)
	fmt.Printf("[NeboLoop] Connection code detected: %s\n", code)

	// Emit tool call event
	send(map[string]any{
		"type": "stream",
		"id":   requestID,
		"payload": map[string]any{
			"tool":  "neboloop_connect",
			"input": map[string]string{"code": code},
		},
	})

	// Resolve API server: env override > plugin store > default
	apiServer := neboloopapi.DefaultAPIServer
	if env := os.Getenv("NEBOLOOP_API_SERVER"); env != "" {
		apiServer = env
	} else if pluginStore != nil {
		if settings, err := pluginStore.GetSettingsByName(ctx, "neboloop"); err == nil && settings["api_server"] != "" {
			apiServer = settings["api_server"]
		}
	}

	// Get a name: hostname or "Nebo"
	botName, _ := os.Hostname()
	if botName == "" {
		botName = "Nebo"
	}

	// Step 1: Redeem code
	redeemed, err := neboloopapi.RedeemCode(ctx, apiServer, code, botName, "AI assistant")
	if err != nil {
		errMsg := fmt.Sprintf("Failed to redeem connection code: %s", err)
		fmt.Printf("[NeboLoop] %s\n", errMsg)
		send(map[string]any{
			"type": "stream",
			"id":   requestID,
			"payload": map[string]any{
				"tool_result": errMsg,
			},
		})
		send(map[string]any{
			"type": "stream",
			"id":   requestID,
			"payload": map[string]any{
				"chunk": "Couldn't connect to NeboLoop. The connection code may have expired or already been used. Please try generating a new one.",
			},
		})
		send(map[string]any{"type": "res", "id": requestID, "ok": true, "payload": map[string]any{"result": errMsg}})
		return true
	}

	// Step 2: Store credentials via PluginStore (triggers OnSettingsChanged → SDK auto-reconnect)
	if pluginStore != nil {
		p, err := pluginStore.GetPlugin(ctx, "neboloop")
		if err == nil {
			newSettings := map[string]string{
				"api_server": apiServer,
				"bot_id":     redeemed.ID,
				"api_key":    redeemed.ConnectionToken,
			}
			secrets := map[string]bool{
				"api_key": true,
			}
			if err := pluginStore.UpdateSettings(ctx, p.ID, newSettings, secrets); err != nil {
				fmt.Printf("[NeboLoop] Warning: failed to save credentials: %v\n", err)
			}
		} else {
			fmt.Printf("[NeboLoop] Warning: neboloop plugin not registered: %v\n", err)
		}
	}

	// Step 3: Activate the neboloop comm plugin and persist via settings
	if state != nil && state.commManager != nil {
		if err := state.commManager.SetActive("neboloop"); err != nil {
			fmt.Printf("[NeboLoop] Warning: failed to activate comm plugin: %v\n", err)
		} else if active := state.commManager.GetActive(); active != nil {
			commConfig := map[string]string{
				"api_server": apiServer,
				"bot_id":     redeemed.ID,
				"api_key":    redeemed.ConnectionToken,
			}
			if err := active.Connect(ctx, commConfig); err != nil {
				fmt.Printf("[NeboLoop] Warning: failed to connect comm plugin: %v\n", err)
			} else {
				card := buildAgentCard(state.registry, state.skillLoader)
				active.Register(ctx, state.commAgentID, card)
				fmt.Printf("[NeboLoop] Comm plugin activated and connected (agent: %s)\n", state.commAgentID)
			}
		}

		// Persist to settings so it survives restart
		if store := local.GetAgentSettings(); store != nil {
			s := store.Get()
			s.CommEnabled = true
			s.CommPlugin = "neboloop"
			if err := store.Update(s); err != nil {
				fmt.Printf("[NeboLoop] Warning: failed to persist comm settings: %v\n", err)
			} else {
				fmt.Println("[NeboLoop] Comm settings persisted (commEnabled=true, commPlugin=neboloop)")
			}
		}
	}

	// Emit success tool result
	resultText := fmt.Sprintf("Connected as %s (ID: %s)", redeemed.Name, redeemed.ID)
	fmt.Printf("[NeboLoop] %s\n", resultText)
	send(map[string]any{
		"type": "stream",
		"id":   requestID,
		"payload": map[string]any{
			"tool_result": resultText,
		},
	})

	// Emit text response
	successMsg := fmt.Sprintf("You're connected to NeboLoop! Your agent **%s** is now linked and ready to go.", redeemed.Name)
	send(map[string]any{
		"type": "stream",
		"id":   requestID,
		"payload": map[string]any{
			"chunk": successMsg,
		},
	})

	// Complete the request
	send(map[string]any{"type": "res", "id": requestID, "ok": true, "payload": map[string]any{"result": successMsg}})
	return true
}

// handleLoopCode processes a loop invite code and joins the bot to the loop.
// Returns true if the prompt was a loop code (handled), false otherwise.
// The bot must already be connected to NeboLoop (has credentials in plugin store).
func handleLoopCode(ctx context.Context, prompt, requestID string, pluginStore *settings.Store, send func(map[string]any)) bool {
	if !isLoopCode(prompt) {
		return false
	}

	code := strings.TrimSpace(prompt)
	fmt.Printf("[NeboLoop] Loop invite code detected: %s\n", code)

	// Emit tool call event
	send(map[string]any{
		"type": "stream",
		"id":   requestID,
		"payload": map[string]any{
			"tool":  "loop_join",
			"input": map[string]string{"code": code},
		},
	})

	// Get NeboLoop credentials from plugin store (bot must already be connected)
	if pluginStore == nil {
		errMsg := "Cannot join loop: settings not available"
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"tool_result": errMsg}})
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"chunk": errMsg}})
		send(map[string]any{"type": "res", "id": requestID, "ok": true, "payload": map[string]any{"result": errMsg}})
		return true
	}

	neboloopSettings, err := pluginStore.GetSettingsByName(ctx, "neboloop")
	if err != nil || neboloopSettings["bot_id"] == "" {
		errMsg := "You need to connect to NeboLoop first. Paste a NEBO-XXXX-XXXX-XXXX connection code to get started."
		fmt.Printf("[NeboLoop] %s\n", errMsg)
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"tool_result": errMsg}})
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"chunk": errMsg}})
		send(map[string]any{"type": "res", "id": requestID, "ok": true, "payload": map[string]any{"result": errMsg}})
		return true
	}

	// Create NeboLoop API client from stored credentials
	client, err := neboloopapi.NewClient(neboloopSettings)
	if err != nil {
		errMsg := fmt.Sprintf("Failed to create NeboLoop client: %s", err)
		fmt.Printf("[NeboLoop] %s\n", errMsg)
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"tool_result": errMsg}})
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"chunk": "Couldn't connect to NeboLoop. Please check your connection settings."}})
		send(map[string]any{"type": "res", "id": requestID, "ok": true, "payload": map[string]any{"result": errMsg}})
		return true
	}

	// Join the loop
	result, err := client.JoinLoop(ctx, code)
	if err != nil {
		errMsg := fmt.Sprintf("Failed to join loop: %s", err)
		fmt.Printf("[NeboLoop] %s\n", errMsg)
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"tool_result": errMsg}})
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"chunk": "Couldn't join the loop. The invite code may have expired or already been used. Please try a new one."}})
		send(map[string]any{"type": "res", "id": requestID, "ok": true, "payload": map[string]any{"result": errMsg}})
		return true
	}

	// Emit success
	resultText := fmt.Sprintf("Joined loop: %s (ID: %s)", result.Name, result.ID)
	fmt.Printf("[NeboLoop] %s\n", resultText)
	send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"tool_result": resultText}})

	successMsg := fmt.Sprintf("You've joined the **%s** loop! You can now communicate with other agents in this loop.", result.Name)
	send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"chunk": successMsg}})
	send(map[string]any{"type": "res", "id": requestID, "ok": true, "payload": map[string]any{"result": successMsg}})
	return true
}

// runAgent connects to the server and runs the agent loop.
// This is the single code path for all agent modes (RunAll, standalone, etc).
func runAgent(ctx context.Context, cfg *agentcfg.Config, serverURL string, opts AgentOptions) error {
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
		lanes:           agenthub.NewLaneManager(),
		heartbeat:       opts.Heartbeat,
	}

	// Forward lane events to the server for UI clients
	state.lanes.OnEvent(func(event agenthub.LaneEvent) {
		state.sendFrame(map[string]any{
			"type":   "event",
			"method": "lane_update",
			"payload": map[string]any{
				"event_type": event.Type,
				"lane":       event.Lane,
				"task":       event.Task,
			},
		})
	})

	// Use shared database if provided, otherwise open our own
	var sessions *session.Manager
	var sqlDB *sql.DB
	var store *db.Store
	if opts.Database != nil {
		sqlDB = opts.Database
		SetSharedDB(sqlDB)
	} else {
		store, err = db.NewSQLite(cfg.DBPath())
		if err != nil {
			return fmt.Errorf("failed to open database: %w", err)
		}
		defer store.Close()
		sqlDB = store.GetDB()
		SetSharedDB(sqlDB)
	}

	if opts.SvcCtx != nil {
		SetJanusURL(opts.SvcCtx.Config.NeboLoop.JanusURL)
	}

	sessions, err = session.New(sqlDB)
	if err != nil {
		return fmt.Errorf("failed to initialize sessions: %w", err)
	}

	// Initialize crash logger for persistent error tracking
	crashlog.Init(sqlDB)

	// Initialize recovery manager for task persistence across restarts
	state.recovery = recovery.NewManager(sqlDB)

	providers := createProviders(cfg)
	if len(providers) == 0 && !opts.Quiet {
		fmt.Fprintln(os.Stderr, "Warning: No AI providers configured. Tasks requiring AI will fail.")
	}

	// Initialize settings singleton if DB is available
	if sqlDB != nil {
		local.InitSettings(sqlDB)
	}

	policy := tools.NewPolicyFromConfig(
		cfg.Policy.Level,
		cfg.Policy.AskMode,
		cfg.Policy.Allowlist,
	)

	// Wire live autonomous check — reads from the singleton on every approval call
	if opts.Dangerously {
		policy.IsAutonomous = func() bool { return true }
	} else {
		policy.IsAutonomous = func() bool {
			if store := local.GetAgentSettings(); store != nil {
				return store.Get().AutonomousMode
			}
			return false
		}
	}

	// Store policy reference in state for "always" approval handling
	state.policy = policy

	var approvalCounter int64
	policy.ApprovalCallback = func(ctx context.Context, toolName string, input json.RawMessage) (bool, error) {
		approvalCounter++
		requestID := fmt.Sprintf("approval-%d-%d", time.Now().UnixNano(), approvalCounter)
		return state.requestApproval(ctx, requestID, toolName, input)
	}

	registry := tools.NewRegistry(policy)
	registry.RegisterDefaultsWithPermissions(loadToolPermissions(sqlDB))

	// Start browser manager for web automation
	browserMgr := browser.GetManager()
	if err := browserMgr.Start(browser.Config{
		Enabled:  true,
		Headless: true, // Default to headless for managed browser
	}); err != nil {
		fmt.Printf("[agent] Warning: failed to start browser manager: %v\n", err)
	} else {
		fmt.Println("[agent] Browser manager started")
		defer browserMgr.Stop()
	}

	// Create embedding service for hybrid memory search (only when enabled in config)
	// Prefer OpenAI, fall back to Ollama if available
	var embeddingService *embeddings.Service
	if opts.Database != nil && cfg.Memory.Embeddings {
		embeddingService = createEmbeddingService(opts.Database)
	}

	// Create memory tool for auto-extraction (requires shared database)
	var memoryTool *tools.MemoryTool
	if opts.Database != nil {
		memoryTool, err = tools.NewMemoryTool(tools.MemoryConfig{
			DB:              opts.Database,
			Embedder:        embeddingService,
			SanitizeContent: cfg.Memory.SanitizeContent,
		})
		if err == nil {
			// NOTE: memoryTool is NOT registered as a standalone tool.
			// Memory is only accessible via agent(resource: memory, action: ...).
			// The memoryTool instance is still used for r.SetMemoryTool() (auto-extraction)
			// and embedding backfill below.

			// Migrate stale embeddings then backfill (runs in background)
			if embeddingService != nil && embeddingService.HasProvider() {
				go func() {
					bgCtx := context.Background()
					// First: clear embeddings from old models (e.g., nomic-embed-text → qwen3-embedding)
					if stale, deleted, err := memoryTool.MigrateEmbeddings(bgCtx); err != nil {
						fmt.Printf("[agent] Embedding migration error: %v\n", err)
					} else if stale > 0 {
						fmt.Printf("[agent] Migrated embeddings: %d stale → %d deleted\n", stale, deleted)
					}
					// Then: backfill any memories without embeddings
					n, err := memoryTool.BackfillEmbeddings(bgCtx)
					if err != nil {
						fmt.Printf("[agent] Embedding backfill error: %v\n", err)
					} else if n > 0 {
						fmt.Printf("[agent] Backfilled embeddings for %d memories\n", n)
					}
				}()
			}
		}
	}

	// Load advisors (internal deliberation system)
	// Advisors are enabled/disabled via config and invoked by the agent when needed
	advisorLoader := advisors.NewLoader(cfg.AdvisorsDir())
	if err := advisorLoader.LoadAll(); err != nil {
		fmt.Printf("[agent] Warning: failed to load advisors: %v\n", err)
	} else if advisorLoader.Count() > 0 {
		fmt.Printf("[agent] Loaded %d advisors from %s\n", advisorLoader.Count(), cfg.AdvisorsDir())
	}
	// Load DB-backed advisors (override file-based ones with same name)
	if opts.Database != nil {
		dbAdvisors, err := db.New(opts.Database).ListAdvisors(context.Background())
		if err == nil && len(dbAdvisors) > 0 {
			advisorLoader.LoadFromDB(dbAdvisors)
		}
	}

	// Create advisors tool (the agent decides when to consult advisors)
	advisorsTool := tools.NewAdvisorsTool(advisorLoader)
	if len(providers) > 0 {
		advisorsTool.SetProvider(providers[0])
	}
	advisorsTool.SetSessionManager(sessions)
	if opts.Database != nil {
		advisorsTool.SetSearcher(embeddings.NewHybridSearcher(embeddings.HybridSearchConfig{
			DB:       opts.Database,
			Embedder: embeddingService,
		}))
	}
	registry.RegisterAdvisorsTool(advisorsTool)

	// Register message tool (channel sender wired after app registry is created)
	messageTool := tools.NewMessageTool()
	registry.Register(messageTool)

	// Register cron tool for scheduled tasks (requires shared database)
	var cronTool *tools.CronTool
	var schedulerMgr *tools.SchedulerManager
	if opts.Database != nil {
		cronTool, err = tools.NewCronTool(tools.CronConfig{DB: opts.Database})
		if err == nil {
			registry.Register(cronTool)
			schedulerMgr = tools.NewSchedulerManager(tools.NewCronScheduler(cronTool))
		}
	}

	taskTool := tools.NewTaskTool()

	// Wire up cron → agent execution (after runner is created)
	// We'll set this callback after creating the runner below
	taskTool.CreateOrchestrator(cfg, sessions, providers, registry)
	registry.Register(taskTool)
	defer taskTool.GetOrchestrator().Shutdown(context.Background())

	agentStatusTool := tools.NewAgentStatusTool()
	agentStatusTool.SetOrchestrator(taskTool.GetOrchestrator())
	registry.Register(agentStatusTool)

	// Create agent MCP server for CLI provider loopback (exposes all tools via MCP)
	mcpSrv := agentmcp.NewServer(registry)
	if opts.AgentMCPProxy != nil {
		opts.AgentMCPProxy.Set(mcpSrv.Handler())
		fmt.Println("[Agent] MCP server ready at /agent/mcp")
	}

	r := runner.New(cfg, sessions, providers, registry)

	// Provider loader is set below after appRegistry creation (needs gateway providers)

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

	// Forward-declare appRegistry so closures below can capture it
	var appRegistry *apps.AppRegistry

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
			// Re-register gateway aliases on the new matcher
			if appRegistry != nil {
				for _, gp := range appRegistry.GatewayProviders() {
					providerID := gp.ID()
					parts := strings.Split(providerID, ".")
					shortName := parts[len(parts)-1]
					newFuzzyMatcher.AddAlias(shortName, providerID+"/default")
				}
			}
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

	// Set up profile tracking for usage/error recording
	// Uses AuthProfileManager to track cooldowns and usage stats per auth profile
	if sqlDB != nil {
		if profileMgr, err := agentcfg.NewAuthProfileManager(sqlDB); err == nil {
			r.SetProfileTracker(profileMgr)
			fmt.Println("[agent] Profile tracking enabled")
		}
	}

	// Bridge MCP server context for CLI providers (claude-code, gemini-cli)
	r.SetMCPServer(mcpSrv)

	// Set up subagent persistence for surviving restarts
	if state.recovery != nil {
		r.SetupSubagentPersistence(state.recovery)
		// Recover any pending subagent tasks from previous run
		if recovered, err := r.RecoverSubagents(ctx); err != nil {
			fmt.Printf("[agent] Warning: failed to recover subagents: %v\n", err)
		} else if recovered > 0 {
			fmt.Printf("[agent] Recovered %d subagent task(s)\n", recovered)
		}
	}

	// Wire up cron agent task callback now that runner exists
	if cronTool != nil {
		cronTool.SetAgentCallback(func(ctx context.Context, name, message string, deliver *tools.DeliverConfig) error {

			// Run the agent task
			events, err := r.Run(ctx, &runner.RunRequest{
				SessionKey: fmt.Sprintf("routine-%s", name),
				Prompt:     message,
				Origin:     tools.OriginSystem,
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

			// Optionally deliver result to channel via app registry
			if deliver != nil && appRegistry != nil {
				if err := appRegistry.SendToChannel(ctx, deliver.Channel, deliver.To, result.String()); err != nil {
					fmt.Printf("[agent] Channel delivery failed: %v\n", err)
				}
			}

			return nil
		})
	}

	// Initialize comm system for inter-agent communication
	commManager := comm.NewCommPluginManager()
	agentID := cfg.Comm.AgentID
	if agentID == "" {
		agentID, _ = os.Hostname()
	}
	commHandler := comm.NewCommHandler(commManager, agentID)
	commManager.Register(comm.NewLoopbackPlugin())
	neboloopPlugin := neboloop.New()
	commManager.Register(neboloopPlugin)

	// Register Configurable plugins so their manifests are persisted
	// and settings changes trigger hot-reload via OnSettingsChanged
	if opts.PluginStore != nil {
		configurables := map[string]settings.Configurable{
			"neboloop": neboloopPlugin,
		}
		for name, c := range configurables {
			if err := opts.PluginStore.RegisterConfigurable(ctx, name, c); err != nil {
				fmt.Printf("[agent] Warning: failed to register %s configurable: %v\n", name, err)
			}
		}
	}

	// Resolve NeboLoop API URL from server config for signature verification
	var neboLoopURL string
	if opts.SvcCtx != nil {
		neboLoopURL = opts.SvcCtx.Config.NeboLoop.ApiURL
	}

	// Create unified skill tool — all apps and standalone skills register here
	userSkillsDir := filepath.Join(cfg.DataDir, "skills")
	os.MkdirAll(userSkillsDir, 0755)
	skillDomainTool := tools.NewSkillDomainTool(userSkillsDir)
	registry.Register(skillDomainTool)
	r.SetSkillProvider(skillDomainTool)

	// Discover and launch apps (NeboLoop app store + local dev)
	appRegistry = apps.NewAppRegistry(apps.AppRegistryConfig{
		DataDir:     cfg.DataDir,
		NeboLoopURL: neboLoopURL,
		Queries:     db.New(sqlDB),
		PluginStore: opts.PluginStore,
		ToolReg:     registry,
		SkillTool:   skillDomainTool,
		CommMgr:     commManager,
	})
	if err := appRegistry.DiscoverAndLaunch(ctx); err != nil {
		fmt.Printf("[agent] Warning: failed to discover apps: %v\n", err)
	}
	appRegistry.StartSupervisor(ctx)
	defer appRegistry.Stop()

	// Inject app catalog into agent system prompt so it knows about installed apps
	r.SetAppCatalog(appRegistry)
	appRegistry.OnQuarantine(func(event apps.QuarantineEvent) {
		state.sendFrame(map[string]any{
			"type":   "event",
			"method": "app_quarantined",
			"payload": map[string]any{
				"app_id":   event.AppID,
				"app_name": event.AppName,
				"reason":   event.Reason,
			},
		})
	})
	appRegistry.StartRevocationSweep(ctx)
	go func() {
		if err := appRegistry.Watch(ctx); err != nil {
			fmt.Printf("[agent] Warning: app watcher failed: %v\n", err)
		}
	}()

	// Wire schedule app adapter into the SchedulerManager if a schedule app was discovered
	if schedulerMgr != nil {
		if sa := appRegistry.ScheduleAdapter(); sa != nil {
			schedulerMgr.SetAppScheduler(sa)
			fmt.Println("[agent] Schedule app adapter connected to SchedulerManager")
		}
	}

	// Set provider loader for dynamic reload (after onboarding adds API key).
	// Must be after appRegistry creation so gateway providers are included.
	r.SetProviderLoader(func() []ai.Provider {
		base := createProviders(cfg)
		if appRegistry != nil {
			base = append(base, appRegistry.GatewayProviders()...)
		}
		return base
	})

	// Load gateway providers into runner (reload rebuilds providerMap)
	r.ReloadProviders()

	// Register gateway app names as fuzzy matcher aliases so users can "switch to janus"
	if modelsConfig != nil {
		fuzzyMatcher := ai.NewFuzzyMatcher(modelsConfig)
		for _, gp := range appRegistry.GatewayProviders() {
			// Provider ID is "gateway-com.neboloop.janus", alias is the app name
			providerID := gp.ID()
			// Extract short name from ID: "gateway-com.neboloop.janus" -> "janus"
			parts := strings.Split(providerID, ".")
			shortName := parts[len(parts)-1]
			fuzzyMatcher.AddAlias(shortName, providerID+"/default")
			fmt.Printf("[agent] Registered gateway alias: %s -> %s\n", shortName, providerID)
		}
		r.SetFuzzyMatcher(fuzzyMatcher)
	}

	// Trigger provider reload when a new gateway app is installed at runtime
	appRegistry.OnGatewayRegistered(func() {
		r.ReloadProviders()
		// Re-register gateway aliases
		if modelsConfig != nil {
			fuzzyMatcher := ai.NewFuzzyMatcher(modelsConfig)
			for _, gp := range appRegistry.GatewayProviders() {
				providerID := gp.ID()
				parts := strings.Split(providerID, ".")
				shortName := parts[len(parts)-1]
				fuzzyMatcher.AddAlias(shortName, providerID+"/default")
			}
			r.SetFuzzyMatcher(fuzzyMatcher)
		}
	})

	commManager.SetMessageHandler(commHandler.Handle)
	commHandler.SetRunner(r)
	commHandler.SetLanes(state.lanes)
	defer commManager.Shutdown(context.Background())

	// Load standalone skills and register them in the unified skill tool
	skillLoader := loadSkills(cfg)
	for _, s := range skillLoader.List() {
		if s.Enabled {
			skillDomainTool.Register(tools.Slugify(s.Name), s.Name, s.Description, s.Template, nil, s.Triggers, s.Priority)
		}
	}
	fmt.Printf("[agent] Registered %d standalone skills, %d total skills\n", skillLoader.Count(), skillDomainTool.Count())

	// Re-register skills when enabled/disabled state changes via the UI toggle
	opts.SvcCtx.SkillSettings.OnChange(func(name string, enabled bool) {
		slug := tools.Slugify(name)
		if enabled {
			// Find the skill from a fresh load and register it
			fresh := loadSkills(cfg)
			for _, s := range fresh.List() {
				if tools.Slugify(s.Name) == slug {
					skillDomainTool.Register(slug, s.Name, s.Description, s.Template, nil, s.Triggers, s.Priority)
					fmt.Printf("[agent] Skill %q enabled and registered\n", name)
					return
				}
			}
		} else {
			skillDomainTool.Unregister(slug)
			fmt.Printf("[agent] Skill %q disabled and unregistered\n", name)
		}
	})

	// Watch user skills directory for hot-reload (CRUD from API writes here)
	userSkillWatcher := skills.NewLoader(userSkillsDir)
	userSkillWatcher.OnChange(func(_ []*skills.Skill) {
		skillDomainTool.UnregisterStandalone()
		fresh := loadSkills(cfg)
		for _, s := range fresh.List() {
			if s.Enabled {
				skillDomainTool.Register(tools.Slugify(s.Name), s.Name, s.Description, s.Template, nil, s.Triggers, s.Priority)
			}
		}
		fmt.Printf("[agent] Skills reloaded: %d standalone, %d total\n", fresh.Count(), skillDomainTool.Count())
	})
	if err := userSkillWatcher.Watch(ctx); err != nil {
		fmt.Printf("[agent] Warning: failed to watch user skills dir: %v\n", err)
	}
	defer userSkillWatcher.Stop()

	// Store comm references on state for dynamic settings updates
	state.commManager = commManager
	state.commAgentID = agentID
	state.registry = registry
	state.skillLoader = skillLoader
	state.appRegistry = appRegistry

	// Wire SDK channel messages → agentic loop (replaces old MQTT channel bridge).
	// Install events and channel messages are delivered via the SDK's single WebSocket.
	neboloopPlugin.OnChannelMessage(func(msg sdk.ChannelMessage) {
		sessionKey := fmt.Sprintf("channel-%s-%s", msg.ChannelType, msg.ConversationID)

		fmt.Printf("[sdk:channels] Processing %s message from %s in session %s\n",
			msg.ChannelType, msg.SenderName, sessionKey)

		state.lanes.EnqueueAsync(ctx, agenthub.LaneMain, func(taskCtx context.Context) error {
			events, err := r.Run(taskCtx, &runner.RunRequest{
				SessionKey: sessionKey,
				Prompt:     msg.Text,
				Origin:     tools.OriginUser,
			})
			if err != nil {
				fmt.Printf("[sdk:channels] Run failed for %s: %v\n", msg.ChannelType, err)
				return err
			}

			var result strings.Builder
			for event := range events {
				if event.Type == ai.EventTypeText {
					result.WriteString(event.Text)
				} else if event.Type == ai.EventTypeMessage {
					if event.Message != nil && event.Message.Content != "" && result.Len() == 0 {
						result.WriteString(event.Message.Content)
					}
				}
			}

			if result.Len() > 0 {
				client := neboloopPlugin.Client()
				if client != nil {
					outMsg := sdk.ChannelMessage{
						ChannelType: msg.ChannelType,
						Text:        result.String(),
						ReplyTo:     msg.MessageID,
					}
					if err := client.SendChannelMessage(taskCtx, msg.ConversationID, outMsg); err != nil {
						fmt.Printf("[sdk:channels] SendChannelMessage failed: %v\n", err)
					}
				}
			}

			return nil
		}, agenthub.WithDescription(fmt.Sprintf("Channel: %s from %s", msg.ChannelType, msg.SenderName)))
	})

	// Wire SDK install events → app registry
	neboloopPlugin.OnInstall(func(evt sdk.InstallEvent) {
		appRegistry.HandleInstallEvent(ctx, evt)
	})

	// Wire local channel apps (e.g., voice) so their inbound messages are processed
	// through the agentic loop. Responses are sent back via SendToChannel.
	appRegistry.SetChannelHandler(func(channelType, channelID, userID, text, metadata string) {
		sessionKey := fmt.Sprintf("channel-%s-%s", channelType, channelID)

		fmt.Printf("[apps:local-channel] Processing %s message from %s in session %s\n",
			channelType, userID, sessionKey)

		state.lanes.EnqueueAsync(ctx, agenthub.LaneMain, func(taskCtx context.Context) error {
			events, err := r.Run(taskCtx, &runner.RunRequest{
				SessionKey: sessionKey,
				Prompt:     text,
				Origin:     tools.OriginUser,
			})
			if err != nil {
				fmt.Printf("[apps:local-channel] Run failed for %s/%s: %v\n", channelType, channelID, err)
				return err
			}

			var result strings.Builder
			for event := range events {
				if event.Type == ai.EventTypeText {
					result.WriteString(event.Text)
				} else if event.Type == ai.EventTypeMessage {
					if event.Message != nil && event.Message.Content != "" && result.Len() == 0 {
						result.WriteString(event.Message.Content)
					}
				}
			}

			if result.Len() > 0 {
				if err := appRegistry.SendToChannel(taskCtx, channelType, channelID, result.String()); err != nil {
					fmt.Printf("[apps:local-channel] SendToChannel failed: %v\n", err)
				}
			}

			return nil
		}, agenthub.WithDescription(fmt.Sprintf("Local channel: %s from %s", channelType, userID)))
	})

	// Register app UI provider, app registry, tool registry, and scheduler with the HTTP layer
	if opts.SvcCtx != nil {
		opts.SvcCtx.SetAppUIProvider(appRegistry)
		opts.SvcCtx.SetAppRegistry(appRegistry)
		opts.SvcCtx.SetToolRegistry(registry)
		if schedulerMgr != nil {
			opts.SvcCtx.SetScheduler(schedulerMgr)
		}

		// Wire OAuth broker to push tokens to apps
		if opts.SvcCtx.OAuthBroker != nil {
			opts.SvcCtx.OAuthBroker.SetAppReceiver(appRegistry)
			opts.SvcCtx.OAuthBroker.StartRefreshLoop(ctx)
		}
	}

	// Create agent domain tool with comm support
	// Reuses the existing memoryTool and cronTool instances (single DB connection)
	agentTool, agentToolErr := tools.NewAgentDomainTool(tools.AgentDomainConfig{
		Sessions:      sessions,
		ChannelSender: appRegistry,
		MemoryTool:    memoryTool,
		Scheduler:     schedulerMgr,
	})
	if agentToolErr == nil {
		agentTool.SetCommService(commHandler)
		// Share the orchestrator from taskTool so agent(resource:task) can spawn sub-agents
		agentTool.SetOrchestrator(taskTool.GetOrchestrator())
		registry.RegisterAgentDomainTool(agentTool)
	}

	// Wire channel sender to message tool (now that appRegistry is available)
	messageTool.SetChannelSender(appRegistry)

	// Wire MCP bridge for external tool integrations
	if opts.SvcCtx != nil && opts.SvcCtx.MCPClient != nil {
		mcpBridge := mcpbridge.New(registry, db.New(sqlDB), opts.SvcCtx.MCPClient)
		state.mcpBridge = mcpBridge
		go func() {
			if err := mcpBridge.SyncAll(ctx); err != nil {
				fmt.Printf("[agent] MCP bridge sync: %v\n", err)
			}
		}()
		defer mcpBridge.Close()
	}

	// Connect comm plugin: settings take priority, then config.yaml
	// This allows the UI/NeboLoop code to persist comm activation via settings
	commEnabled := cfg.Comm.Enabled
	commPlugin := cfg.Comm.Plugin

	// Check agent settings for comm overrides
	if store := local.GetAgentSettings(); store != nil {
		commSettings := store.Get()
		if commSettings.CommEnabled {
			commEnabled = true
			if commSettings.CommPlugin != "" {
				commPlugin = commSettings.CommPlugin
			}
		}
	}

	if commEnabled {
		if commPlugin == "" {
			commPlugin = "loopback"
		}
		if err := commManager.SetActive(commPlugin); err != nil {
			fmt.Printf("[agent] Warning: failed to set active comm plugin: %v\n", err)
		} else if active := commManager.GetActive(); active != nil {
			// Load settings from DB with fallback to config.yaml
			commConfig := cfg.Comm.Config
			if opts.PluginStore != nil {
				if dbSettings, err := opts.PluginStore.GetSettingsByName(ctx, commPlugin); err == nil && len(dbSettings) > 0 {
					commConfig = dbSettings
					fmt.Printf("[agent] Loaded %s settings from database (%d keys, broker=%s)\n", commPlugin, len(dbSettings), dbSettings["broker"])
				}
			}
			if err := active.Connect(ctx, commConfig); err != nil {
				fmt.Printf("[agent] Warning: failed to connect comm plugin %s: %v\n", commPlugin, err)
			} else {
				card := buildAgentCard(registry, skillLoader)
				active.Register(ctx, agentID, card)
				fmt.Printf("[agent] Comm plugin %s connected (agent: %s)\n", commPlugin, agentID)

				// Install events and channel messages are handled by the SDK
				// via neboloopPlugin.OnInstall and neboloopPlugin.OnChannelMessage
				// which were wired above — no separate connections needed.
			}
		}
	}

	// Background update checker: checks every 6 hours, notifies frontend once per new version.
	if opts.SvcCtx != nil && opts.SvcCtx.Version != "" && opts.SvcCtx.Version != "dev" {
		checker := updater.NewBackgroundChecker(opts.SvcCtx.Version, 6*time.Hour, func(result *updater.Result) {
			state.sendFrame(map[string]any{
				"type":   "event",
				"method": "update_available",
				"payload": map[string]any{
					"current_version": result.CurrentVersion,
					"latest_version":  result.LatestVersion,
					"release_url":     result.ReleaseURL,
					"release_notes":   result.ReleaseNotes,
				},
			})
		})
		go checker.Run(ctx)
	}

	// Agent-side keepalive: send pings to hub so the hub's readPump
	// receives data and resets its read deadline. This complements the
	// hub-side pings (which trigger auto-pong via gorilla/websocket).
	go func() {
		ticker := time.NewTicker(30 * time.Second)
		defer ticker.Stop()
		for {
			select {
			case <-ticker.C:
				state.connMu.Lock()
				err := conn.WriteMessage(websocket.PingMessage, nil)
				state.connMu.Unlock()
				if err != nil {
					return
				}
			case <-ctx.Done():
				return
			}
		}
	}()

	// Close connection when context is cancelled to unblock ReadMessage
	go func() {
		<-ctx.Done()
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
		go handleAgentMessageWithState(ctx, state, r, sessions, opts.PluginStore, message)
	}
}

// maybeIntroduceSelf checks if a user needs onboarding and proactively introduces the agent
// This is called on agent startup and checks the global companion session (legacy)
// For per-user introduction, see maybeIntroduceToUser
func maybeIntroduceSelf(ctx context.Context, state *agentState, r *runner.Runner, sessions *session.Manager) {
	// Legacy: Check global companion session for backwards compatibility
	// New multi-user onboarding is handled per-user in maybeIntroduceToUser
	companionSession, err := sessions.GetOrCreate("companion", "")
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
		System:     "You are starting a conversation with a new user. Follow the onboarding instructions in your system prompt to introduce yourself and get to know them.",
		Origin:     tools.OriginSystem,
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
		case ai.EventTypeMessage:
			// CLI provider may send text only in the assistant envelope
			if event.Message != nil && event.Message.Content != "" && result.Len() == 0 {
				result.WriteString(event.Message.Content)
				state.sendFrame(map[string]any{
					"type": "stream",
					"id":   "introduction",
					"payload": map[string]any{
						"chunk":      event.Message.Content,
						"session_id": companionSession.ID,
					},
				})
			}
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
	sess, err := sessions.GetOrCreate(sessionKey, userID)
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
	dbContext, err := memory.LoadContext(sessions.GetDB(), userID)
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
- Use layer "tacit", namespace "user", and key "name" to remember their name
- This ensures you'll remember them next time`
	}

	// Run the agent with appropriate introduction prompt
	events, err := r.Run(ctx, &runner.RunRequest{
		SessionKey: sessionKey,
		UserID:     userID,
		Prompt:     introPrompt,
		System:     introSystem,
		Origin:     tools.OriginSystem,
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
		case ai.EventTypeMessage:
			// CLI provider may send text only in the assistant envelope
			if event.Message != nil && event.Message.Content != "" && result.Len() == 0 {
				result.WriteString(event.Message.Content)
				state.sendFrame(map[string]any{
					"type": "stream",
					"id":   requestID,
					"payload": map[string]any{
						"chunk":      event.Message.Content,
						"session_id": sess.ID,
					},
				})
			}
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
	companionSession, err := sessions.GetOrCreate("companion", userID)
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
		System:     "You are starting a conversation with a new user. Follow the onboarding instructions in your system prompt to introduce yourself and get to know them.",
		Origin:     tools.OriginSystem,
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
		case ai.EventTypeMessage:
			// CLI provider may send text only in the assistant envelope
			if event.Message != nil && event.Message.Content != "" && result.Len() == 0 {
				result.WriteString(event.Message.Content)
				state.sendFrame(map[string]any{
					"type": "stream",
					"id":   "introduction-" + userID,
					"payload": map[string]any{
						"chunk":      event.Message.Content,
						"session_id": companionSession.ID,
					},
				})
			}
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

// handleAgentMessageWithState processes a message from the server (with approval support)
func handleAgentMessageWithState(ctx context.Context, state *agentState, r *runner.Runner, sessions *session.Manager, pluginStore *settings.Store, message []byte) {
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
			System     string `json:"system"`
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
			requestID := frame.ID

			fmt.Printf("[Agent-WS] Enqueueing introduce request: session=%s user=%s\n", sessionKey, userID)

			// SUPERVISOR PATTERN: Enqueue to main lane, don't block
			state.lanes.EnqueueAsync(ctx, agenthub.LaneMain, func(taskCtx context.Context) error {
				handleIntroduction(taskCtx, state, r, sessions, requestID, sessionKey, userID)
				return nil
			}, agenthub.WithDescription("Introduction"))

		case "get_lanes":
			stats := state.lanes.GetLaneStats()
			state.sendFrame(map[string]any{
				"type":    "res",
				"id":      frame.ID,
				"ok":      true,
				"payload": stats,
			})

		case "cancel":
			// Cancel active task in the main lane.
			// NOTE: We intentionally do NOT clear the lane queue here.
			// The frontend manages its own message queue and sends one at a time.
			// Clearing the lane queue races with new "run" frames arriving from
			// processQueue() — since each incoming frame is dispatched to a goroutine
			// (line ~1043), a new run frame can be enqueued before ClearLane runs,
			// causing the next queued message to be silently dropped.
			cancelled := state.lanes.CancelActive(agenthub.LaneMain)
			fmt.Printf("[Agent-WS] Cancel: cancelled %d active\n", cancelled)
			state.sendFrame(map[string]any{
				"type": "res",
				"id":   frame.ID,
				"ok":   true,
				"payload": map[string]any{
					"cancelled": cancelled,
				},
			})

		case "run", "generate_title":
			sessionKey := frame.Params.SessionKey
			if sessionKey == "" {
				sessionKey = "agent-" + frame.ID
			}
			userID := frame.Params.UserID
			requestID := frame.ID
			method := frame.Method
			prompt := frame.Params.Prompt

			// Intercept NeboLoop connection codes before enqueueing to LLM
			if handleNeboLoopCode(ctx, prompt, requestID, pluginStore, state, func(f map[string]any) {
				state.sendFrame(f)
			}) {
				break
			}

			// Intercept loop invite codes before enqueueing to LLM
			if handleLoopCode(ctx, prompt, requestID, pluginStore, func(f map[string]any) {
				state.sendFrame(f)
			}) {
				break
			}

			// Determine which lane this request belongs to
			isHeartbeat := strings.HasPrefix(sessionKey, "heartbeat-")
			isCronJob := strings.HasPrefix(sessionKey, "routine-")
			isCommMsg := strings.HasPrefix(sessionKey, "comm-")
			isDev := strings.HasPrefix(sessionKey, "dev-")
			lane := agenthub.LaneMain
			if isHeartbeat {
				lane = agenthub.LaneHeartbeat // Heartbeats run independently
			} else if isCronJob {
				lane = agenthub.LaneEvents // Scheduled/triggered tasks
			} else if isCommMsg {
				lane = agenthub.LaneComm // Inter-agent communication
			} else if isDev {
				lane = agenthub.LaneDev // Developer assistant
			}

			// Build a description for lane monitoring
			taskDesc := "User chat"
			if method == "generate_title" {
				taskDesc = "Generate title"
			} else if isHeartbeat {
				taskDesc = "Heartbeat tick"
			} else if isCronJob {
				taskDesc = fmt.Sprintf("Scheduled: %s", sessionKey)
			} else if isCommMsg {
				taskDesc = fmt.Sprintf("Comm: %s", sessionKey)
			} else if isDev {
				taskDesc = fmt.Sprintf("Dev: %s", sessionKey)
			}

			fmt.Printf("[Agent-WS] Enqueueing %s request: session=%s user=%s lane=%s prompt=%q\n",
				method, sessionKey, userID, lane, prompt)

			// SUPERVISOR PATTERN: Enqueue work to lane, don't block
			state.lanes.EnqueueAsync(ctx, lane, func(taskCtx context.Context) error {
				// Derive origin from lane (same logic that determined the lane)
				origin := tools.OriginUser
				if isHeartbeat || isCronJob {
					origin = tools.OriginSystem
				} else if isCommMsg {
					origin = tools.OriginComm
				}

				// This runs in a worker goroutine managed by the lane
				events, err := r.Run(taskCtx, &runner.RunRequest{
					SessionKey:        sessionKey,
					Prompt:            prompt,
					System:            frame.Params.System,
					UserID:            userID,
					SkipMemoryExtract: isHeartbeat,
					Origin:            origin,
				})

				if err != nil {
					state.sendFrame(map[string]any{
						"type":  "res",
						"id":    requestID,
						"ok":    false,
						"error": err.Error(),
					})
					return err
				}

				var result strings.Builder
				for event := range events {
					switch event.Type {
					case ai.EventTypeText:
						if event.Text == "" {
							continue // Skip empty text events
						}
						result.WriteString(event.Text)
						state.sendFrame(map[string]any{
							"type": "stream",
							"id":   requestID,
							"payload": map[string]any{
								"chunk": event.Text,
							},
						})

					case ai.EventTypeToolCall:
						fmt.Printf("[Agent-WS] Tool call: %s (id=%s)\n", event.ToolCall.Name, event.ToolCall.ID)
						state.sendFrame(map[string]any{
							"type": "stream",
							"id":   requestID,
							"payload": map[string]any{
								"tool":    event.ToolCall.Name,
								"tool_id": event.ToolCall.ID,
								"input":   event.ToolCall.Input,
							},
						})

					case ai.EventTypeToolResult:
						toolName := ""
						toolID := ""
						if event.ToolCall != nil {
							toolName = event.ToolCall.Name
							toolID = event.ToolCall.ID
						}
						fmt.Printf("[Agent-WS] Tool result: %s (id=%s) len=%d\n", toolName, toolID, len(event.Text))
						state.sendFrame(map[string]any{
							"type": "stream",
							"id":   requestID,
							"payload": map[string]any{
								"tool_result": event.Text,
								"tool_name":   toolName,
								"tool_id":     toolID,
							},
						})

					case ai.EventTypeThinking:
						// Send thinking/reasoning content to frontend
						state.sendFrame(map[string]any{
							"type": "stream",
							"id":   requestID,
							"payload": map[string]any{
								"thinking": event.Text,
							},
						})

					case ai.EventTypeMessage:
						if event.Message == nil {
							continue
						}

						// Forward text content (CLI provider may send text only in the assistant envelope)
						if event.Message.Content != "" && result.Len() == 0 {
							result.WriteString(event.Message.Content)
							state.sendFrame(map[string]any{
								"type": "stream",
								"id":   requestID,
								"payload": map[string]any{
									"chunk": event.Message.Content,
								},
							})
						}

						// NOTE: ToolCalls are NOT forwarded here — they arrive via
						// EventTypeToolCall from the CLI provider's content_block pre-parse.
						// Forwarding them here would cause duplicate ToolCards.

						// Forward tool results (CLI provider embeds these in user messages)
						if len(event.Message.ToolResults) > 0 {
							var toolResults []session.ToolResult
							if err := json.Unmarshal(event.Message.ToolResults, &toolResults); err == nil {
								for _, tr := range toolResults {
									fmt.Printf("[Agent-WS] Tool result (from message): id=%s len=%d\n", tr.ToolCallID, len(tr.Content))
									state.sendFrame(map[string]any{
										"type": "stream",
										"id":   requestID,
										"payload": map[string]any{
											"tool_result": tr.Content,
											"tool_id":     tr.ToolCallID,
										},
									})
								}
							}
						}

					case ai.EventTypeError:
						fmt.Printf("[Agent-WS] Error event: %v\n", event.Error)
					}
				}

				// If a cron job just completed, enqueue its result into heartbeat
				if isCronJob && state.heartbeat != nil && *state.heartbeat != nil {
					summary := result.String()
					if len(summary) > 200 {
						summary = summary[:200]
					}
					(*state.heartbeat).Enqueue(daemon.HeartbeatEvent{
						Source:    "cron:" + sessionKey,
						Summary:  summary,
						Timestamp: time.Now(),
					})
					(*state.heartbeat).Wake("cron:" + sessionKey)
				}

				state.sendFrame(map[string]any{
					"type": "res",
					"id":   requestID,
					"ok":   true,
					"payload": map[string]any{
						"result": result.String(),
					},
				})
				fmt.Printf("[Agent-WS] Completed request %s\n", requestID)
				return nil
			}, agenthub.WithDescription(taskDesc))

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
				AutonomousMode   bool   `json:"autonomousMode"`
				AutoApproveRead  bool   `json:"autoApproveRead"`
				AutoApproveWrite bool   `json:"autoApproveWrite"`
				AutoApproveBash  bool   `json:"autoApproveBash"`
				CommEnabled      bool   `json:"commEnabled"`
				CommPlugin       string `json:"commPlugin"`
			} `json:"payload"`
		}
		if err := json.Unmarshal(message, &eventFrame); err == nil {
			switch eventFrame.Method {
			case "ready":
				// Server signals agent is fully connected and ready
				// Introduction is now handled by frontend via request_introduction message
				fmt.Println("[Agent] Received ready event from server")

				// Recover any incomplete tasks from previous session
				go recoverPendingTasks(ctx, state, r, sessions)

			case "settings_updated":
				// Reload providers — a new NeboLoop profile may have been created via OAuth
				r.ReloadProviders()

				// Policy reads from the singleton live — no need to replace it.
				// Handle comm plugin activation/deactivation.
				p := eventFrame.Payload
				if state.commManager != nil {
					handleCommSettingsUpdate(ctx, state, p.CommEnabled, p.CommPlugin, pluginStore)
				}

			case "integrations_changed":
				// Re-sync MCP bridge when integrations are modified via API
				if state.mcpBridge != nil {
					go func() {
						if err := state.mcpBridge.SyncAll(ctx); err != nil {
							fmt.Printf("[agent] MCP bridge re-sync: %v\n", err)
						}
					}()
				}
			}
		}
	}
}

// recoverPendingTasks checks for incomplete tasks from previous sessions and re-runs them
func recoverPendingTasks(ctx context.Context, state *agentState, r *runner.Runner, sessions *session.Manager) {
	if state.recovery == nil {
		return
	}

	tasks, err := state.recovery.RecoverTasks(ctx)
	if err != nil {
		fmt.Printf("[Recovery] Failed to recover tasks: %v\n", err)
		return
	}

	if len(tasks) == 0 {
		fmt.Println("[Recovery] No incomplete tasks to recover")
		return
	}

	fmt.Printf("[Recovery] Found %d incomplete task(s) to recover\n", len(tasks))

	for _, task := range tasks {
		// Skip if too many attempts
		if task.Attempts >= task.MaxAttempts {
			fmt.Printf("[Recovery] Task %s exceeded max attempts (%d), marking failed\n", task.ID, task.MaxAttempts)
			state.recovery.MarkFailed(ctx, task.ID, "exceeded max attempts")
			continue
		}

		// Determine which lane to use based on task type
		lane := agenthub.LaneMain
		switch task.TaskType {
		case recovery.TaskTypeEventAgent:
			lane = agenthub.LaneEvents
		case recovery.TaskTypeSubagent:
			lane = agenthub.LaneSubagent
		}

		fmt.Printf("[Recovery] Re-enqueueing task %s: type=%s lane=%s prompt=%q\n",
			task.ID, task.TaskType, lane, truncatePrompt(task.Prompt, 50))

		// Capture task for closure
		t := task

		// Enqueue the recovered task
		state.lanes.EnqueueAsync(ctx, lane, func(taskCtx context.Context) error {
			// Mark task as running
			if err := state.recovery.MarkRunning(taskCtx, t.ID); err != nil {
				fmt.Printf("[Recovery] Failed to mark task %s as running: %v\n", t.ID, err)
			}

			// Run the task
			events, err := r.Run(taskCtx, &runner.RunRequest{
				SessionKey: t.SessionKey,
				Prompt:     t.Prompt,
				System:     t.SystemPrompt,
				UserID:     t.UserID,
				Origin:     tools.OriginSystem,
			})

			if err != nil {
				fmt.Printf("[Recovery] Task %s failed: %v\n", t.ID, err)
				state.recovery.MarkFailed(taskCtx, t.ID, err.Error())
				return err
			}

			// Consume the event stream
			var result strings.Builder
			for event := range events {
				if event.Type == ai.EventTypeText {
					result.WriteString(event.Text)
				}
			}

			// Mark task as completed
			if err := state.recovery.MarkCompleted(taskCtx, t.ID); err != nil {
				fmt.Printf("[Recovery] Failed to mark task %s as completed: %v\n", t.ID, err)
			} else {
				fmt.Printf("[Recovery] Task %s completed successfully\n", t.ID)
			}

			return nil
		}, agenthub.WithDescription(fmt.Sprintf("Recovery: %s", t.Description)))
	}

	// Schedule periodic cleanup of old tasks
	go func() {
		ticker := time.NewTicker(24 * time.Hour)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				if deleted, err := state.recovery.CleanupOldTasks(ctx); err != nil {
					fmt.Printf("[Recovery] Cleanup error: %v\n", err)
				} else if deleted > 0 {
					fmt.Printf("[Recovery] Cleaned up %d old tasks\n", deleted)
				}
			}
		}
	}()
}

// truncatePrompt truncates a prompt string for logging
func truncatePrompt(s string, maxLen int) string {
	if len(s) <= maxLen {
		return s
	}
	return s[:maxLen] + "..."
}

// persistAndRunTask creates a persistent task record and runs it
// This enables recovery if the agent restarts mid-task
func persistAndRunTask(
	ctx context.Context,
	state *agentState,
	r *runner.Runner,
	req *runner.RunRequest,
	taskType recovery.TaskType,
	description string,
) (taskID string, events <-chan ai.StreamEvent, err error) {
	// Create a persistent task record
	if state.recovery != nil {
		task := &recovery.PendingTask{
			TaskType:     taskType,
			SessionKey:   req.SessionKey,
			UserID:       req.UserID,
			Prompt:       req.Prompt,
			SystemPrompt: req.System,
			Description:  description,
			Lane:         string(agenthub.LaneMain),
		}
		if err := state.recovery.CreateTask(ctx, task); err != nil {
			fmt.Printf("[Recovery] Warning: failed to persist task: %v\n", err)
		} else {
			taskID = task.ID
			// Mark as running
			state.recovery.MarkRunning(ctx, taskID)
		}
	}

	// Run the task
	events, err = r.Run(ctx, req)
	return taskID, events, err
}

// completeTask marks a persistent task as completed
func completeTask(ctx context.Context, state *agentState, taskID string) {
	if taskID != "" && state.recovery != nil {
		state.recovery.MarkCompleted(ctx, taskID)
	}
}

// failTask marks a persistent task as failed
func failTask(ctx context.Context, state *agentState, taskID string, errMsg string) {
	if taskID != "" && state.recovery != nil {
		state.recovery.MarkFailed(ctx, taskID, errMsg)
	}
}

// createEmbeddingService creates an embedding service for hybrid memory search.
// Uses DB auth_profiles as the single source of truth for API keys.
func createEmbeddingService(db *sql.DB) *embeddings.Service {
	if db == nil {
		return nil
	}

	var embeddingProvider embeddings.Provider

	// Load from DB auth_profiles — same source as createProviders
	mgr, err := agentcfg.NewAuthProfileManager(db)
	if err == nil {
		defer mgr.Close()
		ctx := context.Background()

		// Try OpenAI first (most common, high quality embeddings)
		if profiles, err := mgr.ListActiveProfiles(ctx, "openai"); err == nil {
			for _, p := range profiles {
				if p.APIKey != "" {
					embeddingProvider = embeddings.NewOpenAIProvider(embeddings.OpenAIConfig{
						APIKey: p.APIKey,
					})
					fmt.Println("[agent] Embeddings: using OpenAI text-embedding-3-small")
					break
				}
			}
		}

		// Fall back to Ollama if configured
		if embeddingProvider == nil {
			if profiles, err := mgr.ListActiveProfiles(ctx, "ollama"); err == nil {
				for _, p := range profiles {
					baseURL := p.BaseURL
					if baseURL == "" {
						baseURL = "http://localhost:11434"
					}
					// Auto-pull the embedding model if not present
					embModel := "qwen3-embedding"
					if err := ai.EnsureOllamaModel(baseURL, embModel); err != nil {
						fmt.Printf("[agent] Warning: could not ensure embedding model %s: %v\n", embModel, err)
					}
					embeddingProvider = embeddings.NewOllamaProvider(embeddings.OllamaConfig{
						BaseURL: baseURL,
					})
					fmt.Println("[agent] Embeddings: using Ollama qwen3-embedding")
					break
				}
			}
		}
	}

	// No embedding provider available
	if embeddingProvider == nil {
		fmt.Println("[agent] Embeddings: disabled (no OpenAI or Ollama auth profile configured)")
		return nil
	}

	// Create service with caching
	service, err := embeddings.NewService(embeddings.Config{
		DB:       db,
		Provider: embeddingProvider,
	})
	if err != nil {
		fmt.Printf("[agent] Warning: failed to create embedding service: %v\n", err)
		return nil
	}

	return service
}

// handleCommSettingsUpdate activates or deactivates the comm plugin based on
// a settings_updated event from the server.
func handleCommSettingsUpdate(ctx context.Context, state *agentState, enabled bool, pluginName string, pluginStore *settings.Store) {
	if !enabled {
		// Deactivate: disconnect the current plugin
		if active := state.commManager.GetActive(); active != nil {
			if err := active.Disconnect(ctx); err != nil {
				fmt.Printf("[Comm] Warning: failed to disconnect comm plugin: %v\n", err)
			}
			fmt.Println("[Comm] Comm plugin disconnected via settings update")
		}
		return
	}

	if pluginName == "" {
		pluginName = "loopback"
	}

	// Activate the requested plugin
	if err := state.commManager.SetActive(pluginName); err != nil {
		fmt.Printf("[Comm] Warning: failed to activate %s: %v\n", pluginName, err)
		return
	}

	active := state.commManager.GetActive()
	if active == nil {
		return
	}

	// Load settings from DB
	var commConfig map[string]string
	if pluginStore != nil {
		if dbSettings, err := pluginStore.GetSettingsByName(ctx, pluginName); err == nil && len(dbSettings) > 0 {
			commConfig = dbSettings
			fmt.Printf("[Comm] Loaded %s settings from database (%d keys)\n", pluginName, len(dbSettings))
		}
	}

	if err := active.Connect(ctx, commConfig); err != nil {
		fmt.Printf("[Comm] Warning: failed to connect %s: %v\n", pluginName, err)
		return
	}

	card := buildAgentCard(state.registry, state.skillLoader)
	active.Register(ctx, state.commAgentID, card)
	fmt.Printf("[Comm] Plugin %s activated and connected via settings update (agent: %s)\n", pluginName, state.commAgentID)

	// Install events and channel messages are handled by the SDK
	// via neboloopPlugin.OnInstall and neboloopPlugin.OnChannelMessage
	// which were wired during agent startup — no separate connections needed.
}

// buildAgentCard collects tool and skill metadata into an A2A-spec-compliant
// AgentCard for comm registration and NeboLoop discovery.
func buildAgentCard(registry *tools.Registry, skillLoader *skills.Loader) *comm.AgentCard {
	card := &comm.AgentCard{
		Name:               "Nebo",
		Description:        "Nebo AI agent with lane-based concurrency",
		PreferredTransport: "jsonrpc",
		ProtocolVersion:    "1.0",
		DefaultInputModes:  []string{"text/plain"},
		DefaultOutputModes: []string{"text/plain"},
		Capabilities:       map[string]any{"streaming": false},
		Provider:           &comm.AgentCardProvider{Organization: "Nebo"},
	}

	if skillLoader != nil {
		for _, skill := range skillLoader.List() {
			if !skill.Enabled {
				continue
			}
			card.Skills = append(card.Skills, comm.AgentCardSkill{
				ID:          skill.Name,
				Name:        skill.Name,
				Description: skill.Description,
				Tags:        skill.Tags,
			})
		}
	}

	return card
}

// loadSkills loads standalone skills from extensions and user directories.
func loadSkills(cfg *agentcfg.Config) *skills.Loader {
	loader := skills.NewLoader(filepath.Join("extensions", "skills"))
	if err := loader.LoadAll(); err != nil {
		fmt.Printf("[agent] Warning: failed to load skills: %v\n", err)
	}

	// Merge user skills from data directory
	userLoader := skills.NewLoader(filepath.Join(cfg.DataDir, "skills"))
	if err := userLoader.LoadAll(); err == nil {
		for _, s := range userLoader.List() {
			loader.Add(s)
		}
	}

	// Apply disabled skills from settings
	disabledSkills := loadDisabledSkills(cfg.DataDir)
	if len(disabledSkills) > 0 {
		loader.SetDisabledSkills(disabledSkills)
	}

	return loader
}

// loadDisabledSkills reads the skill-settings.json file and returns disabled skill names.
func loadDisabledSkills(dataDir string) []string {
	data, err := os.ReadFile(filepath.Join(dataDir, "skill-settings.json"))
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
