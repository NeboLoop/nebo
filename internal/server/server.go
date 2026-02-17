package server

import (
	"context"
	"fmt"
	"net"
	"net/http"
	"sync"
	"time"

	"github.com/go-chi/chi/v5"
	chimw "github.com/go-chi/chi/v5/middleware"

	"github.com/neboloop/nebo/app"
	"github.com/neboloop/nebo/internal/browser"
	"github.com/neboloop/nebo/internal/config"
	"github.com/neboloop/nebo/internal/handler"
	"github.com/neboloop/nebo/internal/handler/agent"
	"github.com/neboloop/nebo/internal/handler/appoauth"
	"github.com/neboloop/nebo/internal/handler/appui"
	"github.com/neboloop/nebo/internal/handler/auth"
	"github.com/neboloop/nebo/internal/handler/chat"
	"github.com/neboloop/nebo/internal/handler/dev"
	"github.com/neboloop/nebo/internal/handler/extensions"
	"github.com/neboloop/nebo/internal/handler/files"
	"github.com/neboloop/nebo/internal/handler/integration"
	"github.com/neboloop/nebo/internal/handler/memory"
	"github.com/neboloop/nebo/internal/handler/neboloop"
	"github.com/neboloop/nebo/internal/handler/notification"
	"github.com/neboloop/nebo/internal/handler/oauth"
	"github.com/neboloop/nebo/internal/handler/plugins"
	"github.com/neboloop/nebo/internal/handler/provider"
	"github.com/neboloop/nebo/internal/handler/setup"
	"github.com/neboloop/nebo/internal/handler/tasks"
	"github.com/neboloop/nebo/internal/handler/user"
	"github.com/neboloop/nebo/internal/mcp"
	mcpoauth "github.com/neboloop/nebo/internal/mcp/oauth"
	"github.com/neboloop/nebo/internal/middleware"
	extOAuth "github.com/neboloop/nebo/internal/oauth"
	"github.com/neboloop/nebo/internal/realtime"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/voice"
	"github.com/neboloop/nebo/internal/webview"
	"github.com/neboloop/nebo/internal/websocket"
)

// ServerOptions holds optional dependencies for the server
type ServerOptions struct {
	SvcCtx          *svc.ServiceContext // Pre-initialized service context (single binary mode)
	Quiet           bool                // Suppress startup messages for clean CLI output
	AgentMCPHandler *AgentMCPProxy      // Lazy handler for agent MCP tools at /agent/mcp
	DevMode         bool                // Enable developer routes (desktop mode only)
}

// AgentMCPProxy is a lazy http.Handler that serves 503 until the real handler is set.
// This allows the HTTP server to mount /agent/mcp before the agent MCP server is ready.
type AgentMCPProxy struct {
	mu      sync.RWMutex
	handler http.Handler
}

// NewAgentMCPProxy creates a new lazy proxy for the agent MCP handler.
func NewAgentMCPProxy() *AgentMCPProxy {
	return &AgentMCPProxy{}
}

// Set installs the real MCP handler once the agent is initialized.
func (p *AgentMCPProxy) Set(h http.Handler) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.handler = h
}

// ServeHTTP delegates to the real handler, or returns 503 if not yet set.
func (p *AgentMCPProxy) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	p.mu.RLock()
	h := p.handler
	p.mu.RUnlock()
	if h == nil {
		http.Error(w, "Agent MCP server not ready", http.StatusServiceUnavailable)
		return
	}
	h.ServeHTTP(w, r)
}

// Run starts the Nebo server with the given configuration.
// It blocks until the context is cancelled or an error occurs.
// Pass a ServerOptions to configure shared dependencies, dev mode, etc.
func Run(ctx context.Context, c config.Config, opts ...ServerOptions) error {
	var o ServerOptions
	if len(opts) > 0 {
		o = opts[0]
	}
	return run(ctx, c, o)
}

