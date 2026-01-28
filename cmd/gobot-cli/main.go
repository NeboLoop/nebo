package main

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"sync"
	"syscall"

	"github.com/gorilla/websocket"
	"github.com/spf13/cobra"

	"gobot/agent/ai"
	"gobot/agent/config"
	agentmcp "gobot/agent/mcp"
	"gobot/agent/plugins"
	"gobot/agent/runner"
	"gobot/agent/session"
	"gobot/agent/skills"
	"gobot/agent/tools"
)

var (
	cfgFile     string
	sessionKey  string
	providerArg string
	verbose     bool
)

func main() {
	rootCmd := &cobra.Command{
		Use:   "gobot",
		Short: "GoBot - Enterprise AI Assistant",
		Long:  `GoBot is an AI assistant with tool use capabilities for software development and automation.`,
	}

	// Global flags
	rootCmd.PersistentFlags().StringVar(&cfgFile, "config", "", "config file (default: ~/.gobot/config.yaml)")
	rootCmd.PersistentFlags().StringVarP(&sessionKey, "session", "s", "default", "session key for conversation history")
	rootCmd.PersistentFlags().StringVarP(&providerArg, "provider", "p", "", "provider to use (default: first available)")
	rootCmd.PersistentFlags().BoolVarP(&verbose, "verbose", "v", false, "verbose output")

	// Add commands
	rootCmd.AddCommand(chatCmd())
	rootCmd.AddCommand(configCmd())
	rootCmd.AddCommand(sessionCmd())
	rootCmd.AddCommand(agentCmd())
	rootCmd.AddCommand(mcpCmd())
	rootCmd.AddCommand(skillsCmd())
	rootCmd.AddCommand(pluginsCmd())
	rootCmd.AddCommand(daemonCmd())

	if err := rootCmd.Execute(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

// chatCmd creates the chat command
func chatCmd() *cobra.Command {
	var interactive bool
	var dangerously bool

	cmd := &cobra.Command{
		Use:   "chat [prompt]",
		Short: "Chat with the AI assistant",
		Long: `Send a message to the AI assistant and receive a streaming response.
The assistant has access to tools for file operations, shell commands, and more.

Examples:
  gobot chat "Hello, what can you do?"
  gobot chat "List all Go files in this directory"
  gobot chat --interactive
  gobot chat --dangerously "deploy to production"`,
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadConfig()
			runChat(cfg, args, interactive, dangerously)
		},
	}

	cmd.Flags().BoolVarP(&interactive, "interactive", "i", false, "start interactive chat session")
	cmd.Flags().BoolVar(&dangerously, "dangerously", false, "100% autonomous mode - bypass ALL tool approval prompts (use with caution!)")

	return cmd
}

// configCmd creates the config command
func configCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "config",
		Short: "Show or manage configuration",
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadConfig()
			showConfig(cfg)
		},
	}

	cmd.AddCommand(&cobra.Command{
		Use:   "init",
		Short: "Initialize configuration file",
		Run: func(cmd *cobra.Command, args []string) {
			initConfig()
		},
	})

	return cmd
}

// sessionCmd creates the session command
func sessionCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "session",
		Short: "Manage chat sessions",
	}

	cmd.AddCommand(&cobra.Command{
		Use:   "list",
		Short: "List all sessions",
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadConfig()
			listSessions(cfg)
		},
	})

	cmd.AddCommand(&cobra.Command{
		Use:   "clear [session-key]",
		Short: "Clear a session's history",
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadConfig()
			key := sessionKey
			if len(args) > 0 {
				key = args[0]
			}
			clearSession(cfg, key)
		},
	})

	return cmd
}

// loadConfig loads the configuration
func loadConfig() *config.Config {
	var cfg *config.Config
	var err error

	if cfgFile != "" {
		cfg, err = config.LoadFrom(cfgFile)
	} else {
		cfg, err = config.Load()
	}

	if err != nil {
		fmt.Fprintf(os.Stderr, "Error loading config: %v\n", err)
		os.Exit(1)
	}

	// Ensure data directory exists
	if err := cfg.EnsureDataDir(); err != nil {
		fmt.Fprintf(os.Stderr, "Error creating data directory: %v\n", err)
		os.Exit(1)
	}

	return cfg
}

