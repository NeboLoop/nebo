package svc

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"sync/atomic"

	"github.com/neboloop/nebo/internal/agent/ai"
	"github.com/neboloop/nebo/internal/agenthub"
	"github.com/neboloop/nebo/internal/config"
	"github.com/neboloop/nebo/internal/credential"
	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/defaults"
	"github.com/neboloop/nebo/internal/local"
	mcpclient "github.com/neboloop/nebo/internal/mcp/client"
	"github.com/neboloop/nebo/internal/middleware"
	"github.com/neboloop/nebo/internal/oauth/broker"
	"github.com/neboloop/nebo/internal/apps/settings"
	"github.com/neboloop/nebo/internal/provider"

	"github.com/neboloop/nebo/internal/logging"
)

// AppUIProvider is the interface for accessing app UI capabilities.
// Implemented by apps.AppRegistry — defined here to avoid circular imports.
type AppUIProvider interface {
	// HandleRequest proxies an HTTP request to a UI app and returns the response.
	HandleRequest(ctx context.Context, appID string, req *AppHTTPRequest) (*AppHTTPResponse, error)
	// ListUIApps returns metadata about apps that provide UI.
	ListUIApps() []AppUIInfo
	// AppsDir returns the base directory where apps are installed.
	AppsDir() string
}

// AppHTTPRequest represents an HTTP request to proxy to a UI app.
type AppHTTPRequest struct {
	Method  string
	Path    string
	Query   string
	Headers map[string]string
	Body    []byte
}

// AppHTTPResponse represents an HTTP response from a UI app.
type AppHTTPResponse struct {
	StatusCode int
	Headers    map[string]string
	Body       []byte
}

// AppUIInfo describes a UI-capable app (returned by ListUIApps).
type AppUIInfo struct {
	ID      string `json:"id"`
	Name    string `json:"name"`
	Version string `json:"version"`
}

type ServiceContext struct {
	Config             config.Config
	SecurityMiddleware *middleware.SecurityMiddleware
	NeboDir            string // Root Nebo data directory (e.g. ~/Library/Application Support/Nebo)
	Version            string // Build version (e.g. "v0.2.0" or "dev")

	DB             *db.Store
	Auth           *local.AuthService
	Email          *local.EmailService
	SkillSettings  *local.SkillSettingsStore
	PluginStore    *settings.Store

	AgentHub    *agenthub.Hub
	MCPClient   *mcpclient.Client
	OAuthBroker *broker.Broker

	appUI       AppUIProvider
	appUIMu     sync.RWMutex
	appRegistry  any // apps.AppRegistry (use any to avoid import cycle)
	appRegMu     sync.RWMutex
	toolRegistry any // tools.Registry (use any to avoid import cycle)
	toolRegMu    sync.RWMutex
	scheduler    any // tools.Scheduler (use any to avoid import cycle)
	schedulerMu  sync.RWMutex

	JanusUsage atomic.Pointer[ai.RateLimitInfo] // Latest Janus rate limit (also persisted to janus_usage.json)

	browseDir   func() (string, error)   // Native directory picker (desktop only)
	browseDirMu sync.RWMutex
	browseFiles func() ([]string, error) // Native file picker (desktop only)
	browseFilesMu sync.RWMutex

	openDevWindow   func() // Open dev window (desktop only)
	openDevWindowMu sync.RWMutex

	openPopup   func(url, title string, width, height int) // Open popup window (desktop only)
	openPopupMu sync.RWMutex

	updateMgr   *UpdateMgr
	updateMgrMu sync.RWMutex

	clientHub   any // realtime.Hub (use any to avoid import cycle)
	clientHubMu sync.RWMutex
}

// UpdateMgr tracks a pending auto-update binary path (in-memory only).
type UpdateMgr struct {
	mu          sync.Mutex
	pendingPath string
	version     string
}

// SetPending records a verified binary ready for installation.
func (u *UpdateMgr) SetPending(path, version string) {
	u.mu.Lock()
	defer u.mu.Unlock()
	u.pendingPath = path
	u.version = version
}

