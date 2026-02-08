package svc

import (
	"context"
	"fmt"
	"path/filepath"
	"sync"

	"github.com/nebolabs/nebo/internal/agenthub"
	"github.com/nebolabs/nebo/internal/config"
	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/defaults"
	"github.com/nebolabs/nebo/internal/local"
	mcpclient "github.com/nebolabs/nebo/internal/mcp/client"
	"github.com/nebolabs/nebo/internal/middleware"
	"github.com/nebolabs/nebo/internal/plugin"
	"github.com/nebolabs/nebo/internal/provider"

	"github.com/nebolabs/nebo/internal/logging"
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

	DB             *db.Store
	Auth           *local.AuthService
	Email          *local.EmailService
	AgentSettings  *local.AgentSettingsStore
	SkillSettings  *local.SkillSettingsStore
	PluginStore    *plugin.Store

	AgentHub  *agenthub.Hub
	MCPClient *mcpclient.Client

	appUI   AppUIProvider
	appUIMu sync.RWMutex
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

// NewServiceContext creates a new service context, initializing database if not provided
func NewServiceContext(c config.Config) *ServiceContext {
	return NewServiceContextWithDB(c, nil)
}

// NewServiceContextWithDB creates a new service context with an optional pre-initialized database
func NewServiceContextWithDB(c config.Config, database *db.Store) *ServiceContext {
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
		AgentHub:           agenthub.NewHub(),
		AgentSettings:      local.NewAgentSettingsStore(dataDir),
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
		svc.Auth = local.NewAuthService(svc.DB, c)
		logging.Info("Auth service initialized")

		svc.PluginStore = plugin.NewStore(svc.DB.GetDB())
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
		encKey, err := mcpclient.GetEncryptionKey()
		if err != nil {
			logging.Warnf("MCP encryption key not configured: %v", err)
		}
		baseURL := fmt.Sprintf("http://localhost:%d", c.Port)
		svc.MCPClient = mcpclient.NewClient(svc.DB, encKey, baseURL)
		logging.Info("MCP OAuth client initialized")
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
