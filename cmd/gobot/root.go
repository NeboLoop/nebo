package cli

import (
	"context"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"os/signal"
	"runtime"
	"sync"
	"syscall"
	"time"

	"github.com/spf13/cobra"
	"github.com/zeromicro/go-zero/core/logx"

	"gobot/app"
	agentcfg "gobot/agent/config"
	agentmcp "gobot/agent/mcp"
	"gobot/agent/tools"
	"gobot/internal/agenthub"
	"gobot/internal/channels"
	"gobot/internal/daemon"
	"gobot/internal/db"
	"gobot/internal/db/migrations"
	"gobot/internal/defaults"
	"gobot/internal/lifecycle"
	"gobot/internal/server"
)

// RunAll starts both server and agent together (default mode)
func RunAll() {
	// Suppress go-zero verbose logging
	logx.Disable()

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
		fmt.Println("\033[33mGoBot is already running. Only one instance allowed per computer.\033[0m")
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

	// Initialize shared database ONCE for all components
	database, err := db.NewSQLite(c.Database.SQLitePath)
	if err != nil {
		fmt.Printf("\033[31mError: Failed to initialize database: %v\033[0m\n", err)
		os.Exit(1)
	}
	defer database.Close()

	// Create shared components (single binary = shared state)
	channelMgr := channels.NewManager()
	agentHub := agenthub.NewHub()

	var wg sync.WaitGroup
	errCh := make(chan error, 4)

	// Start server in goroutine (uses shared database)
	wg.Add(1)
	go func() {
		defer func() {
			fmt.Println("[Server] Goroutine exiting")
			wg.Done()
		}()
		opts := server.ServerOptions{
			ChannelManager: channelMgr,
			AgentHub:       agentHub,
			Database:       database,
			Quiet:          true, // Suppress server startup messages
		}
		if err := server.RunWithOptions(ctx, *c, opts); err != nil {
			fmt.Printf("[Server] Error: %v\n", err)
			errCh <- fmt.Errorf("server error: %w", err)
		}
	}()

	// Wait for server to be ready
	serverURL := fmt.Sprintf("http://localhost:%d", c.Port)
	if !waitForServer(serverURL, 10*time.Second) {
		fmt.Println("\033[31mError: Server failed to start\033[0m")
		cancel()
		wg.Wait()
		os.Exit(1)
	}

	// Load agent config
	agentCfg := loadAgentConfig()

	// Start MCP server in goroutine
	mcpPort := 27896
	mcpURL := fmt.Sprintf("http://localhost:%d/mcp", mcpPort)
	wg.Add(1)
	go func() {
		defer func() {
			fmt.Println("[MCP] Goroutine exiting")
			wg.Done()
		}()
		registry := createMCPRegistry(agentCfg)
		if err := runMCPServerDaemon(ctx, registry, mcpPort, true); err != nil {
			fmt.Printf("[MCP] Error: %v\n", err)
			if ctx.Err() == nil {
				errCh <- fmt.Errorf("MCP server error: %w", err)
			}
		}
	}()

	// Start agent in goroutine (uses shared database)
	wg.Add(1)
	go func() {
		defer func() {
			fmt.Println("[AgentLoop] Goroutine exiting")
			wg.Done()
		}()
		agentOpts := AgentOptions{
			ChannelManager: channelMgr,
			Database:       database.GetDB(),
			Quiet:          true,
			Dangerously:    dangerouslyAll,
		}
		if err := runAgentLoopWithOptions(ctx, agentCfg, serverURL, agentOpts); err != nil {
			fmt.Printf("[AgentLoop] Error: %v\n", err)
			if ctx.Err() == nil {
				errCh <- fmt.Errorf("agent error: %w", err)
			}
		}
	}()

	// Heartbeat daemon - started when agent connects via lifecycle hook
	var heartbeat *daemon.Heartbeat
	var heartbeatOnce sync.Once
	agentReady := make(chan struct{})

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
				Interval: 30 * time.Minute,
				OnHeartbeat: func(hbCtx context.Context, tasks string) error {
					agent := agentHub.GetAnyAgent()
					if agent == nil {
						return nil
					}

					prompt := daemon.FormatHeartbeatPrompt(tasks)
					// Use unique session key each time to avoid accumulating history
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
					return agentHub.SendToAgent(agent.ID, frame)
				},
			})
			heartbeat.Start(ctx)
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
	printStartupBanner(serverURL, mcpURL, dataDir)

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
	fmt.Println("\n\033[32mGoBot stopped.\033[0m")
}