// runChat runs the chat command
func runChat(cfg *config.Config, args []string, interactive bool, dangerously bool) {
	// Handle dangerous mode confirmation
	if dangerously {
		if !confirmDangerousMode() {
			fmt.Println("Aborted.")
			os.Exit(0)
		}
	}

	// Create session manager
	sessions, err := session.New(cfg.DBPath())
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error opening database: %v\n", err)
		os.Exit(1)
	}
	defer sessions.Close()

	// Create providers
	providers := createProviders(cfg)
	if len(providers) == 0 {
		fmt.Fprintln(os.Stderr, "No providers configured. Set ANTHROPIC_API_KEY or configure providers in ~/.gobot/config.yaml")
		os.Exit(1)
	}

	// Create tool registry with policy
	var policy *tools.Policy
	if dangerously {
		// 100% autonomous mode - no approval prompts
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

	// Register task tools with orchestrator for sub-agent support
	taskTool := tools.NewTaskTool()
	taskTool.CreateOrchestrator(cfg, sessions, providers, registry)
	registry.Register(taskTool)

	agentStatusTool := tools.NewAgentStatusTool()
	agentStatusTool.SetOrchestrator(taskTool.GetOrchestrator())
	registry.Register(agentStatusTool)

	// Create runner
	r := runner.New(cfg, sessions, providers, registry)

	// Handle signals
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
	go func() {
		<-sigCh
		fmt.Println("\n\033[33mInterrupted\033[0m")
		cancel()
	}()

	if interactive || len(args) == 0 {
		runInteractive(ctx, r, sessions)
	} else {
		prompt := strings.Join(args, " ")
		runOnce(ctx, r, prompt)
	}
}

// runOnce runs a single prompt
func runOnce(ctx context.Context, r *runner.Runner, prompt string) {
	events, err := r.Run(ctx, &runner.RunRequest{
		SessionKey: sessionKey,
		Prompt:     prompt,
	})
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		os.Exit(1)
	}

	for event := range events {
		handleEvent(event)
	}
	fmt.Println()
}

// runInteractive runs an interactive chat session
func runInteractive(ctx context.Context, r *runner.Runner, sessions *session.Manager) {
	fmt.Println("\033[1mGoBot Interactive Mode\033[0m")
	fmt.Println("Type your message and press Enter. Use /help for commands, Ctrl+C to exit.")
	fmt.Println()

	reader := bufio.NewReader(os.Stdin)

	for {
		fmt.Print("\033[36m> \033[0m")

		line, err := reader.ReadString('\n')
		if err != nil {
			break
		}

		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}

		// Handle commands
		if strings.HasPrefix(line, "/") {
			if handleCommand(line, sessions) {
				continue
			}
		}

		events, err := r.Run(ctx, &runner.RunRequest{
			SessionKey: sessionKey,
			Prompt:     line,
		})
		if err != nil {
			fmt.Fprintf(os.Stderr, "\033[31mError: %v\033[0m\n", err)
			continue
		}

		fmt.Print("\033[32m")
		for event := range events {
			handleEvent(event)
		}
		fmt.Print("\033[0m\n\n")
	}
}

// handleCommand handles interactive commands
func handleCommand(cmd string, sessions *session.Manager) bool {
	switch {
	case cmd == "/help":
		fmt.Println(`Commands:
  /help     - Show this help
  /clear    - Clear current session
  /sessions - List all sessions
  /quit     - Exit`)
		return true

	case cmd == "/clear":
		sess, err := sessions.GetOrCreate(sessionKey)
		if err == nil {
			sessions.Reset(sess.ID)
			fmt.Println("Session cleared.")
		}
		return true

	case cmd == "/sessions":
		list, _ := sessions.ListSessions()
		fmt.Println("Sessions:")
		for _, s := range list {
			marker := " "
			if s.SessionKey == sessionKey {
				marker = "*"
			}
			fmt.Printf("  %s %s (updated: %s)\n", marker, s.SessionKey, s.UpdatedAt.Format("2006-01-02 15:04"))
		}
		return true

	case cmd == "/quit" || cmd == "/exit":
		os.Exit(0)
		return true
	}

	return false
}

// handleEvent handles a streaming event
func handleEvent(event ai.StreamEvent) {
	switch event.Type {
	case ai.EventTypeText:
		fmt.Print(event.Text)

	case ai.EventTypeThinking:
		if verbose {
			fmt.Printf("\033[90m[thinking] %s\033[0m", event.Text)
		}

	case ai.EventTypeToolCall:
		if verbose {
			fmt.Printf("\n\033[33m[tool: %s]\033[0m\n", event.ToolCall.Name)
		}

	case ai.EventTypeToolResult:
		if verbose {
			preview := event.Text
			if len(preview) > 200 {
				preview = preview[:200] + "..."
			}
			fmt.Printf("\033[90m%s\033[0m\n", preview)
		}

	case ai.EventTypeError:
		fmt.Printf("\n\033[31mError: %v\033[0m\n", event.Error)

	case ai.EventTypeDone:
		// No output needed
	}
}

