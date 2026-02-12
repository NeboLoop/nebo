package apps

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/eclipse/paho.golang/autopaho"
	"github.com/eclipse/paho.golang/paho"
)

// InstallListenerConfig holds MQTT connection settings for the install listener.
// These are the same credentials used by the NeboLoop comm plugin.
type InstallListenerConfig struct {
	Broker        string // MQTT broker address (e.g., "tcp://localhost:1883")
	APIServer     string // NeboLoop REST API URL (e.g., "http://localhost:8888")
	BotID         string // Bot UUID assigned by NeboLoop
	MQTTUsername  string // MQTT username
	MQTTPassword  string // MQTT password
}

// InstallListener subscribes to NeboLoop MQTT install notifications and
// automatically downloads, extracts, verifies, and launches apps.
//
// Topic: neboloop/bot/{botID}/installs
// Events: app_installed, app_updated, app_uninstalled
type InstallListener struct {
	registry  *AppRegistry
	config    InstallListenerConfig
	cm        *autopaho.ConnectionManager
	connected bool
	cancel    context.CancelFunc
	mu        sync.RWMutex
}

// installEvent is the MQTT message format for app install notifications.
type installEvent struct {
	Event   string `json:"event"`   // app_installed, app_updated, app_uninstalled
	AppID   string `json:"app_id"`  // e.g., "com.neboloop.janus"
	AppName string `json:"app_name"`
	Version string `json:"version"`
	// Download URL may be provided directly or constructed from APIServer
	DownloadURL string `json:"download_url,omitempty"`
	// SettingsSchema is the app's settings fields from NeboLoop.
	// Used to persist the schema when the .napp manifest doesn't declare settings.
	SettingsSchema json.RawMessage `json:"settings_schema,omitempty"`
}

// newInstallListener creates an install listener bound to the given registry.
func newInstallListener(registry *AppRegistry) *InstallListener {
	return &InstallListener{
		registry: registry,
	}
}

// Start connects to the NeboLoop MQTT broker and subscribes to install events.
// Blocks until the initial connection is established or the context is cancelled.
func (il *InstallListener) Start(ctx context.Context, config InstallListenerConfig) error {
	il.mu.Lock()
	defer il.mu.Unlock()

	if il.connected {
		return fmt.Errorf("install listener already running")
	}

	if config.BotID == "" {
		return fmt.Errorf("install listener: bot_id is required")
	}
	if config.Broker == "" {
		return fmt.Errorf("install listener: broker is required")
	}

	il.config = config

	serverURL, err := brokerToInstallURL(config.Broker)
	if err != nil {
		return fmt.Errorf("install listener: invalid broker URL: %w", err)
	}

	connCtx, cancel := context.WithCancel(context.Background())
	il.cancel = cancel

	cfg := autopaho.ClientConfig{
		ServerUrls:                    []*url.URL{serverURL},
		KeepAlive:                     30,
		CleanStartOnInitialConnection: true,
		ConnectUsername:                config.MQTTUsername,
		ConnectPassword:               []byte(config.MQTTPassword),
		ConnectTimeout:                10 * time.Second,

		ReconnectBackoff: autopaho.NewExponentialBackoff(
			1*time.Second,
			60*time.Second,
			2*time.Second,
			2.0,
		),

		OnConnectionUp: func(cm *autopaho.ConnectionManager, connack *paho.Connack) {
			il.mu.Lock()
			il.connected = true
			il.mu.Unlock()

			fmt.Printf("[apps:install] Connected to MQTT broker\n")
			il.onConnect(cm)
		},

		OnConnectionDown: func() bool {
			il.mu.Lock()
			il.connected = false
			il.mu.Unlock()
			fmt.Printf("[apps:install] Connection lost, will reconnect\n")
			return true
		},

		OnConnectError: func(err error) {
			fmt.Printf("[apps:install] Connect error: %v\n", err)
		},

		ClientConfig: paho.ClientConfig{
			ClientID: fmt.Sprintf("nebo-install-%s", config.BotID),
			OnPublishReceived: []func(paho.PublishReceived) (bool, error){
				func(pr paho.PublishReceived) (bool, error) {
					il.onMessage(pr.Packet)
					return true, nil
				},
			},
		},
	}

	cm, err := autopaho.NewConnection(connCtx, cfg)
	if err != nil {
		cancel()
		return fmt.Errorf("install listener: failed to create connection: %w", err)
	}
	il.cm = cm

	if err := cm.AwaitConnection(ctx); err != nil {
		cancel()
		return fmt.Errorf("install listener: initial connection failed: %w", err)
	}

	return nil
}