func run(ctx context.Context, c config.Config, opts ServerOptions) error {
	serverPort := c.Port

	// Check if port is available
	if err := checkPortAvailable(serverPort); err != nil {
		return fmt.Errorf("port %d is already in use - only one Nebo instance allowed per computer", serverPort)
	}

	if !opts.Quiet {
		fmt.Printf("Starting server on http://localhost:%d\n", serverPort)
	}

	app.SetServerHost("localhost", serverPort, false)

	// Load embedded SPA files
	spaFS, spaErr := app.FileSystem()
	if spaErr != nil {
		fmt.Printf("Warning: Could not load embedded SPA files: %v\n", spaErr)
		fmt.Println("Run 'cd app && pnpm build' to build the frontend")
	}

	// Use pre-initialized service context if provided, otherwise create one
	var svcCtx *svc.ServiceContext
	if opts.SvcCtx != nil {
		svcCtx = opts.SvcCtx
	} else {
		svcCtx = svc.NewServiceContext(c)
		defer svcCtx.Close()
	}

	// Create chi router
	r := chi.NewRouter()

	// Global middleware
	if !opts.Quiet {
		r.Use(chimw.Logger)
	}
	r.Use(chimw.Recoverer)
	r.Use(chimw.RealIP)

	// CORS middleware
	r.Use(corsMiddleware())

	// Health check at root
	r.Get("/health", handler.HealthCheckHandler(svcCtx))
	r.Get("/api/v1/update/check", handler.UpdateCheckHandler(svcCtx))

	// Rate limiters
	authLimiter := middleware.NewRateLimiter(middleware.AuthRateLimitConfig())
	apiLimiter := middleware.NewRateLimiter(middleware.APIRateLimitConfig())

	// API v1 routes - apply strict security headers only to API
	r.Route("/api/v1", func(r chi.Router) {
		if c.IsSecurityHeadersEnabled() {
			r.Use(securityHeadersMiddleware())
		}

		// Apply general API rate limit to all /api/v1 routes
		r.Use(apiLimiter.Middleware())

		// CSRF token endpoint
		r.Get("/csrf-token", svcCtx.SecurityMiddleware.GetCSRFTokenHandler())

		// Voice endpoints
		r.Post("/voice/transcribe", voice.TranscribeHandler)
		r.Post("/voice/tts", voice.TTSHandler)
		r.Get("/voice/voices", voice.VoicesHandler)

		// Auth routes with stricter rate limiting
		r.Group(func(r chi.Router) {
			r.Use(authLimiter.Middleware())
			registerAuthRoutes(r, svcCtx)
		})

		// Public routes (no auth required)
		registerPublicRoutes(r, svcCtx)

		// Protected routes (JWT required)
		r.Group(func(r chi.Router) {
			r.Use(middleware.JWTMiddleware(svcCtx.Config.Auth.AccessSecret))
			registerProtectedRoutes(r, svcCtx)
		})

		// Developer mode routes (desktop only)
		if opts.DevMode {
			registerDevRoutes(r, svcCtx)
		}
	})

	// WebSocket routes
	hub := realtime.NewHub()
	go hub.Run(ctx)
	go svcCtx.AgentHub.Run(ctx)

	// Initialize chat context
	chatCtx, err := realtime.NewChatContext(svcCtx, hub)
	if err != nil {
		return fmt.Errorf("failed to create chat context: %w", err)
	}
	chatCtx.SetHub(svcCtx.AgentHub)
	realtime.RegisterChatHandler(chatCtx)

	rewriteHandler := realtime.NewRewriteHandler(svcCtx)
	rewriteHandler.Register()

	r.Get("/ws", websocket.Handler(hub))
	r.Get("/api/v1/agent/ws", agentWebSocketHandler(svcCtx))

	// Browser relay for Chrome extension
	relayBaseURL := fmt.Sprintf("http://%s:%d/relay", c.App.Domain, serverPort)
	browserRelay, err := browser.NewRelayHandler(relayBaseURL)
	if err != nil {
		fmt.Printf("Warning: failed to create browser relay: %v\n", err)
	} else {
		// Register relay routes
		r.Get("/relay", browserRelay.HandleRoot)
		r.Head("/relay", browserRelay.HandleRoot)
		r.Get("/relay/extension/status", browserRelay.HandleExtensionStatus)
		r.Get("/relay/extension/token", browserRelay.HandleExtensionToken)
		r.Get("/relay/json/version", browserRelay.HandleJSONVersion)
		r.Get("/relay/json", browserRelay.HandleJSONList)
		r.Get("/relay/json/list", browserRelay.HandleJSONList)
		r.Get("/relay/json/activate/{targetId}", browserRelay.HandleJSONActivate)
		r.Get("/relay/json/close/{targetId}", browserRelay.HandleJSONClose)
		r.HandleFunc("/relay/extension", browserRelay.HandleExtensionWS)
		r.HandleFunc("/relay/cdp", browserRelay.HandleCdpWS)
		if !opts.Quiet {
			fmt.Println("Browser relay mounted at /relay")
		}
	}

	// OAuth routes (external provider callbacks)
	if svcCtx.UseLocal() && c.IsOAuthEnabled() {
		oauthHandler := extOAuth.NewHandler(svcCtx)
		oauthHandler.RegisterRoutes(r)
		if !opts.Quiet {
			fmt.Println("OAuth callbacks registered at /oauth/{provider}/callback")
		}
	}

	// MCP routes
	if svcCtx.UseLocal() {
		baseURL := fmt.Sprintf("http://localhost:%d", serverPort)
		mcpHandler := mcp.NewHandler(svcCtx, baseURL)
		r.Handle("/mcp", mcpHandler)
		r.Handle("/mcp/*", mcpHandler)

		mcpOAuthHandler := mcpoauth.NewHandler(svcCtx, baseURL)
		mcpOAuthHandler.RegisterRoutes(r)
	}

	// Agent MCP routes - exposes STRAP tools to Claude CLI via MCP protocol.
	// The proxy starts returning 503 until the agent sets the real handler.
	if opts.AgentMCPHandler != nil {
		r.Handle("/agent/mcp", opts.AgentMCPHandler)
		r.Handle("/agent/mcp/*", opts.AgentMCPHandler)
	}

	// Native webview callback — used by agent-controlled browser windows
	// to send DOM interaction results back to Go via fetch().
	// HandleFunc so both POST and OPTIONS (CORS preflight) reach the handler.
	handler := webview.CallbackHandler()
	r.Post("/internal/webview/callback", handler)
	r.Options("/internal/webview/callback", handler)

	// SPA fallback - serve frontend for all other routes
	if spaErr == nil {
		r.NotFound(app.SPAHandler(spaFS).ServeHTTP)
	}

	// Apply compression and cache control
	finalHandler := middleware.Gzip(middleware.CacheControl(r))

	// Create and start HTTP server
	// Note: ReadTimeout/WriteTimeout are intentionally omitted — they set deadlines
	// on the underlying net.Conn which interfere with hijacked WebSocket connections.
	// WebSocket keepalive is handled via ping/pong in agenthub and realtime packages.
	httpServer := &http.Server{
		Addr:        fmt.Sprintf(":%d", serverPort),
		Handler:     finalHandler,
		IdleTimeout: 120 * time.Second,
	}

	if !opts.Quiet {
		fmt.Printf("Server ready at http://localhost:%d\n", serverPort)
	}

	// Start server in goroutine
	go func() {
		if err := httpServer.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			fmt.Printf("HTTP server error: %v\n", err)
		}
	}()

	// Wait for context cancellation
	<-ctx.Done()

	if !opts.Quiet {
		fmt.Println("\nShutting down server gracefully...")
	}
	shutdownCtx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	httpServer.Shutdown(shutdownCtx)
	return nil
}