// createProviders creates AI providers from config
func createProviders(cfg *config.Config) []ai.Provider {
	var providers []ai.Provider

	for _, pcfg := range cfg.Providers {
		if providerArg != "" && pcfg.Name != providerArg {
			continue
		}

		switch pcfg.Type {
		case "api":
			if pcfg.APIKey == "" {
				continue
			}
			switch {
			case strings.Contains(pcfg.Name, "anthropic") || pcfg.Name == "claude":
				providers = append(providers, ai.NewAnthropicProvider(pcfg.APIKey, pcfg.Model))
			case strings.Contains(pcfg.Name, "openai") || pcfg.Name == "gpt":
				providers = append(providers, ai.NewOpenAIProvider(pcfg.APIKey, pcfg.Model))
			case strings.Contains(pcfg.Name, "gemini") || strings.Contains(pcfg.Name, "google"):
				providers = append(providers, ai.NewGeminiProvider(pcfg.APIKey, pcfg.Model))
			}

		case "ollama":
			// Ollama local model provider
			baseURL := pcfg.BaseURL
			if baseURL == "" {
				baseURL = "http://localhost:11434"
			}
			if ai.CheckOllamaAvailable(baseURL) {
				providers = append(providers, ai.NewOllamaProvider(baseURL, pcfg.Model))
			} else if verbose {
				fmt.Fprintf(os.Stderr, "Ollama provider %s: not available at %s\n", pcfg.Name, baseURL)
			}

		case "cli":
			// CLI providers wrap official CLI tools
			if pcfg.Command == "" {
				continue
			}
			if !ai.CheckCLIAvailable(pcfg.Command) {
				if verbose {
					fmt.Fprintf(os.Stderr, "CLI provider %s: command '%s' not found in PATH\n", pcfg.Name, pcfg.Command)
				}
				continue
			}
			providers = append(providers, ai.NewCLIProvider(pcfg.Name, pcfg.Command, pcfg.Args))

		default:
			// Default to API type for backwards compatibility
			if pcfg.APIKey != "" {
				// Try to infer provider from name
				if strings.Contains(pcfg.Name, "openai") || strings.Contains(pcfg.Name, "gpt") {
					providers = append(providers, ai.NewOpenAIProvider(pcfg.APIKey, pcfg.Model))
				} else if strings.Contains(pcfg.Name, "gemini") || strings.Contains(pcfg.Name, "google") {
					providers = append(providers, ai.NewGeminiProvider(pcfg.APIKey, pcfg.Model))
				} else {
					providers = append(providers, ai.NewAnthropicProvider(pcfg.APIKey, pcfg.Model))
				}
			}
		}
	}

	return providers
}

// showConfig displays the current configuration
func showConfig(cfg *config.Config) {
	fmt.Println("GoBot Configuration")
	fmt.Println("===================")
	fmt.Printf("Data Directory: %s\n", cfg.DataDir)
	fmt.Printf("Database: %s\n", cfg.DBPath())
	fmt.Printf("Max Context: %d messages\n", cfg.MaxContext)
	fmt.Printf("Max Iterations: %d\n", cfg.MaxIterations)
	fmt.Println()

	fmt.Println("Providers:")
	for _, p := range cfg.Providers {
		status := "\033[31m✗\033[0m"
		statusInfo := ""
		if p.Type == "cli" && p.Command != "" {
			if ai.CheckCLIAvailable(p.Command) {
				status = "\033[32m✓\033[0m"
				statusInfo = fmt.Sprintf(" (command: %s)", p.Command)
			} else {
				status = "\033[31m✗\033[0m"
				statusInfo = fmt.Sprintf(" (command '%s' not found)", p.Command)
			}
		} else if p.APIKey != "" {
			status = "\033[32m✓\033[0m"
		}
		fmt.Printf("  %s %s (%s)%s\n", status, p.Name, p.Type, statusInfo)
		if p.Model != "" {
			fmt.Printf("      Model: %s\n", p.Model)
		}
	}
	fmt.Println()

	fmt.Println("Policy:")
	fmt.Printf("  Level: %s\n", cfg.Policy.Level)
	fmt.Printf("  Ask Mode: %s\n", cfg.Policy.AskMode)
}

// initConfig initializes a new configuration file
func initConfig() {
	cfg := config.DefaultConfig()

	// Check if config already exists
	configPath := cfg.DataDir + "/config.yaml"
	if _, err := os.Stat(configPath); err == nil {
		fmt.Printf("Config file already exists: %s\n", configPath)
		return
	}

	if err := cfg.Save(); err != nil {
		fmt.Fprintf(os.Stderr, "Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("Created config file: %s\n", configPath)
	fmt.Println("\nEdit this file to configure providers and settings.")
	fmt.Println("Or set ANTHROPIC_API_KEY environment variable to get started.")
}

// listSessions lists all sessions
func listSessions(cfg *config.Config) {
	sessions, err := session.New(cfg.DBPath())
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		os.Exit(1)
	}
	defer sessions.Close()

	list, err := sessions.ListSessions()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		os.Exit(1)
	}

	if len(list) == 0 {
		fmt.Println("No sessions found.")
		return
	}

	fmt.Println("Sessions:")
	for _, s := range list {
		fmt.Printf("  %s (updated: %s)\n", s.SessionKey, s.UpdatedAt.Format("2006-01-02 15:04:05"))
	}
}

// clearSession clears a session's history
func clearSession(cfg *config.Config, key string) {
	sessions, err := session.New(cfg.DBPath())
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		os.Exit(1)
	}
	defer sessions.Close()

	sess, err := sessions.GetOrCreate(key)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		os.Exit(1)
	}

	if err := sessions.Reset(sess.ID); err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("Cleared session: %s\n", key)
}

