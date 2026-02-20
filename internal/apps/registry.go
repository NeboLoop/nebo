package apps

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"database/sql"

	"github.com/google/uuid"
	"github.com/neboloop/nebo/internal/agent/ai"
	"github.com/neboloop/nebo/internal/agent/comm"
	"github.com/neboloop/nebo/internal/agent/tools"
	pb "github.com/neboloop/nebo/internal/apps/pb"
	"github.com/neboloop/nebo/internal/apps/inspector"
	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/apps/settings"
	"github.com/neboloop/nebo/internal/svc"
)

// AppRegistryConfig holds dependencies for the app registry.
type AppRegistryConfig struct {
	DataDir     string
	NeboLoopURL string // If set, enables signature verification + revocation checks
	Queries     db.Querier
	PluginStore *settings.Store
	ToolReg     *tools.Registry
	SkillTool   *tools.SkillDomainTool // Unified skill tool — apps register here
	CommMgr     *comm.CommPluginManager
}

// QuarantineEvent is emitted when an app is quarantined so the UI can notify the user.
type QuarantineEvent struct {
	AppID   string `json:"app_id"`
	AppName string `json:"app_name"`
	Reason  string `json:"reason"`
}

// AppRegistry discovers, launches, and integrates apps with Nebo systems.
type AppRegistry struct {
	runtime       *Runtime
	appsDir       string
	queries       db.Querier
	pluginStore   *settings.Store
	toolReg       *tools.Registry
	skillTool     *tools.SkillDomainTool
	commMgr       *comm.CommPluginManager
	grpcInspector *inspector.Inspector

	supervisor          *Supervisor
	onQuarantine        func(QuarantineEvent)             // callback for UI notification
	onGatewayRegistered func()                             // callback when a new gateway provider is registered
	onChannelMsg        func(channelType, channelID, userID, text, metadata string) // callback for inbound channel messages
	providers           []ai.Provider
	uiApps          map[string]*AppProcess            // apps that provide UI
	channelAdapters map[string]*AppChannelAdapter      // registered channel apps
	scheduleAdapter *AppScheduleAdapter               // app-provided scheduler (replaces built-in)
	mu              sync.RWMutex
}

// OnQuarantine sets a callback invoked when an app is quarantined.
// Used to push notifications to the web UI via WebSocket.
func (ar *AppRegistry) OnQuarantine(fn func(QuarantineEvent)) {
	ar.mu.Lock()
	defer ar.mu.Unlock()
	ar.onQuarantine = fn
}

// OnGatewayRegistered sets a callback invoked when a new gateway provider is
// registered (e.g. after Janus is installed). Used to trigger ReloadProviders
// so the runner picks up new gateway providers at runtime.
func (ar *AppRegistry) OnGatewayRegistered(fn func()) {
	ar.mu.Lock()
	defer ar.mu.Unlock()
	ar.onGatewayRegistered = fn
}

// NewAppRegistry creates a new app registry.
func NewAppRegistry(cfg AppRegistryConfig) *AppRegistry {
	appsDir := filepath.Join(cfg.DataDir, "apps")
	os.MkdirAll(appsDir, 0755)

	rt := NewRuntime(cfg.DataDir, DefaultSandboxConfig())

	// Enable signature verification + revocation checks when NeboLoop URL is configured
	if cfg.NeboLoopURL != "" {
		rt.keyProvider = NewSigningKeyProvider(cfg.NeboLoopURL)
		rt.revChecker = NewRevocationChecker(cfg.NeboLoopURL)
		fmt.Printf("[apps] Signature verification enabled (NeboLoop: %s)\n", cfg.NeboLoopURL)
	} else {
		fmt.Println("[apps] Warning: NeboLoopURL not configured — signature verification disabled (dev mode)")
	}

	// gRPC inspector: always-on ring buffer with zero-cost fast path when no subscribers
	ins := inspector.New(1024)
	rt.inspector = ins

	ar := &AppRegistry{
		runtime:       rt,
		appsDir:       appsDir,
		queries:       cfg.Queries,
		pluginStore:   cfg.PluginStore,
		toolReg:       cfg.ToolReg,
		skillTool:     cfg.SkillTool,
		commMgr:       cfg.CommMgr,
		grpcInspector: ins,
	}

	return ar
}

