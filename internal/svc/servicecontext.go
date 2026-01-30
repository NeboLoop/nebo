package svc

import (
	"os"
	"path/filepath"

	"nebo/internal/agenthub"
	"nebo/internal/config"
	"nebo/internal/db"
	"nebo/internal/local"
	"nebo/internal/middleware"
	"nebo/internal/provider"

	"nebo/internal/logging"
)

type ServiceContext struct {
	Config             config.Config
	SecurityMiddleware *middleware.SecurityMiddleware

	DB             *db.Store
	Auth           *local.AuthService
	Email          *local.EmailService
	AgentSettings  *local.AgentSettingsStore
	SkillSettings  *local.SkillSettingsStore

	AgentHub *agenthub.Hub
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

	// Initialize models store (loads ~/.nebo/models.yaml singleton)
	home, _ := os.UserHomeDir()
	gobotDir := filepath.Join(home, ".nebo")
	provider.InitModelsStore(gobotDir)
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