// agentCmd creates the agent command for connecting to SaaS
func agentCmd() *cobra.Command {
	var orgID string
	var agentID string
	var serverURL string
	var token string
	var dangerously bool

	cmd := &cobra.Command{
		Use:   "agent",
		Short: "Connect to SaaS as an agent",
		Long: `Connect to a GoBot SaaS instance and receive tasks via WebSocket.

The agent connects to the SaaS server and waits for incoming tasks from
users or channel integrations.

Examples:
  gobot agent --org acme --token <jwt-token>
  gobot agent --org acme --server https://gobot.example.com
  gobot agent --org acme --dangerously --token <token>`,
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadConfig()
			runAgent(cfg, orgID, agentID, serverURL, token, dangerously)
		},
	}

	cmd.Flags().StringVar(&orgID, "org", "", "organization ID to connect to (required)")
	cmd.Flags().StringVar(&agentID, "agent-id", "", "agent ID (default: auto-generated)")
	cmd.Flags().StringVar(&serverURL, "server", "", "SaaS server URL (default: from config)")
	cmd.Flags().StringVar(&token, "token", "", "authentication token")
	cmd.Flags().BoolVar(&dangerously, "dangerously", false, "100% autonomous mode - bypass ALL tool approval prompts")
	cmd.MarkFlagRequired("org")

	return cmd
}

// confirmDangerousMode displays warnings and requires explicit confirmation
func confirmDangerousMode() bool {
	fmt.Println()
	fmt.Println("\033[1;31m╔══════════════════════════════════════════════════════════════════╗")
	fmt.Println("║                    ⚠️  DANGEROUS MODE WARNING ⚠️                    ║")
	fmt.Println("╠══════════════════════════════════════════════════════════════════╣")
	fmt.Println("║                                                                  ║")
	fmt.Println("║  You are about to run in FULLY AUTONOMOUS mode.                 ║")
	fmt.Println("║                                                                  ║")
	fmt.Println("║  This means:                                                    ║")
	fmt.Println("║    • ALL tool approval prompts will be BYPASSED                 ║")
	fmt.Println("║    • The AI can execute ANY shell command without asking        ║")
	fmt.Println("║    • The AI can delete, modify, or create ANY files             ║")
	fmt.Println("║    • The AI can make network requests without approval          ║")
	fmt.Println("║    • The AI can run browser automation unattended               ║")
	fmt.Println("║                                                                  ║")
	fmt.Println("║  \033[1;33mPOTENTIAL RISKS:\033[1;31m                                               ║")
	fmt.Println("║    • Accidental deletion of important files                     ║")
	fmt.Println("║    • Unintended system modifications                            ║")
	fmt.Println("║    • Execution of destructive commands (rm -rf, etc.)           ║")
	fmt.Println("║    • Data loss or corruption                                    ║")
	fmt.Println("║                                                                  ║")
	fmt.Println("║  \033[1;37mOnly use this mode if you:\033[1;31m                                    ║")
	fmt.Println("║    ✓ Fully trust the prompts you're sending                     ║")
	fmt.Println("║    ✓ Have backups of important data                             ║")
	fmt.Println("║    ✓ Understand the AI may make mistakes                        ║")
	fmt.Println("║    ✓ Are in a safe/sandboxed environment                        ║")
	fmt.Println("║                                                                  ║")
	fmt.Println("╚══════════════════════════════════════════════════════════════════╝\033[0m")
	fmt.Println()
	fmt.Print("\033[1;33mType 'yes' to continue in dangerous mode: \033[0m")

	reader := bufio.NewReader(os.Stdin)
	response, err := reader.ReadString('\n')
	if err != nil {
		return false
	}

	response = strings.TrimSpace(strings.ToLower(response))
	if response == "yes" {
		fmt.Println()
		fmt.Println("\033[1;31m🔓 DANGEROUS MODE ENABLED - All approvals bypassed!\033[0m")
		fmt.Println()
		return true
	}

	return false
}

