package apps

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sync"

	"github.com/google/uuid"
	"github.com/nebolabs/nebo/internal/agent/ai"
	"github.com/nebolabs/nebo/internal/agent/comm"
	"github.com/nebolabs/nebo/internal/agent/tools"
	pb "github.com/nebolabs/nebo/internal/apps/pb"
	"github.com/nebolabs/nebo/internal/db"
	pluginpkg "github.com/nebolabs/nebo/internal/plugin"
	"github.com/nebolabs/nebo/internal/svc"
)

// AppRegistryConfig holds dependencies for the app registry.
type AppRegistryConfig struct {
	DataDir     string
	NeboLoopURL string // If set, enables signature verification + revocation checks
	Queries     db.Querier
	PluginStore *pluginpkg.Store
	ToolReg     *tools.Registry
	CommMgr     *comm.CommPluginManager
}

// AppRegistry discovers, launches, and integrates apps with Nebo systems.
type AppRegistry struct {
	runtime       *Runtime
	appsDir       string
	queries       db.Querier
	pluginStore   *pluginpkg.Store
	toolReg       *tools.Registry
	commMgr       *comm.CommPluginManager
	installer     *InstallListener
	channelBridge *ChannelBridge

	providers []ai.Provider
	uiApps    map[string]*AppProcess // apps that provide UI
	mu        sync.RWMutex
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

	ar := &AppRegistry{
		runtime:       rt,
		appsDir:       appsDir,
		queries:       cfg.Queries,
		pluginStore:   cfg.PluginStore,
		toolReg:       cfg.ToolReg,
		commMgr:       cfg.CommMgr,
		channelBridge: NewChannelBridge(),
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
		if !entry.IsDir() {
			continue
		}
		appDir := filepath.Join(ar.appsDir, entry.Name())

		// Skip directories without a manifest
		if _, err := os.Stat(filepath.Join(appDir, "manifest.json")); err != nil {
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
			if ar.toolReg != nil {
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
			// Channel apps need channel: permission for the specific channel type
			if !HasPermissionPrefix(manifest, PermPrefixChannel) {
				fmt.Printf("[apps] Warning: %s provides %s but lacks channel: permissions — skipping\n", manifest.ID, cap)
				continue
			}
			fmt.Printf("[apps] Channel capability detected: %s (registration deferred)\n", cap)

		case cap == CapUI && proc.UIClient != nil:
			ar.mu.Lock()
			if ar.uiApps == nil {
				ar.uiApps = make(map[string]*AppProcess)
			}
			ar.uiApps[manifest.ID] = proc
			ar.mu.Unlock()
			fmt.Printf("[apps] Registered UI app: %s\n", manifest.Name)
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

func (ac *appConfigurable) Manifest() pluginpkg.SettingsManifest {
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
	return nil
}

func settingsToProto(settings map[string]string) *pb.SettingsMap {
	return &pb.SettingsMap{Values: settings}
}