// DiscoverAndLaunch scans the apps directory, validates manifests, launches binaries,
// and registers capabilities with appropriate Nebo systems.
func (ar *AppRegistry) DiscoverAndLaunch(ctx context.Context) error {
	entries, err := os.ReadDir(ar.appsDir)
	if err != nil {
		if os.IsNotExist(err) {
			return nil
		}
		return fmt.Errorf("read apps dir: %w", err)
	}

	for _, entry := range entries {
		// Use os.Stat (not entry.IsDir) to follow symlinks — sideloaded apps are symlinks
		appDir := filepath.Join(ar.appsDir, entry.Name())
		info, err := os.Stat(appDir)
		if err != nil || !info.IsDir() {
			continue
		}

		// Skip directories without a manifest
		if _, err := os.Stat(filepath.Join(appDir, "manifest.json")); err != nil {
			continue
		}

		// Skip quarantined apps (revoked by NeboLoop)
		if _, err := os.Stat(filepath.Join(appDir, ".quarantined")); err == nil {
			fmt.Printf("[apps] Skipping quarantined app: %s\n", entry.Name())
			continue
		}

		if err := ar.launchAndRegister(ctx, appDir); err != nil {
			fmt.Printf("[apps] Warning: failed to launch %s: %v\n", entry.Name(), err)
			continue
		}
	}

	return nil
}

// InstallFromURL downloads a .napp from the given URL and launches the app.
// Used by the HTTP install handler for immediate installation without waiting for MQTT.
func (ar *AppRegistry) InstallFromURL(ctx context.Context, downloadURL string) error {
	// Download to temp dir inside appsDir (same filesystem → rename is atomic)
	tmpDir, err := os.MkdirTemp(ar.appsDir, ".installing-*")
	if err != nil {
		return fmt.Errorf("create temp dir: %w", err)
	}
	defer os.RemoveAll(tmpDir)

	if err := DownloadAndExtractNapp(downloadURL, tmpDir); err != nil {
		return fmt.Errorf("download: %w", err)
	}

	manifest, err := LoadManifest(tmpDir)
	if err != nil {
		return fmt.Errorf("invalid app: %w", err)
	}

	appDir := filepath.Join(ar.appsDir, manifest.ID)

	// Already installed on disk — just ensure it's running
	if _, err := os.Stat(filepath.Join(appDir, "manifest.json")); err == nil {
		if !ar.IsRunning(manifest.ID) {
			return ar.launchAndRegister(ctx, appDir)
		}
		return nil
	}

	// Move downloaded app to permanent location
	if err := os.Rename(tmpDir, appDir); err != nil {
		return fmt.Errorf("move app to %s: %w", appDir, err)
	}

	return ar.launchAndRegister(ctx, appDir)
}