// runAgent connects to SaaS and runs as an agent
func runAgent(cfg *config.Config, orgID, agentID, serverURL, token string, dangerously bool) {
	// Handle dangerous mode confirmation
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
		fmt.Fprintln(os.Stderr, "Error: server URL not configured. Use --server or set server_url in config.")
		os.Exit(1)
	}

	if token == "" {
		token = cfg.Token
	}
	if token == "" {
		fmt.Fprintln(os.Stderr, "Error: token not configured. Use --token or set token in config.")
		os.Exit(1)
	}

	if agentID == "" {
		hostname, _ := os.Hostname()
		agentID = fmt.Sprintf("agent-%s-%d", hostname, os.Getpid())
	}

	// Create WebSocket connection URL
	wsURL := strings.Replace(serverURL, "http://", "ws://", 1)
	wsURL = strings.Replace(wsURL, "https://", "wss://", 1)
	wsURL = fmt.Sprintf("%s/api/v1/agent/ws?org_id=%s&agent_id=%s&token=%s", wsURL, orgID, agentID, token)

	fmt.Printf("Connecting to SaaS: %s\n", serverURL)
	fmt.Printf("Organization: %s\n", orgID)
	fmt.Printf("Agent ID: %s\n", agentID)

	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error connecting to SaaS: %v\n", err)
		os.Exit(1)
	}
	defer conn.Close()

	fmt.Println("\033[32m✓ Connected to SaaS\033[0m")
	fmt.Println("Waiting for tasks... (Ctrl+C to exit)")

	// Create session manager
	sessions, err := session.New(cfg.DBPath())
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error opening database: %v\n", err)
		os.Exit(1)
	}
	defer sessions.Close()

	// Create providers
	providers := createProviders(cfg)
	if len(providers) == 0 {
		fmt.Fprintln(os.Stderr, "Warning: No AI providers configured. Tasks requiring AI will fail.")
	}

	// Create tool registry with policy
	var policy *tools.Policy
	if dangerously {
		// 100% autonomous mode - no approval prompts
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

	// Register task tools with orchestrator for sub-agent support
	taskTool := tools.NewTaskTool()
	taskTool.CreateOrchestrator(cfg, sessions, providers, registry)
	registry.Register(taskTool)

	agentStatusTool := tools.NewAgentStatusTool()
	agentStatusTool.SetOrchestrator(taskTool.GetOrchestrator())
	registry.Register(agentStatusTool)

	// Create runner
	r := runner.New(cfg, sessions, providers, registry)

	// Handle signals
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

	// Read messages from WebSocket
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

// handleAgentMessage processes a message from SaaS
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

		case "run":
			sessionKey := frame.Params.SessionKey
			if sessionKey == "" {
				sessionKey = "agent-" + frame.ID
			}

			fmt.Printf("\n\033[36m[Task %s]\033[0m %s\n", frame.ID, frame.Params.Prompt)

			events, err := r.Run(ctx, &runner.RunRequest{
				SessionKey: sessionKey,
				Prompt:     frame.Params.Prompt,
			})

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
			for event := range events {
				if event.Type == ai.EventTypeText {
					result.WriteString(event.Text)
					fmt.Print(event.Text)
				}
			}
			fmt.Println()

			response := map[string]any{
				"type": "res",
				"id":   frame.ID,
				"ok":   true,
				"payload": map[string]any{
					"result": result.String(),
				},
			}
			data, _ := json.Marshal(response)
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
		fmt.Printf("[Event] %s\n", string(message))
	}
}

// mcpCmd creates the MCP server command
func mcpCmd() *cobra.Command {
	var host string
	var port int

	cmd := &cobra.Command{
		Use:   "mcp",
		Short: "Start an MCP server exposing agent tools",
		Long: `Start an MCP server that exposes the agent's tools via the Model Context Protocol.

This allows external MCP clients to use the agent's tools (bash, read, write, etc.)
for their own AI assistants.

Examples:
  gobot mcp                          # Start on default port 8080
  gobot mcp --port 9000              # Start on custom port
  gobot mcp --host 0.0.0.0           # Listen on all interfaces`,
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadConfig()
			runMCPServer(cfg, host, port)
		},
	}

	cmd.Flags().StringVar(&host, "host", "localhost", "host to listen on")
	cmd.Flags().IntVar(&port, "port", 8080, "port to listen on")

	return cmd
}

// runMCPServer starts the MCP server
func runMCPServer(cfg *config.Config, host string, port int) {
	// Create policy and registry
	policy := tools.NewPolicyFromConfig(
		cfg.Policy.Level,
		cfg.Policy.AskMode,
		cfg.Policy.Allowlist,
	)
	registry := tools.NewRegistry(policy)
	registry.RegisterDefaults()

	// Create MCP server
	mcpServer := agentmcp.NewServer(registry)

	// Handle signals
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
	go func() {
		<-sigCh
		fmt.Println("\nShutting down MCP server...")
		cancel()
	}()

	addr := fmt.Sprintf("%s:%d", host, port)
	fmt.Printf("Starting MCP server at http://%s/mcp\n", addr)
	fmt.Println("Tools exposed:")
	for _, tool := range registry.List() {
		fmt.Printf("  - %s: %s\n", tool.Name, tool.Description)
	}
	fmt.Println("\nPress Ctrl+C to stop")

	mux := http.NewServeMux()
	mux.Handle("/mcp", mcpServer.Handler())
	mux.Handle("/mcp/", mcpServer.Handler())

	server := &http.Server{
		Addr:    addr,
		Handler: mux,
	}

	go func() {
		<-ctx.Done()
		server.Shutdown(context.Background())
	}()

	if err := server.ListenAndServe(); err != nil && err != http.ErrServerClosed {
		fmt.Fprintf(os.Stderr, "Server error: %v\n", err)
		os.Exit(1)
	}
}

