//go:build desktop

package cli

import (
	"context"
	_ "embed"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"sync/atomic"
	"time"

	"github.com/wailsapp/wails/v3/pkg/application"
	"github.com/wailsapp/wails/v3/pkg/events"

	"github.com/nebolabs/nebo/internal/agenthub"
	"github.com/nebolabs/nebo/internal/channels"
	"github.com/nebolabs/nebo/internal/daemon"
	"github.com/nebolabs/nebo/internal/db/migrations"
	"github.com/nebolabs/nebo/internal/defaults"
	"github.com/nebolabs/nebo/internal/lifecycle"
	"github.com/nebolabs/nebo/internal/logging"
	"github.com/nebolabs/nebo/internal/server"
	"github.com/nebolabs/nebo/internal/svc"

	neboapp "github.com/nebolabs/nebo/app"
)

// windowState persists the desktop window position and size between restarts.
// Uses absolute screen coordinates so it restores to the correct monitor.
type windowState struct {
	X      int `json:"x"`
	Y      int `json:"y"`
	Width  int `json:"width"`
	Height int `json:"height"`
}

// windowStatePath returns the path to the window state file.
func windowStatePath(dataDir string) string {
	return filepath.Join(dataDir, "window-state.json")
}

// loadWindowState reads saved window state from disk.
// Returns nil if the file doesn't exist or can't be read.
func loadWindowState(dataDir string) *windowState {
	data, err := os.ReadFile(windowStatePath(dataDir))
	if err != nil {
		return nil
	}
	var state windowState
	if err := json.Unmarshal(data, &state); err != nil {
		return nil
	}
	// Sanity check: reject nonsensical sizes
	if state.Width < 400 || state.Height < 300 {
		return nil
	}
	return &state
}


// saveWindowState persists the current window position and size to disk.
func saveWindowState(dataDir string, window *application.WebviewWindow) {
	x, y := window.Position()
	w, h := window.Size()
	state := windowState{X: x, Y: y, Width: w, Height: h}
	data, err := json.Marshal(state)
	if err != nil {
		return
	}
	_ = os.WriteFile(windowStatePath(dataDir), data, 0644)
}

//go:embed icons/appicon.png
var appIcon []byte

//go:embed icons/tray-icon.png
var trayIcon []byte