// launchAndRegister launches a single app and registers its capabilities.
func (ar *AppRegistry) launchAndRegister(ctx context.Context, appDir string) error {
	// Refuse to launch quarantined apps
	if _, err := os.Stat(filepath.Join(appDir, ".quarantined")); err == nil {
		return fmt.Errorf("app is quarantined (revoked by NeboLoop)")
	}

	// Kill orphaned process from a previous nebo run that died without cleanup
	cleanupStaleProcess(appDir)

	proc, err := ar.runtime.Launch(appDir)
	if err != nil {
		return err
	}

	manifest := proc.Manifest

	// Register in plugin_registry DB for UI visibility
	if err := ar.registerInDB(ctx, manifest); err != nil {
		fmt.Printf("[apps] Warning: DB registration failed for %s: %v\n", manifest.ID, err)
	}

	// Validate and register capabilities.
	// Each capability type requires specific permissions — deny by default.
	for _, cap := range manifest.Provides {
		switch {
		case cap == CapGateway && proc.GatewayClient != nil:
			// Gateway apps need at least one network permission to connect to AI backends
			if !HasPermissionPrefix(manifest, PermPrefixNetwork) {
				fmt.Printf("[apps] Warning: %s provides gateway but lacks network: permissions — skipping\n", manifest.ID)
				continue
			}
			adapter := NewGatewayProviderAdapter(proc.GatewayClient, manifest, "")
			ar.mu.Lock()
			ar.providers = append(ar.providers, adapter)
			cb := ar.onGatewayRegistered
			ar.mu.Unlock()
			fmt.Printf("[apps] Registered gateway provider: %s\n", manifest.Name)
			if cb != nil {
				cb()
			}

			// Auto-inject NeboLoop JWT for apps with user:token permission
			if CheckPermission(manifest, "user:token") {
				if err := ar.autoConfigureUserToken(ctx, manifest.Name); err != nil {
					fmt.Printf("[apps] Warning: auto-configure token for %s: %v\n", manifest.Name, err)
				}
			}

		case (cap == CapVision || cap == CapBrowser || hasPrefix(cap, CapPrefixTool)) && proc.ToolClient != nil:
			adapter, err := NewAppToolAdapter(ctx, proc.ToolClient)
			if err != nil {
				fmt.Printf("[apps] Warning: tool adapter failed for %s: %v\n", manifest.ID, err)
				continue
			}
			// Register through the unified skill tool if available
			if ar.skillTool != nil {
				slug := tools.Slugify(manifest.Name)
				skillMD := loadSkillMD(appDir)
				ar.skillTool.Register(slug, manifest.Name, manifest.Description, skillMD, adapter, nil, nil, 0, 0)
				fmt.Printf("[apps] Registered skill: %s (app-backed)\n", slug)
			} else if ar.toolReg != nil {
				// Fallback: register directly in tool registry (no skill tool wired)
				ar.toolReg.Register(adapter)
				fmt.Printf("[apps] Registered tool: %s\n", adapter.Name())
			}

		case cap == CapComm && proc.CommClient != nil:
			// Comm apps need comm: permission to access inter-agent communication
			if !HasPermissionPrefix(manifest, PermPrefixComm) {
				fmt.Printf("[apps] Warning: %s provides comm but lacks comm: permissions — skipping\n", manifest.ID)
				continue
			}
			adapter, err := NewAppCommAdapter(ctx, proc.CommClient)
			if err != nil {
				fmt.Printf("[apps] Warning: comm adapter failed for %s: %v\n", manifest.ID, err)
				continue
			}
			if ar.commMgr != nil {
				ar.commMgr.Register(adapter)
				fmt.Printf("[apps] Registered comm plugin: %s\n", adapter.Name())
			}

		case hasPrefix(cap, CapPrefixChannel) && proc.ChannelClient != nil:
			if !HasPermissionPrefix(manifest, PermPrefixChannel) {
				fmt.Printf("[apps] Warning: %s provides %s but lacks channel: permissions — skipping\n", manifest.ID, cap)
				continue
			}
			adapter, err := NewAppChannelAdapter(ctx, proc.ChannelClient)
			if err != nil {
				fmt.Printf("[apps] Warning: channel adapter failed for %s: %v\n", manifest.ID, err)
				continue
			}
			ar.registerChannel(adapter)
			fmt.Printf("[apps] Registered channel: %s\n", adapter.ID())

		case cap == CapUI && proc.UIClient != nil:
			ar.mu.Lock()
			if ar.uiApps == nil {
				ar.uiApps = make(map[string]*AppProcess)
			}
			ar.uiApps[manifest.ID] = proc
			ar.mu.Unlock()
			fmt.Printf("[apps] Registered UI app: %s\n", manifest.Name)

		case cap == CapSchedule && proc.ScheduleClient != nil:
			if !HasPermissionPrefix(manifest, PermPrefixSchedule) {
				fmt.Printf("[apps] Warning: %s provides schedule but lacks schedule: permissions — skipping\n", manifest.ID)
				continue
			}
			adapter, err := NewAppScheduleAdapter(ctx, proc.ScheduleClient)
			if err != nil {
				fmt.Printf("[apps] Warning: schedule adapter failed for %s: %v\n", manifest.ID, err)
				continue
			}
			ar.mu.Lock()
			ar.scheduleAdapter = adapter
			ar.mu.Unlock()
			fmt.Printf("[apps] Registered schedule provider: %s\n", manifest.Name)
		}
	}

	// Register as configurable if the app has settings
	if ar.pluginStore != nil && len(manifest.Settings) > 0 {
		configurable := &appConfigurable{
			manifest: manifest,
			proc:     proc,
		}
		if err := ar.pluginStore.RegisterConfigurable(ctx, manifest.Name, configurable); err != nil {
			fmt.Printf("[apps] Warning: settings registration failed for %s: %v\n", manifest.ID, err)
		}
	}

	// Update connection_status to "connected" now that the app is running
	if ar.queries != nil {
		if plugin, err := ar.queries.GetPluginByName(ctx, manifest.Name); err == nil {
			_ = ar.queries.UpdatePluginStatus(ctx, db.UpdatePluginStatusParams{
				ConnectionStatus: "connected",
				LastConnectedAt:  sql.NullInt64{Int64: time.Now().Unix(), Valid: true},
				ID:               plugin.ID,
			})
		}
	}

	return nil
}

