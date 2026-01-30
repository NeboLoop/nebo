package server

import (
	"context"
	"fmt"
	"io/fs"
	"net"
	"net/http"
	"strings"
	"time"

	"github.com/go-chi/chi/v5"
	chimw "github.com/go-chi/chi/v5/middleware"

	"nebo/app"
	"nebo/internal/agenthub"
	"nebo/internal/channels"
	"nebo/internal/config"
	"nebo/internal/db"
	"nebo/internal/handler"
	"nebo/internal/handler/agent"
	"nebo/internal/handler/auth"
	"nebo/internal/handler/chat"
	"nebo/internal/handler/extensions"
	"nebo/internal/handler/notification"
	"nebo/internal/handler/oauth"
	"nebo/internal/handler/provider"
	"nebo/internal/handler/setup"
	"nebo/internal/handler/user"
	"nebo/internal/mcp"
	mcpoauth "nebo/internal/mcp/oauth"
	"nebo/internal/middleware"
	extOAuth "nebo/internal/oauth"
	"nebo/internal/realtime"
	"nebo/internal/router"
	"nebo/internal/svc"
	"nebo/internal/voice"
	"nebo/internal/websocket"
)

// ServerOptions holds optional dependencies for the server
type ServerOptions struct {
	ChannelManager *channels.Manager
	AgentHub       *agenthub.Hub // Shared agent hub for single binary mode
	Database       *db.Store     // Pre-initialized database (optional, for single binary mode)
	Quiet          bool          // Suppress startup messages for clean CLI output
}

// Run starts the GoBot server with the given configuration.
// It blocks until the context is cancelled or an error occurs.
func Run(ctx context.Context, c config.Config) error {
	return RunWithOptions(ctx, c, ServerOptions{})
}

