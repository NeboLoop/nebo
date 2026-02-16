package cli

import (
	"context"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/spf13/cobra"
	"github.com/neboloop/nebo/internal/logging"

	"github.com/neboloop/nebo/app"
	"github.com/neboloop/nebo/internal/agenthub"
	"github.com/neboloop/nebo/internal/daemon"
	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/db/migrations"
	"github.com/neboloop/nebo/internal/defaults"
	"github.com/neboloop/nebo/internal/lifecycle"
	"github.com/neboloop/nebo/internal/local"
	"github.com/neboloop/nebo/internal/server"
	"github.com/neboloop/nebo/internal/svc"
)

// ensureUserPath augments PATH with common CLI tool locations.
// macOS GUI apps (launched from Finder/Dock) inherit a minimal PATH that
// excludes Homebrew, user-local, and nvm/cargo/go paths. Without this,
// exec.LookPath cannot find tools like "claude", "codex", or "gemini".
func ensureUserPath() {
	current := os.Getenv("PATH")
	home, _ := os.UserHomeDir()

	// Directories where CLI tools are commonly installed
	extra := []string{
		"/opt/homebrew/bin",      // Homebrew (Apple Silicon)
		"/usr/local/bin",         // Homebrew (Intel) / manual installs
		filepath.Join(home, ".local/bin"),      // pipx, user installs
		filepath.Join(home, "go/bin"),          // Go binaries
		filepath.Join(home, ".cargo/bin"),      // Rust/cargo
		filepath.Join(home, ".nvm/current/bin"), // nvm-managed node
	}

	var added []string
	for _, dir := range extra {
		if dir == "" {
			continue
		}
		// Skip if already in PATH
		if strings.Contains(current, dir) {
			continue
		}
		// Only add if the directory actually exists
		if info, err := os.Stat(dir); err == nil && info.IsDir() {
			added = append(added, dir)
		}
	}

	if len(added) > 0 {
		os.Setenv("PATH", current+":"+strings.Join(added, ":"))
	}
}

