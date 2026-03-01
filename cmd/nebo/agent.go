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

	"github.com/google/uuid"
	"github.com/gorilla/websocket"
	"github.com/spf13/cobra"

	"github.com/neboloop/nebo/extensions"
	"github.com/neboloop/nebo/internal/agent/advisors"
	"github.com/neboloop/nebo/internal/agent/ai"
	"github.com/neboloop/nebo/internal/agent/comm"
	"github.com/neboloop/nebo/internal/agent/comm/neboloop"
	neboloophandler "github.com/neboloop/nebo/internal/handler/neboloop"
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
	neboloopsdk "github.com/NeboLoop/neboloop-go-sdk"
	"github.com/neboloop/nebo/internal/agenthub"
	"github.com/neboloop/nebo/internal/apps"
	"github.com/neboloop/nebo/internal/crashlog"
	"github.com/neboloop/nebo/internal/defaults"
	"github.com/neboloop/nebo/internal/devlog"
	"github.com/neboloop/nebo/internal/daemon"
	"github.com/neboloop/nebo/internal/browser"
	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/local"
	"github.com/neboloop/nebo/internal/markdown"
	"github.com/neboloop/nebo/internal/apps/settings"
	"github.com/neboloop/nebo/internal/notify"
	"github.com/neboloop/nebo/internal/provider"
	"github.com/neboloop/nebo/internal/server"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/updater"
	"github.com/neboloop/nebo/internal/voice"
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
	pendingAsk      map[string]chan string
	pendingAskMu    sync.RWMutex
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

	// NeboLoop: stored for dynamic reconnection on settings_updated
	sqlDB    *sql.DB
	botID    string
	commsURL string // Gateway URL from config, injected into comm plugin
	apiURL   string // API URL from config, used as fallback for code redemption

	// MCP bridge for external tool integrations
	mcpBridge *mcpbridge.Bridge

	// Heartbeat daemon for cron wake/enqueue (pointer-to-pointer: filled in by root.go/desktop.go after creation)
	heartbeat **daemon.Heartbeat

	// Cached companion chat session ID (set by first web UI chat request, reused by DM handler)
	companionChatID   string
	companionChatIDMu sync.RWMutex
}

// sendFrame sends a JSON frame to the server
func (s *agentState) sendFrame(frame map[string]any) error {
	s.connMu.Lock()
	defer s.connMu.Unlock()
	data, err := json.Marshal(frame)
	if err != nil {
		devlog.Printf("[Agent-WS] ERROR marshaling frame: %v (type=%v)\n", err, frame["type"])
		return err
	}
	if err := s.conn.WriteMessage(websocket.TextMessage, data); err != nil {
		devlog.Printf("[Agent-WS] ERROR sending frame: %v\n", err)
		return err
	}
	return nil
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
			if toolName == "bash" || toolName == "shell" {
				var shellInput struct {
					Command string `json:"command"`
				}
				if err := json.Unmarshal(input, &shellInput); err == nil {
					inputStr = shellInput.Command
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

// requestAsk sends an interactive prompt to the UI and blocks until the user responds
func (s *agentState) requestAsk(ctx context.Context, requestID, prompt string, widgets []tools.AskWidget) (string, error) {
	respCh := make(chan string, 1)
	s.pendingAskMu.Lock()
	s.pendingAsk[requestID] = respCh
	s.pendingAskMu.Unlock()

	defer func() {
		s.pendingAskMu.Lock()
		delete(s.pendingAsk, requestID)
		s.pendingAskMu.Unlock()
	}()

	widgetsJSON, _ := json.Marshal(widgets)
	frame := map[string]any{
		"type": "ask_request",
		"id":   requestID,
		"payload": map[string]any{
			"prompt":  prompt,
			"widgets": json.RawMessage(widgetsJSON),
		},
	}
	if err := s.sendFrame(frame); err != nil {
		return "", err
	}

	select {
	case value := <-respCh:
		return value, nil
	case <-ctx.Done():
		return "", ctx.Err()
	}
}

// handleAskResponse processes an ask response from the server
func (s *agentState) handleAskResponse(requestID, value string) {
	s.pendingAskMu.RLock()
	ch, ok := s.pendingAsk[requestID]
	s.pendingAskMu.RUnlock()
	if ok && ch != nil {
		select {
		case ch <- value:
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
	AgentMCPProxy       *server.AgentMCPProxy    // Lazy handler for CLI provider MCP loopback
	VoiceDuplexProxy    *server.VoiceDuplexProxy // Lazy handler for full-duplex voice at /ws/voice
	Heartbeat           **daemon.Heartbeat       // Pointer-to-pointer: set by root.go/desktop.go after creation
}

// friendlyNeboLoopError extracts a human-readable message from a NeboLoop API error.
// NeboLoop errors look like: 'NeboLoop returned 400: {"error":"some message"}'
// This returns just the message part, with common technical errors mapped to plain language.
func friendlyNeboLoopError(err error) string {
	s := err.Error()

	// Try to extract the JSON error field
	if idx := strings.Index(s, "{"); idx >= 0 {
		var parsed struct {
			Error string `json:"error"`
		}
		if json.Unmarshal([]byte(s[idx:]), &parsed) == nil && parsed.Error != "" {
			s = parsed.Error
		}
	}

	// Map common technical errors to plain language
	switch {
	case strings.Contains(s, "already a member"):
		return "You're already in this loop."
	case strings.Contains(s, "duplicate key"):
		return "This device is already registered. Try generating a new connection code."
	case strings.Contains(s, "invalid token"):
		return "Your session has expired. Please reconnect to NeboLoop in Settings."
	case strings.Contains(s, "not found"):
		return "That code wasn't recognized. Please double-check it and try again."
	case strings.Contains(s, "expired"):
		return "That code has expired. Please generate a new one."
	case strings.Contains(s, "maximum uses"):
		return "That code has already been used. Please generate a new one."
	default:
		return s
	}
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

// isSkillCode checks if a prompt is a SKILL-XXXX-XXXX-XXXX install code.
// SKILL is 5 chars (vs 4 for NEBO/LOOP), so total length is 20.
func isSkillCode(prompt string) bool {
	prompt = strings.TrimSpace(prompt)
	if len(prompt) != 20 {
		return false
	}
	// Pattern: SKILL-XXXX-XXXX-XXXX (uppercase alphanumeric)
	if prompt[:6] != "SKILL-" || prompt[10] != '-' || prompt[15] != '-' {
		return false
	}
	for _, c := range prompt[6:10] + prompt[11:15] + prompt[16:] {
		if !((c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9')) {
			return false
		}
	}
	return true
}

// ensureBotID returns the bot_id, resolving with file-first priority:
//  1. File (<data_dir>/bot_id) — source of truth, survives DB deletion
//  2. DB (plugin_settings) — backward compat, migrated to file on first read
//  3. Generate — new UUID, persisted to both file and DB
func ensureBotID(ctx context.Context, pluginStore *settings.Store) string {
	// 1. File — source of truth
	if id := defaults.ReadBotID(); id != "" {
		syncBotIDToDB(ctx, pluginStore, id)
		return id
	}

	// 2. DB — migrate existing installs to file
	if pluginStore != nil {
		s, err := pluginStore.GetSettingsByName(ctx, "neboloop")
		if err == nil && s["bot_id"] != "" {
			if err := defaults.WriteBotID(s["bot_id"]); err != nil {
				devlog.Printf("[NeboLoop] Warning: failed to write bot_id file: %v\n", err)
			}
			return s["bot_id"]
		}
	}

	// 3. Generate new
	botID := uuid.New().String()
	if err := defaults.WriteBotID(botID); err != nil {
		devlog.Printf("[NeboLoop] Warning: failed to write bot_id file: %v\n", err)
	}
	syncBotIDToDB(ctx, pluginStore, botID)
	devlog.Printf("[NeboLoop] Generated bot_id: %s\n", botID)
	return botID
}

// syncBotIDToDB writes the bot_id to the plugin_settings DB table for backward compatibility.
func syncBotIDToDB(ctx context.Context, pluginStore *settings.Store, botID string) {
	if pluginStore == nil {
		return
	}
	p, err := pluginStore.GetPlugin(ctx, "neboloop")
	if err != nil {
		return
	}
	_ = pluginStore.UpdateSettings(ctx, p.ID, map[string]string{"bot_id": botID}, nil)
}

// getNeboLoopJWT retrieves the owner's OAuth JWT from auth_profiles.
func getNeboLoopJWT(ctx context.Context, sqlDB *sql.DB) string {
	store := db.New(sqlDB)
	profiles, err := store.ListAllActiveAuthProfilesByProvider(ctx, "neboloop")
	if err != nil || len(profiles) == 0 {
		return ""
	}
	return profiles[0].ApiKey // JWT is stored in api_key column
}

// getNeboLoopRefreshToken reads the refresh_token and API URL from the
// NeboLoop auth profile in the database.
func getNeboLoopRefreshToken(ctx context.Context, sqlDB *sql.DB) (refreshToken, apiURL string) {
	store := db.New(sqlDB)
	profiles, err := store.ListAllActiveAuthProfilesByProvider(ctx, "neboloop")
	if err != nil || len(profiles) == 0 {
		return "", ""
	}
	p := profiles[0]
	apiURL = p.BaseUrl.String
	if !p.Metadata.Valid {
		return "", apiURL
	}
	var meta map[string]string
	if err := json.Unmarshal([]byte(p.Metadata.String), &meta); err != nil {
		return "", apiURL
	}
	return meta["refresh_token"], apiURL
}

// tryRefreshNeboLoopToken attempts to refresh the NeboLoop JWT using the stored
// refresh_token. Returns the fresh access token on success, empty string on failure.
// Persists the new tokens to auth_profiles so subsequent connects use the fresh JWT.
func tryRefreshNeboLoopToken(ctx context.Context, sqlDB *sql.DB) string {
	refreshToken, apiURL := getNeboLoopRefreshToken(ctx, sqlDB)
	if refreshToken == "" {
		fmt.Println("[Comm:neboloop] No refresh token available, re-authenticate via Settings")
		return ""
	}
	tokenResp, err := neboloophandler.RefreshNeboLoopToken(ctx, apiURL, refreshToken)
	if err != nil {
		devlog.Printf("[Comm:neboloop] Token refresh failed: %v\n", err)
		return ""
	}

	// Preserve existing metadata from current profile
	store := db.New(sqlDB)
	profiles, _ := store.ListAllActiveAuthProfilesByProvider(ctx, "neboloop")
	var ownerID, email string
	var janusProvider bool
	if len(profiles) > 0 && profiles[0].Metadata.Valid {
		var meta map[string]string
		if json.Unmarshal([]byte(profiles[0].Metadata.String), &meta) == nil {
			ownerID = meta["owner_id"]
			email = meta["email"]
			janusProvider = meta["janus_provider"] == "true"
		}
	}

	// Build metadata with new refresh token
	metadata := map[string]string{
		"owner_id":      ownerID,
		"email":         email,
		"refresh_token": tokenResp.RefreshToken,
	}
	if janusProvider {
		metadata["janus_provider"] = "true"
	}
	metadataJSON, _ := json.Marshal(metadata)

	// Deactivate existing profiles
	for _, p := range profiles {
		store.ToggleAuthProfile(ctx, db.ToggleAuthProfileParams{
			ID:       p.ID,
			IsActive: sql.NullInt64{Int64: 0, Valid: true},
		})
	}

	// Create new profile with fresh tokens
	_, err = store.CreateAuthProfile(ctx, db.CreateAuthProfileParams{
		ID:       uuid.New().String(),
		Name:     email,
		Provider: "neboloop",
		ApiKey:   tokenResp.AccessToken,
		BaseUrl:  sql.NullString{String: apiURL, Valid: true},
		AuthType: sql.NullString{String: "oauth", Valid: true},
		IsActive: sql.NullInt64{Int64: 1, Valid: true},
		Metadata: sql.NullString{String: string(metadataJSON), Valid: true},
	})
	if err != nil {
		devlog.Printf("[Comm:neboloop] Failed to store refreshed token: %v\n", err)
		return ""
	}

	fmt.Println("[Comm:neboloop] Token refreshed successfully")
	return tokenResp.AccessToken
}

// handleNeboLoopCode processes a connection code and emits tool-use-style events.
// Returns true if the prompt was a connection code (handled), false otherwise.
// When state is provided, a successful connection also activates the neboloop comm plugin.
func handleNeboLoopCode(ctx context.Context, prompt, requestID string, pluginStore *settings.Store, state *agentState, send func(map[string]any)) bool {
	if !isNeboLoopCode(prompt) {
		return false
	}

	code := strings.TrimSpace(prompt)
	devlog.Printf("[NeboLoop] Connection code detected: %s\n", code)

	// Emit tool call event
	send(map[string]any{
		"type": "stream",
		"id":   requestID,
		"payload": map[string]any{
			"tool":  "neboloop_connect",
			"input": map[string]string{"code": code},
		},
	})

	// Resolve API server: env override > plugin store > config default > const fallback
	apiServer := neboloopapi.DefaultAPIServer
	if state != nil && state.apiURL != "" {
		apiServer = state.apiURL
	}
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

	// Ensure we have a stable bot_id before redeeming
	botID := ensureBotID(ctx, pluginStore)

	// Step 1: Redeem code (pass our immutable bot_id so the server registers it)
	redeemed, err := neboloopapi.RedeemCode(ctx, apiServer, code, botName, "AI companion", botID)
	if err != nil {
		devlog.Printf("[NeboLoop] Failed to redeem connection code: %s\n", err)
		userMsg := friendlyNeboLoopError(err)
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"tool_result": userMsg}})
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"chunk": userMsg}})
		send(map[string]any{"type": "res", "id": requestID, "ok": true, "payload": map[string]any{"result": userMsg}})
		return true
	}

	// Step 2: Store connection settings via PluginStore (bot_id is already persisted by ensureBotID)
	if pluginStore != nil {
		p, err := pluginStore.GetPlugin(ctx, "neboloop")
		if err == nil {
			newSettings := map[string]string{
				"api_server": apiServer,
				"token":      redeemed.ConnectionToken,
			}
			if err := pluginStore.UpdateSettings(ctx, p.ID, newSettings, nil); err != nil {
				devlog.Printf("[NeboLoop] Warning: failed to save connection settings: %v\n", err)
			}
		} else {
			devlog.Printf("[NeboLoop] Warning: neboloop plugin not registered: %v\n", err)
		}
	}

	// Step 3: Activate the neboloop comm plugin and persist via settings
	if state != nil && state.commManager != nil {
		state.botID = botID // Use our immutable local bot_id
		if err := state.commManager.SetActive("neboloop"); err != nil {
			devlog.Printf("[NeboLoop] Warning: failed to activate comm plugin: %v\n", err)
		} else if active := state.commManager.GetActive(); active != nil {
			commConfig := injectNeboLoopAuth(ctx, state.sqlDB, botID, map[string]string{
				"api_server": apiServer,
				"token":      redeemed.ConnectionToken,
			})
			if commConfig["gateway"] == "" && state.commsURL != "" {
				commConfig["gateway"] = state.commsURL
			}
			if commConfig["token"] != "" {
				if err := active.Connect(ctx, commConfig); err != nil {
					devlog.Printf("[NeboLoop] Warning: failed to connect comm plugin: %v\n", err)
				} else {
					card := buildAgentCard(state.registry, state.skillLoader)
					active.Register(ctx, state.commAgentID, card)
					devlog.Printf("[NeboLoop] Comm plugin activated and connected (agent: %s)\n", state.commAgentID)
				}
			} else {
				fmt.Println("[NeboLoop] Bot registered, but no JWT yet. Do OAuth login to connect.")
			}
		}

		// Persist to settings so it survives restart
		if store := local.GetAgentSettings(); store != nil {
			s := store.Get()
			s.CommEnabled = true
			s.CommPlugin = "neboloop"
			if err := store.Update(s); err != nil {
				devlog.Printf("[NeboLoop] Warning: failed to persist comm settings: %v\n", err)
			} else {
				fmt.Println("[NeboLoop] Comm settings persisted (commEnabled=true, commPlugin=neboloop)")
			}
		}
	}

	// Emit success tool result
	resultText := fmt.Sprintf("Connected as %s (ID: %s)", redeemed.Name, botID)
	devlog.Printf("[NeboLoop] %s\n", resultText)
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
func handleLoopCode(ctx context.Context, prompt, requestID string, pluginStore *settings.Store, state *agentState, send func(map[string]any)) bool {
	if !isLoopCode(prompt) {
		return false
	}

	code := strings.TrimSpace(prompt)
	devlog.Printf("[NeboLoop] Loop invite code detected: %s\n", code)

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
		errMsg := "You need to connect to NeboLoop first. Log in via OAuth to get started."
		devlog.Printf("[NeboLoop] %s\n", errMsg)
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"tool_result": errMsg}})
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"chunk": errMsg}})
		send(map[string]any{"type": "res", "id": requestID, "ok": true, "payload": map[string]any{"result": errMsg}})
		return true
	}

	// Resolve API server: env override > config default > const fallback
	if neboloopSettings["api_server"] == "" {
		if env := os.Getenv("NEBOLOOP_API_SERVER"); env != "" {
			neboloopSettings["api_server"] = env
		} else if state != nil && state.apiURL != "" {
			neboloopSettings["api_server"] = state.apiURL
		} else {
			neboloopSettings["api_server"] = neboloopapi.DefaultAPIServer
		}
	}

	// Inject JWT from auth_profiles for API authentication
	if state != nil && state.sqlDB != nil {
		neboloopSettings = injectNeboLoopAuth(ctx, state.sqlDB, neboloopSettings["bot_id"], neboloopSettings)
	}

	// Create NeboLoop API client
	client, err := neboloopapi.NewClient(neboloopSettings)
	if err != nil {
		devlog.Printf("[NeboLoop] Failed to create client: %s\n", err)
		userMsg := "Couldn't connect to NeboLoop. Please check your connection settings."
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"tool_result": userMsg}})
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"chunk": userMsg}})
		send(map[string]any{"type": "res", "id": requestID, "ok": true, "payload": map[string]any{"result": userMsg}})
		return true
	}

	// Join the loop
	result, err := client.JoinLoop(ctx, code)
	if err != nil {
		devlog.Printf("[NeboLoop] Failed to join loop: %s\n", err)
		userMsg := friendlyNeboLoopError(err)
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"tool_result": userMsg}})
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"chunk": userMsg}})
		send(map[string]any{"type": "res", "id": requestID, "ok": true, "payload": map[string]any{"result": userMsg}})
		return true
	}

	// Emit success
	loopName := result.Name
	if loopName == "" {
		loopName = result.ID
	}
	resultText := fmt.Sprintf("Joined loop: %s (ID: %s)", loopName, result.ID)
	devlog.Printf("[NeboLoop] %s\n", resultText)
	send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"tool_result": resultText}})

	successMsg := fmt.Sprintf("You've joined the **%s** loop! You can now communicate with other agents in this loop.", loopName)
	send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"chunk": successMsg}})
	send(map[string]any{"type": "res", "id": requestID, "ok": true, "payload": map[string]any{"result": successMsg}})
	return true
}