// RunWithOptions starts the server with optional shared dependencies
func RunWithOptions(ctx context.Context, c config.Config, opts ServerOptions) error {
	serverPort := c.Port

	// Check if port is available
	if err := checkPortAvailable(serverPort); err != nil {
		return fmt.Errorf("port %d is already in use - only one GoBot instance allowed per computer", serverPort)
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

	// Initialize service context
	var svcCtx *svc.ServiceContext
	if opts.Database != nil {
		svcCtx = svc.NewServiceContextWithDB(c, opts.Database)
	} else {
		svcCtx = svc.NewServiceContext(c)
	}
	defer svcCtx.Close()

	// Use shared AgentHub if provided (single binary mode)
	if opts.AgentHub != nil {
		svcCtx.AgentHub = opts.AgentHub
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

	// API v1 routes - apply strict security headers only to API
	r.Route("/api/v1", func(r chi.Router) {
		if c.IsSecurityHeadersEnabled() {
			r.Use(securityHeadersMiddleware())
		}
		// CSRF token endpoint
		r.Get("/csrf-token", svcCtx.SecurityMiddleware.GetCSRFTokenHandler())

		// Voice endpoints
		r.Post("/voice/transcribe", voice.TranscribeHandler)
		r.Post("/voice/tts", voice.TTSHandler)
		r.Get("/voice/voices", voice.VoicesHandler)

		// Public routes (no auth required)
		registerPublicRoutes(r, svcCtx)

		// Protected routes (JWT required)
		r.Group(func(r chi.Router) {
			r.Use(middleware.JWTMiddleware(svcCtx.Config.Auth.AccessSecret))
			registerProtectedRoutes(r, svcCtx)
		})
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

	// Initialize message router for channel → agent routing
	channelMgr := opts.ChannelManager
	if channelMgr == nil {
		channelMgr = channels.NewManager()
	}
	msgRouter := router.NewRouter(channelMgr, svcCtx.AgentHub)
	_ = msgRouter

	rewriteHandler := realtime.NewRewriteHandler(svcCtx)
	rewriteHandler.Register()

	r.Get("/ws", websocket.Handler(hub))
	r.Get("/api/v1/agent/ws", agentWebSocketHandler(svcCtx))

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

	// SPA fallback - serve frontend for all other routes
	if spaErr == nil {
		r.NotFound(spaHandler(spaFS))
	}

	// Apply compression and cache control
	finalHandler := middleware.Gzip(middleware.CacheControl(r))

	// Create and start HTTP server
	httpServer := &http.Server{
		Addr:         fmt.Sprintf(":%d", serverPort),
		Handler:      finalHandler,
		ReadTimeout:  10 * time.Second,
		WriteTimeout: 30 * time.Second,
		IdleTimeout:  120 * time.Second,
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

// registerPublicRoutes registers routes that don't require authentication
func registerPublicRoutes(r chi.Router, svcCtx *svc.ServiceContext) {
	// Auth routes
	r.Get("/auth/config", auth.GetAuthConfigHandler(svcCtx))
	r.Get("/auth/dev-login", auth.DevLoginHandler(svcCtx))
	r.Post("/auth/forgot-password", auth.ForgotPasswordHandler(svcCtx))
	r.Post("/auth/login", auth.LoginHandler(svcCtx))
	r.Post("/auth/refresh", auth.RefreshTokenHandler(svcCtx))
	r.Post("/auth/register", auth.RegisterHandler(svcCtx))
	r.Post("/auth/resend-verification", auth.ResendVerificationHandler(svcCtx))
	r.Post("/auth/reset-password", auth.ResetPasswordHandler(svcCtx))
	r.Post("/auth/verify-email", auth.VerifyEmailHandler(svcCtx))

	// Setup routes
	r.Post("/setup/admin", setup.CreateAdminHandler(svcCtx))
	r.Post("/setup/complete", setup.CompleteSetupHandler(svcCtx))
	r.Get("/setup/personality", setup.GetPersonalityHandler(svcCtx))
	r.Put("/setup/personality", setup.UpdatePersonalityHandler(svcCtx))
	r.Get("/setup/status", setup.SetupStatusHandler(svcCtx))

	// OAuth routes (public for initial auth flow)
	r.Post("/oauth/{provider}/callback", oauth.OAuthCallbackHandler(svcCtx))
	r.Get("/oauth/{provider}/url", oauth.GetOAuthUrlHandler(svcCtx))

	// Agent routes
	r.Get("/agent/sessions", agent.ListAgentSessionsHandler(svcCtx))
	r.Delete("/agent/sessions/{id}", agent.DeleteAgentSessionHandler(svcCtx))
	r.Get("/agent/sessions/{id}/messages", agent.GetAgentSessionMessagesHandler(svcCtx))
	r.Get("/agent/settings", agent.GetAgentSettingsHandler(svcCtx))
	r.Put("/agent/settings", agent.UpdateAgentSettingsHandler(svcCtx))
	r.Get("/agent/heartbeat", agent.GetHeartbeatHandler(svcCtx))
	r.Put("/agent/heartbeat", agent.UpdateHeartbeatHandler(svcCtx))
	r.Get("/agent/status", agent.GetSimpleAgentStatusHandler(svcCtx))
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
	r.Get("/skills/{name}", extensions.GetSkillHandler(svcCtx))
	r.Post("/skills/{name}/toggle", extensions.ToggleSkillHandler(svcCtx))

	// Provider/Models routes
	r.Get("/models", provider.ListModelsHandler(svcCtx))
	r.Put("/models/{provider}/{modelId}", provider.UpdateModelHandler(svcCtx))
	r.Put("/models/task-routing", provider.UpdateTaskRoutingHandler(svcCtx))
	r.Get("/providers", provider.ListAuthProfilesHandler(svcCtx))
	r.Post("/providers", provider.CreateAuthProfileHandler(svcCtx))
	r.Get("/providers/{id}", provider.GetAuthProfileHandler(svcCtx))
	r.Put("/providers/{id}", provider.UpdateAuthProfileHandler(svcCtx))
	r.Delete("/providers/{id}", provider.DeleteAuthProfileHandler(svcCtx))
	r.Post("/providers/{id}/test", provider.TestAuthProfileHandler(svcCtx))
}

// registerProtectedRoutes registers routes that require JWT authentication
func registerProtectedRoutes(r chi.Router, svcCtx *svc.ServiceContext) {
	// User profile routes
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

// corsMiddleware handles CORS
func corsMiddleware() func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Access-Control-Allow-Origin", "*")
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

// spaHandler serves the SPA for non-API routes
func spaHandler(spaFS fs.FS) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		// Try to serve the file directly
		path := strings.TrimPrefix(r.URL.Path, "/")
		if path == "" {
			path = "index.html"
		}

		// Check if file exists (static assets, prerendered pages)
		if _, err := fs.Stat(spaFS, path); err == nil {
			http.FileServer(http.FS(spaFS)).ServeHTTP(w, r)
			return
		}

		// Fallback to 200.html for SPA client-side routing
		// SvelteKit adapter-static generates 200.html as the SPA fallback
		// (index.html is prerendered and would show the wrong page)
		fallbackFile, err := spaFS.Open("200.html")
		if err != nil {
			// If 200.html doesn't exist, try index.html as last resort
			fallbackFile, err = spaFS.Open("index.html")
			if err != nil {
				http.Error(w, "SPA not available", http.StatusNotFound)
				return
			}
		}
		defer fallbackFile.Close()

		stat, _ := fallbackFile.Stat()
		http.ServeContent(w, r, "200.html", stat.ModTime(), fallbackFile.(interface {
			Read([]byte) (int, error)
			Seek(int64, int) (int64, error)
		}))
	}
}

func agentWebSocketHandler(ctx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		agentID := "gobot-agent"
		ctx.AgentHub.HandleWebSocket(w, r, agentID)
	}
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