// onConnect subscribes to the installs topic after each (re)connection.
func (il *InstallListener) onConnect(cm *autopaho.ConnectionManager) {
	il.mu.RLock()
	botID := il.config.BotID
	il.mu.RUnlock()

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	topic := fmt.Sprintf("neboloop/bot/%s/installs", botID)
	_, err := cm.Subscribe(ctx, &paho.Subscribe{
		Subscriptions: []paho.SubscribeOptions{
			{Topic: topic, QoS: 1},
		},
	})
	if err != nil {
		fmt.Printf("[apps:install] Subscribe failed for %s: %v\n", topic, err)
		return
	}
	fmt.Printf("[apps:install] Subscribed to %s\n", topic)
}

// onMessage handles incoming install event messages.
func (il *InstallListener) onMessage(pub *paho.Publish) {
	var event installEvent
	if err := json.Unmarshal(pub.Payload, &event); err != nil {
		fmt.Printf("[apps:install] Invalid message on %s: %v\n", pub.Topic, err)
		return
	}

	if event.AppID == "" {
		fmt.Printf("[apps:install] Message missing app_id, ignoring\n")
		return
	}

	fmt.Printf("[apps:install] Event: %s app=%s version=%s\n", event.Event, event.AppID, event.Version)

	switch event.Event {
	case "app_installed":
		il.handleInstall(event)
	case "app_updated":
		il.handleUpdate(event)
	case "app_uninstalled":
		il.handleUninstall(event)
	case "app_revoked":
		il.handleRevoke(event)
	default:
		fmt.Printf("[apps:install] Unknown event type: %s\n", event.Event)
	}
}

// handleInstall downloads and installs a new app.
func (il *InstallListener) handleInstall(event installEvent) {
	ctx := context.Background()

	downloadURL := il.downloadURL(event)
	if downloadURL == "" {
		fmt.Printf("[apps:install] No download URL for %s\n", event.AppID)
		return
	}

	appDir := filepath.Join(il.registry.appsDir, event.AppID)

	// Check if already installed
	if _, err := os.Stat(filepath.Join(appDir, "manifest.json")); err == nil {
		fmt.Printf("[apps:install] App %s already installed, skipping\n", event.AppID)
		return
	}

	if err := il.downloadAndExtract(downloadURL, appDir); err != nil {
		fmt.Printf("[apps:install] Failed to install %s: %v\n", event.AppID, err)
		// Clean up partial install
		os.RemoveAll(appDir)
		return
	}

	if err := il.registry.launchAndRegister(ctx, appDir); err != nil {
		fmt.Printf("[apps:install] Failed to launch %s: %v\n", event.AppID, err)
		return
	}

	// Persist settings schema from the install event if the manifest didn't declare settings.
	if len(event.SettingsSchema) > 0 {
		if err := il.registry.PersistEventSettingsSchema(ctx, event.AppName, event.SettingsSchema); err != nil {
			fmt.Printf("[apps:install] Warning: failed to persist settings schema for %s: %v\n", event.AppID, err)
		}
	}

	fmt.Printf("[apps:install] Installed and launched %s v%s\n", event.AppID, event.Version)
}