// handleSkillCode processes a SKILL-XXXX-XXXX-XXXX install code and installs the skill.
// Returns true if the prompt was a skill code (handled), false otherwise.
// The bot must already be connected to NeboLoop (has credentials in plugin store).
func handleSkillCode(ctx context.Context, prompt, requestID string, pluginStore *settings.Store, state *agentState, send func(map[string]any)) bool {
	if !isSkillCode(prompt) {
		return false
	}

	code := strings.TrimSpace(prompt)
	devlog.Printf("[NeboLoop] Skill install code detected: %s\n", code)

	// Emit tool call event
	send(map[string]any{
		"type": "stream",
		"id":   requestID,
		"payload": map[string]any{
			"tool":  "skill_install",
			"input": map[string]string{"code": code},
		},
	})

	// Get NeboLoop credentials from plugin store (bot must already be connected)
	if pluginStore == nil {
		errMsg := "Cannot install skill: settings not available"
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"tool_result": errMsg}})
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"chunk": errMsg}})
		send(map[string]any{"type": "res", "id": requestID, "ok": true, "payload": map[string]any{"result": errMsg}})
		return true
	}

	neboloopSettings, err := pluginStore.GetSettingsByName(ctx, "neboloop")
	if err != nil || neboloopSettings["bot_id"] == "" {
		errMsg := "You need to connect to NeboLoop first. Log in via OAuth to get started."
		devlog.Printf("[NeboLoop] %s\n", errMsg)
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"tool_result": errMsg}})
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"chunk": errMsg}})
		send(map[string]any{"type": "res", "id": requestID, "ok": true, "payload": map[string]any{"result": errMsg}})
		return true
	}

	// Resolve API server
	if neboloopSettings["api_server"] == "" {
		if env := os.Getenv("NEBOLOOP_API_SERVER"); env != "" {
			neboloopSettings["api_server"] = env
		} else if state != nil && state.apiURL != "" {
			neboloopSettings["api_server"] = state.apiURL
		} else {
			neboloopSettings["api_server"] = neboloopapi.DefaultAPIServer
		}
	}

	// Inject JWT from auth_profiles for API authentication
	if state != nil && state.sqlDB != nil {
		neboloopSettings = injectNeboLoopAuth(ctx, state.sqlDB, neboloopSettings["bot_id"], neboloopSettings)
	}

	// Create NeboLoop API client
	client, err := neboloopapi.NewClient(neboloopSettings)
	if err != nil {
		devlog.Printf("[NeboLoop] Failed to create client: %s\n", err)
		userMsg := "Couldn't connect to NeboLoop. Please check your connection settings."
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"tool_result": userMsg}})
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"chunk": userMsg}})
		send(map[string]any{"type": "res", "id": requestID, "ok": true, "payload": map[string]any{"result": userMsg}})
		return true
	}

	// Redeem the skill install code
	result, err := client.RedeemSkillCode(ctx, code)
	if err != nil {
		devlog.Printf("[NeboLoop] Failed to install skill: %s\n", err)
		userMsg := friendlyNeboLoopError(err)
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"tool_result": userMsg}})
		send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"chunk": userMsg}})
		send(map[string]any{"type": "res", "id": requestID, "ok": true, "payload": map[string]any{"result": userMsg}})
		return true
	}

	// Emit success
	skillName := ""
	if result.Skill != nil {
		skillName = result.Skill.Name
	}
	if skillName == "" {
		skillName = result.ID
	}
	resultText := fmt.Sprintf("Installed skill: %s (ID: %s)", skillName, result.ID)
	devlog.Printf("[NeboLoop] %s\n", resultText)
	send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"tool_result": resultText}})

	successMsg := fmt.Sprintf("Installed **%s**! It'll activate automatically when you need it.", skillName)
	send(map[string]any{"type": "stream", "id": requestID, "payload": map[string]any{"chunk": successMsg}})
	send(map[string]any{"type": "res", "id": requestID, "ok": true, "payload": map[string]any{"result": successMsg}})
	return true
}

// isSilentToolCall returns true for tool calls that should not be shown in the UI.
// Memory operations (store, recall, search) happen silently — the user shouldn't see
// a wall of "agent store Completed" cards when the model learns facts.
func isSilentToolCall(tc *ai.ToolCall) bool {
	if tc == nil || tc.Name != "bot" {
		return false
	}
	var input struct {
		Resource string `json:"resource"`
		Action   string `json:"action"`
	}
	if err := json.Unmarshal(tc.Input, &input); err != nil {
		return false
	}
	switch input.Action {
	case "store", "recall", "search":
		return true
	}
	if input.Resource == "memory" {
		return true
	}
	if input.Resource == "profile" && input.Action == "get" {
		return true
	}
	return false
}