// registerAuthRoutes registers auth routes with stricter rate limiting
func registerAuthRoutes(r chi.Router, svcCtx *svc.ServiceContext) {
	r.Get("/auth/config", auth.GetAuthConfigHandler(svcCtx))
	r.Get("/auth/dev-login", auth.DevLoginHandler(svcCtx))
	r.Post("/auth/forgot-password", auth.ForgotPasswordHandler(svcCtx))
	r.Post("/auth/login", auth.LoginHandler(svcCtx))
	r.Post("/auth/refresh", auth.RefreshTokenHandler(svcCtx))
	r.Post("/auth/register", auth.RegisterHandler(svcCtx))
	r.Post("/auth/resend-verification", auth.ResendVerificationHandler(svcCtx))
	r.Post("/auth/reset-password", auth.ResetPasswordHandler(svcCtx))
	r.Post("/auth/verify-email", auth.VerifyEmailHandler(svcCtx))
}

// registerPublicRoutes registers routes that don't require authentication
func registerPublicRoutes(r chi.Router, svcCtx *svc.ServiceContext) {
	// Setup routes
	r.Post("/setup/admin", setup.CreateAdminHandler(svcCtx))
	r.Post("/setup/complete", setup.CompleteSetupHandler(svcCtx))
	r.Get("/setup/personality", setup.GetPersonalityHandler(svcCtx))
	r.Put("/setup/personality", setup.UpdatePersonalityHandler(svcCtx))
	r.Get("/setup/status", setup.SetupStatusHandler(svcCtx))

	// OAuth routes (public for initial auth flow)
	r.Post("/oauth/{provider}/callback", oauth.OAuthCallbackHandler(svcCtx))
	r.Get("/oauth/{provider}/url", oauth.GetOAuthUrlHandler(svcCtx))

	// NeboLoop account routes (public — entry point for NeboLoop auth)
	r.Post("/neboloop/register", neboloop.NeboLoopRegisterHandler(svcCtx))
	r.Post("/neboloop/login", neboloop.NeboLoopLoginHandler(svcCtx))
	r.Get("/neboloop/account", neboloop.NeboLoopAccountStatusHandler(svcCtx))
	r.Delete("/neboloop/account", neboloop.NeboLoopDisconnectHandler(svcCtx))

	// Agent routes
	r.Get("/agent/sessions", agent.ListAgentSessionsHandler(svcCtx))
	r.Delete("/agent/sessions/{id}", agent.DeleteAgentSessionHandler(svcCtx))
	r.Get("/agent/sessions/{id}/messages", agent.GetAgentSessionMessagesHandler(svcCtx))
	r.Get("/agent/settings", agent.GetAgentSettingsHandler(svcCtx))
	r.Put("/agent/settings", agent.UpdateAgentSettingsHandler(svcCtx))
	r.Get("/agent/heartbeat", agent.GetHeartbeatHandler(svcCtx))
	r.Put("/agent/heartbeat", agent.UpdateHeartbeatHandler(svcCtx))
	r.Get("/agent/advisors", agent.ListAdvisorsHandler(svcCtx))
	r.Get("/agent/advisors/{name}", agent.GetAdvisorHandler(svcCtx))
	r.Post("/agent/advisors", agent.CreateAdvisorHandler(svcCtx))
	r.Put("/agent/advisors/{name}", agent.UpdateAdvisorHandler(svcCtx))
	r.Delete("/agent/advisors/{name}", agent.DeleteAdvisorHandler(svcCtx))
	r.Get("/agent/status", agent.GetSimpleAgentStatusHandler(svcCtx))
	r.Get("/agent/lanes", agent.GetLanesHandler(svcCtx))
	r.Get("/agent/profile", agent.GetAgentProfileHandler(svcCtx))
	r.Put("/agent/profile", agent.UpdateAgentProfileHandler(svcCtx))
	r.Get("/agent/system-info", agent.GetSystemInfoHandler(svcCtx))
	r.Get("/agent/personality-presets", agent.ListPersonalityPresetsHandler(svcCtx))
	r.Get("/agents", agent.ListAgentsHandler(svcCtx))
	r.Get("/agents/{agentId}/status", agent.GetAgentStatusHandler(svcCtx))

	// Chat routes
	r.Get("/chats", chat.ListChatsHandler(svcCtx))
	r.Post("/chats", chat.CreateChatHandler(svcCtx))
	r.Get("/chats/companion", chat.GetCompanionChatHandler(svcCtx))
	r.Get("/chats/days", chat.ListChatDaysHandler(svcCtx))
	r.Get("/chats/history/{day}", chat.GetHistoryByDayHandler(svcCtx))
	r.Post("/chats/message", chat.SendMessageHandler(svcCtx))
	r.Get("/chats/search", chat.SearchChatMessagesHandler(svcCtx))
	r.Get("/chats/{id}", chat.GetChatHandler(svcCtx))
	r.Put("/chats/{id}", chat.UpdateChatHandler(svcCtx))
	r.Delete("/chats/{id}", chat.DeleteChatHandler(svcCtx))

	// Extensions routes
	r.Get("/extensions", extensions.ListExtensionsHandler(svcCtx))
	r.Post("/skills", extensions.CreateSkillHandler(svcCtx))
	r.Get("/skills/{name}", extensions.GetSkillHandler(svcCtx))
	r.Get("/skills/{name}/content", extensions.GetSkillContentHandler(svcCtx))
	r.Put("/skills/{name}", extensions.UpdateSkillHandler(svcCtx))
	r.Delete("/skills/{name}", extensions.DeleteSkillHandler(svcCtx))
	r.Post("/skills/{name}/toggle", extensions.ToggleSkillHandler(svcCtx))

	// Memory routes
	r.Get("/memories", memory.ListMemoriesHandler(svcCtx))
	r.Get("/memories/search", memory.SearchMemoriesHandler(svcCtx))
	r.Get("/memories/stats", memory.GetMemoryStatsHandler(svcCtx))
	r.Get("/memories/{id}", memory.GetMemoryHandler(svcCtx))
	r.Put("/memories/{id}", memory.UpdateMemoryHandler(svcCtx))
	r.Delete("/memories/{id}", memory.DeleteMemoryHandler(svcCtx))

	// Task routes (name-based addressing via Scheduler interface)
	r.Get("/tasks", tasks.ListTasksHandler(svcCtx))
	r.Post("/tasks", tasks.CreateTaskHandler(svcCtx))
	r.Get("/tasks/{name}", tasks.GetTaskHandler(svcCtx))
	r.Put("/tasks/{name}", tasks.UpdateTaskHandler(svcCtx))
	r.Delete("/tasks/{name}", tasks.DeleteTaskHandler(svcCtx))
	r.Post("/tasks/{name}/toggle", tasks.ToggleTaskHandler(svcCtx))
	r.Post("/tasks/{name}/run", tasks.RunTaskHandler(svcCtx))
	r.Get("/tasks/{name}/history", tasks.ListTaskHistoryHandler(svcCtx))

	// MCP Integration routes
	r.Get("/integrations", integration.ListMCPIntegrationsHandler(svcCtx))
	r.Get("/integrations/registry", integration.ListMCPServerRegistryHandler(svcCtx))
	r.Get("/integrations/tools", integration.ListMCPToolsHandler(svcCtx))
	r.Post("/integrations", integration.CreateMCPIntegrationHandler(svcCtx))
	r.Get("/integrations/{id}", integration.GetMCPIntegrationHandler(svcCtx))
	r.Put("/integrations/{id}", integration.UpdateMCPIntegrationHandler(svcCtx))
	r.Delete("/integrations/{id}", integration.DeleteMCPIntegrationHandler(svcCtx))
	r.Post("/integrations/{id}/test", integration.TestMCPIntegrationHandler(svcCtx))
	r.Get("/integrations/{id}/oauth-url", integration.GetMCPOAuthURLHandler(svcCtx))
	r.Post("/integrations/{id}/disconnect", integration.DisconnectMCPIntegrationHandler(svcCtx))
	r.Get("/integrations/oauth/callback", integration.OAuthCallbackHandler(svcCtx, fmt.Sprintf("http://localhost:%d", svcCtx.Config.Port)))

	// Plugin settings routes
	r.Get("/plugins", plugins.ListPluginsHandler(svcCtx))
	r.Get("/plugins/{id}", plugins.GetPluginHandler(svcCtx))
	r.Put("/plugins/{id}/settings", plugins.UpdatePluginSettingsHandler(svcCtx))
	r.Put("/plugins/{id}/toggle", plugins.TogglePluginHandler(svcCtx))

	// App UI routes (structured template rendering)
	r.Get("/apps/ui", appui.ListUIAppsHandler(svcCtx))
	r.Get("/apps/{id}/ui", appui.GetUIViewHandler(svcCtx))
	r.Post("/apps/{id}/ui/event", appui.SendUIEventHandler(svcCtx))

	// App OAuth broker routes
	if svcCtx.OAuthBroker != nil {
		r.Get("/apps/{appId}/oauth/{provider}/connect", appoauth.ConnectHandler(svcCtx.OAuthBroker))
		r.Get("/apps/oauth/callback", appoauth.CallbackHandler(svcCtx.OAuthBroker))
		r.Get("/apps/{appId}/oauth/grants", appoauth.GrantsHandler(svcCtx.OAuthBroker))
		r.Delete("/apps/{appId}/oauth/{provider}", appoauth.DisconnectHandler(svcCtx.OAuthBroker))
	}

	// NeboLoop store routes
	r.Get("/store/apps", plugins.ListStoreAppsHandler(svcCtx))
	r.Get("/store/apps/{id}", plugins.GetStoreAppHandler(svcCtx))
	r.Get("/store/apps/{id}/reviews", plugins.GetStoreAppReviewsHandler(svcCtx))
	r.Post("/store/apps/{id}/install", plugins.InstallStoreAppHandler(svcCtx))
	r.Delete("/store/apps/{id}/install", plugins.UninstallStoreAppHandler(svcCtx))
	r.Get("/store/skills", plugins.ListStoreSkillsHandler(svcCtx))
	r.Post("/store/skills/{id}/install", plugins.InstallStoreSkillHandler(svcCtx))
	r.Delete("/store/skills/{id}/install", plugins.UninstallStoreSkillHandler(svcCtx))

	// NeboLoop Connection routes (bot MQTT)
	r.Post("/neboloop/connect", plugins.NeboLoopConnectHandler(svcCtx))
	r.Get("/neboloop/status", plugins.NeboLoopStatusHandler(svcCtx))

	// Provider/Models routes
	r.Get("/models", provider.ListModelsHandler(svcCtx))
	r.Put("/models/config", provider.UpdateModelConfigHandler(svcCtx))
	r.Put("/models/{provider}/{modelId}", provider.UpdateModelHandler(svcCtx))
	r.Put("/models/task-routing", provider.UpdateTaskRoutingHandler(svcCtx))
	r.Get("/providers", provider.ListAuthProfilesHandler(svcCtx))
	r.Post("/providers", provider.CreateAuthProfileHandler(svcCtx))
	r.Get("/providers/{id}", provider.GetAuthProfileHandler(svcCtx))
	r.Put("/providers/{id}", provider.UpdateAuthProfileHandler(svcCtx))
	r.Delete("/providers/{id}", provider.DeleteAuthProfileHandler(svcCtx))
	r.Post("/providers/{id}/test", provider.TestAuthProfileHandler(svcCtx))

	// Agent files (screenshots, images, downloads)
	r.Get("/files/*", files.ServeFileHandler(svcCtx))

	// User profile routes (public for single-user personal assistant mode)
	r.Get("/user/me/profile", user.GetUserProfileHandler(svcCtx))
	r.Put("/user/me/profile", user.UpdateUserProfileHandler(svcCtx))

	// Tool permissions and terms (public for single-user mode)
	r.Get("/user/me/permissions", user.GetToolPermissionsHandler(svcCtx))
	r.Put("/user/me/permissions", user.UpdateToolPermissionsHandler(svcCtx))
	r.Post("/user/me/accept-terms", user.AcceptTermsHandler(svcCtx))
}