// handleUpdate stops the running app, replaces the binary/manifest (preserving data), and relaunches.
// If the new version adds permissions, the update is staged but not launched until user approves.
func (il *InstallListener) handleUpdate(event installEvent) {
	ctx := context.Background()

	downloadURL := il.downloadURL(event)
	if downloadURL == "" {
		fmt.Printf("[apps:install] No download URL for %s\n", event.AppID)
		return
	}

	appDir := filepath.Join(il.registry.appsDir, event.AppID)

	// Load the old manifest before stopping (for permission diff)
	var oldPermissions []string
	if oldManifest, err := LoadManifest(appDir); err == nil {
		oldPermissions = oldManifest.Permissions
	}

	// Stop the running app if it exists
	if _, ok := il.registry.runtime.Get(event.AppID); ok {
		if err := il.registry.runtime.Stop(event.AppID); err != nil {
			fmt.Printf("[apps:install] Warning: failed to stop %s for update: %v\n", event.AppID, err)
		}
	}

	// Preserve the data directory across updates
	dataDir := filepath.Join(appDir, "data")
	logsDir := filepath.Join(appDir, "logs")
	hasData := dirExists(dataDir)
	hasLogs := dirExists(logsDir)

	// Extract to a temp directory first, then swap
	tmpDir := appDir + ".updating"
	os.RemoveAll(tmpDir)

	if err := il.downloadAndExtract(downloadURL, tmpDir); err != nil {
		fmt.Printf("[apps:install] Failed to download update for %s: %v\n", event.AppID, err)
		os.RemoveAll(tmpDir)
		// Try to relaunch the old version
		if _, err := os.Stat(filepath.Join(appDir, "manifest.json")); err == nil {
			_ = il.registry.launchAndRegister(ctx, appDir)
		}
		return
	}

	// Permission diff: check if new version adds permissions
	newManifest, err := LoadManifest(tmpDir)
	if err != nil {
		fmt.Printf("[apps:install] Failed to load new manifest for %s: %v\n", event.AppID, err)
		os.RemoveAll(tmpDir)
		_ = il.registry.launchAndRegister(ctx, appDir)
		return
	}

	added := permissionDiff(oldPermissions, newManifest.Permissions)
	if len(added) > 0 {
		fmt.Printf("[apps:install] Update for %s adds new permissions: %v — requires user approval\n", event.AppID, added)
		// Stage the update but don't launch — user must approve new permissions
		// Move the downloaded .napp to a pending directory
		pendingDir := appDir + ".pending"
		os.RemoveAll(pendingDir)
		os.Rename(tmpDir, pendingDir)
		// Relaunch old version while waiting for approval
		if _, err := os.Stat(filepath.Join(appDir, "manifest.json")); err == nil {
			_ = il.registry.launchAndRegister(ctx, appDir)
		}
		return
	}

	// No new permissions — safe to auto-update

	// Move data and logs from old install to new
	if hasData {
		os.RemoveAll(filepath.Join(tmpDir, "data"))
		os.Rename(dataDir, filepath.Join(tmpDir, "data"))
	}
	if hasLogs {
		os.RemoveAll(filepath.Join(tmpDir, "logs"))
		os.Rename(logsDir, filepath.Join(tmpDir, "logs"))
	}

	// Swap: remove old, rename new
	os.RemoveAll(appDir)
	if err := os.Rename(tmpDir, appDir); err != nil {
		fmt.Printf("[apps:install] Failed to swap directories for %s: %v\n", event.AppID, err)
		os.RemoveAll(tmpDir)
		return
	}

	if err := il.registry.launchAndRegister(ctx, appDir); err != nil {
		fmt.Printf("[apps:install] Failed to relaunch %s after update: %v\n", event.AppID, err)
		return
	}

	// Persist updated settings schema from the event if present.
	if len(event.SettingsSchema) > 0 {
		if err := il.registry.PersistEventSettingsSchema(ctx, event.AppName, event.SettingsSchema); err != nil {
			fmt.Printf("[apps:install] Warning: failed to persist settings schema for %s: %v\n", event.AppID, err)
		}
	}

	fmt.Printf("[apps:install] Updated and relaunched %s v%s\n", event.AppID, event.Version)
}

// permissionDiff returns permissions present in newPerms but not in oldPerms.
func permissionDiff(oldPerms, newPerms []string) []string {
	old := make(map[string]bool, len(oldPerms))
	for _, p := range oldPerms {
		old[p] = true
	}
	var added []string
	for _, p := range newPerms {
		if !old[p] {
			added = append(added, p)
		}
	}
	return added
}

// handleUninstall stops and removes an app.
func (il *InstallListener) handleUninstall(event installEvent) {
	// Stop if running
	if _, ok := il.registry.runtime.Get(event.AppID); ok {
		if err := il.registry.runtime.Stop(event.AppID); err != nil {
			fmt.Printf("[apps:install] Warning: failed to stop %s: %v\n", event.AppID, err)
		}
	}

	appDir := filepath.Join(il.registry.appsDir, event.AppID)
	if err := os.RemoveAll(appDir); err != nil {
		fmt.Printf("[apps:install] Warning: failed to remove %s: %v\n", event.AppID, err)
	}

	fmt.Printf("[apps:install] Uninstalled %s\n", event.AppID)
}