// RunAll starts both server and agent together (default mode)
func RunAll() {
	// Augment PATH so CLI tools (claude, codex, gemini) are discoverable
	// even when launched as a macOS .app from Finder/Dock
	ensureUserPath()

	// Suppress verbose logging
	logging.Disable()

	// Enable quiet mode for clean CLI output
	migrations.QuietMode = true
	app.QuietMode = true

	// Ensure data directory exists with default files
	dataDir, err := defaults.EnsureDataDir()
	if err != nil {
		fmt.Printf("\033[31mError: Failed to initialize data directory: %v\033[0m\n", err)
		os.Exit(1)
	}

	// Enforce single instance with lock file
	lockFile, err := acquireLock(dataDir)
	if err != nil {
		fmt.Printf("\033[31mError: %v\033[0m\n", err)
		fmt.Println("\033[33mNebo is already running. Only one instance allowed per computer.\033[0m")
		os.Exit(1)
	}
	defer releaseLock(lockFile)

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// Handle Ctrl+C
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
	go func() {
		sig := <-sigCh
		fmt.Printf("\n\033[33mReceived signal: %v - Shutting down...\033[0m\n", sig)
		cancel()
	}()

	c := ServerConfig

	// Initialize shared ServiceContext ONCE — single owner of the database connection
	svcCtx := svc.NewServiceContext(*c)
	svcCtx.Version = AppVersion
	defer svcCtx.Close()

	if svcCtx.DB == nil {
		fmt.Printf("\033[31mError: Failed to initialize database\033[0m\n")
		os.Exit(1)
	}

	// Create shared components (single binary = shared state)
	agentMCPProxy := server.NewAgentMCPProxy() // Set by agent after MCP server init

	var wg sync.WaitGroup
	errCh := make(chan error, 4)

	// Start server in goroutine (uses shared ServiceContext)
	wg.Add(1)
	go func() {
		defer func() {
			fmt.Println("[Server] Goroutine exiting")
			wg.Done()
		}()
		opts := server.ServerOptions{
			SvcCtx:          svcCtx,
			Quiet:           true, // Suppress server startup messages
			AgentMCPHandler: agentMCPProxy,
		}
		if err := server.Run(ctx, *c, opts); err != nil {
			fmt.Printf("[Server] Error: %v\n", err)
			errCh <- fmt.Errorf("server error: %w", err)
		}
	}()

	// Wait for server to be ready
	// Use BaseURL from config, or construct from domain and port
	serverURL := c.App.BaseURL
	if serverURL == "" {
		serverURL = fmt.Sprintf("http://%s:%d", c.App.Domain, c.Port)
	}
	// For health check, use localhost since DNS resolution may not be ready
	healthURL := fmt.Sprintf("http://localhost:%d", c.Port)
	if !waitForServer(healthURL, 10*time.Second) {
		fmt.Println("\033[31mError: Server failed to start\033[0m")
		cancel()
		wg.Wait()
		os.Exit(1)
	}

	// Load agent config
	agentCfg := loadAgentConfig()
	SetSharedDB(svcCtx.DB.GetDB())

	// Heartbeat daemon - started when agent connects via lifecycle hook
	var heartbeat *daemon.Heartbeat
	var heartbeatOnce sync.Once
	agentReady := make(chan struct{})

	// Start agent in goroutine (uses shared database from ServiceContext)
	wg.Add(1)
	go func() {
		defer func() {
			fmt.Println("[AgentLoop] Goroutine exiting")
			wg.Done()
		}()
		agentOpts := AgentOptions{
			Database:         svcCtx.DB.GetDB(),
			PluginStore:      svcCtx.PluginStore,
			SvcCtx:           svcCtx,
			Quiet:            true,
			Dangerously:      dangerouslyAll,
			AgentMCPProxy:    agentMCPProxy,
			Heartbeat:        &heartbeat,
		}
		if err := runAgent(ctx, agentCfg, serverURL, agentOpts); err != nil {
			fmt.Printf("[AgentLoop] Error: %v\n", err)
			if ctx.Err() == nil {
				errCh <- fmt.Errorf("agent error: %w", err)
			}
		}
	}()

	lifecycle.OnAgentConnected(func(agentID string) {
		// Signal that agent is ready
		select {
		case <-agentReady:
		default:
			close(agentReady)
		}

		// Start heartbeat daemon only once, when first agent connects
		heartbeatOnce.Do(func() {
			heartbeat = daemon.NewHeartbeat(daemon.HeartbeatConfig{
				Interval: heartbeatInterval(),
				OnHeartbeat: func(hbCtx context.Context, prompt string) error {
					agent := svcCtx.AgentHub.GetAnyAgent()
					if agent == nil {
						fmt.Println("[heartbeat] No agent connected, skipping")
						return nil
					}

					sessionKey := fmt.Sprintf("heartbeat-%d", time.Now().UnixNano())
					frame := &agenthub.Frame{
						Type:   "req",
						ID:     sessionKey,
						Method: "run",
						Params: map[string]any{
							"prompt":      prompt,
							"session_key": sessionKey,
						},
					}
					return svcCtx.AgentHub.SendToAgent(agent.ID, frame)
				},
				IsQuietHours: func() bool {
					if rawDB := svcCtx.DB.GetDB(); rawDB != nil {
						q := db.New(rawDB)
						profile, err := q.GetAgentProfile(context.Background())
						if err == nil {
							return daemon.IsInQuietHours(profile.QuietHoursStart, profile.QuietHoursEnd, time.Now())
						}
					}
					return false
				},
			})
			heartbeat.Start(ctx)

			// Update interval at runtime when user changes it in Settings
			local.GetAgentSettings().OnChange(func(s local.AgentSettings) {
				if s.HeartbeatIntervalMinutes > 0 {
					heartbeat.SetInterval(time.Duration(s.HeartbeatIntervalMinutes) * time.Minute)
				}
			})
		})
	})

	lifecycle.OnAgentDisconnected(func(agentID string) {
		// Silent - don't spam console
	})

	// Cleanup heartbeat on shutdown
	defer func() {
		if heartbeat != nil {
			heartbeat.Stop()
		}
	}()

	// Wait for agent to connect (with timeout)
	select {
	case <-agentReady:
	case <-time.After(5 * time.Second):
		// Continue anyway, agent might connect later
	}

	// Print clean startup banner
	printStartupBanner(serverURL, dataDir)

	// Auto-open browser (only if not recently opened)
	openBrowser(serverURL, dataDir)

	// Wait for shutdown or error
	select {
	case <-ctx.Done():
		fmt.Printf("\n[Shutdown] Context cancelled, reason: %v\n", ctx.Err())
	case err := <-errCh:
		fmt.Fprintf(os.Stderr, "\n\033[31mError: %v\033[0m\n", err)
		cancel()
	}

	fmt.Println("[Shutdown] Waiting for goroutines to finish...")
	wg.Wait()
	fmt.Println("\n\033[32mNebo stopped.\033[0m")
}