// registerProtectedRoutes registers routes that require JWT authentication
func registerProtectedRoutes(r chi.Router, svcCtx *svc.ServiceContext) {
	// User account routes (requires auth for multi-user scenarios)
	r.Get("/user/me", user.GetCurrentUserHandler(svcCtx))
	r.Put("/user/me", user.UpdateCurrentUserHandler(svcCtx))
	r.Delete("/user/me", user.DeleteAccountHandler(svcCtx))
	r.Post("/user/me/change-password", user.ChangePasswordHandler(svcCtx))
	r.Get("/user/me/preferences", user.GetPreferencesHandler(svcCtx))
	r.Put("/user/me/preferences", user.UpdatePreferencesHandler(svcCtx))

	// Notifications
	r.Get("/notifications", notification.ListNotificationsHandler(svcCtx))
	r.Delete("/notifications/{id}", notification.DeleteNotificationHandler(svcCtx))
	r.Put("/notifications/{id}/read", notification.MarkNotificationReadHandler(svcCtx))
	r.Put("/notifications/read-all", notification.MarkAllNotificationsReadHandler(svcCtx))
	r.Get("/notifications/unread-count", notification.GetUnreadCountHandler(svcCtx))

	// OAuth management (requires auth)
	r.Delete("/oauth/{provider}", oauth.DisconnectOAuthHandler(svcCtx))
	r.Get("/oauth/providers", oauth.ListOAuthProvidersHandler(svcCtx))
}

