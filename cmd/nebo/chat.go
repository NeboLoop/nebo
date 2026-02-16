package cli

import (
	"bufio"
	"context"
	"fmt"
	"os"
	"os/signal"
	"strings"
	"syscall"

	"github.com/spf13/cobra"

	"github.com/neboloop/nebo/internal/agent/ai"
	agentcfg "github.com/neboloop/nebo/internal/agent/config"
	"github.com/neboloop/nebo/internal/agent/runner"
	"github.com/neboloop/nebo/internal/agent/session"
	"github.com/neboloop/nebo/internal/agent/tools"
	"github.com/neboloop/nebo/internal/agent/voice"
	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/provider"
)

// chatCmd creates the chat command
func ChatCmd() *cobra.Command {
	var interactive bool
	var dangerously bool
	var voiceMode bool

	cmd := &cobra.Command{
		Use:   "chat [prompt]",
		Short: "Chat with the AI assistant",
		Long: `Send a message to the AI assistant and receive a streaming response.
The assistant has access to tools for file operations, shell commands, and more.

Examples:
  nebo chat "Hello, what can you do?"
  nebo chat "List all Go files in this directory"
  nebo chat --interactive
  nebo chat --voice              # Record voice input
  nebo chat --dangerously "deploy to production"`,
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadAgentConfig()
			runChat(cfg, args, interactive, dangerously, voiceMode)
		},
	}

	cmd.Flags().BoolVarP(&interactive, "interactive", "i", false, "start interactive chat session")
	cmd.Flags().BoolVar(&dangerously, "dangerously", false, "100% autonomous mode - bypass ALL tool approval prompts (use with caution!)")
	cmd.Flags().BoolVar(&voiceMode, "voice", false, "use voice input (requires microphone and OPENAI_API_KEY)")

	return cmd
}

// runChat runs the chat command
func runChat(cfg *agentcfg.Config, args []string, interactive bool, dangerously bool, voiceMode bool) {
	if dangerously {
		if !confirmDangerousMode() {
			fmt.Println("Aborted.")
			os.Exit(0)
		}
	}

	if voiceMode {
		voiceRecorder := voice.NewRecorder()
		text, err := voiceRecorder.Record()
		if err != nil {
			fmt.Fprintf(os.Stderr, "Voice error: %v\n", err)
			os.Exit(1)
		}
		if text == "" {
			fmt.Println("No speech detected")
			os.Exit(0)
		}
		fmt.Printf("\n Transcribed: %s\n\n", text)
		args = []string{text}
	}

	// Open database using shared connection pattern
	store, err := db.NewSQLite(cfg.DBPath())
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error opening database: %v\n", err)
		os.Exit(1)
	}
	defer store.Close()

	// Use the shared DB for all components
	SetSharedDB(store.GetDB())

	sessions, err := session.New(store.GetDB())
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error initializing sessions: %v\n", err)
		os.Exit(1)
	}

	providers := createProviders(cfg)
	if len(providers) == 0 {
		fmt.Fprintln(os.Stderr, "No providers configured. Set ANTHROPIC_API_KEY or configure providers in config.yaml (run 'nebo doctor' to find config location)")
		os.Exit(1)
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
	registry.RegisterDefaultsWithPermissions(loadToolPermissions(store.GetDB()))

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
		fmt.Printf("[chat] Warning: could not start config watcher: %v\n", err)
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
			fmt.Printf("[chat] Config reloaded: model selector, fuzzy matcher, and providers updated\n")
		}
	})

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
		Origin:     tools.OriginUser,
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
	fmt.Println("\033[1mNebo Interactive Mode\033[0m")
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

		if strings.HasPrefix(line, "/") {
			if handleCommand(line, sessions) {
				continue
			}
		}

		events, err := r.Run(ctx, &runner.RunRequest{
			SessionKey: sessionKey,
			Prompt:     line,
			Origin:     tools.OriginUser,
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
		sess, err := sessions.GetOrCreate(sessionKey, "")
		if err == nil {
			sessions.Reset(sess.ID)
			fmt.Println("Session cleared.")
		}
		return true

	case cmd == "/sessions":
		list, _ := sessions.ListSessions("")
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

// confirmDangerousMode displays warnings and requires explicit confirmation
func confirmDangerousMode() bool {
	fmt.Println()
	fmt.Println("\033[1;31m╔══════════════════════════════════════════════════════════════════╗")
	fmt.Println("║                     DANGEROUS MODE WARNING                      ║")
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
		fmt.Println("\033[1;31m DANGEROUS MODE ENABLED - All approvals bypassed!\033[0m")
		fmt.Println()
		return true
	}

	return false
}