// skillsCmd creates the skills management command
func skillsCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "skills",
		Short: "Manage skill definitions",
		Long: `Skills are YAML definitions that modify agent behavior without code changes.
They can add context to prompts, require specific tools, and provide examples.

Skills are loaded from ~/.gobot/skills/ or the extensions/skills/ directory.`,
	}

	cmd.AddCommand(&cobra.Command{
		Use:   "list",
		Short: "List all loaded skills",
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadConfig()
			listSkills(cfg)
		},
	})

	cmd.AddCommand(&cobra.Command{
		Use:   "show [name]",
		Short: "Show details of a skill",
		Args:  cobra.ExactArgs(1),
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadConfig()
			showSkill(cfg, args[0])
		},
	})

	cmd.AddCommand(&cobra.Command{
		Use:   "test [name] [input]",
		Short: "Test if a skill matches input",
		Args:  cobra.MinimumNArgs(2),
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadConfig()
			testSkill(cfg, args[0], strings.Join(args[1:], " "))
		},
	})

	return cmd
}

// pluginsCmd creates the plugins management command
func pluginsCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "plugins",
		Short: "Manage external plugins",
		Long: `Plugins are external binaries that extend the agent with new tools and channels.
Plugins are loaded from ~/.gobot/plugins/ or the extensions/plugins/ directory.`,
	}

	cmd.AddCommand(&cobra.Command{
		Use:   "list",
		Short: "List all loaded plugins",
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadConfig()
			listPlugins(cfg)
		},
	})

	return cmd
}

// listSkills lists all loaded skills
func listSkills(cfg *config.Config) {
	loader := createSkillLoader(cfg)
	if err := loader.LoadAll(); err != nil {
		fmt.Fprintf(os.Stderr, "Error loading skills: %v\n", err)
		os.Exit(1)
	}

	skillList := loader.List()
	if len(skillList) == 0 {
		fmt.Println("No skills loaded.")
		fmt.Printf("\nSkills directory: %s\n", skillsDir(cfg))
		fmt.Println("Create YAML files in this directory to define skills.")
		return
	}

	fmt.Println("Loaded skills:")
	for _, s := range skillList {
		status := "\033[32m✓\033[0m"
		if !s.Enabled {
			status = "\033[31m✗\033[0m"
		}
		fmt.Printf("  %s %s (priority: %d)\n", status, s.Name, s.Priority)
		fmt.Printf("      %s\n", s.Description)
		if len(s.Triggers) > 0 {
			fmt.Printf("      Triggers: %s\n", strings.Join(s.Triggers, ", "))
		}
	}
}

// showSkill shows details of a specific skill
func showSkill(cfg *config.Config, name string) {
	loader := createSkillLoader(cfg)
	if err := loader.LoadAll(); err != nil {
		fmt.Fprintf(os.Stderr, "Error loading skills: %v\n", err)
		os.Exit(1)
	}

	skill, ok := loader.Get(name)
	if !ok {
		fmt.Fprintf(os.Stderr, "Skill not found: %s\n", name)
		os.Exit(1)
	}

	fmt.Printf("Skill: %s\n", skill.Name)
	fmt.Printf("Version: %s\n", skill.Version)
	fmt.Printf("Description: %s\n", skill.Description)
	fmt.Printf("Priority: %d\n", skill.Priority)
	fmt.Printf("Enabled: %v\n", skill.Enabled)
	fmt.Printf("File: %s\n", skill.FilePath)
	fmt.Println()

	if len(skill.Triggers) > 0 {
		fmt.Println("Triggers:")
		for _, t := range skill.Triggers {
			fmt.Printf("  - %s\n", t)
		}
		fmt.Println()
	}

	if len(skill.Tools) > 0 {
		fmt.Println("Required tools:")
		for _, t := range skill.Tools {
			fmt.Printf("  - %s\n", t)
		}
		fmt.Println()
	}

	if skill.Template != "" {
		fmt.Println("Template:")
		fmt.Println(skill.Template)
	}

	if len(skill.Examples) > 0 {
		fmt.Println("\nExamples:")
		for i, ex := range skill.Examples {
			fmt.Printf("  Example %d:\n", i+1)
			fmt.Printf("    User: %s\n", truncateString(ex.User, 60))
			fmt.Printf("    Assistant: %s\n", truncateString(ex.Assistant, 60))
		}
	}
}

// testSkill tests if a skill matches the given input
func testSkill(cfg *config.Config, name, input string) {
	loader := createSkillLoader(cfg)
	if err := loader.LoadAll(); err != nil {
		fmt.Fprintf(os.Stderr, "Error loading skills: %v\n", err)
		os.Exit(1)
	}

	skill, ok := loader.Get(name)
	if !ok {
		fmt.Fprintf(os.Stderr, "Skill not found: %s\n", name)
		os.Exit(1)
	}

	if skill.Matches(input) {
		fmt.Printf("\033[32m✓ Skill '%s' matches input\033[0m\n", name)
		fmt.Println("\nPrompt would be modified with:")
		modified := skill.ApplyToPrompt("")
		fmt.Println(modified)
	} else {
		fmt.Printf("\033[31m✗ Skill '%s' does not match input\033[0m\n", name)
		fmt.Printf("\nTriggers: %s\n", strings.Join(skill.Triggers, ", "))
	}
}