// securityHeadersMiddleware adds security headers to responses
func securityHeadersMiddleware() func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			headers := middleware.APISecurityHeaders()
			w.Header().Set("Content-Security-Policy", headers.ContentSecurityPolicy)
			w.Header().Set("X-Content-Type-Options", headers.XContentTypeOptions)
			w.Header().Set("X-Frame-Options", headers.XFrameOptions)
			w.Header().Set("X-XSS-Protection", headers.XXSSProtection)
			w.Header().Set("Referrer-Policy", headers.ReferrerPolicy)
			w.Header().Set("Permissions-Policy", headers.PermissionsPolicy)
			w.Header().Set("Cache-Control", headers.CacheControl)
			w.Header().Set("Pragma", headers.Pragma)
			next.ServeHTTP(w, r)
		})
	}
}

// corsMiddleware handles CORS — Nebo is a local app, so only localhost origins are allowed.
func corsMiddleware() func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			origin := r.Header.Get("Origin")

			// Allow localhost origins (any port) for the local SvelteKit dev server
			// and the embedded SPA. Also allow no origin (same-origin requests).
			if origin == "" || middleware.IsLocalhostOrigin(origin) {
				w.Header().Set("Access-Control-Allow-Origin", origin)
				w.Header().Set("Access-Control-Allow-Credentials", "true")
			}
			// Non-localhost origins get no CORS headers → browser blocks the request

			w.Header().Set("Access-Control-Allow-Methods", "GET, POST, PUT, DELETE, OPTIONS, PATCH")
			w.Header().Set("Access-Control-Allow-Headers", "Content-Type, Authorization, X-CSRF-Token")
			w.Header().Set("Access-Control-Expose-Headers", "X-CSRF-Token")

			if r.Method == "OPTIONS" {
				w.WriteHeader(http.StatusOK)
				return
			}

			next.ServeHTTP(w, r)
		})
	}
}