// handleRevoke quarantines a revoked app — stops it immediately but preserves
// data/ for forensic analysis. Different from uninstall: the binary is removed
// so it can never re-launch, but all app data is preserved for investigation.
// This is the "kill switch" — NeboLoop detected a security issue and needs
// this app stopped on ALL instances immediately.
func (il *InstallListener) handleRevoke(event installEvent) {
	if err := il.registry.Quarantine(event.AppID); err != nil {
		fmt.Printf("[apps:install] Warning: quarantine failed for %s: %v\n", event.AppID, err)
	}
}

// downloadAndExtract downloads a .napp from the URL and extracts it to destDir.
func (il *InstallListener) downloadAndExtract(downloadURL, destDir string) error {
	// Download to temp file
	resp, err := http.Get(downloadURL)
	if err != nil {
		return fmt.Errorf("download: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("download returned HTTP %d", resp.StatusCode)
	}

	// Limit download size (max binary 500MB + metadata overhead)
	maxDownloadSize := int64(600 * 1024 * 1024)

	tmpFile, err := os.CreateTemp("", "nebo-app-*.napp")
	if err != nil {
		return fmt.Errorf("create temp file: %w", err)
	}
	tmpPath := tmpFile.Name()
	defer os.Remove(tmpPath)

	written, err := io.Copy(tmpFile, io.LimitReader(resp.Body, maxDownloadSize+1))
	tmpFile.Close()
	if err != nil {
		return fmt.Errorf("download write: %w", err)
	}
	if written > maxDownloadSize {
		return fmt.Errorf("download too large (%d bytes, max %d)", written, maxDownloadSize)
	}

	// Create destination and extract
	if err := os.MkdirAll(destDir, 0700); err != nil {
		return fmt.Errorf("create app dir: %w", err)
	}

	if err := ExtractNapp(tmpPath, destDir); err != nil {
		return fmt.Errorf("extract: %w", err)
	}

	return nil
}

// downloadURL returns the download URL for an app, either from the event
// or constructed from the API server.
func (il *InstallListener) downloadURL(event installEvent) string {
	if event.DownloadURL != "" {
		return event.DownloadURL
	}

	il.mu.RLock()
	apiServer := il.config.APIServer
	il.mu.RUnlock()

	if apiServer == "" {
		return ""
	}

	return fmt.Sprintf("%s/api/v1/apps/%s/download?version=%s",
		strings.TrimRight(apiServer, "/"), event.AppID, event.Version)
}

// Stop disconnects from the MQTT broker and cleans up.
func (il *InstallListener) Stop() {
	il.mu.Lock()
	defer il.mu.Unlock()

	if il.cancel != nil {
		il.cancel()
	}

	if il.cm != nil {
		select {
		case <-il.cm.Done():
		case <-time.After(5 * time.Second):
		}
		il.cm = nil
	}

	il.connected = false
	fmt.Printf("[apps:install] Stopped\n")
}

// IsRunning returns true if the listener is connected to the MQTT broker.
func (il *InstallListener) IsRunning() bool {
	il.mu.RLock()
	defer il.mu.RUnlock()
	return il.connected
}

// brokerToInstallURL converts a broker address to a *url.URL for autopaho.
// Same logic as the comm plugin's brokerToURL.
func brokerToInstallURL(broker string) (*url.URL, error) {
	if !strings.Contains(broker, "://") {
		broker = "mqtt://" + broker
	}

	u, err := url.Parse(broker)
	if err != nil {
		return nil, err
	}

	switch u.Scheme {
	case "tcp":
		u.Scheme = "mqtt"
	case "ssl", "tls":
		u.Scheme = "mqtts"
	case "mqtt", "mqtts", "ws", "wss":
		// Already valid
	default:
		return nil, fmt.Errorf("unsupported scheme: %s", u.Scheme)
	}

	return u, nil
}

// dirExists returns true if the path exists and is a directory.
func dirExists(path string) bool {
	info, err := os.Stat(path)
	return err == nil && info.IsDir()
}