// registerInDB ensures the app has a plugin_registry row.
func (ar *AppRegistry) registerInDB(ctx context.Context, manifest *AppManifest) error {
	if ar.queries == nil {
		return nil
	}

	// Check if already registered
	_, err := ar.queries.GetPluginByName(ctx, manifest.Name)
	if err == nil {
		return nil // Already exists
	}

	manifestJSON, _ := json.Marshal(manifest.ToSettingsManifest())
	metadataJSON, _ := json.Marshal(map[string]any{
		"app_id":      manifest.ID,
		"provides":    manifest.Provides,
		"permissions": manifest.Permissions,
		"runtime":     manifest.Runtime,
	})

	_, err = ar.queries.CreatePlugin(ctx, db.CreatePluginParams{
		ID:               uuid.New().String(),
		Name:             manifest.Name,
		PluginType:       "app",
		DisplayName:      manifest.Name,
		Description:      manifest.Description,
		Version:          manifest.Version,
		IsEnabled:        1,
		IsInstalled:      1,
		SettingsManifest: string(manifestJSON),
		Metadata:         string(metadataJSON),
	})
	return err
}

// PersistEventSettingsSchema persists a settings schema from an MQTT install event
// to the plugin_registry row. This supplements the manifest — if the DB row's
// settings_manifest is empty (manifest had no settings), the event schema is used.
// If the manifest already declared settings, the event schema is ignored (manifest wins).
func (ar *AppRegistry) PersistEventSettingsSchema(ctx context.Context, appName string, schemaJSON json.RawMessage) error {
	if ar.queries == nil || appName == "" {
		return nil
	}

	row, err := ar.queries.GetPluginByName(ctx, appName)
	if err != nil {
		return fmt.Errorf("app %q not found in DB: %w", appName, err)
	}

	// If the manifest already declared settings, don't override
	if row.SettingsManifest != "" && row.SettingsManifest != "{}" {
		var existing settings.SettingsManifest
		if json.Unmarshal([]byte(row.SettingsManifest), &existing) == nil && len(existing.Groups) > 0 {
			return nil // manifest settings take precedence
		}
	}

	// Parse the event schema (array of settings fields) into a SettingsManifest
	var fields []settings.SettingsField
	if err := json.Unmarshal(schemaJSON, &fields); err != nil {
		return fmt.Errorf("parse settings_schema: %w", err)
	}
	if len(fields) == 0 {
		return nil
	}

	manifest := settings.SettingsManifest{
		Groups: []settings.SettingsGroup{
			{
				Title:  appName + " Settings",
				Fields: fields,
			},
		},
	}
	manifestJSON, err := json.Marshal(manifest)
	if err != nil {
		return fmt.Errorf("marshal settings manifest: %w", err)
	}

	return ar.queries.UpdatePlugin(ctx, db.UpdatePluginParams{
		DisplayName:      row.DisplayName,
		Description:      row.Description,
		Icon:             row.Icon,
		Version:          row.Version,
		IsEnabled:        row.IsEnabled,
		SettingsManifest: string(manifestJSON),
		Metadata:         row.Metadata,
		ID:               row.ID,
	})
}

// GatewayProviders returns all registered gateway provider adapters.
func (ar *AppRegistry) GatewayProviders() []ai.Provider {
	ar.mu.RLock()
	defer ar.mu.RUnlock()
	result := make([]ai.Provider, len(ar.providers))
	copy(result, ar.providers)
	return result
}

// UIApps returns all app processes that provide UI capability.
func (ar *AppRegistry) UIApps() map[string]*AppProcess {
	ar.mu.RLock()
	defer ar.mu.RUnlock()
	result := make(map[string]*AppProcess, len(ar.uiApps))
	for k, v := range ar.uiApps {
		result[k] = v
	}
	return result
}

// GetUIApp returns a specific UI app process by app ID.
func (ar *AppRegistry) GetUIApp(appID string) (*AppProcess, bool) {
	ar.mu.RLock()
	defer ar.mu.RUnlock()
	proc, ok := ar.uiApps[appID]
	return proc, ok
}

// --- AppUIProvider interface implementation ---

// HandleRequest proxies an HTTP request to a UI app via gRPC.
func (ar *AppRegistry) HandleRequest(ctx context.Context, appID string, req *svc.AppHTTPRequest) (*svc.AppHTTPResponse, error) {
	proc, ok := ar.GetUIApp(appID)
	if !ok {
		return nil, fmt.Errorf("UI app not found: %s", appID)
	}
	resp, err := proc.UIClient.HandleRequest(ctx, &pb.HttpRequest{
		Method:  req.Method,
		Path:    req.Path,
		Query:   req.Query,
		Headers: req.Headers,
		Body:    req.Body,
	})
	if err != nil {
		return nil, fmt.Errorf("handle request: %w", err)
	}
	return &svc.AppHTTPResponse{
		StatusCode: int(resp.StatusCode),
		Headers:    resp.Headers,
		Body:       resp.Body,
	}, nil
}