// PendingPath returns the path to the pending binary, or "" if none.
func (u *UpdateMgr) PendingPath() string {
	u.mu.Lock()
	defer u.mu.Unlock()
	return u.pendingPath
}

// PendingVersion returns the version of the pending update, or "" if none.
func (u *UpdateMgr) PendingVersion() string {
	u.mu.Lock()
	defer u.mu.Unlock()
	return u.version
}

// Clear removes the pending update.
func (u *UpdateMgr) Clear() {
	u.mu.Lock()
	defer u.mu.Unlock()
	u.pendingPath = ""
	u.version = ""
}

// SetAppUIProvider installs the app UI provider (called from agent.go after registry init).
func (svc *ServiceContext) SetAppUIProvider(p AppUIProvider) {
	svc.appUIMu.Lock()
	defer svc.appUIMu.Unlock()
	svc.appUI = p
}

// AppUI returns the current app UI provider (may be nil before agent starts).
func (svc *ServiceContext) AppUI() AppUIProvider {
	svc.appUIMu.RLock()
	defer svc.appUIMu.RUnlock()
	return svc.appUI
}

// SetAppRegistry installs the app registry (called from agent.go after registry init).
func (svc *ServiceContext) SetAppRegistry(r any) {
	svc.appRegMu.Lock()
	defer svc.appRegMu.Unlock()
	svc.appRegistry = r
}

// AppRegistry returns the current app registry (may be nil before agent starts).
func (svc *ServiceContext) AppRegistry() any {
	svc.appRegMu.RLock()
	defer svc.appRegMu.RUnlock()
	return svc.appRegistry
}

// SetToolRegistry installs the tool registry (called from agent.go after registry init).
func (svc *ServiceContext) SetToolRegistry(r any) {
	svc.toolRegMu.Lock()
	defer svc.toolRegMu.Unlock()
	svc.toolRegistry = r
}

// ToolRegistry returns the current tool registry (may be nil before agent starts).
func (svc *ServiceContext) ToolRegistry() any {
	svc.toolRegMu.RLock()
	defer svc.toolRegMu.RUnlock()
	return svc.toolRegistry
}

// SetScheduler installs the scheduler provider (called from agent.go after scheduler init).
func (svc *ServiceContext) SetScheduler(s any) {
	svc.schedulerMu.Lock()
	defer svc.schedulerMu.Unlock()
	svc.scheduler = s
}

// Scheduler returns the current scheduler (may be nil before agent starts).
func (svc *ServiceContext) Scheduler() any {
	svc.schedulerMu.RLock()
	defer svc.schedulerMu.RUnlock()
	return svc.scheduler
}

// SetBrowseDirectory installs the native directory picker callback (desktop mode only).
func (svc *ServiceContext) SetBrowseDirectory(fn func() (string, error)) {
	svc.browseDirMu.Lock()
	defer svc.browseDirMu.Unlock()
	svc.browseDir = fn
}

// BrowseDirectory returns the native directory picker callback, or nil if not in desktop mode.
func (svc *ServiceContext) BrowseDirectory() func() (string, error) {
	svc.browseDirMu.RLock()
	defer svc.browseDirMu.RUnlock()
	return svc.browseDir
}

// SetBrowseFiles installs the native file picker callback (desktop mode only).
func (svc *ServiceContext) SetBrowseFiles(fn func() ([]string, error)) {
	svc.browseFilesMu.Lock()
	defer svc.browseFilesMu.Unlock()
	svc.browseFiles = fn
}

// BrowseFiles returns the native file picker callback, or nil if not in desktop mode.
func (svc *ServiceContext) BrowseFiles() func() ([]string, error) {
	svc.browseFilesMu.RLock()
	defer svc.browseFilesMu.RUnlock()
	return svc.browseFiles
}

// SetOpenDevWindow installs the dev window opener callback (desktop mode only).
func (svc *ServiceContext) SetOpenDevWindow(fn func()) {
	svc.openDevWindowMu.Lock()
	defer svc.openDevWindowMu.Unlock()
	svc.openDevWindow = fn
}