func agentWebSocketHandler(ctx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		agentID := "nebo-agent"
		ctx.AgentHub.HandleWebSocket(w, r, agentID)
	}
}

// registerDevRoutes registers developer-only routes (desktop mode).
func registerDevRoutes(r chi.Router, svcCtx *svc.ServiceContext) {
	r.Post("/dev/sideload", dev.SideloadHandler(svcCtx))
	r.Delete("/dev/sideload/{appId}", dev.UnsideloadHandler(svcCtx))
	r.Get("/dev/apps", dev.ListDevAppsHandler(svcCtx))
	r.Post("/dev/apps/{appId}/relaunch", dev.RelaunchDevAppHandler(svcCtx))
	r.Get("/dev/apps/{appId}/logs", dev.LogStreamHandler(svcCtx))
	r.Get("/dev/apps/{appId}/grpc", dev.GrpcStreamHandler(svcCtx))
	r.Get("/dev/apps/{appId}/context", dev.ProjectContextHandler(svcCtx))
	r.Get("/dev/tools", dev.ListToolsHandler(svcCtx))
	r.Post("/dev/tools/execute", dev.ToolExecuteHandler(svcCtx))
	r.Post("/dev/browse-directory", dev.BrowseDirectoryHandler(svcCtx))
	r.Post("/dev/open-window", dev.OpenDevWindowHandler(svcCtx))
}

// checkPortAvailable checks if a port is available for binding
func checkPortAvailable(port int) error {
	ln, err := net.Listen("tcp", fmt.Sprintf(":%d", port))
	if err != nil {
		return err
	}
	ln.Close()
	return nil
}