// ListUIApps returns metadata about all apps that provide UI.
func (ar *AppRegistry) ListUIApps() []svc.AppUIInfo {
	ar.mu.RLock()
	defer ar.mu.RUnlock()
	var infos []svc.AppUIInfo
	for _, proc := range ar.uiApps {
		infos = append(infos, svc.AppUIInfo{
			ID:      proc.Manifest.ID,
			Name:    proc.Manifest.Name,
			Version: proc.Manifest.Version,
		})
	}
	return infos
}


// registerChannel adds a channel adapter and wires its inbound message handler.
// The handler looks up onChannelMsg lazily at call time so SetChannelHandler can
// be called after DiscoverAndLaunch and already-registered channels still pick it up.
func (ar *AppRegistry) registerChannel(adapter *AppChannelAdapter) {
	ar.mu.Lock()
	if ar.channelAdapters == nil {
		ar.channelAdapters = make(map[string]*AppChannelAdapter)
	}
	ar.channelAdapters[adapter.ID()] = adapter
	ar.mu.Unlock()

	channelType := adapter.ID()
	adapter.SetMessageHandler(func(channelID, userID, text, metadata string) {
		ar.mu.RLock()
		onMsg := ar.onChannelMsg
		ar.mu.RUnlock()
		if onMsg != nil {
			onMsg(channelType, channelID, userID, text, metadata)
		}
	})
}

// SetChannelHandler sets the callback for inbound messages from all channel apps.
// This is called by cmd/nebo/agent.go to wire channel messages into the main lane.
func (ar *AppRegistry) SetChannelHandler(fn func(channelType, channelID, userID, text, metadata string)) {
	ar.mu.Lock()
	defer ar.mu.Unlock()
	ar.onChannelMsg = fn
}

// SendToChannel sends a message to a specific channel app.
func (ar *AppRegistry) SendToChannel(ctx context.Context, channelType, channelID, text string) error {
	ar.mu.RLock()
	adapter, ok := ar.channelAdapters[channelType]
	ar.mu.RUnlock()
	if !ok {
		return fmt.Errorf("no channel adapter for type %q", channelType)
	}
	return adapter.Send(ctx, channelID, text)
}

// ScheduleAdapter returns the app-provided schedule adapter, if any.
func (ar *AppRegistry) ScheduleAdapter() *AppScheduleAdapter {
	ar.mu.RLock()
	defer ar.mu.RUnlock()
	return ar.scheduleAdapter
}

// ListChannels returns the IDs of all registered channel adapters.
func (ar *AppRegistry) ListChannels() []string {
	ar.mu.RLock()
	defer ar.mu.RUnlock()
	ids := make([]string, 0, len(ar.channelAdapters))
	for id := range ar.channelAdapters {
		ids = append(ids, id)
	}
	return ids
}

// StartRevocationSweep runs a background goroutine that periodically checks
// all running apps against the NeboLoop revocation list. If a running app is
// found to be revoked, it is immediately stopped but its data/ directory is
// preserved for forensic analysis.
//
// This closes the gap where an app revoked after launch would keep running
// until Nebo restarts. The sweep interval matches the RevocationChecker TTL (1 hour).
func (ar *AppRegistry) StartRevocationSweep(ctx context.Context) {
	if ar.runtime.revChecker == nil {
		return // dev mode — no revocation checking
	}

	go func() {
		ticker := time.NewTicker(1 * time.Hour)
		defer ticker.Stop()

		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				ar.sweepRevoked()
			}
		}
	}()
}

// sweepRevoked checks all running apps against the revocation list and stops any revoked ones.
func (ar *AppRegistry) sweepRevoked() {
	if ar.runtime.revChecker == nil {
		return
	}

	running := ar.runtime.List()
	for _, appID := range running {
		revoked, err := ar.runtime.revChecker.IsRevoked(appID)
		if err != nil {
			fmt.Printf("[apps:sweep] Warning: revocation check failed for %s: %v\n", appID, err)
			continue
		}
		if revoked {
			fmt.Printf("[apps:sweep] App %s has been revoked — quarantining\n", appID)
			if err := ar.Quarantine(appID); err != nil {
				fmt.Printf("[apps:sweep] Failed to quarantine revoked app %s: %v\n", appID, err)
			}
		}
	}
}