// OpenDevWindow returns the dev window opener callback, or nil if not in desktop mode.
func (svc *ServiceContext) OpenDevWindow() func() {
	svc.openDevWindowMu.RLock()
	defer svc.openDevWindowMu.RUnlock()
	return svc.openDevWindow
}

// SetUpdateManager installs the update manager.
func (svc *ServiceContext) SetUpdateManager(m *UpdateMgr) {
	svc.updateMgrMu.Lock()
	defer svc.updateMgrMu.Unlock()
	svc.updateMgr = m
}

// UpdateManager returns the current update manager (may be nil).
func (svc *ServiceContext) UpdateManager() *UpdateMgr {
	svc.updateMgrMu.RLock()
	defer svc.updateMgrMu.RUnlock()
	return svc.updateMgr
}

// SetClientHub installs the browser WebSocket hub (called from server.go after hub init).
func (svc *ServiceContext) SetClientHub(h any) {
	svc.clientHubMu.Lock()
	defer svc.clientHubMu.Unlock()
	svc.clientHub = h
}

// ClientHub returns the browser WebSocket hub (may be nil before server starts).
func (svc *ServiceContext) ClientHub() any {
	svc.clientHubMu.RLock()
	defer svc.clientHubMu.RUnlock()
	return svc.clientHub
}

// SetOpenPopup installs the popup window opener callback (desktop mode only).
func (svc *ServiceContext) SetOpenPopup(fn func(url, title string, width, height int)) {
	svc.openPopupMu.Lock()
	defer svc.openPopupMu.Unlock()
	svc.openPopup = fn
}

// OpenPopup returns the popup window opener callback, or nil if not in desktop mode.
func (svc *ServiceContext) OpenPopup() func(url, title string, width, height int) {
	svc.openPopupMu.RLock()
	defer svc.openPopupMu.RUnlock()
	return svc.openPopup
}

// NewServiceContext creates a new service context. Pass a *db.Store to reuse
// an existing database connection, or nil to create a new one.
func NewServiceContext(c config.Config, database ...*db.Store) *ServiceContext {
	var db0 *db.Store
	if len(database) > 0 {
		db0 = database[0]
	}
	return newServiceContext(c, db0)
}