// daemonCmd creates the daemon command for background service mode
func daemonCmd() *cobra.Command {
	var pidFile string
	var enableMCP bool
	var mcpPort int
	var enableAgent bool
	var orgID string
	var token string
	var dangerously bool

	cmd := &cobra.Command{
		Use:   "daemon",
		Short: "Run GoBot as a background daemon service",
		Long: `Run GoBot as a background daemon service that provides:
- MCP server for tool access via Model Context Protocol
- Agent connection to SaaS for remote task execution

The daemon can be configured to run one or both services.

Examples:
  gobot daemon                           # Start with MCP server only
  gobot daemon --mcp-port 9000           # MCP server on custom port
  gobot daemon --agent --org acme        # Start with agent connection
  gobot daemon --mcp --agent --org acme  # Start both services
  gobot daemon --pid /var/run/gobot.pid  # Use custom PID file`,
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadConfig()
			runDaemon(cfg, pidFile, enableMCP, mcpPort, enableAgent, orgID, token, dangerously)
		},
	}

	cmd.Flags().StringVar(&pidFile, "pid", "", "PID file path (default: ~/.gobot/gobot.pid)")
	cmd.Flags().BoolVar(&enableMCP, "mcp", true, "enable MCP server")
	cmd.Flags().IntVar(&mcpPort, "mcp-port", 8080, "MCP server port")
	cmd.Flags().BoolVar(&enableAgent, "agent", false, "enable agent connection to SaaS")
	cmd.Flags().StringVar(&orgID, "org", "", "organization ID for agent mode")
	cmd.Flags().StringVar(&token, "token", "", "authentication token for agent mode")
	cmd.Flags().BoolVar(&dangerously, "dangerously", false, "100% autonomous mode - bypass ALL tool approval prompts")

	return cmd
}

// runDaemon runs GoBot as a background daemon
func runDaemon(cfg *config.Config, pidFile string, enableMCP bool, mcpPort int, enableAgent bool, orgID, token string, dangerously bool) {
	// Handle dangerous mode confirmation
	if dangerously {
		if !confirmDangerousMode() {
			fmt.Println("Aborted.")
			os.Exit(0)
		}
	}

	// Validate agent configuration if enabled
	if enableAgent {
		if orgID == "" {
			fmt.Fprintln(os.Stderr, "Error: --org is required when --agent is enabled")
			os.Exit(1)
		}
		if token == "" {
			token = cfg.Token
		}
		if token == "" {
			fmt.Fprintln(os.Stderr, "Error: token required for agent mode. Use --token or set token in config.")
			os.Exit(1)
		}
	}

	// Set default PID file
	if pidFile == "" {
		pidFile = filepath.Join(cfg.DataDir, "gobot.pid")
	}

	// Write PID file
	pid := os.Getpid()
	if err := os.MkdirAll(filepath.Dir(pidFile), 0755); err != nil {
		fmt.Fprintf(os.Stderr, "Error creating PID directory: %v\n", err)
		os.Exit(1)
	}
	if err := os.WriteFile(pidFile, []byte(fmt.Sprintf("%d", pid)), 0644); err != nil {
		fmt.Fprintf(os.Stderr, "Error writing PID file: %v\n", err)
		os.Exit(1)
	}
	defer os.Remove(pidFile)

	fmt.Println("GoBot Daemon Starting")
	fmt.Println("=====================")
	fmt.Printf("PID: %d (file: %s)\n", pid, pidFile)
	fmt.Println()

	// Create session manager
	sessions, err := session.New(cfg.DBPath())
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error opening database: %v\n", err)
		os.Exit(1)
	}
	defer sessions.Close()

	// Create providers
	providers := createProviders(cfg)

	// Create tool registry with policy
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

	// Register task tools with orchestrator for sub-agent support
	taskTool := tools.NewTaskTool()
	taskTool.CreateOrchestrator(cfg, sessions, providers, registry)
	registry.Register(taskTool)

	agentStatusTool := tools.NewAgentStatusTool()
	agentStatusTool.SetOrchestrator(taskTool.GetOrchestrator())
	registry.Register(agentStatusTool)

	// Handle signals
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)

	// Track running services
	var wg sync.WaitGroup
	errCh := make(chan error, 2)

	// Start MCP server if enabled
	if enableMCP {
		wg.Add(1)
		go func() {
			defer wg.Done()
			fmt.Printf("Starting MCP server on port %d...\n", mcpPort)
			if err := runMCPServerDaemon(ctx, registry, mcpPort); err != nil {
				errCh <- fmt.Errorf("MCP server error: %w", err)
			}
		}()
	}

	// Start agent if enabled
	if enableAgent {
		wg.Add(1)
		go func() {
			defer wg.Done()
			fmt.Printf("Connecting agent to org %s...\n", orgID)
			if err := runAgentDaemon(ctx, cfg, sessions, providers, registry, orgID, token); err != nil {
				errCh <- fmt.Errorf("agent error: %w", err)
			}
		}()
	}

	if !enableMCP && !enableAgent {
		fmt.Println("Warning: No services enabled. Use --mcp or --agent to start services.")
	}

	fmt.Println()
	fmt.Println("Daemon running. Press Ctrl+C to stop.")
	fmt.Println()

	// Wait for shutdown signal or error
	select {
	case sig := <-sigCh:
		fmt.Printf("\nReceived signal %v, shutting down...\n", sig)
		cancel()
	case err := <-errCh:
		fmt.Fprintf(os.Stderr, "%v\n", err)
		cancel()
	}

	// Wait for services to stop
	wg.Wait()
	fmt.Println("Daemon stopped.")
}