// runAgent connects to the server and runs the agent loop.
// This is the single code path for all agent modes (RunAll, standalone, etc).
func runAgent(ctx context.Context, cfg *agentcfg.Config, serverURL string, opts AgentOptions) error {
	// Set up log file in data directory
	logPath := filepath.Join(cfg.DataDir, "agent.log")
	logFile, logErr := os.OpenFile(logPath, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0644)
	if logErr != nil {
		devlog.Printf("[agent] Warning: could not open log file %s: %v\n", logPath, logErr)
	} else {
		defer logFile.Close()
		// Tee stdout to both terminal and log file
		origStdout := os.Stdout
		pr, pw, _ := os.Pipe()
		os.Stdout = pw
		go func() {
			buf := make([]byte, 4096)
			for {
				n, err := pr.Read(buf)
				if n > 0 {
					origStdout.Write(buf[:n])
					logFile.Write(buf[:n])
				}
				if err != nil {
					break
				}
			}
		}()
		defer func() {
			pw.Close()
			os.Stdout = origStdout
		}()
		devlog.Printf("[agent] Logging to %s\n", logPath)
	}

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
		pendingAsk:      make(map[string]chan string),
		quiet:           opts.Quiet,
		lanes:           agenthub.NewLaneManager(),
		heartbeat:       opts.Heartbeat,
	}

	// Apply lane concurrency from config (overrides defaults when non-zero)
	if cfg.Lanes.Main > 0 {
		state.lanes.SetConcurrency(agenthub.LaneMain, cfg.Lanes.Main)
	}
	if cfg.Lanes.Events > 0 {
		state.lanes.SetConcurrency(agenthub.LaneEvents, cfg.Lanes.Events)
	}
	if cfg.Lanes.Subagent > 0 {
		state.lanes.SetConcurrency(agenthub.LaneSubagent, cfg.Lanes.Subagent)
	}
	if cfg.Lanes.Nested > 0 {
		state.lanes.SetConcurrency(agenthub.LaneNested, cfg.Lanes.Nested)
	}
	if cfg.Lanes.Heartbeat > 0 {
		state.lanes.SetConcurrency(agenthub.LaneHeartbeat, cfg.Lanes.Heartbeat)
	}
	if cfg.Lanes.Comm > 0 {
		state.lanes.SetConcurrency(agenthub.LaneComm, cfg.Lanes.Comm)
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

	// Purge ghost messages from failed runs on startup
	if purged, err := sessions.PurgeEmptyMessages(); err == nil && purged > 0 {
		fmt.Printf("[agent] Purged %d empty ghost messages on startup\n", purged)
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
	devlog.Printf("[agent] Starting browser manager...\n")
	browserT0 := time.Now()
	browserMgr := browser.GetManager()
	if err := browserMgr.Start(browser.Config{
		Enabled:  true,
		Headless: true, // Default to headless for managed browser
	}); err != nil {
		devlog.Printf("[agent] Warning: failed to start browser manager (%s): %v\n", time.Since(browserT0), err)
	} else {
		devlog.Printf("[agent] Browser manager started (%s)\n", time.Since(browserT0))
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
					bgCtx, bgCancel := context.WithTimeout(context.Background(), 60*time.Second)
				defer bgCancel()
					// First: clear embeddings from old models (e.g., nomic-embed-text → qwen3-embedding)
					if stale, deleted, err := memoryTool.MigrateEmbeddings(bgCtx); err != nil {
						devlog.Printf("[agent] Embedding migration error: %v\n", err)
					} else if stale > 0 {
						devlog.Printf("[agent] Migrated embeddings: %d stale → %d deleted\n", stale, deleted)
					}
					// Then: backfill any memories without embeddings
					n, err := memoryTool.BackfillEmbeddings(bgCtx)
					if err != nil {
						devlog.Printf("[agent] Embedding backfill error: %v\n", err)
					} else if n > 0 {
						devlog.Printf("[agent] Backfilled embeddings for %d memories\n", n)
					}
				}()
			}
		}
	}

	// Load advisors (internal deliberation system)
	// Advisors are enabled/disabled via config and invoked by the agent when needed
	advisorLoader := advisors.NewLoader(cfg.AdvisorsDir())
	if err := advisorLoader.LoadAll(); err != nil {
		devlog.Printf("[agent] Warning: failed to load advisors: %v\n", err)
	} else if advisorLoader.Count() > 0 {
		devlog.Printf("[agent] Loaded %d advisors from %s\n", advisorLoader.Count(), cfg.AdvisorsDir())
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
	// advisorsTool object stays — still passed to botTool.SetAdvisorsTool()
	// but no longer registered as a standalone tool

	// Register cron tool for scheduled tasks (requires shared database)
	var cronTool *tools.CronTool
	var schedulerMgr *tools.SchedulerManager
	if opts.Database != nil {
		cronTool, err = tools.NewCronTool(tools.CronConfig{DB: opts.Database})
		if err == nil {
			schedulerMgr = tools.NewSchedulerManager(tools.NewCronScheduler(cronTool))
		}
	}

	taskTool := tools.NewTaskTool()

	// Wire up cron → agent execution (after runner is created)
	// We'll set this callback after creating the runner below
	taskTool.CreateOrchestrator(cfg, sessions, providers, registry)
	defer taskTool.GetOrchestrator().Shutdown(context.Background())

	// NeboLoop client provider (used by app + skill tools for browse/install)
	neboloopClientProvider := func(ctx context.Context) (*neboloopapi.Client, error) {
		if opts.PluginStore == nil {
			return nil, fmt.Errorf("plugin store not available")
		}
		settings, err := opts.PluginStore.GetSettingsByName(ctx, "neboloop")
		if err != nil {
			return nil, fmt.Errorf("NeboLoop not configured: %w", err)
		}
		settings = injectNeboLoopAuth(ctx, sqlDB, ensureBotID(ctx, opts.PluginStore), settings)
		return neboloopapi.NewClient(settings)
	}

	// --- STRAP tools ---

	// Bot domain: task, memory, session, profile, context, advisors, vision, ask
	botTool := tools.NewBotTool(tools.BotToolConfig{
		Sessions:   sessions,
		MemoryTool: memoryTool,
	})
	botTool.SetOrchestrator(taskTool.GetOrchestrator())
	botTool.SetVisionTool(tools.NewVisionTool(tools.VisionConfig{}))
	botTool.SetAdvisorsTool(advisorsTool)
	botTool.SetAskCallback(func(ctx context.Context, reqID, prompt string, widgets []tools.AskWidget) (string, error) {
		return state.requestAsk(ctx, reqID, prompt, widgets)
	})
	registry.RegisterBotTool(botTool)

	// Event domain: cron/reminders (flat)
	if schedulerMgr != nil {
		eventTool := tools.NewEventTool(schedulerMgr)
		registry.RegisterEventTool(eventTool)
	}

	// App domain: list, launch, stop, browse, install
	appTool := tools.NewAppTool(neboloopClientProvider)
	registry.RegisterAppTool(appTool)

	// Wire skill tool with NeboLoop client provider for browse/install
	if skillTool := registry.GetSkillTool(); skillTool != nil {
		skillTool.SetClientProvider(neboloopClientProvider)
	}

	// Create agent MCP server for CLI provider loopback (exposes all tools via MCP)
	mcpSrv := agentmcp.NewServer(registry)
	if opts.AgentMCPProxy != nil {
		opts.AgentMCPProxy.Set(mcpSrv.Handler())
		fmt.Println("[Agent] MCP server ready at /agent/mcp")
	}

	r := runner.New(cfg, sessions, providers, registry)

	// Voice duplex deps — shared by both direct WebSocket and comms relay handlers
	voiceDeps := voice.DuplexDeps{
		RunnerFunc: func(runCtx context.Context, sessionKey, prompt, channel string) (<-chan string, error) {
			textCh := make(chan string, 50)
			// Route voice through LaneMain so it shares backpressure and
			// ordering with desktop text and phone text input.
			go func() {
				defer close(textCh)
				_ = state.lanes.Enqueue(runCtx, agenthub.LaneMain, func(taskCtx context.Context) error {
					events, err := r.Run(taskCtx, &runner.RunRequest{
						SessionKey: sessionKey,
						Prompt:     prompt,
						Origin:     tools.OriginUser,
						UserID:     "default-user",
						Channel:    channel,
					})
					if err != nil {
						return err
					}
					for event := range events {
						if event.Type == ai.EventTypeText && event.Text != "" {
							textCh <- event.Text
						}
					}
					return nil
				}, agenthub.WithDescription("Voice: "+prompt[:min(len(prompt), 40)]))
			}()
			return textCh, nil
		},
		SendFrame: func(frame map[string]any) error {
			return state.sendFrame(frame)
		},
		SampleRate: 16000,
	}

	// Wire full-duplex voice WebSocket handler
	if opts.VoiceDuplexProxy != nil {
		opts.VoiceDuplexProxy.Set(voice.DuplexHandler(voiceDeps))
		fmt.Println("[Agent] Voice duplex handler ready at /ws/voice")
	}

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
		devlog.Printf("[agent] Warning: could not start config watcher: %v\n", err)
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
			devlog.Printf("[agent] Config reloaded: model selector, fuzzy matcher, and providers updated\n")
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

	// Store Janus rate-limit snapshots on ServiceContext for the usage API
	// and persist to janus_usage.json so usage data survives restarts
	if opts.SvcCtx != nil {
		r.SetRateLimitStore(func(rl *ai.RateLimitInfo) {
			opts.SvcCtx.JanusUsage.Store(rl)
			opts.SvcCtx.SaveJanusUsage()
		})
	}

	// Bridge MCP server context for CLI providers (claude-code, gemini-cli)
	r.SetMCPServer(mcpSrv)

	// Wire bot tool's embedded vision tool with AnalyzeFunc
	if bt := registry.GetBotTool(); bt != nil {
		if bv := bt.GetVisionTool(); bv != nil {
			bv.SetAnalyzeFunc(func(ctx context.Context, imageBase64, mediaType, prompt string) (string, error) {
				if len(providers) == 0 {
					return "", fmt.Errorf("no AI providers configured")
				}
				content := fmt.Sprintf("[Image: data:%s;base64,%s]\n\n%s", mediaType, imageBase64, prompt)
				req := &ai.ChatRequest{
					Messages: []session.Message{
						{Role: "user", Content: content},
					},
					MaxTokens: 2048,
				}
				events, err := providers[0].Stream(ctx, req)
				if err != nil {
					return "", err
				}
				var result strings.Builder
				for event := range events {
					switch event.Type {
					case ai.EventTypeText:
						result.WriteString(event.Text)
					case ai.EventTypeError:
						if event.Error != nil {
							return "", fmt.Errorf("%s", event.Error.Error())
						}
						return "", fmt.Errorf("vision provider error")
					case ai.EventTypeDone:
						// Stream complete
					}
				}
				return result.String(), nil
			})
		}
	}

	// Set up subagent persistence for surviving restarts
	if state.recovery != nil {
		r.SetupSubagentPersistence(state.recovery)
		// Recover any pending subagent tasks from previous run
		if recovered, err := r.RecoverSubagents(ctx); err != nil {
			devlog.Printf("[agent] Warning: failed to recover subagents: %v\n", err)
		} else if recovered > 0 {
			devlog.Printf("[agent] Recovered %d subagent task(s)\n", recovered)
		}
	}

	// Wire up cron agent task callback now that runner exists.
	// Reminders run on the events lane so they don't block user conversations on main.
	if cronTool != nil {
		cronTool.SetAgentCallback(func(ctx context.Context, name, message, instructions string, deliver *tools.DeliverConfig) error {
			sessionKey := fmt.Sprintf("reminder-%s", name)
			fmt.Printf("[Reminders] Firing %q → enqueueing on events lane\n", name)

			state.lanes.EnqueueAsync(ctx, agenthub.LaneEvents, func(taskCtx context.Context) error {
				events, err := r.Run(taskCtx, &runner.RunRequest{
					SessionKey: sessionKey,
					Prompt:     message,
					System:     instructions,
					Origin:     tools.OriginSystem,
				})
				if err != nil {
					fmt.Printf("[Reminders] %q failed: %v\n", name, err)
					return err
				}

				// Collect result
				var result strings.Builder
				for event := range events {
					if event.Type == ai.EventTypeText {
						result.WriteString(event.Text)
					}
				}

				resultStr := result.String()
				logStr := resultStr
				if len(logStr) > 100 {
					logStr = logStr[:100] + "..."
				}
				fmt.Printf("[Reminders] %q completed: %s\n", name, logStr)

				// Always show a native OS notification
				notify.Send("Nebo — Reminder", message)

				// Push result to UI clients so it appears in the chat
				state.sendFrame(map[string]any{
					"type":   "event",
					"method": "reminder_complete",
					"payload": map[string]any{
						"name":    name,
						"message": message,
						"result":  resultStr,
					},
				})

				// Deliver result to channel if configured
				if deliver != nil && appRegistry != nil {
					if err := appRegistry.SendToChannel(taskCtx, deliver.Channel, deliver.To, resultStr); err != nil {
						fmt.Printf("[Reminders] %q delivery failed: %v\n", name, err)
					}
				}

				return nil
			}, agenthub.WithDescription(fmt.Sprintf("Reminder: %s", name)))

			return nil
		})

		// Fire any reminders that were missed while the process was down
		cronTool.CatchUpMissedJobs()
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

	// Register Configurable plugins so settings changes trigger hot-reload
	if opts.PluginStore != nil {
		opts.PluginStore.RegisterConfigurable("neboloop", neboloopPlugin)
	}

	// Wire token refresher so the plugin can obtain a fresh JWT on auth failure
	// instead of permanently dying. The callback reads the refresh_token from DB,
	// exchanges it at NeboLoop's /oauth/token endpoint, and stores the new tokens.
	neboloopPlugin.SetTokenRefresher(func(ctx context.Context) (string, error) {
		refreshToken, apiURL := getNeboLoopRefreshToken(ctx, sqlDB)
		if refreshToken == "" {
			return "", fmt.Errorf("no refresh token available, re-authenticate via Settings")
		}
		tokenResp, err := neboloophandler.RefreshNeboLoopToken(ctx, apiURL, refreshToken)
		if err != nil {
			return "", fmt.Errorf("refresh failed: %w", err)
		}

		// Preserve existing metadata (owner_id, email, janus_provider) from current profile
		store := db.New(sqlDB)
		profiles, _ := store.ListAllActiveAuthProfilesByProvider(ctx, "neboloop")
		var ownerID, email string
		var janusProvider bool
		if len(profiles) > 0 && profiles[0].Metadata.Valid {
			var meta map[string]string
			if json.Unmarshal([]byte(profiles[0].Metadata.String), &meta) == nil {
				ownerID = meta["owner_id"]
				email = meta["email"]
				janusProvider = meta["janus_provider"] == "true"
			}
		}

		// Build metadata with new refresh token
		metadata := map[string]string{
			"owner_id":      ownerID,
			"email":         email,
			"refresh_token": tokenResp.RefreshToken,
		}
		if janusProvider {
			metadata["janus_provider"] = "true"
		}
		metadataJSON, _ := json.Marshal(metadata)

		// Deactivate existing profiles
		for _, p := range profiles {
			store.ToggleAuthProfile(ctx, db.ToggleAuthProfileParams{
				ID:       p.ID,
				IsActive: sql.NullInt64{Int64: 0, Valid: true},
			})
		}

		// Create new profile with fresh tokens
		_, err = store.CreateAuthProfile(ctx, db.CreateAuthProfileParams{
			ID:       uuid.New().String(),
			Name:     email,
			Provider: "neboloop",
			ApiKey:   tokenResp.AccessToken,
			BaseUrl:  sql.NullString{String: apiURL, Valid: true},
			AuthType: sql.NullString{String: "oauth", Valid: true},
			IsActive: sql.NullInt64{Int64: 1, Valid: true},
			Metadata: sql.NullString{String: string(metadataJSON), Valid: true},
		})
		if err != nil {
			return "", fmt.Errorf("failed to store refreshed token: %w", err)
		}

		devlog.Printf("[Comm:neboloop] Token refreshed successfully\n")
		return tokenResp.AccessToken, nil
	})

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
		devlog.Printf("[agent] Warning: failed to discover apps: %v\n", err)
	}
	appRegistry.StartSupervisor(ctx)
	defer appRegistry.Stop()

	// Inject app catalog into agent system prompt so it knows about installed apps
	r.SetAppCatalog(appRegistry)

	// Wire app hooks into tool registry, runner, and bot tool
	hookDispatcher := appRegistry.HookDispatcher()
	registry.SetHookDispatcher(hookDispatcher)
	r.SetHookDispatcher(hookDispatcher)
	if botTool := registry.GetBotTool(); botTool != nil {
		botTool.SetHookDispatcher(hookDispatcher)
	}
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
			devlog.Printf("[agent] Warning: app watcher failed: %v\n", err)
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
			devlog.Printf("[agent] Registered gateway alias: %s -> %s\n", shortName, providerID)
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
			skillDomainTool.Register(tools.Slugify(s.Name), s.Name, s.Description, s.Template, nil, s.Triggers, s.Tools, s.Priority, s.MaxTurns)
		}
	}
	devlog.Printf("[agent] Registered %d standalone skills, %d total skills\n", skillLoader.Count(), skillDomainTool.Count())

	// Re-register skills when enabled/disabled state changes via the UI toggle
	opts.SvcCtx.SkillSettings.OnChange(func(name string, enabled bool) {
		slug := tools.Slugify(name)
		if enabled {
			// Find the skill from a fresh load and register it
			fresh := loadSkills(cfg)
			for _, s := range fresh.List() {
				if tools.Slugify(s.Name) == slug {
					skillDomainTool.Register(slug, s.Name, s.Description, s.Template, nil, s.Triggers, s.Tools, s.Priority, s.MaxTurns)
					devlog.Printf("[agent] Skill %q enabled and registered\n", name)
					return
				}
			}
		} else {
			skillDomainTool.Unregister(slug)
			devlog.Printf("[agent] Skill %q disabled and unregistered\n", name)
		}
	})

	// Watch user skills directory for hot-reload (CRUD from API writes here)
	userSkillWatcher := skills.NewLoader(userSkillsDir)
	userSkillWatcher.OnChange(func(_ []*skills.Skill) {
		skillDomainTool.UnregisterStandalone()
		fresh := loadSkills(cfg)
		for _, s := range fresh.List() {
			if s.Enabled {
				skillDomainTool.Register(tools.Slugify(s.Name), s.Name, s.Description, s.Template, nil, s.Triggers, s.Tools, s.Priority, s.MaxTurns)
			}
		}
		devlog.Printf("[agent] Skills reloaded: %d standalone, %d total\n", fresh.Count(), skillDomainTool.Count())
	})
	if err := userSkillWatcher.Watch(ctx); err != nil {
		devlog.Printf("[agent] Warning: failed to watch user skills dir: %v\n", err)
	}
	defer userSkillWatcher.Stop()

	// Store comm references on state for dynamic settings updates
	state.commManager = commManager
	state.commAgentID = agentID
	state.registry = registry
	state.skillLoader = skillLoader
	state.appRegistry = appRegistry

	// Wire SDK loop channel messages → per-channel lanes
	neboloopPlugin.OnLoopChannelMessage(func(msg neboloop.LoopChannelMessage) {
		// Skip own messages to prevent echo loops
		if msg.SenderID == neboloopPlugin.BotID() {
			return
		}

		// Detect if the sender is the bot's owner
		isOwner := neboloopPlugin.OwnerID() != "" && msg.SenderID == neboloopPlugin.OwnerID()

		var laneName, sessionKey string
		var origin tools.Origin
		var prompt string
		var modelOverride string

		if isOwner {
			// Owner loop messages route to main lane and share the companion chat,
			// just like owner DMs — so the web UI shows them in real-time.
			laneName = agenthub.LaneMain
			origin = tools.OriginUser
			prompt = msg.Text

			state.companionChatIDMu.RLock()
			cachedID := state.companionChatID
			state.companionChatIDMu.RUnlock()

			if cachedID != "" {
				sessionKey = cachedID
			} else {
				sessionKey = "main"
				if state.sqlDB != nil {
					queries := db.New(state.sqlDB)
					chat, err := queries.GetOrCreateCompanionChat(context.Background(), db.GetOrCreateCompanionChatParams{
						ID:     uuid.New().String(),
						UserID: sql.NullString{String: "companion-default", Valid: true},
					})
					if err == nil {
						sessionKey = chat.ID
					}
				}
			}
		} else {
			// External bot messages go to a per-channel lane with comm origin
			laneName = fmt.Sprintf("loop-%s", msg.ChannelID)
			sessionKey = fmt.Sprintf("loop-channel-%s-%s", msg.ChannelID, msg.ConversationID)
			origin = tools.OriginComm
			prompt = fmt.Sprintf("[Loop Channel: %s | From: %s]\n%s",
				msg.ChannelName, msg.SenderName, msg.Text)
			if cfg := provider.GetModelsConfig(); cfg != nil && cfg.LaneRouting != nil && cfg.LaneRouting.Comm != "" {
				modelOverride = cfg.LaneRouting.Comm
			}
		}

		fmt.Printf("[sdk:loop-channel] Processing message from %s in channel %s (owner=%v session=%s lane=%s)\n",
			msg.SenderName, msg.ChannelName, isOwner, sessionKey, laneName)

		// Check for channel skill bindings
		var forceSkill string
		if state.sqlDB != nil {
			bindings, err := db.New(state.sqlDB).ListChannelSkills(context.Background(), msg.ChannelID)
			if err == nil && len(bindings) > 0 {
				forceSkill = bindings[0].SkillName
				fmt.Printf("[sdk:loop-channel] Channel %s has skill binding: %s\n", msg.ChannelID, forceSkill)
			}
		}

		state.lanes.EnqueueAsync(ctx, laneName, func(taskCtx context.Context) error {
			// For owner messages, broadcast the user message to the web UI
			if isOwner {
				state.sendFrame(map[string]any{
					"type":   "event",
					"method": "dm_user_message",
					"payload": map[string]any{
						"session_id": sessionKey,
						"content":    prompt,
						"source":     "neboloop_loop",
					},
				})
			}

			events, err := r.Run(taskCtx, &runner.RunRequest{
				SessionKey:    sessionKey,
				Prompt:        prompt,
				Origin:        origin,
				UserID:        "default-user",
				ModelOverride: modelOverride,
				ForceSkill:    forceSkill,
			})
			if err != nil {
				fmt.Printf("[sdk:loop-channel] Run failed: %v\n", err)
				return err
			}

			var result strings.Builder
			for event := range events {
				switch event.Type {
				case ai.EventTypeText:
					if event.Text != "" {
						result.WriteString(event.Text)
						if isOwner {
							state.sendFrame(map[string]any{
								"type":   "event",
								"method": "chat_stream",
								"payload": map[string]any{
									"session_id": sessionKey,
									"content":    event.Text,
									"source":     "loop",
								},
							})
						}
					}
				case ai.EventTypeToolCall:
					if isOwner && event.ToolCall != nil {
						state.sendFrame(map[string]any{
							"type":   "event",
							"method": "tool_start",
							"payload": map[string]any{
								"session_id": sessionKey,
								"tool":       event.ToolCall.Name,
								"tool_id":    event.ToolCall.ID,
								"input":      event.ToolCall.Input,
								"source":     "loop",
							},
						})
					}
				case ai.EventTypeToolResult:
					if isOwner {
						toolName, toolID := "", ""
						if event.ToolCall != nil {
							toolName = event.ToolCall.Name
							toolID = event.ToolCall.ID
						}
						payload := map[string]any{
							"session_id": sessionKey,
							"result":     event.Text,
							"tool_name":  toolName,
							"tool_id":    toolID,
							"source":     "loop",
						}
						if event.ImageURL != "" {
							payload["image_url"] = event.ImageURL
						}
						state.sendFrame(map[string]any{
							"type":    "event",
							"method":  "tool_result",
							"payload": payload,
						})
					}
				case ai.EventTypeMessage:
					if event.Message != nil && event.Message.Content != "" && result.Len() == 0 {
						result.WriteString(event.Message.Content)
						if isOwner {
							state.sendFrame(map[string]any{
								"type":   "event",
								"method": "chat_stream",
								"payload": map[string]any{
									"session_id": sessionKey,
									"content":    event.Message.Content,
									"source":     "loop",
								},
							})
						}
					}
				case ai.EventTypeThinking:
					if isOwner && event.Text != "" {
						state.sendFrame(map[string]any{
							"type":   "event",
							"method": "thinking",
							"payload": map[string]any{
								"session_id": sessionKey,
								"content":    event.Text,
								"source":     "loop",
							},
						})
					}
				}
			}

			// Notify web UI of completion
			if isOwner {
				state.sendFrame(map[string]any{
					"type":   "event",
					"method": "chat_complete",
					"payload": map[string]any{
						"session_id": sessionKey,
						"source":     "loop",
					},
				})
			}

			// Send response back to loop channel
			if result.Len() > 0 {
				if err := neboloopPlugin.SendLoopChannelMessage(taskCtx, msg.ChannelID, msg.ConversationID, result.String()); err != nil {
					fmt.Printf("[sdk:loop-channel] SendLoopMessage failed: %v\n", err)
				}
			}

			return nil
		}, agenthub.WithDescription(fmt.Sprintf("Loop channel: %s from %s (owner=%v)", msg.ChannelName, msg.SenderName, isOwner)))
	})

	// Wire SDK DM messages → owner gets main lane, external gets comm lane
	neboloopPlugin.OnDMMessage(func(msg neboloop.DMMessage) {
		var lane string
		var sessionKey string
		var origin tools.Origin
		var prompt string
		var laneModelOverride string

		if msg.IsOwner {
			lane = agenthub.LaneMain
			origin = tools.OriginUser
			prompt = msg.Text

			// Resolve the companion chat session so DMs share context with the web UI.
			// Use the cached session ID from the web UI's first chat request to ensure
			// both paths use the same companion chat (avoids user ID mismatch).
			state.companionChatIDMu.RLock()
			cachedID := state.companionChatID
			state.companionChatIDMu.RUnlock()

			if cachedID != "" {
				sessionKey = cachedID
			} else {
				// Web UI hasn't sent a message yet — fall back to DB lookup
				sessionKey = "main"
				if state.sqlDB != nil {
					queries := db.New(state.sqlDB)
					chat, err := queries.GetOrCreateCompanionChat(context.Background(), db.GetOrCreateCompanionChatParams{
						ID:     uuid.New().String(),
						UserID: sql.NullString{String: "companion-default", Valid: true},
					})
					if err == nil {
						sessionKey = chat.ID
					} else {
						fmt.Printf("[sdk:dm] Could not resolve companion chat, using fallback: %v\n", err)
					}
				}
			}
		} else {
			lane = agenthub.LaneComm
			sessionKey = fmt.Sprintf("dm-%s", msg.ConversationID)
			origin = tools.OriginComm
			prompt = fmt.Sprintf("[DM from %s]\nYour text response will be sent back as a DM automatically. Do not use tools to reply.\n\n%s", msg.SenderID, msg.Text)
			// Resolve comm lane model override for external DMs
			if cfg := provider.GetModelsConfig(); cfg != nil && cfg.LaneRouting != nil && cfg.LaneRouting.Comm != "" {
				laneModelOverride = cfg.LaneRouting.Comm
			}
		}

		fmt.Printf("[sdk:dm] Dispatching DM: is_owner=%v lane=%s session=%s\n", msg.IsOwner, lane, sessionKey)

		state.lanes.EnqueueAsync(ctx, lane, func(taskCtx context.Context) error {
			fmt.Printf("[sdk:dm] Lane task started: session=%s\n", sessionKey)

			// Show typing indicator while processing
			neboloopPlugin.SendTyping(taskCtx, msg.ConversationID, true)

			// For owner DMs, broadcast to web UI so companion chat updates live.
			// Message persistence is handled by the runner (single write path).
			if msg.IsOwner {
				if err := state.sendFrame(map[string]any{
					"type":   "event",
					"method": "dm_user_message",
					"payload": map[string]any{
						"session_id": sessionKey,
						"content":    prompt,
						"source":     "neboloop_dm",
					},
				}); err != nil {
					fmt.Printf("[sdk:dm] sendFrame dm_user_message error: %v\n", err)
				} else {
					fmt.Printf("[sdk:dm] sendFrame dm_user_message ok: session=%s\n", sessionKey)
				}
			}

			events, err := r.Run(taskCtx, &runner.RunRequest{
				SessionKey:    sessionKey,
				Prompt:        prompt,
				Origin:        origin,
				UserID:        "default-user",
				ModelOverride: laneModelOverride,
			})
			if err != nil {
				fmt.Printf("[sdk:dm] Run failed: %v\n", err)
				_ = neboloopPlugin.SendDM(taskCtx, msg.ConversationID, err.Error())
				return err
			}

			var result strings.Builder
			eventCount := 0
			for event := range events {
				eventCount++
				switch event.Type {
				case ai.EventTypeText:
					if event.Text != "" {
						result.WriteString(event.Text)
						// Stream to web UI via hub event handler → broadcasts to all chat clients
						if msg.IsOwner {
							if err := state.sendFrame(map[string]any{
								"type":   "event",
								"method": "chat_stream",
								"payload": map[string]any{
									"session_id": sessionKey,
									"content":    event.Text,
									"source":     "dm",
								},
							}); err != nil {
								fmt.Printf("[sdk:dm] sendFrame chat_stream error: %v\n", err)
							} else {
								fmt.Printf("[sdk:dm] sendFrame chat_stream ok: session=%s len=%d\n", sessionKey, len(event.Text))
							}
						}
					}
				case ai.EventTypeToolCall:
					if msg.IsOwner && event.ToolCall != nil {
						state.sendFrame(map[string]any{
							"type":   "event",
							"method": "tool_start",
							"payload": map[string]any{
								"session_id": sessionKey,
								"tool":       event.ToolCall.Name,
								"tool_id":    event.ToolCall.ID,
								"input":      event.ToolCall.Input,
								"source":     "dm",
							},
						})
					}
				case ai.EventTypeToolResult:
					if msg.IsOwner {
						toolName, toolID := "", ""
						if event.ToolCall != nil {
							toolName = event.ToolCall.Name
							toolID = event.ToolCall.ID
						}
						dmPayload := map[string]any{
							"session_id": sessionKey,
							"result":     event.Text,
							"tool_name":  toolName,
							"tool_id":    toolID,
							"source":     "dm",
						}
						if event.ImageURL != "" {
							dmPayload["image_url"] = event.ImageURL
						}
						state.sendFrame(map[string]any{
							"type":    "event",
							"method":  "tool_result",
							"payload": dmPayload,
						})
					}
				case ai.EventTypeMessage:
					// Fallback: only use EventTypeMessage text if no EventTypeText arrived
					// (prevents duplicate streaming for CLI providers that emit both)
					if event.Message != nil && event.Message.Content != "" && result.Len() == 0 {
						result.WriteString(event.Message.Content)
						if msg.IsOwner {
							state.sendFrame(map[string]any{
								"type":   "event",
								"method": "chat_stream",
								"payload": map[string]any{
									"session_id": sessionKey,
									"content":    event.Message.Content,
									"source":     "dm",
								},
							})
						}
					}
				case ai.EventTypeThinking:
					if msg.IsOwner && event.Text != "" {
						state.sendFrame(map[string]any{
							"type":   "event",
							"method": "thinking",
							"payload": map[string]any{
								"session_id": sessionKey,
								"content":    event.Text,
								"source":     "dm",
							},
						})
					}
				case ai.EventTypeError:
					fmt.Printf("[sdk:dm] Error event: %v\n", event.Error)
				}
			}

			// Log tail of response to diagnose mid-sentence cutoffs
			tail := result.String()
			if len(tail) > 100 {
				tail = tail[len(tail)-100:]
			}
			fmt.Printf("[sdk:dm] Run complete: session=%s result_len=%d events=%d tail=%q\n", sessionKey, result.Len(), eventCount, tail)

			// Notify web UI of completion (message persistence handled by runner)
			if msg.IsOwner {
				state.sendFrame(map[string]any{
					"type":   "event",
					"method": "chat_complete",
					"payload": map[string]any{
						"session_id": sessionKey,
						"source":     "dm",
					},
				})
			}

			// Clear typing indicator before sending response
			neboloopPlugin.SendTyping(taskCtx, msg.ConversationID, false)

			if result.Len() > 0 {
				if err := neboloopPlugin.SendDM(taskCtx, msg.ConversationID, result.String()); err != nil {
					fmt.Printf("[sdk:dm] SendDM failed: %v\n", err)
				}
			} else {
				fmt.Printf("[sdk:dm] No response text produced for session=%s\n", sessionKey)
			}
			return nil
		}, agenthub.WithDescription(fmt.Sprintf("DM from %s (owner=%v)", msg.SenderID, msg.IsOwner)))
	})

	// Wire SDK voice stream → full-duplex voice over comms relay.
	// Phone sends voice frames via NeboLoop comms (stream=voice, ephemeral=true).
	// Nebo processes ASR→LLM→TTS and streams audio back.
	var (
		commsVoiceSessions   = make(map[string]*voice.VoiceConn)
		commsVoiceTransports = make(map[string]*voice.CommsTransport)
		commsVoiceMu         sync.Mutex
	)
	neboloopPlugin.OnVoiceMessage(func(msg neboloop.VoiceMessage) {
		commsVoiceMu.Lock()

		// Parse the content to check message type
		var content voice.CommsVoiceMessage
		if err := json.Unmarshal(msg.Content, &content); err != nil {
			commsVoiceMu.Unlock()
			fmt.Printf("[sdk:voice] Failed to parse voice message: %v\n", err)
			return
		}

		// voice_start → create a new session
		if content.Type == "voice_start" {
			// Clean up any existing session for this conversation
			if old, ok := commsVoiceTransports[msg.ConversationID]; ok {
				old.Close()
				delete(commsVoiceSessions, msg.ConversationID)
				delete(commsVoiceTransports, msg.ConversationID)
			}

			transport := voice.NewCommsTransport(func(outMsg voice.CommsVoiceMessage) error {
				data, err := json.Marshal(outMsg)
				if err != nil {
					return err
				}
				sendCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
				defer cancel()
				return neboloopPlugin.SendVoice(sendCtx, msg.ConversationID, data)
			})

			vc := voice.NewVoiceConn(transport, voiceDeps)
			commsVoiceSessions[msg.ConversationID] = vc
			commsVoiceTransports[msg.ConversationID] = transport
			commsVoiceMu.Unlock()

			fmt.Printf("[sdk:voice] New comms voice session for conversation %s\n", msg.ConversationID)

			// Send voice_ready to the phone
			readyMsg, _ := json.Marshal(voice.CommsVoiceMessage{Type: "voice_ready"})
			sendCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			_ = neboloopPlugin.SendVoice(sendCtx, msg.ConversationID, readyMsg)
			cancel()

			// Run the voice pipeline in a goroutine (blocks until session ends)
			go func() {
				vc.Serve(ctx)
				commsVoiceMu.Lock()
				delete(commsVoiceSessions, msg.ConversationID)
				delete(commsVoiceTransports, msg.ConversationID)
				commsVoiceMu.Unlock()
				fmt.Printf("[sdk:voice] Comms voice session ended for conversation %s\n", msg.ConversationID)
			}()
			return
		}

		// voice_end → close the session
		if content.Type == "voice_end" {
			if t, ok := commsVoiceTransports[msg.ConversationID]; ok {
				t.Close()
				delete(commsVoiceSessions, msg.ConversationID)
				delete(commsVoiceTransports, msg.ConversationID)
			}
			commsVoiceMu.Unlock()
			fmt.Printf("[sdk:voice] Voice session ended by client for conversation %s\n", msg.ConversationID)
			return
		}

		// All other messages (audio, interrupt, config) → feed to existing session
		transport, ok := commsVoiceTransports[msg.ConversationID]
		commsVoiceMu.Unlock()
		if !ok {
			return
		}
		transport.Feed(content)
	})

	// Wire SDK history requests → return recent companion chat messages.
	// NeboLoop sends stream=history when the owner opens the P2P DM chat.
	neboloopPlugin.OnHistoryRequest(func(req neboloop.HistoryRequest) []neboloop.HistoryMessage {
		if state.sqlDB == nil {
			return nil
		}

		// Use cached companion chat ID (same session the web UI uses)
		state.companionChatIDMu.RLock()
		chatID := state.companionChatID
		state.companionChatIDMu.RUnlock()

		if chatID == "" {
			// Fallback: query DB with "companion-default" user ID
			queries := db.New(state.sqlDB)
			chat, err := queries.GetOrCreateCompanionChat(context.Background(), db.GetOrCreateCompanionChatParams{
				ID:     uuid.New().String(),
				UserID: sql.NullString{String: "companion-default", Valid: true},
			})
			if err != nil {
				fmt.Printf("[sdk:history] Could not resolve companion chat: %v\n", err)
				return nil
			}
			chatID = chat.ID
		}

		limit := req.Limit
		if limit <= 0 || limit > 50 {
			limit = 20
		}

		queries := db.New(state.sqlDB)
		rows, err := queries.GetRecentChatMessages(context.Background(), db.GetRecentChatMessagesParams{
			ChatID: chatID,
			Limit:  int64(limit),
		})
		if err != nil {
			fmt.Printf("[sdk:history] Failed to load messages: %v\n", err)
			return nil
		}

		out := make([]neboloop.HistoryMessage, 0, len(rows))
		for _, row := range rows {
			out = append(out, neboloop.HistoryMessage{
				Role:      row.Role,
				Content:   row.Content,
				Timestamp: row.CreatedAt,
			})
		}
		fmt.Printf("[sdk:history] Returning %d messages for chat %s\n", len(out), chatID)
		return out
	})

	// Wire SDK install events → app registry
	neboloopPlugin.OnInstall(func(evt neboloopsdk.InstallEvent) {
		appRegistry.HandleInstallEvent(ctx, evt)
	})

	// Wire account events (plan changes) → token refresh + provider reload + frontend notification
	neboloopPlugin.OnAccountEvent(func(evt neboloop.AccountEvent) {
		if evt.Type == "plan_changed" {
			devlog.Printf("[Comm:neboloop] Plan changed, refreshing token\n")
			fresh := tryRefreshNeboLoopToken(ctx, sqlDB)
			if fresh == "" {
				devlog.Printf("[Comm:neboloop] Token refresh failed after plan change\n")
				return
			}
			r.ReloadProviders()
			var payload struct {
				Plan string `json:"plan"`
			}
			_ = json.Unmarshal(evt.Payload, &payload)
			state.sendFrame(map[string]any{
				"type":   "event",
				"method": "plan_changed",
				"payload": map[string]any{"plan": payload.Plan},
			})
			devlog.Printf("[Comm:neboloop] Providers reloaded with plan=%s\n", payload.Plan)
		}
	})

	// Wire post-connect hook → background token refresh so Janus sees latest plan.
	// Cooldown prevents creating new DB profiles on every reconnect cycle.
	var lastTokenRefresh time.Time
	var lastTokenRefreshMu sync.Mutex
	neboloopPlugin.OnConnected(func() {
		lastTokenRefreshMu.Lock()
		if time.Since(lastTokenRefresh) < 10*time.Minute {
			lastTokenRefreshMu.Unlock()
			return
		}
		lastTokenRefresh = time.Now()
		lastTokenRefreshMu.Unlock()

		if fresh := tryRefreshNeboLoopToken(ctx, sqlDB); fresh != "" {
			r.ReloadProviders()
			devlog.Printf("[Comm:neboloop] Post-connect token refresh, providers reloaded\n")
		}
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
				UserID:     "default-user",
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

	// --- Wire STRAP tools ---

	// Wire loop tool: comm service + channel lister + querier
	if loopTool := registry.GetLoopTool(); loopTool != nil {
		loopTool.SetCommService(commHandler)
		loopTool.SetLoopChannelLister(func(ctx context.Context) ([]tools.LoopChannelInfo, error) {
			channels, err := neboloopPlugin.ListLoopChannels(ctx)
			if err != nil {
				return nil, err
			}
			result := make([]tools.LoopChannelInfo, len(channels))
			for i, ch := range channels {
				result[i] = tools.LoopChannelInfo{
					ChannelID:   ch.ChannelID,
					ChannelName: ch.ChannelName,
					LoopID:      ch.LoopID,
					LoopName:    ch.LoopName,
				}
			}
			return result, nil
		})
		loopTool.SetLoopQuerier(&loopQuerierAdapter{plugin: neboloopPlugin})
	}

	// Wire bot tool: identity syncer + session querier + current user
	if bt := registry.GetBotTool(); bt != nil {
		bt.SetIdentitySyncer(func(ctx context.Context, name, role string) {
			_ = neboloopPlugin.UpdateBotIdentity(ctx, name, role)
		})
		bt.SetSessionQuerier(&sessionQuerierAdapter{mgr: sessions})
		bt.SetCurrentUser("default-user")
	}

	// Wire message tool: owner callbacks (append to companion session + send WS frame)
	if msgTool := registry.GetMsgTool(); msgTool != nil {
		msgTool.SetOwnerCallbacks(
			func(content string) error {
				sess, err := sessions.GetOrCreate("companion", "")
				if err != nil {
					return err
				}
				return sessions.AppendMessage(sess.ID, session.Message{
					Role:    "assistant",
					Content: content,
				})
			},
			func(frame map[string]any) error {
				return state.sendFrame(frame)
			},
		)
	}

	// Wire app tool: app manager (after AppRegistry gains ListInstalled/LaunchApp/StopApp)
	// TODO: Wire at.SetAppManager(appRegistry) once AppRegistry implements AppManager

	// notify_owner and query_sessions are now wired via MsgTool and BotTool above

	// Wire desktop queue: route desktop tools through LaneDesktop for serialization
	registry.SetDesktopQueue(func(ctx context.Context, execute func(ctx context.Context) *tools.ToolResult) *tools.ToolResult {
		var result *tools.ToolResult
		err := state.lanes.Enqueue(ctx, agenthub.LaneDesktop, func(laneCtx context.Context) error {
			result = execute(laneCtx)
			return nil
		})
		if err != nil {
			return &tools.ToolResult{Content: fmt.Sprintf("Desktop queue error: %v", err), IsError: true}
		}
		return result
	})

	// Wire MCP bridge for external tool integrations
	if opts.SvcCtx != nil && opts.SvcCtx.MCPClient != nil {
		mcpBridge := mcpbridge.New(registry, db.New(sqlDB), opts.SvcCtx.MCPClient)
		state.mcpBridge = mcpBridge
		
		// Start health checker to detect and reconnect to stale MCP sessions
		opts.SvcCtx.MCPClient.StartHealthChecker(ctx)
		
		// Initial sync
		go func() {
			if err := mcpBridge.SyncAll(ctx); err != nil {
				devlog.Printf("[agent] MCP bridge sync: %v\n", err)
			}
		}()
		
		// Periodic re-sync every 15 minutes to handle any reconnection needs
		go func() {
			ticker := time.NewTicker(15 * time.Minute)
			defer ticker.Stop()
			for {
				select {
				case <-ctx.Done():
					return
				case <-ticker.C:
					if err := mcpBridge.SyncAll(ctx); err != nil {
						devlog.Printf("[agent] MCP bridge periodic sync: %v\n", err)
					}
				}
			}
		}()
		
		defer mcpBridge.Close()
	}

	// Ensure bot_id exists (generated locally on first startup, immutable)
	botID := ensureBotID(ctx, opts.PluginStore)
	state.sqlDB = sqlDB
	state.botID = botID
	if opts.SvcCtx != nil {
		state.commsURL = opts.SvcCtx.Config.NeboLoop.CommsURL
		state.apiURL = opts.SvcCtx.Config.NeboLoop.ApiURL
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

	devlog.Printf("[agent] Comm startup: enabled=%v plugin=%q (config: enabled=%v plugin=%q)\n",
		commEnabled, commPlugin, cfg.Comm.Enabled, cfg.Comm.Plugin)

	if commEnabled {
		if commPlugin == "" {
			commPlugin = "loopback"
		}
		if err := commManager.SetActive(commPlugin); err != nil {
			devlog.Printf("[agent] Warning: failed to set active comm plugin: %v\n", err)
		} else if active := commManager.GetActive(); active != nil {
			// Build comm config: start with DB settings, then inject JWT + bot_id
			commConfig := cfg.Comm.Config
			if opts.PluginStore != nil {
				if dbSettings, err := opts.PluginStore.GetSettingsByName(ctx, commPlugin); err == nil && len(dbSettings) > 0 {
					commConfig = dbSettings
				}
			}

			// For neboloop plugin: inject owner JWT from auth_profiles + bot_id
			if commPlugin == "neboloop" {
				commConfig = injectNeboLoopAuth(ctx, sqlDB, botID, commConfig)
				if commConfig["gateway"] == "" && state.commsURL != "" {
					commConfig["gateway"] = state.commsURL
				}
			}

			if commConfig["token"] != "" || commPlugin != "neboloop" {
				devlog.Printf("[agent] Connecting comm plugin %s...\n", commPlugin)
				connectT0 := time.Now()
				if err := active.Connect(ctx, commConfig); err != nil {
					devlog.Printf("[agent] Comm connect failed (%s): %v\n", time.Since(connectT0), err)
					// On auth failure for neboloop, try refreshing the expired token
					if commPlugin == "neboloop" && strings.Contains(err.Error(), "auth failed") {
						if freshToken := tryRefreshNeboLoopToken(ctx, sqlDB); freshToken != "" {
							commConfig["token"] = freshToken
							err = active.Connect(ctx, commConfig)
						}
					}
					if err != nil {
						devlog.Printf("[agent] Warning: failed to connect comm plugin %s: %v\n", commPlugin, err)
					}
				} else {
					devlog.Printf("[agent] Comm connected (%s)\n", time.Since(connectT0))
				}
				if active.IsConnected() {
					card := buildAgentCard(registry, skillLoader)
					active.Register(ctx, agentID, card)
					devlog.Printf("[agent] Comm plugin %s registered (agent: %s)\n", commPlugin, agentID)
				}
			} else {
				fmt.Println("[agent] NeboLoop comm: bot_id ready, waiting for OAuth login to connect")
			}
		}
	}

	// Background update checker: checks every 6 hours, notifies frontend once per new version.
	// Auto-downloads the update binary for direct installs.
	if opts.SvcCtx != nil && opts.SvcCtx.Version != "" && opts.SvcCtx.Version != "dev" {
		installMethod := updater.DetectInstallMethod()
		checker := updater.NewBackgroundChecker(opts.SvcCtx.Version, 6*time.Hour, func(result *updater.Result) {
			state.sendFrame(map[string]any{
				"type":   "event",
				"method": "update_available",
				"payload": map[string]any{
					"current_version": result.CurrentVersion,
					"latest_version":  result.LatestVersion,
					"release_url":     result.ReleaseURL,
					"install_method":  installMethod,
					"can_auto_update": installMethod == "direct",
				},
			})

			// Auto-download for direct installs
			if installMethod == "direct" {
				go func() {
					tmpPath, err := updater.Download(ctx, result.LatestVersion, func(dl, total int64) {
						pct := int64(0)
						if total > 0 {
							pct = dl * 100 / total
						}
						state.sendFrame(map[string]any{
							"type":   "event",
							"method": "update_progress",
							"payload": map[string]any{
								"downloaded": dl,
								"total":      total,
								"percent":    pct,
							},
						})
					})
					if err != nil {
						state.sendFrame(map[string]any{
							"type":   "event",
							"method": "update_error",
							"payload": map[string]any{
								"error": err.Error(),
							},
						})
						return
					}
					if err := updater.VerifyChecksum(ctx, tmpPath, result.LatestVersion); err != nil {
						os.Remove(tmpPath)
						state.sendFrame(map[string]any{
							"type":   "event",
							"method": "update_error",
							"payload": map[string]any{
								"error": "checksum verification failed: " + err.Error(),
							},
						})
						return
					}
					if um := opts.SvcCtx.UpdateManager(); um != nil {
						um.SetPending(tmpPath, result.LatestVersion)
					}
					state.sendFrame(map[string]any{
						"type":   "event",
						"method": "update_ready",
						"payload": map[string]any{
							"version": result.LatestVersion,
						},
					})
				}()
			}
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

// introductionInProgress tracks which sessions have an introduction running to prevent duplicates.
var introductionInProgress sync.Map

// handleIntroduction handles an explicit introduction request from the server
// This is called when a user loads an empty companion chat
func handleIntroduction(ctx context.Context, state *agentState, r *runner.Runner, sessions *session.Manager, requestID, sessionKey, userID string) {
	fmt.Printf("[Agent] Handling introduction request: id=%s session=%s user=%s\n", requestID, sessionKey, userID)

	// Deduplicate: only one introduction per session at a time
	if _, running := introductionInProgress.LoadOrStore(sessionKey, true); running {
		fmt.Printf("[Agent] Introduction already in progress for session %s, skipping duplicate\n", sessionKey)
		state.sendFrame(map[string]any{
			"type": "res",
			"id":   requestID,
			"ok":   true,
			"payload": map[string]any{
				"result":  "",
				"skipped": true,
			},
		})
		return
	}
	defer introductionInProgress.Delete(sessionKey)

	// Check if user has already completed onboarding — skip introduction if so
	dbContext, loadErr := memory.LoadContext(sessions.GetDB(), userID)
	if loadErr != nil {
		fmt.Printf("[Agent] LoadContext error (userID=%q): %v\n", userID, loadErr)
	}
	fmt.Printf("[Agent] Introduction check: dbContext=%v, needsOnboarding=%v, userID=%q\n",
		dbContext != nil, dbContext != nil && dbContext.NeedsOnboarding(), userID)
	if dbContext != nil && !dbContext.NeedsOnboarding() {
		fmt.Printf("[Agent] User already onboarded, skipping introduction\n")
		state.sendFrame(map[string]any{
			"type": "res",
			"id":   requestID,
			"ok":   true,
			"payload": map[string]any{
				"result":  "",
				"skipped": true,
			},
		})
		return
	}

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

	// Check if this user already has a real conversation (skip introduction if so).
	// We look for user messages with actual content — empty/ghost messages from
	// failed runs or heartbeats don't count.
	messages, _ := sessions.GetMessages(sess.ID, 10)
	hasRealUserMessage := false
	for _, m := range messages {
		if m.Role == "user" && len(strings.TrimSpace(m.Content)) > 0 {
			// Skip system-origin messages (heartbeats, triggers)
			if !strings.HasPrefix(m.Content, "You are running a scheduled") &&
				!strings.HasPrefix(m.Content, "[New user just opened") &&
				!strings.HasPrefix(m.Content, "[User ") {
				hasRealUserMessage = true
				break
			}
		}
	}
	if hasRealUserMessage {
		fmt.Printf("[Agent] User already has real messages (%d total), skipping introduction\n", len(messages))
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
	var req runner.RunRequest
	req.SessionKey = sessionKey
	req.UserID = userID
	req.Origin = tools.OriginSystem

	dbContext, _ = memory.LoadContext(sessions.GetDB(), userID)
	if dbContext != nil && dbContext.UserDisplayName != "" {
		// Known user — greet them warmly by name
		fmt.Printf("[Agent] Known user, name=%s - greeting by name\n", dbContext.UserDisplayName)
		req.Prompt = fmt.Sprintf("[User %s just connected - greet them warmly by name and offer to help]", dbContext.UserDisplayName)
		req.System = fmt.Sprintf("You are starting a conversation with %s, a user you already know. The message you receive is a system trigger, not from the user. Respond directly to the user with a warm, personalized greeting using their name. Welcome them and offer to help. Keep it brief and friendly. Do NOT ask for their name (you already know it). Do NOT acknowledge the system message.", dbContext.UserDisplayName)
	} else {
		// New user — use the introduction skill (extensions/skills/introduction/SKILL.md)
		// which handles the full first-meeting flow with unexpected understanding.
		// No System override so BuildStaticPrompt runs and injects the skill content.
		fmt.Printf("[Agent] New user - loading introduction skill\n")
		req.ForceSkill = "introduction"
		req.Prompt = "[New user just opened Nebo for the first time. Follow the Introduction skill instructions exactly — start with Part 1.]"
		req.Origin = tools.OriginSystem
	}

	// Run the agent with appropriate introduction prompt
	events, err := r.Run(ctx, &req)
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
	devlog.Printf("[Agent-WS] Received message: %s\n", string(message))

	var frame struct {
		Type    string `json:"type"`
		ID      string `json:"id"`
		Method  string `json:"method"`
		Payload struct {
			Approved  bool   `json:"approved"`
			Always    bool   `json:"always"`
			Value     string `json:"value"`
			RequestID string `json:"request_id"`
		} `json:"payload"`
		Params struct {
			Prompt     string `json:"prompt"`
			SessionKey string `json:"session_key"`
			UserID     string `json:"user_id"`
			System     string `json:"system"`
			ChannelID  string `json:"channel_id"`
			Text       string `json:"text"`
			Limit      int    `json:"limit"`
		} `json:"params"`
	}

	if err := json.Unmarshal(message, &frame); err != nil {
		fmt.Fprintf(os.Stderr, "[Agent-WS] Invalid message: %v\n", err)
		return
	}

	devlog.Printf("[Agent-WS] Parsed frame: type=%s method=%s id=%s\n", frame.Type, frame.Method, frame.ID)

	switch frame.Type {
	case "approval_response":
		state.handleApprovalResponse(frame.ID, frame.Payload.Approved, frame.Payload.Always)

	case "ask_response":
		// The request_id comes in the payload (routed through hub/chat)
		reqID := frame.Payload.RequestID
		if reqID == "" {
			reqID = frame.ID // fallback
		}
		state.handleAskResponse(reqID, frame.Payload.Value)

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

			devlog.Printf("[Agent-WS] Enqueueing introduce request: session=%s user=%s\n", sessionKey, userID)

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

		case "get_loops":
			var loops []map[string]any
			if active := state.commManager.GetActive(); active != nil {
				// Fetch loop names from REST API
				loopNames := make(map[string]string) // loopID → name
				if ll, ok := active.(comm.LoopLister); ok {
					if loopList, err := ll.ListLoops(ctx); err == nil {
						for _, l := range loopList {
							loopNames[l.ID] = l.Name
						}
					}
				}

				// Fetch channels and group by loop
				if lister, ok := active.(comm.LoopChannelLister); ok {
					channels, err := lister.ListLoopChannels(ctx)
					if err == nil {
						type loopEntry struct {
							ID       string
							Name     string
							Channels []map[string]any
						}
						loopMap := make(map[string]*loopEntry)
						for _, ch := range channels {
							l, ok := loopMap[ch.LoopID]
							if !ok {
								name := loopNames[ch.LoopID]
								if name == "" {
									name = ch.LoopName // fallback to channel metadata
								}
								l = &loopEntry{ID: ch.LoopID, Name: name}
								loopMap[ch.LoopID] = l
							}
							l.Channels = append(l.Channels, map[string]any{
								"channel_id":   ch.ChannelID,
								"channel_name": ch.ChannelName,
							})
						}
						// If any loop still has no name, try per-loop fetch via LoopGetter
						if lg, ok := active.(comm.LoopGetter); ok {
							for _, l := range loopMap {
								if l.Name == "" || l.Name == l.ID {
									if info, err := lg.GetLoopInfo(ctx, l.ID); err == nil && info.Name != "" {
										l.Name = info.Name
									}
								}
							}
						}
						for _, l := range loopMap {
							loops = append(loops, map[string]any{
								"id":       l.ID,
								"name":     l.Name,
								"channels": l.Channels,
							})
						}
					}
				}
			}
			if loops == nil {
				loops = []map[string]any{}
			}

			// Include lane activity summary for heartbeat/events
			laneStats := state.lanes.GetLaneStats()
			var heartbeatActive bool
			var eventsActive int
			if hs, ok := laneStats[agenthub.LaneHeartbeat]; ok {
				heartbeatActive = hs.Active > 0
			}
			if es, ok := laneStats[agenthub.LaneEvents]; ok {
				eventsActive = es.Active + es.Queued
			}
			var desktopActive bool
			if ds, ok := laneStats[agenthub.LaneDesktop]; ok {
				desktopActive = ds.Active > 0
			}

			state.sendFrame(map[string]any{
				"type": "res",
				"id":   frame.ID,
				"ok":   true,
				"payload": map[string]any{
					"loops":            loops,
					"heartbeat_active": heartbeatActive,
					"events_active":    eventsActive,
					"desktop_active":   desktopActive,
				},
			})

		case "get_channel_messages":
			channelID := frame.Params.ChannelID
			limit := frame.Params.Limit
			if limit <= 0 {
				limit = 50
			}
			if channelID == "" {
				state.sendFrame(map[string]any{
					"type": "res", "id": frame.ID, "ok": false,
					"payload": map[string]any{"error": "channel_id is required"},
				})
				break
			}
			var messages []comm.ChannelMessageItem
			var members []comm.ChannelMemberItem
			if active := state.commManager.GetActive(); active != nil {
				if lister, ok := active.(comm.ChannelMessageLister); ok {
					msgs, err := lister.ListChannelMessages(ctx, channelID, limit)
					if err != nil {
						state.sendFrame(map[string]any{
							"type": "res", "id": frame.ID, "ok": false,
							"payload": map[string]any{"error": err.Error()},
						})
						break
					}
					messages = msgs
				}
				if ml, ok := active.(comm.ChannelMemberLister); ok {
					if m, err := ml.ListChannelMembers(ctx, channelID); err == nil {
						members = m
					}
				}
			}
			if messages == nil {
				messages = []comm.ChannelMessageItem{}
			}
			if members == nil {
				members = []comm.ChannelMemberItem{}
			}
			// Render markdown server-side (same pattern as companion chat)
			type renderedMsg struct {
				comm.ChannelMessageItem
				ContentHtml string `json:"content_html"`
			}
			rendered := make([]renderedMsg, len(messages))
			for i, m := range messages {
				rendered[i] = renderedMsg{
					ChannelMessageItem: m,
					ContentHtml:        markdown.Render(m.Content),
				}
			}
			state.sendFrame(map[string]any{
				"type": "res", "id": frame.ID, "ok": true,
				"payload": map[string]any{"messages": rendered, "members": members},
			})

		case "send_channel_message":
			channelID := frame.Params.ChannelID
			text := frame.Params.Text
			if channelID == "" || text == "" {
				state.sendFrame(map[string]any{
					"type": "res", "id": frame.ID, "ok": false,
					"payload": map[string]any{"error": "channel_id and text are required"},
				})
				break
			}
			err := state.commManager.Send(ctx, comm.CommMessage{
				To:      channelID,
				Type:    comm.CommTypeLoopChannel,
				Content: text,
			})
			if err != nil {
				state.sendFrame(map[string]any{
					"type": "res", "id": frame.ID, "ok": false,
					"payload": map[string]any{"error": err.Error()},
				})
				break
			}
			state.sendFrame(map[string]any{
				"type": "res", "id": frame.ID, "ok": true,
				"payload": map[string]any{"success": true},
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
			devlog.Printf("[Agent-WS] Cancel: cancelled %d active\n", cancelled)
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
			if handleLoopCode(ctx, prompt, requestID, pluginStore, state, func(f map[string]any) {
				state.sendFrame(f)
			}) {
				break
			}

			// Intercept skill install codes before enqueueing to LLM
			if handleSkillCode(ctx, prompt, requestID, pluginStore, state, func(f map[string]any) {
				state.sendFrame(f)
			}) {
				break
			}

			// Determine which lane this request belongs to
			isHeartbeat := strings.HasPrefix(sessionKey, "heartbeat-")
			isCronJob := strings.HasPrefix(sessionKey, "reminder-") || strings.HasPrefix(sessionKey, "routine-")
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

			// Cache companion chat session ID for DM handler to reuse
			if lane == agenthub.LaneMain && !isHeartbeat && !isCronJob && !isCommMsg && !isDev {
				state.companionChatIDMu.Lock()
				state.companionChatID = sessionKey
				state.companionChatIDMu.Unlock()
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

			devlog.Printf("[Agent-WS] Enqueueing %s request: session=%s user=%s lane=%s prompt=%q\n",
				method, sessionKey, userID, lane, prompt)

			// Resolve lane model override from config
			var laneModelOverride string
			if cfg := provider.GetModelsConfig(); cfg != nil && cfg.LaneRouting != nil {
				lr := cfg.LaneRouting
				switch {
				case isHeartbeat && lr.Heartbeat != "":
					laneModelOverride = lr.Heartbeat
				case isCronJob && lr.Events != "":
					laneModelOverride = lr.Events
				case isCommMsg && lr.Comm != "":
					laneModelOverride = lr.Comm
				}
			}

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
					ModelOverride:     laneModelOverride,
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
						devlog.Printf("[Agent-WS] Tool call: %s (id=%s)\n", event.ToolCall.Name, event.ToolCall.ID)
						// Memory operations are silent — don't show tool cards in the UI
						if !isSilentToolCall(event.ToolCall) {
							state.sendFrame(map[string]any{
								"type": "stream",
								"id":   requestID,
								"payload": map[string]any{
									"tool":    event.ToolCall.Name,
									"tool_id": event.ToolCall.ID,
									"input":   event.ToolCall.Input,
								},
							})
						}

					case ai.EventTypeToolResult:
						toolName := ""
						toolID := ""
						if event.ToolCall != nil {
							toolName = event.ToolCall.Name
							toolID = event.ToolCall.ID
						}
						devlog.Printf("[Agent-WS] Tool result: %s (id=%s) len=%d\n", toolName, toolID, len(event.Text))
						// Memory operations are silent — don't show tool cards in the UI
						if !isSilentToolCall(event.ToolCall) {
							payload := map[string]any{
								"tool_result": event.Text,
								"tool_name":   toolName,
								"tool_id":     toolID,
							}
							if event.ImageURL != "" {
								payload["image_url"] = event.ImageURL
							}
							state.sendFrame(map[string]any{
								"type":    "stream",
								"id":      requestID,
								"payload": payload,
							})
						}

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

						// DON'T send text as chunk — already streamed via EventTypeText.
						// DON'T accumulate into result — already accumulated from EventTypeText.
						// EventTypeMessage is for session persistence (handled by runner).
						// Fallback: if no text events arrived (non-streaming provider), accumulate.
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
									devlog.Printf("[Agent-WS] Tool result (from message): id=%s len=%d\n", tr.ToolCallID, len(tr.Content))
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
						devlog.Printf("[Agent-WS] Error event: %v\n", event.Error)
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

				// Forward main lane responses to loop channel so the owner's
				// loop stays in sync with the companion chat.
				if lane == agenthub.LaneMain && method == "run" && result.Len() > 0 {
					if active := state.commManager.GetActive(); active != nil {
						if nlp, ok := active.(*neboloop.Plugin); ok {
							if client := nlp.Client(); client != nil {
								metas := client.ChannelMetas()
								for chID := range metas {
									loopContent, _ := json.Marshal(map[string]string{
										"channel_id": chID,
										"text":       result.String(),
									})
									convs := client.Channels()
									if convIDStr, ok := convs[chID]; ok {
										if convID, err := uuid.Parse(convIDStr); err == nil {
											if err := client.Send(taskCtx, convID, "channel", loopContent); err != nil {
												devlog.Printf("[Agent-WS] Loop forward failed for channel %s: %v\n", chID, err)
											}
										}
									}
									break // send to first channel only
								}
							}
						}
					}
				}

				state.sendFrame(map[string]any{
					"type": "res",
					"id":   requestID,
					"ok":   true,
					"payload": map[string]any{
						"result": result.String(),
					},
				})
				devlog.Printf("[Agent-WS] Completed request %s\n", requestID)
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
							devlog.Printf("[agent] MCP bridge re-sync: %v\n", err)
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

	// Load from DB auth_profiles — same source as createProviders.
	// Priority: Janus (centralised) → OpenAI (direct) → Ollama (local).
	mgr, err := agentcfg.NewAuthProfileManager(db)
	if err == nil {
		defer mgr.Close()
		ctx := context.Background()

		// Try Janus first — when configured, all model routing goes through it
		if sharedJanusURL != "" {
			if profiles, err := mgr.ListActiveProfiles(ctx, "neboloop"); err == nil {
				for _, p := range profiles {
					if p.APIKey != "" {
						embeddingProvider = embeddings.NewOpenAIProvider(embeddings.OpenAIConfig{
							APIKey:  p.APIKey,
							Model:   "janus/text-embedding-small",
							BaseURL: sharedJanusURL + "/v1",
						})
						devlog.Printf("[agent] Embeddings: using Janus text-embedding-small (%s)\n", sharedJanusURL)
						break
					}
				}
			}
		}

		// Fall back to direct OpenAI
		if embeddingProvider == nil {
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
						devlog.Printf("[agent] Warning: could not ensure embedding model %s: %v\n", embModel, err)
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
		devlog.Printf("[agent] Warning: failed to create embedding service: %v\n", err)
		return nil
	}

	return service
}

// injectNeboLoopAuth merges the owner's OAuth JWT and bot_id into a comm config map.
// Returns a new map (does not mutate the input).
func injectNeboLoopAuth(ctx context.Context, sqlDB *sql.DB, botID string, base map[string]string) map[string]string {
	out := make(map[string]string, len(base)+2)
	for k, v := range base {
		out[k] = v
	}
	out["bot_id"] = botID

	// Only inject OAuth JWT if no token already present (e.g. from connection_token)
	if out["token"] == "" {
		jwt := getNeboLoopJWT(ctx, sqlDB)
		if jwt != "" {
			out["token"] = jwt
		}
	}
	return out
}

// handleCommSettingsUpdate activates or deactivates the comm plugin based on
// a settings_updated event from the server.
func handleCommSettingsUpdate(ctx context.Context, state *agentState, enabled bool, pluginName string, pluginStore *settings.Store) {
	if !enabled {
		// Deactivate: disconnect the current plugin
		if active := state.commManager.GetActive(); active != nil {
			if err := active.Disconnect(ctx); err != nil {
				devlog.Printf("[Comm] Warning: failed to disconnect comm plugin: %v\n", err)
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
		devlog.Printf("[Comm] Warning: failed to activate %s: %v\n", pluginName, err)
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
			devlog.Printf("[Comm] Loaded %s settings from database (%d keys)\n", pluginName, len(dbSettings))
		}
	}

	// For neboloop plugin: inject owner JWT from auth_profiles + bot_id
	if pluginName == "neboloop" && state.sqlDB != nil {
		commConfig = injectNeboLoopAuth(ctx, state.sqlDB, state.botID, commConfig)
		if commConfig["gateway"] == "" && state.commsURL != "" {
			commConfig["gateway"] = state.commsURL
		}
		if commConfig["token"] == "" {
			fmt.Println("[Comm] NeboLoop: no JWT available yet, skipping connect")
			return
		}
	}

	if err := active.Connect(ctx, commConfig); err != nil {
		// On auth failure for neboloop, try refreshing the expired token
		if pluginName == "neboloop" && state.sqlDB != nil && strings.Contains(err.Error(), "auth failed") {
			if freshToken := tryRefreshNeboLoopToken(ctx, state.sqlDB); freshToken != "" {
				commConfig["token"] = freshToken
				err = active.Connect(ctx, commConfig)
			}
		}
		if err != nil {
			devlog.Printf("[Comm] Warning: failed to connect %s: %v\n", pluginName, err)
			return
		}
	}

	card := buildAgentCard(state.registry, state.skillLoader)
	active.Register(ctx, state.commAgentID, card)
	devlog.Printf("[Comm] Plugin %s activated and connected via settings update (agent: %s)\n", pluginName, state.commAgentID)
}

// loopQuerierAdapter wraps the NeboLoop comm plugin to implement tools.LoopQuerier.
// Converts neboloop API types → tools info types at the boundary.
type loopQuerierAdapter struct {
	plugin *neboloop.Plugin
}

func (a *loopQuerierAdapter) ListLoops(ctx context.Context) ([]tools.LoopInfo, error) {
	loops, err := a.plugin.ListBotLoops(ctx)
	if err != nil {
		return nil, err
	}
	result := make([]tools.LoopInfo, len(loops))
	for i, l := range loops {
		result[i] = tools.LoopInfo{ID: l.ID, Name: l.Name}
	}
	return result, nil
}

func (a *loopQuerierAdapter) GetLoop(ctx context.Context, loopID string) (*tools.LoopInfo, error) {
	loop, err := a.plugin.GetLoop(ctx, loopID)
	if err != nil {
		return nil, err
	}
	return &tools.LoopInfo{ID: loop.ID, Name: loop.Name, Description: loop.Description, MemberCount: loop.MemberCount}, nil
}

func (a *loopQuerierAdapter) ListLoopMembers(ctx context.Context, loopID string) ([]tools.MemberInfo, error) {
	members, err := a.plugin.ListLoopMembers(ctx, loopID)
	if err != nil {
		return nil, err
	}
	result := make([]tools.MemberInfo, len(members))
	for i, m := range members {
		result[i] = tools.MemberInfo{BotID: m.BotID, BotName: m.BotName, Role: m.Role, IsOnline: m.IsOnline}
	}
	return result, nil
}

func (a *loopQuerierAdapter) ListChannelMembers(ctx context.Context, channelID string) ([]tools.MemberInfo, error) {
	members, err := a.plugin.ListChannelMembers(ctx, channelID)
	if err != nil {
		return nil, err
	}
	result := make([]tools.MemberInfo, len(members))
	for i, m := range members {
		result[i] = tools.MemberInfo{BotID: m.BotID, BotName: m.BotName, Role: m.Role, IsOnline: m.IsOnline}
	}
	return result, nil
}

func (a *loopQuerierAdapter) ListChannelMessages(ctx context.Context, channelID string, limit int) ([]tools.MessageInfo, error) {
	messages, err := a.plugin.ListChannelMessages(ctx, channelID, limit)
	if err != nil {
		return nil, err
	}
	result := make([]tools.MessageInfo, len(messages))
	for i, msg := range messages {
		result[i] = tools.MessageInfo{ID: msg.ID, From: msg.From, Content: msg.Content, CreatedAt: msg.CreatedAt}
	}
	return result, nil
}

// sessionQuerierAdapter wraps session.Manager to implement tools.SessionQuerier.
// Converts db.AgentSession/AgentMessage → tools.SessionInfo/SessionMessage at the boundary.
type sessionQuerierAdapter struct {
	mgr *session.Manager
}

func (a *sessionQuerierAdapter) ListSessions(userID string) ([]tools.SessionInfo, error) {
	sessions, err := a.mgr.ListSessions(userID)
	if err != nil {
		return nil, err
	}
	result := make([]tools.SessionInfo, len(sessions))
	for i, s := range sessions {
		result[i] = tools.SessionInfo{ID: s.ID, SessionKey: s.SessionKey, CreatedAt: s.CreatedAt, UpdatedAt: s.UpdatedAt}
	}
	return result, nil
}

func (a *sessionQuerierAdapter) GetMessages(sessionID string, limit int) ([]tools.SessionMessage, error) {
	messages, err := a.mgr.GetMessages(sessionID, limit)
	if err != nil {
		return nil, err
	}
	result := make([]tools.SessionMessage, len(messages))
	for i, m := range messages {
		result[i] = tools.SessionMessage{Role: m.Role, Content: m.Content}
	}
	return result, nil
}

func (a *sessionQuerierAdapter) GetOrCreate(sessionKey, userID string) (*tools.SessionInfo, error) {
	sess, err := a.mgr.GetOrCreate(sessionKey, userID)
	if err != nil {
		return nil, err
	}
	return &tools.SessionInfo{ID: sess.ID, SessionKey: sess.SessionKey, CreatedAt: sess.CreatedAt, UpdatedAt: sess.UpdatedAt}, nil
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

// loadSkills loads standalone skills from embedded bundle and user directories.
func loadSkills(cfg *agentcfg.Config) *skills.Loader {
	loader := skills.NewLoader(filepath.Join(cfg.DataDir, "skills"))

	// Load bundled skills from the embedded filesystem (always available regardless of cwd)
	if err := loader.LoadFromEmbedFS(extensions.BundledSkills, "skills"); err != nil {
		devlog.Printf("[agent] Warning: failed to load bundled skills: %v\n", err)
	}

	// Merge user skills from data directory (user skills override bundled by name)
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