// Uninstall stops a running app, unregisters its capabilities, and removes its directory.
// Returns an error if the app directory does not exist.
func (ar *AppRegistry) Uninstall(appID string) error {
	appDir := filepath.Join(ar.appsDir, appID)
	if _, err := os.Stat(appDir); err != nil {
		return fmt.Errorf("app not found: %s", appID)
	}

	// Stop the app if running
	if _, ok := ar.runtime.Get(appID); ok {
		if err := ar.runtime.Stop(appID); err != nil {
			fmt.Printf("[apps] Warning: failed to stop %s during uninstall: %v\n", appID, err)
		}
	}

	// Remove UI registration
	ar.mu.Lock()
	delete(ar.uiApps, appID)
	ar.mu.Unlock()

	// Remove DB registration
	if ar.queries != nil {
		manifest, err := LoadManifest(appDir)
		if err == nil {
			if plugin, err := ar.queries.GetPluginByName(context.Background(), manifest.Name); err == nil {
				_ = ar.queries.DeletePlugin(context.Background(), plugin.ID)
			}
		}
	}

	// Remove directory entirely
	if err := os.RemoveAll(appDir); err != nil {
		return fmt.Errorf("remove app directory: %w", err)
	}

	// Also clean up any pending update
	os.RemoveAll(appDir + ".pending")
	os.RemoveAll(appDir + ".updating")

	fmt.Printf("[apps] Uninstalled %s\n", appID)
	return nil
}

// Quarantine stops a running app but preserves its data/ directory for forensic analysis.
// Used when NeboLoop revokes an app (security issue detected globally).
func (ar *AppRegistry) Quarantine(appID string) error {
	appDir := filepath.Join(ar.appsDir, appID)

	// Stop the app if running
	if _, ok := ar.runtime.Get(appID); ok {
		if err := ar.runtime.Stop(appID); err != nil {
			fmt.Printf("[apps] Warning: failed to stop %s during quarantine: %v\n", appID, err)
		}
	}

	// Remove UI registration
	ar.mu.Lock()
	delete(ar.uiApps, appID)
	ar.mu.Unlock()

	// Remove the binary so it can never re-launch, but preserve data/ and logs/
	os.Remove(filepath.Join(appDir, "binary"))
	os.Remove(filepath.Join(appDir, "app"))
	os.Remove(filepath.Join(appDir, "app.sock"))

	// Mark as quarantined (creates a marker file)
	marker := filepath.Join(appDir, ".quarantined")
	os.WriteFile(marker, []byte(fmt.Sprintf("revoked at %s\n", time.Now().UTC().Format(time.RFC3339))), 0600)

	fmt.Printf("[apps:quarantine] App %s quarantined — binary removed, data preserved for forensics\n", appID)

	// Notify UI so the user sees a banner
	ar.mu.RLock()
	cb := ar.onQuarantine
	ar.mu.RUnlock()
	if cb != nil {
		appName := appID
		if manifest, err := LoadManifest(appDir); err == nil {
			appName = manifest.Name
		}
		cb(QuarantineEvent{
			AppID:   appID,
			AppName: appName,
			Reason:  "Removed due to a security concern",
		})
	}

	return nil
}