// printStartupBanner prints a clean, clickable startup message
func printStartupBanner(serverURL, mcpURL, dataDir string) {
	fmt.Println()
	fmt.Println("\033[1;32m  ╭─────────────────────────────────────────╮\033[0m")
	fmt.Println("\033[1;32m  │           \033[1;37m🤖 GoBot is running\033[1;32m           │\033[0m")
	fmt.Println("\033[1;32m  ╰─────────────────────────────────────────╯\033[0m")
	fmt.Println()
	fmt.Printf("  \033[1;36m→\033[0m Web UI:     \033[4;34m%s\033[0m\n", serverURL)
	fmt.Printf("  \033[1;36m→\033[0m MCP Server: \033[4;34m%s\033[0m\n", mcpURL)
	fmt.Println()
	fmt.Printf("  \033[2mData: %s\033[0m\n", dataDir)
	fmt.Println()
	fmt.Println("  \033[2mPress Ctrl+C to stop\033[0m")
	fmt.Println()
}

// openBrowser opens the default browser to the specified URL
// Skips opening if browser was recently opened (within last 8 hours)
func openBrowser(url string, dataDir string) {
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
		Long:  `Start the GoBot web server without the AI agent.`,
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

// createMCPRegistry creates a tool registry for MCP server
func createMCPRegistry(cfg *agentcfg.Config) *tools.Registry {
	policy := tools.NewPolicyFromConfig(
		cfg.Policy.Level,
		cfg.Policy.AskMode,
		cfg.Policy.Allowlist,
	)
	registry := tools.NewRegistry(policy)
	registry.RegisterDefaults()
	return registry
}

// runMCPServerDaemon runs the MCP server in daemon mode
func runMCPServerDaemon(ctx context.Context, registry *tools.Registry, port int, quiet bool) error {
	mcpServer := agentmcp.NewServer(registry)

	addr := fmt.Sprintf("localhost:%d", port)
	if !quiet {
		fmt.Printf("MCP server listening at http://%s/mcp\n", addr)
	}

	mux := http.NewServeMux()
	mux.Handle("/mcp", mcpServer.Handler())
	mux.Handle("/mcp/", mcpServer.Handler())

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

// acquireLock creates a lock file to ensure only one gobot instance runs
func acquireLock(dataDir string) (*os.File, error) {
	lockPath := dataDir + "/gobot.lock"

	// Try to create/open the lock file
	file, err := os.OpenFile(lockPath, os.O_CREATE|os.O_RDWR, 0600)
	if err != nil {
		return nil, fmt.Errorf("cannot open lock file: %w", err)
	}

	// Try to get exclusive lock (non-blocking)
	err = syscall.Flock(int(file.Fd()), syscall.LOCK_EX|syscall.LOCK_NB)
	if err != nil {
		file.Close()
		return nil, fmt.Errorf("cannot acquire lock")
	}

	// Write our PID to the lock file
	file.Truncate(0)
	file.Seek(0, 0)
	fmt.Fprintf(file, "%d\n", os.Getpid())
	file.Sync()

	return file, nil
}

// releaseLock releases the lock file
func releaseLock(file *os.File) {
	if file != nil {
		syscall.Flock(int(file.Fd()), syscall.LOCK_UN)
		file.Close()
	}
}