// RunDesktop starts Nebo in desktop mode with a native window and system tray.
func RunDesktop() {
	// Suppress verbose logging
	logging.Disable()

	// Enable quiet mode for clean output
	migrations.QuietMode = true
	neboapp.QuietMode = true

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

	c := ServerConfig

	// Initialize shared ServiceContext ONCE — single owner of the database connection
	svcCtx := svc.NewServiceContext(*c)
	defer svcCtx.Close()

	if svcCtx.DB == nil {
		fmt.Printf("\033[31mError: Failed to initialize database\033[0m\n")
		os.Exit(1)
	}

	// Create shared components
	channelMgr := channels.NewManager()

	// Compute server URL
	serverURL := c.App.BaseURL
	if serverURL == "" {
		serverURL = fmt.Sprintf("http://%s:%d", c.App.Domain, c.Port)
	}
	healthURL := fmt.Sprintf("http://localhost:%d", c.Port)

	// Create Wails application
	wailsApp := application.New(application.Options{
		Name: "Nebo",
		Icon: appIcon,
		Mac: application.MacOptions{
			// Don't terminate when last window closed — keep running in tray
			ApplicationShouldTerminateAfterLastWindowClosed: false,
		},
		Windows: application.WindowsOptions{
			DisableQuitOnLastWindowClosed: true,
		},
		Linux: application.LinuxOptions{
			DisableQuitOnLastWindowClosed: true,
			ProgramName:                  "nebo",
		},
		OnShutdown: func() {
			fmt.Println("\n\033[32mNebo stopped.\033[0m")
		},
	})

	// Restore saved window position/size or use defaults
	winWidth, winHeight := 1280, 860
	saved := loadWindowState(dataDir)
	if saved != nil {
		winWidth = saved.Width
		winHeight = saved.Height
	}

	// Create the main window with saved size (position restored after runtime init)
	window := wailsApp.Window.NewWithOptions(application.WebviewWindowOptions{
		Name:      "main",
		Title:     "Nebo",
		Width:     winWidth,
		Height:    winHeight,
		MinWidth:  800,
		MinHeight: 600,
		URL:       serverURL,
		Mac: application.MacWindow{
			Backdrop: application.MacBackdropTranslucent,
		},
		Windows: application.WindowsWindow{
			HiddenOnTaskbar: false,
		},
	})

	// Gate saves until after restore is complete so initial placement doesn't overwrite saved state
	var stateRestored atomic.Bool
	// Track when app is quitting so WindowClosing hook allows the close instead of hiding
	var quitting atomic.Bool

	// Restore position after a short delay to ensure the window is fully initialized.
	if saved != nil {
		go func() {
			time.Sleep(200 * time.Millisecond)
			window.SetPosition(saved.X, saved.Y)
			stateRestored.Store(true)
		}()
	} else {
		stateRestored.Store(true)
	}

	// Auto-save window state on move/resize (only after initial restore, skip during quit)
	window.RegisterHook(events.Common.WindowDidMove, func(event *application.WindowEvent) {
		if stateRestored.Load() && !quitting.Load() {
			saveWindowState(dataDir, window)
		}
	})
	window.RegisterHook(events.Common.WindowDidResize, func(event *application.WindowEvent) {
		if stateRestored.Load() && !quitting.Load() {
			saveWindowState(dataDir, window)
		}
	})

	// Hide window on close instead of destroying it (minimize to tray).
	// When quitting, let the close proceed so the app can exit.
	window.RegisterHook(events.Common.WindowClosing, func(event *application.WindowEvent) {
		if quitting.Load() {
			return
		}
		saveWindowState(dataDir, window)
		window.Hide()
		event.Cancel()
	})

	// Create system tray
	systray := wailsApp.SystemTray.New()
	systray.SetIcon(trayIcon)
	systray.SetLabel("Nebo")

	// Tray menu
	trayMenu := wailsApp.NewMenu()

	trayMenu.Add("Show").OnClick(func(ctx *application.Context) {
		window.Show()
		window.Focus()
	})
	trayMenu.Add("Hide").OnClick(func(ctx *application.Context) {
		saveWindowState(dataDir, window)
		window.Hide()
	})
	trayMenu.AddSeparator()

	statusItem := trayMenu.Add("Status: Starting...")
	statusItem.SetEnabled(false)

	trayMenu.AddSeparator()
	trayMenu.Add("Quit Nebo").OnClick(func(ctx *application.Context) {
		saveWindowState(dataDir, window)
		quitting.Store(true)
		wailsApp.Quit()
	})
	systray.SetMenu(trayMenu)

	// Use a context that cancels when Wails quits
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	var wg sync.WaitGroup
	errCh := make(chan error, 4)

	// Heartbeat daemon (declared here so defer can access it)
	var heartbeat *daemon.Heartbeat
	var heartbeatOnce sync.Once

	defer func() {
		if heartbeat != nil {
			heartbeat.Stop()
		}
	}()

	// Start all background services in a goroutine so wailsApp.Run() can
	// start the macOS event loop immediately on the main thread.
	// Add to WaitGroup BEFORE spawning goroutines to avoid race with wg.Wait().
	wg.Add(2) // server, agent
	go func() {
		// Start server (uses shared database)
		go func() {
			defer wg.Done()
			opts := server.ServerOptions{
				ChannelManager: channelMgr,
				SvcCtx:         svcCtx,
				Quiet:          true,
			}
			if err := server.RunWithOptions(ctx, *c, opts); err != nil {
				fmt.Printf("[Server] Error: %v\n", err)
				errCh <- fmt.Errorf("server error: %w", err)
			}
		}()

		// Wait for server to be ready
		if !waitForServer(healthURL, 10*time.Second) {
			fmt.Println("\033[31mError: Server failed to start\033[0m")
			errCh <- fmt.Errorf("server failed to start")
			wg.Done() // agent never spawned
			return
		}

		// Load agent config
		agentCfg := loadAgentConfig()
		SetSharedDB(svcCtx.DB.GetDB())

		// Start agent
		go func() {
			defer wg.Done()
			settingsDir := filepath.Dir(c.Database.SQLitePath)
			agentOpts := AgentOptions{
				ChannelManager:   channelMgr,
				Database:         svcCtx.DB.GetDB(),
				Quiet:            true,
				Dangerously:      dangerouslyAll,
				SettingsFilePath: filepath.Join(settingsDir, "agent-settings.json"),
			}
			if err := runAgent(ctx, agentCfg, serverURL, agentOpts); err != nil {
				if ctx.Err() == nil {
					errCh <- fmt.Errorf("agent error: %w", err)
				}
			}
		}()

		// Heartbeat daemon
		agentReady := make(chan struct{})

		lifecycle.OnAgentConnected(func(agentID string) {
			select {
			case <-agentReady:
			default:
				close(agentReady)
			}

			heartbeatOnce.Do(func() {
				heartbeat = daemon.NewHeartbeat(daemon.HeartbeatConfig{
					Interval: 30 * time.Minute,
					OnHeartbeat: func(hbCtx context.Context, tasks string) error {
						agent := svcCtx.AgentHub.GetAnyAgent()
						if agent == nil {
							return nil
						}
						prompt := daemon.FormatHeartbeatPrompt(tasks)
						sk := fmt.Sprintf("heartbeat-%d", time.Now().UnixNano())
						frame := &agenthub.Frame{
							Type:   "req",
							ID:     sk,
							Method: "run",
							Params: map[string]any{
								"prompt":      prompt,
								"session_key": sk,
							},
						}
						return svcCtx.AgentHub.SendToAgent(agent.ID, frame)
					},
				})
				heartbeat.Start(ctx)
			})

			// Update tray status
			statusItem.SetLabel("Status: Connected")
		})

		lifecycle.OnAgentDisconnected(func(agentID string) {
			statusItem.SetLabel("Status: Disconnected")
		})

		// Wait for agent (with timeout)
		select {
		case <-agentReady:
		case <-time.After(5 * time.Second):
		}

		// Print to console (for users who launched from terminal)
		fmt.Println()
		fmt.Printf("  Nebo desktop running\n")
		fmt.Printf("  Web UI: %s\n", serverURL)
		fmt.Printf("  Data: %s\n", dataDir)
		fmt.Println()
	}()

	// Handle errors from goroutines — quit the Wails app
	go func() {
		select {
		case err := <-errCh:
			fmt.Fprintf(os.Stderr, "\033[31mError: %v\033[0m\n", err)
			cancel()
			wailsApp.Quit()
		case <-ctx.Done():
		}
	}()

	// Run Wails event loop on main thread (blocks until app.Quit()).
	// This MUST be called immediately — macOS requires the event loop
	// on the main thread for window operations to work.
	if err := wailsApp.Run(); err != nil {
		fmt.Fprintf(os.Stderr, "Desktop error: %v\n", err)
	}

	// Cancel context to stop all goroutines
	cancel()
	wg.Wait()
}