func newServiceContext(c config.Config, database *db.Store) *ServiceContext {
	securityMw := middleware.NewSecurityMiddleware(c)
	logging.Info("Security middleware initialized")

	// Get data directory from SQLite path
	dataDir := filepath.Dir(c.Database.SQLitePath)
	if dataDir == "" {
		dataDir = "."
	}

	// Ensure data directory exists with default files (models.yaml, config.yaml, etc.)
	neboDir, err := defaults.EnsureDataDir()
	if err != nil {
		logging.Errorf("Failed to ensure data directory: %v", err)
		neboDir, _ = defaults.DataDir()
	}

	// Initialize models store (loads models.yaml singleton)
	provider.InitModelsStore(neboDir)
	logging.Info("Models store initialized")

	svc := &ServiceContext{
		Config:             c,
		SecurityMiddleware: securityMw,
		NeboDir:            neboDir,
		AgentHub:           agenthub.NewHub(),
		SkillSettings:      local.NewSkillSettingsStore(dataDir),
	}

	emailService := local.NewEmailService(c)
	if emailService.IsConfigured() {
		svc.Email = emailService
		logging.Info("Email service initialized")
	} else {
		logging.Info("Email not configured - transactional emails disabled")
	}

	// Use provided database or create new one
	if database != nil {
		svc.DB = database
		logging.Info("Using shared database connection")
	} else {
		var err error
		database, err = db.NewSQLite(c.Database.SQLitePath)
		if err != nil {
			logging.Errorf("Failed to initialize SQLite database: %v", err)
		} else {
			svc.DB = database
			logging.Infof("SQLite database initialized at %s", c.Database.SQLitePath)
		}
	}

	if svc.DB != nil {
		local.InitSettings(svc.DB.GetDB())
		logging.Info("Agent settings singleton initialized")

		svc.Auth = local.NewAuthService(svc.DB, c)
		logging.Info("Auth service initialized")

		svc.PluginStore = settings.NewStore(svc.DB.GetDB())
		logging.Info("Plugin store initialized")

		// Broadcast plugin settings changes to connected agents/UI
		svc.PluginStore.OnChange(func(pluginName string, _ map[string]string) {
			svc.AgentHub.Broadcast(&agenthub.Frame{
				Type:   "event",
				Method: "plugin_settings_updated",
				Payload: map[string]any{
					"plugin": pluginName,
				},
			})
			logging.Infof("Plugin settings updated: %s", pluginName)
		})

		// Initialize MCP OAuth client
		encKey, err := mcpclient.GetEncryptionKey(neboDir)
		if err != nil {
			logging.Warnf("MCP encryption key not configured: %v", err)
		}
		credential.Init(encKey)

		// Encrypt any plaintext credentials from before the encryption feature
		if err := credential.Migrate(context.Background(), svc.DB.GetDB()); err != nil {
			logging.Errorf("Credential migration failed: %v", err)
		}

		baseURL := fmt.Sprintf("http://localhost:%d", c.Port)
		svc.MCPClient = mcpclient.NewClient(svc.DB, encKey, baseURL)
		logging.Info("MCP OAuth client initialized")

		// Initialize App OAuth Broker
		brokerProviders := broker.BuiltinProviders()
		for name, provCfg := range c.AppOAuth {
			if p, ok := brokerProviders[name]; ok {
				p.ClientID = provCfg.ClientID
				p.ClientSecret = provCfg.ClientSecret
				if provCfg.TenantID != "" {
					p.TenantID = provCfg.TenantID
				}
				brokerProviders[name] = p
			}
		}
		svc.OAuthBroker = broker.New(broker.Config{
			DB:            svc.DB,
			EncryptionKey: encKey,
			BaseURL:       baseURL,
			Providers:     brokerProviders,
		})
		logging.Info("App OAuth broker initialized")
	}

	// Restore Janus rate-limit data from previous session
	svc.LoadJanusUsage()

	return svc
}

func (svc *ServiceContext) Close() {
	if svc.DB != nil {
		svc.DB.Close()
		logging.Info("SQLite database connection closed")
	}
	logging.Info("Service context closed")
}

func (svc *ServiceContext) UseLocal() bool {
	return svc.DB != nil
}

// janusUsagePath returns the path to janus_usage.json inside the data directory.
func (svc *ServiceContext) janusUsagePath() string {
	return filepath.Join(svc.NeboDir, "janus_usage.json")
}

// SaveJanusUsage persists the current in-memory rate-limit snapshot to disk.
func (svc *ServiceContext) SaveJanusUsage() {
	rl := svc.JanusUsage.Load()
	if rl == nil {
		return
	}
	data, err := json.Marshal(rl)
	if err != nil {
		logging.Errorf("Failed to marshal Janus usage: %v", err)
		return
	}
	if err := os.WriteFile(svc.janusUsagePath(), data, 0600); err != nil {
		logging.Errorf("Failed to save Janus usage: %v", err)
	}
}

// LoadJanusUsage restores Janus rate-limit data from disk into the in-memory pointer.
func (svc *ServiceContext) LoadJanusUsage() {
	data, err := os.ReadFile(svc.janusUsagePath())
	if err != nil {
		return // File doesn't exist yet — normal on first run
	}
	var rl ai.RateLimitInfo
	if err := json.Unmarshal(data, &rl); err != nil {
		logging.Errorf("Failed to parse janus_usage.json: %v", err)
		return
	}
	svc.JanusUsage.Store(&rl)
	logging.Infof("Loaded Janus usage from disk (weekly %d/%d tokens)",
		rl.WeeklyRemainingTokens, rl.WeeklyLimitTokens)
}