// Sideload validates a developer's project directory and creates a symlink in appsDir.
// The existing watcher detects the symlink and auto-launches the app.
func (ar *AppRegistry) Sideload(ctx context.Context, projectPath string) (*AppManifest, error) {
	// Verify the path exists and is a directory
	info, err := os.Stat(projectPath)
	if err != nil {
		return nil, fmt.Errorf("path does not exist: %w", err)
	}
	if !info.IsDir() {
		return nil, fmt.Errorf("path is not a directory: %s", projectPath)
	}

	// Verify manifest.json exists and parses
	manifest, err := LoadManifest(projectPath)
	if err != nil {
		return nil, fmt.Errorf("invalid app directory: %w", err)
	}

	// Build from source if Makefile exists
	makefilePath := filepath.Join(projectPath, "Makefile")
	if _, err := os.Stat(makefilePath); err == nil {
		fmt.Printf("[apps] Building dev app: make build in %s\n", projectPath)
		cmd := exec.Command("make", "build")
		cmd.Dir = projectPath
		output, err := cmd.CombinedOutput()
		if err != nil {
			return nil, fmt.Errorf("build failed: %s\n%s", err, string(output))
		}
	}

	// Verify binary exists (checks root, then tmp/)
	if _, err := FindBinary(projectPath); err != nil {
		return nil, fmt.Errorf("no binary found after build: %w", err)
	}

	// Check for collision — the target symlink path
	symlinkPath := filepath.Join(ar.appsDir, manifest.ID)
	if existing, err := os.Lstat(symlinkPath); err == nil {
		// Something already exists at this path
		if existing.Mode()&os.ModeSymlink != 0 {
			// It's already a symlink — check if it points to the same path
			target, _ := os.Readlink(symlinkPath)
			if target == projectPath {
				// Same path — just ensure it's launched
				if _, ok := ar.runtime.Get(manifest.ID); !ok {
					if err := ar.launchAndRegister(ctx, symlinkPath); err != nil {
						return nil, fmt.Errorf("launch failed: %w", err)
					}
				}
				return manifest, nil
			}
			// Different path — remove old symlink first
			os.Remove(symlinkPath)
		} else {
			return nil, fmt.Errorf("app %s already exists as a non-sideloaded app", manifest.ID)
		}
	}

	// Create symlink
	if err := os.Symlink(projectPath, symlinkPath); err != nil {
		return nil, fmt.Errorf("create symlink: %w", err)
	}

	// Launch immediately for instant feedback (don't wait for watcher)
	if err := ar.launchAndRegister(ctx, symlinkPath); err != nil {
		// Clean up symlink on launch failure
		os.Remove(symlinkPath)
		return nil, fmt.Errorf("launch failed: %w", err)
	}

	fmt.Printf("[apps] Sideloaded dev app: %s (%s)\n", manifest.Name, projectPath)
	return manifest, nil
}

// Unsideload stops a sideloaded app and removes its symlink.
func (ar *AppRegistry) Unsideload(appID string) error {
	symlinkPath := filepath.Join(ar.appsDir, appID)

	// Verify it's a symlink (safety check — don't delete real app directories)
	info, err := os.Lstat(symlinkPath)
	if err != nil {
		return fmt.Errorf("app not found: %s", appID)
	}
	if info.Mode()&os.ModeSymlink == 0 {
		return fmt.Errorf("app %s is not sideloaded (not a symlink)", appID)
	}

	// Stop the app if running
	if _, ok := ar.runtime.Get(appID); ok {
		if err := ar.runtime.Stop(appID); err != nil {
			fmt.Printf("[apps] Warning: failed to stop %s during unsideload: %v\n", appID, err)
		}
	}

	// Remove UI registration
	ar.mu.Lock()
	delete(ar.uiApps, appID)
	ar.mu.Unlock()

	// Remove the symlink
	if err := os.Remove(symlinkPath); err != nil {
		return fmt.Errorf("remove symlink: %w", err)
	}

	fmt.Printf("[apps] Unsideloaded dev app: %s\n", appID)
	return nil
}

// IsSideloaded returns true if the app directory is a symlink (dev-loaded).
func (ar *AppRegistry) IsSideloaded(appID string) bool {
	symlinkPath := filepath.Join(ar.appsDir, appID)
	info, err := os.Lstat(symlinkPath)
	if err != nil {
		return false
	}
	return info.Mode()&os.ModeSymlink != 0
}

// Inspector returns the gRPC traffic inspector for dev tooling.
func (ar *AppRegistry) Inspector() *inspector.Inspector {
	return ar.grpcInspector
}

// IsRunning returns true if the app is currently running.
func (ar *AppRegistry) IsRunning(appID string) bool {
	_, ok := ar.runtime.Get(appID)
	return ok
}

// StartSupervisor starts the background process supervisor that auto-restarts crashed apps.
func (ar *AppRegistry) StartSupervisor(ctx context.Context) {
	ar.supervisor = NewSupervisor(ar, ar.runtime)
	ar.supervisor.Start(ctx)
}

// restartApp re-launches an app from its directory. Used by the supervisor
// to restart crashed apps. Wraps launchAndRegister with error recovery.
func (ar *AppRegistry) restartApp(ctx context.Context, appDir string) error {
	return ar.launchAndRegister(ctx, appDir)
}

// Stop stops all running app processes.
func (ar *AppRegistry) Stop() error {
	if ar.supervisor != nil {
		ar.supervisor.Stop()
	}
	return ar.runtime.StopAll()
}

// AppsDir returns the apps directory path.
func (ar *AppRegistry) AppsDir() string {
	return ar.appsDir
}

// appConfigurable implements plugin.Configurable for apps with settings.
type appConfigurable struct {
	manifest *AppManifest
	proc     *AppProcess
}