// runMCPServerDaemon runs the MCP server in daemon mode
func runMCPServerDaemon(ctx context.Context, registry *tools.Registry, port int) error {
	mcpServer := agentmcp.NewServer(registry)

	addr := fmt.Sprintf("localhost:%d", port)
	fmt.Printf("MCP server listening at http://%s/mcp\n", addr)

	mux := http.NewServeMux()
	mux.Handle("/mcp", mcpServer.Handler())
	mux.Handle("/mcp/", mcpServer.Handler())

	// Health check endpoint
	mux.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		w.Write([]byte(`{"status":"ok"}`))
	})

	server := &http.Server{
		Addr:    addr,
		Handler: mux,
	}

	go func() {
		<-ctx.Done()
		server.Shutdown(context.Background())
	}()

	if err := server.ListenAndServe(); err != nil && err != http.ErrServerClosed {
		return err
	}
	return nil
}

// runAgentDaemon runs the agent in daemon mode
func runAgentDaemon(ctx context.Context, cfg *config.Config, sessions *session.Manager, providers []ai.Provider, registry *tools.Registry, orgID, token string) error {
	serverURL := cfg.ServerURL
	if serverURL == "" {
		return fmt.Errorf("server URL not configured")
	}

	hostname, _ := os.Hostname()
	agentID := fmt.Sprintf("daemon-%s-%d", hostname, os.Getpid())

	// Create WebSocket connection URL
	wsURL := strings.Replace(serverURL, "http://", "ws://", 1)
	wsURL = strings.Replace(wsURL, "https://", "wss://", 1)
	wsURL = fmt.Sprintf("%s/api/v1/agent/ws?org_id=%s&agent_id=%s&token=%s", wsURL, orgID, agentID, token)

	fmt.Printf("Agent connecting to: %s\n", serverURL)
	fmt.Printf("Organization: %s\n", orgID)
	fmt.Printf("Agent ID: %s\n", agentID)

	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		return fmt.Errorf("failed to connect: %w", err)
	}
	defer conn.Close()

	fmt.Println("\033[32m✓ Agent connected to SaaS\033[0m")

	// Create runner
	r := runner.New(cfg, sessions, providers, registry)

	// Read messages from WebSocket
	for {
		select {
		case <-ctx.Done():
			return nil
		default:
			_, message, err := conn.ReadMessage()
			if err != nil {
				if ctx.Err() != nil {
					return nil
				}
				return fmt.Errorf("connection closed: %w", err)
			}

			handleAgentMessage(ctx, conn, r, message)
		}
	}
}

// listPlugins lists all loaded plugins
func listPlugins(cfg *config.Config) {
	loader := createPluginLoader(cfg)
	if err := loader.LoadAll(); err != nil {
		fmt.Fprintf(os.Stderr, "Error loading plugins: %v\n", err)
		// Don't exit - continue showing what we have
	}
	defer loader.Stop()

	tools := loader.ListTools()
	channels := loader.ListChannels()

	if len(tools) == 0 && len(channels) == 0 {
		fmt.Println("No plugins loaded.")
		fmt.Printf("\nPlugins directory: %s\n", pluginsDir(cfg))
		fmt.Println("Place compiled plugin binaries in tools/ or channels/ subdirectories.")
		return
	}

	if len(tools) > 0 {
		fmt.Println("Tool plugins:")
		for _, name := range tools {
			tool, _ := loader.GetTool(name)
			fmt.Printf("  - %s: %s\n", tool.Name(), tool.Description())
		}
	}

	if len(channels) > 0 {
		fmt.Println("Channel plugins:")
		for _, id := range channels {
			fmt.Printf("  - %s\n", id)
		}
	}
}

// Helper functions

func skillsDir(cfg *config.Config) string {
	// Try user directory first
	userDir := filepath.Join(cfg.DataDir, "skills")
	if _, err := os.Stat(userDir); err == nil {
		return userDir
	}

	// Fall back to extensions directory
	return "extensions/skills"
}

func pluginsDir(cfg *config.Config) string {
	// Try user directory first
	userDir := filepath.Join(cfg.DataDir, "plugins")
	if _, err := os.Stat(userDir); err == nil {
		return userDir
	}

	// Fall back to extensions directory
	return "extensions/plugins"
}

func createSkillLoader(cfg *config.Config) *skills.Loader {
	return skills.NewLoader(skillsDir(cfg))
}

func createPluginLoader(cfg *config.Config) *plugins.Loader {
	return plugins.NewLoader(pluginsDir(cfg))
}

func truncateString(s string, max int) string {
	s = strings.ReplaceAll(s, "\n", " ")
	if len(s) <= max {
		return s
	}
	return s[:max] + "..."
}
