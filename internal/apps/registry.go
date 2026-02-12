package apps

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/nebolabs/nebo/internal/agent/ai"
	"github.com/nebolabs/nebo/internal/agent/comm"
	"github.com/nebolabs/nebo/internal/agent/tools"
	pb "github.com/nebolabs/nebo/internal/apps/pb"
	"github.com/nebolabs/nebo/internal/apps/inspector"
	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/apps/settings"
	"github.com/nebolabs/nebo/internal/svc"
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
	installer     *InstallListener
	channelBridge *ChannelBridge
	grpcInspector *inspector.Inspector

	onQuarantine    func(QuarantineEvent)             // callback for UI notification
	onChannelMsg    func(channelType, channelID, userID, text, metadata string) // callback for inbound channel messages
	providers       []ai.Provider
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
		channelBridge: NewChannelBridge(),
		grpcInspector: ins,
	}
	ar.installer = newInstallListener(ar)

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

// launchAndRegister launches a single app and registers its capabilities.
func (ar *AppRegistry) launchAndRegister(ctx context.Context, appDir string) error {
	// Refuse to launch quarantined apps
	if _, err := os.Stat(filepath.Join(appDir, ".quarantined")); err == nil {
		return fmt.Errorf("app is quarantined (revoked by NeboLoop)")
	}

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
			ar.mu.Unlock()
			fmt.Printf("[apps] Registered gateway provider: %s\n", manifest.Name)

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
				ar.skillTool.Register(slug, manifest.Name, manifest.Description, skillMD, adapter)
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

// GetUIView fetches the current view from a UI app via gRPC.
func (ar *AppRegistry) GetUIView(ctx context.Context, appID string) (any, error) {
	proc, ok := ar.GetUIApp(appID)
	if !ok {
		return nil, fmt.Errorf("UI app not found: %s", appID)
	}
	view, err := proc.UIClient.GetView(ctx, &pb.GetViewRequest{})
	if err != nil {
		return nil, fmt.Errorf("get view: %w", err)
	}
	return uiViewToJSON(view), nil
}

// SendUIEvent sends a user interaction event to a UI app via gRPC.
func (ar *AppRegistry) SendUIEvent(ctx context.Context, appID string, event any) (any, error) {
	proc, ok := ar.GetUIApp(appID)
	if !ok {
		return nil, fmt.Errorf("UI app not found: %s", appID)
	}
	ev, ok := event.(*UIEventPayload)
	if !ok {
		return nil, fmt.Errorf("invalid event type")
	}
	resp, err := proc.UIClient.SendEvent(ctx, &pb.UIEvent{
		ViewId:  ev.ViewID,
		BlockId: ev.BlockID,
		Action:  ev.Action,
		Value:   ev.Value,
	})
	if err != nil {
		return nil, fmt.Errorf("send event: %w", err)
	}
	result := map[string]any{}
	if resp.Error != "" {
		result["error"] = resp.Error
	}
	if resp.Toast != "" {
		result["toast"] = resp.Toast
	}
	if resp.View != nil {
		result["view"] = uiViewToJSON(resp.View)
	}
	return result, nil
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

// UIEventPayload is the JSON payload for a UI event from the HTTP API.
type UIEventPayload struct {
	ViewID  string `json:"view_id"`
	BlockID string `json:"block_id"`
	Action  string `json:"action"`
	Value   string `json:"value"`
}

// uiViewToJSON converts a proto UIView to a JSON-friendly map.
func uiViewToJSON(view *pb.UIView) map[string]any {
	blocks := make([]map[string]any, 0, len(view.Blocks))
	for _, b := range view.Blocks {
		block := map[string]any{
			"block_id": b.BlockId,
			"type":     b.Type,
		}
		if b.Text != "" {
			block["text"] = b.Text
		}
		if b.Value != "" {
			block["value"] = b.Value
		}
		if b.Placeholder != "" {
			block["placeholder"] = b.Placeholder
		}
		if b.Hint != "" {
			block["hint"] = b.Hint
		}
		if b.Variant != "" {
			block["variant"] = b.Variant
		}
		if b.Src != "" {
			block["src"] = b.Src
		}
		if b.Alt != "" {
			block["alt"] = b.Alt
		}
		if b.Disabled {
			block["disabled"] = true
		}
		if b.Style != "" {
			block["style"] = b.Style
		}
		if len(b.Options) > 0 {
			opts := make([]map[string]string, len(b.Options))
			for i, o := range b.Options {
				opts[i] = map[string]string{"label": o.Label, "value": o.Value}
			}
			block["options"] = opts
		}
		blocks = append(blocks, block)
	}
	return map[string]any{
		"view_id": view.ViewId,
		"title":   view.Title,
		"blocks":  blocks,
	}
}

// StartInstallListener connects to NeboLoop's MQTT broker and subscribes to
// app install events. This enables the full install flow: user clicks Install
// in NeboLoop → MQTT notification → download .napp → extract → verify → launch.
func (ar *AppRegistry) StartInstallListener(ctx context.Context, config InstallListenerConfig) error {
	return ar.installer.Start(ctx, config)
}

// StartChannelBridge connects to NeboLoop's MQTT broker and subscribes to
// channel inbound messages. This enables Nebo to receive messages from
// NeboLoop's channel bridges (Telegram, Discord, etc.) and send responses back.
func (ar *AppRegistry) StartChannelBridge(ctx context.Context, config ChannelBridgeConfig) error {
	return ar.channelBridge.Start(ctx, config)
}

// ChannelBridge returns the channel bridge instance for setting message handlers
// and sending responses.
func (ar *AppRegistry) ChannelBridge() *ChannelBridge {
	return ar.channelBridge
}

// registerChannel adds a channel adapter and wires its inbound message handler.
func (ar *AppRegistry) registerChannel(adapter *AppChannelAdapter) {
	ar.mu.Lock()
	if ar.channelAdapters == nil {
		ar.channelAdapters = make(map[string]*AppChannelAdapter)
	}
	ar.channelAdapters[adapter.ID()] = adapter
	onMsg := ar.onChannelMsg
	ar.mu.Unlock()

	channelType := adapter.ID()
	adapter.SetMessageHandler(func(channelID, userID, text, metadata string) {
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

// Stop stops all running app processes, the install listener, and the channel bridge.
func (ar *AppRegistry) Stop() error {
	ar.installer.Stop()
	ar.channelBridge.Stop()
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

func settingsToProto(settings map[string]string) *pb.SettingsMap {
	return &pb.SettingsMap{Values: settings}
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
