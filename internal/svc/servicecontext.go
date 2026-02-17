package svc

import (
	"context"
	"fmt"
	"path/filepath"
	"sync"

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
	// GetUIView fetches the current view from a UI app.
	GetUIView(ctx context.Context, appID string) (any, error)
	// SendUIEvent sends a user interaction event to a UI app.
	SendUIEvent(ctx context.Context, appID string, event any) (any, error)
	// ListUIApps returns metadata about apps that provide UI.
	ListUIApps() []AppUIInfo
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

	browseDir   func() (string, error) // Native directory picker (desktop only)
	browseDirMu sync.RWMutex

	openDevWindow   func() // Open dev window (desktop only)
	openDevWindowMu sync.RWMutex

	openPopup   func(url, title string, width, height int) // Open popup window (desktop only)
	openPopupMu sync.RWMutex
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
