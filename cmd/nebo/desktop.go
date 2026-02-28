//go:build desktop

package cli

import (
	"context"
	_ "embed"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	goruntime "runtime"
	"sync"
	"sync/atomic"
	"time"

	"strings"

	"github.com/wailsapp/wails/v3/pkg/application"
	"github.com/wailsapp/wails/v3/pkg/events"

	"github.com/neboloop/nebo/internal/agenthub"
	"github.com/neboloop/nebo/internal/daemon"
	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/db/migrations"
	"github.com/neboloop/nebo/internal/defaults"
	"github.com/neboloop/nebo/internal/lifecycle"
	"github.com/neboloop/nebo/internal/local"
	"github.com/neboloop/nebo/internal/logging"
	"github.com/neboloop/nebo/internal/server"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/updater"
	"github.com/neboloop/nebo/internal/devlog"
	"github.com/neboloop/nebo/internal/webview"

	neboapp "github.com/neboloop/nebo/app"
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
	w, h := window.Size()
	// Don't save zero/invalid dimensions (can happen if window is minimized or not yet visible)
	if w < 400 || h < 300 {
		return
	}
	x, y := window.Position()
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
	// Augment PATH so CLI tools (claude, codex, gemini) are discoverable
	// even when launched as a macOS .app from Finder/Dock
	ensureUserPath()

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

	// Release lock before binary restart so the new process can acquire it
	updater.SetPreApplyHook(func() { releaseLock(lockFile) })

	c := ServerConfig

	// Initialize shared ServiceContext ONCE — single owner of the database connection
	svcCtx := svc.NewServiceContext(*c)
	svcCtx.Version = AppVersion
	svcCtx.SetUpdateManager(&svc.UpdateMgr{})
	defer svcCtx.Close()

	if svcCtx.DB == nil {
		fmt.Printf("\033[31mError: Failed to initialize database\033[0m\n")
		os.Exit(1)
	}

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
		// Route native bridge messages from agent-controlled browser windows
		// to the webview callback collector. JS sends "nebo:cb:{json}" via
		// window._wails.invoke() which bypasses CORS and mixed content blocking.
		RawMessageHandler: func(_ application.Window, message string, _ *application.OriginInfo) {
			t := time.Now()
			const prefix = "nebo:cb:"
			if !strings.HasPrefix(message, prefix) {
				return
			}
			jsonStr := message[len(prefix):]
			var result webview.CallbackResult
			if err := json.Unmarshal([]byte(jsonStr), &result); err != nil || result.RequestID == "" {
				return
			}
			devlog.Printf("[Desktop] RawMessageHandler: delivering callback reqID=%s (%s)\n", result.RequestID, time.Since(t))
			webview.GetCollector().Deliver(result)
		},
		OnShutdown: func() {
			fmt.Println("\n\033[32mNebo stopped.\033[0m")
		},
	})

	// Inject media capture permission handler into Wails' WebviewWindowDelegate
	// so microphone/camera access is auto-granted without a permission dialog.
	InjectWebViewMediaPermissions()

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
	// Windows (WebView2) needs a longer delay than macOS (WebKit).
	restoreDelay := 200 * time.Millisecond
	if goruntime.GOOS == "windows" {
		restoreDelay = 500 * time.Millisecond
	}
	if saved != nil {
		go func() {
			time.Sleep(restoreDelay)
			window.SetPosition(saved.X, saved.Y)
			stateRestored.Store(true)
		}()
	} else {
		stateRestored.Store(true)
	}

	// Auto-save window state on move/resize (only after initial restore, skip during quit).
	//
	// On Windows, Wails v3 emits events.Windows.WindowDidMove (ID 1205) and
	// events.Windows.WindowDidResize (ID 1206). There IS a mapping from
	// Windows→Common events via setupEventMapping, but it goes through an
	// async goroutine chain (listener → emit → channel → goroutine) which can
	// be unreliable in the alpha framework. We hook both the platform-specific
	// events and Common events directly for maximum reliability.
	saveMoveResize := func(event *application.WindowEvent) {
		if stateRestored.Load() && !quitting.Load() {
			saveWindowState(dataDir, window)
		}
	}
	window.RegisterHook(events.Common.WindowDidMove, saveMoveResize)
	window.RegisterHook(events.Common.WindowDidResize, saveMoveResize)
	if goruntime.GOOS == "windows" {
		window.RegisterHook(events.Windows.WindowDidMove, saveMoveResize)
		window.RegisterHook(events.Windows.WindowDidResize, saveMoveResize)
	}

	// Hide window on close instead of destroying it (minimize to tray).
	// When quitting, let the close proceed so the app can exit.
	// Same platform-specific hook pattern as above for reliability.
	closeHandler := func(event *application.WindowEvent) {
		if quitting.Load() {
			return
		}
		saveWindowState(dataDir, window)
		window.Hide()
		event.Cancel()
	}
	window.RegisterHook(events.Common.WindowClosing, closeHandler)
	if goruntime.GOOS == "windows" {
		window.RegisterHook(events.Windows.WindowClosing, closeHandler)
	}

	// Create system tray
	systray := wailsApp.SystemTray.New()
	systray.SetIcon(trayIcon)
	systray.SetLabel("")

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

	updateItem := trayMenu.Add("Check for Updates")
	updateItem.OnClick(func(_ *application.Context) {
		installMethod := updater.DetectInstallMethod()

		// Package manager installs: show hint, no download
		if installMethod == "homebrew" {
			updateItem.SetLabel("Managed by Homebrew")
			time.AfterFunc(3*time.Second, func() {
				updateItem.SetLabel("Check for Updates")
			})
			return
		}
		if installMethod == "package_manager" {
			updateItem.SetLabel("Use apt upgrade")
			time.AfterFunc(3*time.Second, func() {
				updateItem.SetLabel("Check for Updates")
			})
			return
		}

		// Check if there's already a pending update ready to apply
		if um := svcCtx.UpdateManager(); um != nil {
			if pending := um.PendingPath(); pending != "" {
				updateItem.SetLabel("Restarting...")
				updateItem.SetEnabled(false)
				go func() {
					if err := updater.Apply(pending); err != nil {
						fmt.Printf("[updater] apply failed: %v\n", err)
						updateItem.SetLabel("Update Failed")
						time.AfterFunc(3*time.Second, func() {
							updateItem.SetLabel("Check for Updates")
							updateItem.SetEnabled(true)
						})
					}
				}()
				return
			}
		}

		updateItem.SetLabel("Checking...")
		updateItem.SetEnabled(false)
		go func() {
			defer func() { updateItem.SetEnabled(true) }()

			result, err := updater.Check(context.Background(), AppVersion)
			if err != nil || result == nil {
				updateItem.SetLabel("Check for Updates")
				return
			}
			if !result.Available {
				updateItem.SetLabel("Up to Date (" + result.CurrentVersion + ")")
				time.AfterFunc(5*time.Second, func() {
					updateItem.SetLabel("Check for Updates")
				})
				return
			}

			// Download the update
			updateItem.SetLabel("Downloading...")
			tmpPath, err := updater.Download(context.Background(), result.LatestVersion, func(dl, total int64) {
				if total > 0 {
					pct := dl * 100 / total
					updateItem.SetLabel(fmt.Sprintf("Downloading %d%%...", pct))
				}
			})
			if err != nil {
				updateItem.SetLabel("Download Failed")
				time.AfterFunc(3*time.Second, func() {
					updateItem.SetLabel("Check for Updates")
				})
				return
			}

			// Verify checksum
			updateItem.SetLabel("Verifying...")
			if err := updater.VerifyChecksum(context.Background(), tmpPath, result.LatestVersion); err != nil {
				os.Remove(tmpPath)
				updateItem.SetLabel("Verification Failed")
				time.AfterFunc(3*time.Second, func() {
					updateItem.SetLabel("Check for Updates")
				})
				return
			}

			// Store pending and update label
			if um := svcCtx.UpdateManager(); um != nil {
				um.SetPending(tmpPath, result.LatestVersion)
			}
			updateItem.SetLabel("Restart to Update (" + result.LatestVersion + ")")
			updateItem.OnClick(func(_ *application.Context) {
				updateItem.SetLabel("Restarting...")
				updateItem.SetEnabled(false)
				go func() {
					if err := updater.Apply(tmpPath); err != nil {
						fmt.Printf("[updater] apply failed: %v\n", err)
						updateItem.SetLabel("Update Failed")
						time.AfterFunc(3*time.Second, func() {
							updateItem.SetLabel("Check for Updates")
							updateItem.SetEnabled(true)
						})
					}
				}()
			})
		}()
	})

	trayMenu.AddSeparator()
	trayMenu.Add("Quit Nebo").OnClick(func(_ *application.Context) {
		saveWindowState(dataDir, window)
		quitting.Store(true)
		safeQuit(wailsApp)
	})
	systray.SetMenu(trayMenu)

	// Use a context that cancels when Wails quits
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	startBackgroundUpdater(ctx, svcCtx)

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
		// Install native directory picker for dev sideload
		svcCtx.SetBrowseDirectory(func() (string, error) {
			return wailsApp.Dialog.OpenFile().
				CanChooseDirectories(true).
				CanChooseFiles(false).
				SetTitle("Select App Directory").
				PromptForSingleSelection()
		})
		svcCtx.SetBrowseFiles(func() ([]string, error) {
			return wailsApp.Dialog.OpenFile().
				CanChooseFiles(true).
				CanChooseDirectories(false).
				SetTitle("Select Files").
				PromptForMultipleSelection()
		})

		// Install dev window opener — creates or focuses the dev window
		svcCtx.SetOpenDevWindow(func() {
			if existing, ok := wailsApp.Window.Get("dev"); ok {
				existing.Show()
				existing.Focus()
				return
			}
			wailsApp.Window.NewWithOptions(application.WebviewWindowOptions{
				Name:      "dev",
				Title:     "Nebo Developer",
				Width:     1400,
				Height:    900,
				MinWidth:  800,
				MinHeight: 600,
				URL:       serverURL + "/dev",
			})
		})

		// Install popup window creator for OAuth and similar flows
		svcCtx.SetOpenPopup(func(url, title string, width, height int) {
			wailsApp.Window.NewWithOptions(application.WebviewWindowOptions{
				Name:      "popup-" + fmt.Sprintf("%d", time.Now().UnixMilli()),
				Title:     title,
				Width:     width,
				Height:    height,
				MinWidth:  400,
				MinHeight: 300,
				URL:       url,
			})
		})

		// Install native browser window creator for agent-controlled webview windows.
		// Each call creates a new Wails webview window that the agent can control
		// via ExecJS for DOM interaction — native WebKit/WebView2, not detectable as bot.
		wvm := webview.GetManager()
		wvm.SetCreator(func(opts webview.WindowCreatorOptions) webview.WindowHandle {
			devlog.Printf("[Desktop] Window creator called: name=%s url=%s\n", opts.Name, opts.URL)
			creatorT0 := time.Now()
			name := opts.Name
			if existing, ok := wailsApp.Window.Get(name); ok {
				if ww, ok := existing.(*application.WebviewWindow); ok {
					ww.Show()
					ww.Focus()
					return wailsWindowHandle{win: ww}
				}
			}
			width := opts.Width
			if width <= 0 {
				width = 1200
			}
			height := opts.Height
			if height <= 0 {
				height = 800
			}
			title := opts.Title
			if title == "" {
				title = "Nebo Browser"
			}
			var w *application.WebviewWindow
			if goruntime.GOOS == "windows" {
				// Wails v3 Windows bug: WebviewWindowOptions.JS is only applied for
				// HTML-mode windows, not URL-mode. On macOS/Linux, JS is injected
				// via a NavigationCompleted event handler, but Windows skips it.
				//
				// Workaround: Create with a blank HTML page to trigger chromium.Init()
				// which registers neboBootstrapJS via AddScriptToExecuteOnDocumentCreated.
				// This persists across ALL future navigations. Then immediately navigate
				// to the target URL.
				w = wailsApp.Window.NewWithOptions(application.WebviewWindowOptions{
					Name:      name,
					Title:     title,
					Width:     width,
					Height:    height,
					MinWidth:  400,
					MinHeight: 300,
					HTML:      " ", // Non-empty to trigger chromium.Init() with JS
					JS:        neboBootstrapJS,
				})
				if opts.URL != "" && opts.URL != "about:blank" {
					w.SetURL(opts.URL)
				}
			} else {
				w = wailsApp.Window.NewWithOptions(application.WebviewWindowOptions{
					Name:      name,
					Title:     title,
					Width:     width,
					Height:    height,
					MinWidth:  400,
					MinHeight: 300,
					URL:       opts.URL,
					// Bootstrap JS runs via impl-level execJS after EVERY navigation,
					// bypassing the runtimeLoaded gate on public ExecJS. Forces the
					// Wails runtime to be "ready" and pre-defines the callback function.
					JS: neboBootstrapJS,
				})
			}
			devlog.Printf("[Desktop] Window created (%s)\n", time.Since(creatorT0))
			return wailsWindowHandle{win: w}
		})
		wvm.SetCallbackURL(fmt.Sprintf("http://localhost:%d/internal/webview/callback", c.Port))

		agentMCPProxy := server.NewAgentMCPProxy()
		voiceDuplexProxy := server.NewVoiceDuplexProxy()

		devlog.Printf("[Desktop] Starting HTTP server...\n")
		go func() {
			defer wg.Done()
			opts := server.ServerOptions{
				SvcCtx:             svcCtx,
				Quiet:              true,
				DevMode:            true,
				AgentMCPHandler:    agentMCPProxy,
				VoiceDuplexHandler: voiceDuplexProxy,
			}
			if err := server.Run(ctx, *c, opts); err != nil {
				devlog.Printf("[Server] Error: %v\n", err)
				errCh <- fmt.Errorf("server error: %w", err)
			}
		}()

		// Wait for server to be ready
		devlog.Printf("[Desktop] Waiting for server readiness...\n")
		if !waitForServer(healthURL, 10*time.Second) {
			fmt.Println("\033[31mError: Server failed to start\033[0m")
			errCh <- fmt.Errorf("server failed to start")
			wg.Done() // agent never spawned
			return
		}

		devlog.Printf("[Desktop] Server ready, loading agent config...\n")
		// Load agent config
		agentCfg := loadAgentConfig()
		SetSharedDB(svcCtx.DB.GetDB())
		SetJanusURL(svcCtx.Config.NeboLoop.JanusURL)

		// Start agent with auto-reconnect on connection errors.
		// The agent goroutine retries with backoff until the context is cancelled.
		devlog.Printf("[Desktop] Starting agent...\n")
		go func() {
			defer wg.Done()
			agentOpts := AgentOptions{
				Database:         svcCtx.DB.GetDB(),
				PluginStore:      svcCtx.PluginStore,
				SvcCtx:           svcCtx,
				Quiet:            true,
				Dangerously:      dangerouslyAll,
				AgentMCPProxy:    agentMCPProxy,
				VoiceDuplexProxy: voiceDuplexProxy,
				Heartbeat:        &heartbeat,
			}
			backoff := time.Second
			maxBackoff := 30 * time.Second
			for {
				err := runAgent(ctx, agentCfg, serverURL, agentOpts)
				if ctx.Err() != nil {
					return // App is shutting down
				}
				if err == nil {
					return // Clean exit
				}
				devlog.Printf("[Desktop] Agent disconnected: %v — reconnecting in %s\n", err, backoff)
				statusItem.SetLabel("Status: Reconnecting...")
				select {
				case <-time.After(backoff):
					backoff *= 2
					if backoff > maxBackoff {
						backoff = maxBackoff
					}
				case <-ctx.Done():
					return
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

	// Handle errors from goroutines.
	// Only fatal errors (server startup failure) quit the Wails app.
	// Agent connection drops are recoverable — the agent will reconnect.
	go func() {
		for {
			select {
			case err := <-errCh:
				errStr := err.Error()
				isFatal := strings.Contains(errStr, "server error") ||
					strings.Contains(errStr, "server failed to start")

				if isFatal {
					fmt.Fprintf(os.Stderr, "\033[31mFatal: %v\033[0m\n", err)
					cancel()
					safeQuit(wailsApp)
					return
				}

				// Non-fatal (agent disconnect, etc.) — log and keep running
				fmt.Fprintf(os.Stderr, "\033[33mWarning: %v (will reconnect)\033[0m\n", err)
				statusItem.SetLabel("Status: Reconnecting...")
			case <-ctx.Done():
				return
			}
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

// safeQuit calls App.Quit() with recovery from Wails v3 alpha panics.
// Wails alpha.67 has a known issue where windowsSystemTray.destroy() can panic
// with a nil pointer dereference on globalApplication during cleanup.
func safeQuit(app *application.App) {
	defer func() {
		if r := recover(); r != nil {
			fmt.Fprintf(os.Stderr, "[Desktop] Recovered from quit panic: %v\n", r)
			os.Exit(0)
		}
	}()
	app.Quit()
}

// neboBootstrapJS is injected into every browser window via WebviewWindowOptions.JS.
// It runs via the impl-level execJS after EVERY navigation — this is critical because
// it bypasses the runtimeLoaded check that gates the public ExecJS method.
//
// Without this, ExecJS queues all JS forever on external pages where the Wails runtime
// fails to send "wails:runtime:ready", leaving runtimeLoaded permanently false.
//
// This script:
// 1. Defines window.__nebo_cb — the native callback function for agent actions
// 2. Sends "wails:runtime:ready" via the platform's native message handler,
//    forcing runtimeLoaded=true so queued ExecJS calls flush
const neboBootstrapJS = `
(function(){
  // Define the callback function on window so ExecJS-injected code can use it.
  // Uses native platform message handlers (not HTTP fetch) to bypass CORS/mixed content.
  window.__nebo_cb = function(d) {
    var m = "nebo:cb:" + JSON.stringify(d);
    try {
      if (window._wails && window._wails.invoke) {
        window._wails.invoke(m);
      } else if (window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.external) {
        window.webkit.messageHandlers.external.postMessage(m);
      } else if (window.chrome && window.chrome.webview) {
        window.chrome.webview.postMessage(m);
      }
    } catch(e) {}
  };

  // Force "wails:runtime:ready" so runtimeLoaded becomes true and ExecJS works.
  // Use a short delay to let the Wails runtime attempt its own initialization first.
  setTimeout(function() {
    try {
      if (window._wails && window._wails.invoke) {
        window._wails.invoke("wails:runtime:ready");
      } else if (window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.external) {
        window.webkit.messageHandlers.external.postMessage("wails:runtime:ready");
      } else if (window.chrome && window.chrome.webview) {
        window.chrome.webview.postMessage("wails:runtime:ready");
      }
    } catch(e) {}
  }, 200);
})();
`

// wailsWindowHandle adapts a Wails WebviewWindow to the webview.WindowHandle interface.
type wailsWindowHandle struct {
	win *application.WebviewWindow
}

func (w wailsWindowHandle) SetURL(url string) {
	devlog.Printf("[Desktop] SetURL(%s) dispatching...\n", url)
	t := time.Now()
	w.win.SetURL(url)
	devlog.Printf("[Desktop] SetURL returned (%s)\n", time.Since(t))
}
func (w wailsWindowHandle) ExecJS(js string) {
	preview := js
	if len(preview) > 80 {
		preview = preview[:80] + "..."
	}
	devlog.Printf("[Desktop] ExecJS(%s) dispatching...\n", preview)
	t := time.Now()
	w.win.ExecJS(js)
	devlog.Printf("[Desktop] ExecJS returned (%s)\n", time.Since(t))
}
func (w wailsWindowHandle) SetTitle(title string)    { w.win.SetTitle(title) }
func (w wailsWindowHandle) Show()                    { w.win.Show() }
func (w wailsWindowHandle) Hide()                    { w.win.Hide() }
func (w wailsWindowHandle) Focus()                   { w.win.Focus() }
func (w wailsWindowHandle) Close()                   { w.win.Close() }
func (w wailsWindowHandle) SetSize(width, height int) { w.win.SetSize(width, height) }
func (w wailsWindowHandle) Reload()                  { w.win.Reload() }
func (w wailsWindowHandle) Name() string             { return w.win.Name() }

// openURL opens a URL in the default browser.
func openURL(url string) {
	var cmd string
	var args []string
	switch goruntime.GOOS {
	case "darwin":
		cmd = "open"
		args = []string{url}
	case "windows":
		cmd = "rundll32"
		args = []string{"url.dll,FileProtocolHandler", url}
	default:
		cmd = "xdg-open"
		args = []string{url}
	}
	_ = exec.Command(cmd, args...).Start()
}