func (ac *appConfigurable) Manifest() settings.SettingsManifest {
	return ac.manifest.ToSettingsManifest()
}

func (ac *appConfigurable) OnSettingsChanged(settings map[string]string) error {
	// Forward settings to the app via gRPC Configure RPC
	if ac.proc.GatewayClient != nil {
		_, err := ac.proc.GatewayClient.Configure(context.Background(), settingsToProto(settings))
		return err
	}
	if ac.proc.ToolClient != nil {
		_, err := ac.proc.ToolClient.Configure(context.Background(), settingsToProto(settings))
		return err
	}
	if ac.proc.CommClient != nil {
		_, err := ac.proc.CommClient.Configure(context.Background(), settingsToProto(settings))
		return err
	}
	if ac.proc.ChannelClient != nil {
		_, err := ac.proc.ChannelClient.Configure(context.Background(), settingsToProto(settings))
		return err
	}
	if ac.proc.UIClient != nil {
		_, err := ac.proc.UIClient.Configure(context.Background(), settingsToProto(settings))
		return err
	}
	if ac.proc.ScheduleClient != nil {
		_, err := ac.proc.ScheduleClient.Configure(context.Background(), settingsToProto(settings))
		return err
	}
	return nil
}

// PushOAuthTokens pushes OAuth tokens to a running app via its gRPC Configure RPC.
// Implements broker.AppTokenReceiver.
func (ar *AppRegistry) PushOAuthTokens(appID, provider string, tokens map[string]string) error {
	proc, ok := ar.runtime.Get(appID)
	if !ok {
		return fmt.Errorf("app %s is not running", appID)
	}

	cfg := &appConfigurable{proc: proc}
	return cfg.OnSettingsChanged(tokens)
}

// autoConfigureUserToken reads the NeboLoop JWT from auth_profiles and
// injects it as the app's "token" setting. Apps with user:token permission
// use this JWT to authenticate with NeboLoop backend services.
func (ar *AppRegistry) autoConfigureUserToken(ctx context.Context, appName string) error {
	if ar.queries == nil || ar.pluginStore == nil {
		return nil
	}

	// Read NeboLoop JWT from auth_profiles
	profiles, err := ar.queries.ListAllActiveAuthProfilesByProvider(ctx, "neboloop")
	if err != nil || len(profiles) == 0 {
		return nil // No NeboLoop profile — nothing to configure
	}
	jwt := profiles[0].ApiKey
	if jwt == "" {
		return nil
	}

	// Check if token is already configured
	existing, _ := ar.pluginStore.GetSettingsByName(ctx, appName)
	if existing["token"] != "" {
		return nil // Already configured
	}

	// Store the JWT as the app's token setting
	plugin, err := ar.pluginStore.GetPlugin(ctx, appName)
	if err != nil {
		return fmt.Errorf("app not in plugin registry: %w", err)
	}

	return ar.pluginStore.UpdateSettings(ctx, plugin.ID,
		map[string]string{"token": jwt},
		map[string]bool{"token": true},
	)
}

func settingsToProto(settings map[string]string) *pb.SettingsMap {
	return &pb.SettingsMap{Values: settings}
}

// AppCatalog returns a formatted markdown section listing all running apps
// with their capabilities and descriptions. Intended for injection into the
// agent's system prompt so it knows what apps are installed.
func (ar *AppRegistry) AppCatalog() string {
	running := ar.runtime.List()
	if len(running) == 0 {
		return ""
	}

	var b strings.Builder
	b.WriteString("\n\n## Installed Apps\n\n")

	for _, appID := range running {
		proc, ok := ar.runtime.Get(appID)
		if !ok || proc.Manifest == nil {
			continue
		}
		m := proc.Manifest
		b.WriteString(fmt.Sprintf("- **%s** (%s)", m.Name, m.ID))
		if m.Description != "" {
			b.WriteString(fmt.Sprintf(" — %s", m.Description))
		}
		if len(m.Provides) > 0 {
			b.WriteString(fmt.Sprintf(". Provides: %s.", strings.Join(m.Provides, ", ")))
		}
		b.WriteString(" Status: running.\n")
	}

	return b.String()
}

// loadSkillMD reads the SKILL.md file from an app directory.
// Returns empty string if not found (backwards compatibility with apps installed before SKILL.md was required).
func loadSkillMD(appDir string) string {
	for _, name := range []string{"SKILL.md", "skill.md"} {
		data, err := os.ReadFile(filepath.Join(appDir, name))
		if err == nil {
			return string(data)
		}
	}
	return ""
}