// printStartupBanner prints a clean, clickable startup message
func printStartupBanner(serverURL, dataDir string) {
	fmt.Println()
	fmt.Println("\033[1;32m  ╭─────────────────────────────────────────╮\033[0m")
	fmt.Println("\033[1;32m  │           \033[1;37m🤖 Nebo is running\033[1;32m            │\033[0m")
	fmt.Println("\033[1;32m  ╰─────────────────────────────────────────╯\033[0m")
	fmt.Println()
	fmt.Printf("  \033[1;36m→\033[0m Web UI: \033[4;34m%s\033[0m\n", serverURL)
	fmt.Println()
	fmt.Printf("  \033[2mData: %s\033[0m\n", dataDir)
	fmt.Println()
	fmt.Println("  \033[2mPress Ctrl+C to stop\033[0m")
	fmt.Println()
}

// openBrowser opens the default browser to the specified URL
// Skips opening if browser was recently opened (within last 8 hours)
func openBrowser(url string, dataDir string) {
	// Skip if running in development mode (air hot reload)
	if os.Getenv("NEBO_NO_BROWSER") == "1" || os.Getenv("AIR_TMP_DIR") != "" {
		return
	}

	// Check if browser was recently opened
	browserFile := dataDir + "/browser_opened"
	if info, err := os.Stat(browserFile); err == nil {
		// File exists - check if it's recent (within 8 hours)
		if time.Since(info.ModTime()) < 8*time.Hour {
			// Browser was opened recently, skip
			return
		}
	}

	var cmd *exec.Cmd
	switch runtime.GOOS {
	case "darwin":
		cmd = exec.Command("open", url)
	case "linux":
		cmd = exec.Command("xdg-open", url)
	case "windows":
		cmd = exec.Command("rundll32", "url.dll,FileProtocolHandler", url)
	default:
		return
	}

	if err := cmd.Start(); err == nil {
		// Mark browser as opened
		os.WriteFile(browserFile, []byte(time.Now().Format(time.RFC3339)), 0600)
	}
}

// waitForServer polls the server until it's ready or timeout
func waitForServer(url string, timeout time.Duration) bool {
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		resp, err := http.Get(url + "/api/v1/csrf-token")
		if err == nil {
			resp.Body.Close()
			if resp.StatusCode == http.StatusOK {
				return true
			}
		}
		time.Sleep(100 * time.Millisecond)
	}
	return false
}


// serveCmd creates the serve command (server only)
func ServeCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "serve",
		Short: "Start the web server only",
		Long:  `Start the Nebo web server without the AI agent.`,
		Run: func(cmd *cobra.Command, args []string) {
			runServe()
		},
	}
}

// runServe starts just the server
func runServe() {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
	go func() {
		<-sigCh
		fmt.Println("\nShutting down...")
		cancel()
	}()

	if err := server.Run(ctx, *ServerConfig); err != nil {
		fmt.Fprintf(os.Stderr, "Server error: %v\n", err)
		os.Exit(1)
	}
}

// heartbeatInterval reads the configured interval from AgentSettingsStore,
// falling back to 30 minutes if unset or zero.
func heartbeatInterval() time.Duration {
	if s := local.GetAgentSettings(); s != nil {
		if mins := s.Get().HeartbeatIntervalMinutes; mins > 0 {
			return time.Duration(mins) * time.Minute
		}
	}
	return 30 * time.Minute
}


