package server

import (
	"context"
	"fmt"
	"io"
	"io/fs"
	"net"
	"net/http"
	"net/http/httputil"
	"net/url"
	"strings"
	"time"

	"gobot/app"
	"gobot/internal/agenthub"
	"gobot/internal/channels"
	"gobot/internal/config"
	"gobot/internal/db"
	"gobot/internal/handler"
	"gobot/internal/mcp"
	mcpoauth "gobot/internal/mcp/oauth"
	"gobot/internal/middleware"
	"gobot/internal/oauth"
	"gobot/internal/realtime"
	"gobot/internal/router"
	"gobot/internal/svc"
	"gobot/internal/websocket"

	"github.com/zeromicro/go-zero/rest"
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
	serverPort := c.Port           // User-facing port
	backendPort := c.Port + 1      // Internal go-zero port

	// Check if user-facing port is available
	if err := checkPortAvailable(serverPort); err != nil {
		return fmt.Errorf("port %d is already in use - only one GoBot instance allowed per computer", serverPort)
	}

	// Set go-zero to use internal backend port
	c.RestConf.Port = backendPort

	if !opts.Quiet {
		fmt.Printf("Starting server on http://localhost:%d\n", serverPort)
	}

	app.SetServerHost("localhost", serverPort, false)

	spaFS, err := app.FileSystem()
	if err != nil {
		fmt.Printf("Warning: Could not load embedded SPA files: %v\n", err)
		fmt.Println("Run 'cd app && pnpm build' to build the frontend")
	}

	var serverOpts []rest.RunOption
	if err == nil {
		serverOpts = append(serverOpts,
			rest.WithNotFoundHandler(app.NotFoundHandler(spaFS)),
		)
	}

	// Disable access logging in quiet mode for clean CLI output
	if opts.Quiet {
		serverOpts = append(serverOpts, rest.WithCustomCors(nil, nil, "*"))
		c.RestConf.Log.Mode = "console"
		c.RestConf.Log.Level = "severe"
		c.RestConf.Log.Stat = false
	}

	server := rest.MustNewServer(c.RestConf, serverOpts...)
	defer server.Stop()

	// Use pre-initialized database if provided, otherwise create new
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

	server.Use(func(next http.HandlerFunc) http.HandlerFunc {
		return func(w http.ResponseWriter, r *http.Request) {
			if c.IsSecurityHeadersEnabled() {
				headers := middleware.APISecurityHeaders()
				w.Header().Set("Content-Security-Policy", headers.ContentSecurityPolicy)
				w.Header().Set("X-Content-Type-Options", headers.XContentTypeOptions)
				w.Header().Set("X-Frame-Options", headers.XFrameOptions)
				w.Header().Set("X-XSS-Protection", headers.XXSSProtection)
				w.Header().Set("Referrer-Policy", headers.ReferrerPolicy)
				w.Header().Set("Permissions-Policy", headers.PermissionsPolicy)
				w.Header().Set("Cache-Control", headers.CacheControl)
				w.Header().Set("Pragma", headers.Pragma)
			}
			next(w, r)
		}
	})

	server.AddRoute(rest.Route{
		Method:  http.MethodGet,
		Path:    "/api/v1/csrf-token",
		Handler: svcCtx.SecurityMiddleware.GetCSRFTokenHandler(),
	})

	handler.RegisterHandlers(server, svcCtx)

	if svcCtx.UseLocal() && c.IsOAuthEnabled() {
		oauthHandler := oauth.NewHandler(svcCtx)
		oauthHandler.RegisterRoutes(http.DefaultServeMux)
		if !opts.Quiet {
			fmt.Println("OAuth callbacks registered at /oauth/{provider}/callback")
		}
	}

	if svcCtx.UseLocal() {
		baseURL := fmt.Sprintf("http://localhost:%d", serverPort)

		mcpHandler := mcp.NewHandler(svcCtx, baseURL)
		http.DefaultServeMux.Handle("/mcp", mcpHandler)
		http.DefaultServeMux.Handle("/mcp/", mcpHandler)

		mcpOAuthHandler := mcpoauth.NewHandler(svcCtx, baseURL)
		mcpOAuthHandler.RegisterRoutes(http.DefaultServeMux)
	}

	hub := realtime.NewHub()
	go hub.Run(ctx)

	go svcCtx.AgentHub.Run(ctx)

	// Initialize chat context and register chat handler
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

	server.AddRoute(rest.Route{
		Method:  http.MethodGet,
		Path:    "/ws",
		Handler: websocket.Handler(hub),
	})

	server.AddRoute(rest.Route{
		Method:  http.MethodGet,
		Path:    "/api/v1/agent/ws",
		Handler: agentWebSocketHandler(svcCtx),
	})

	// Run server with proxy
	return runServer(ctx, c, spaFS, err, serverPort, backendPort, server, opts.Quiet)
}

func runServer(ctx context.Context, c config.Config, spaFS fs.FS, spaErr error, serverPort int, backendPort int, server *rest.Server, quiet bool) error {
	// Start go-zero backend on internal port
	go func() {
		server.Start()
	}()

	// Create reverse proxy to backend
	backendURL, _ := url.Parse(fmt.Sprintf("http://%s:%d", c.Host, backendPort))
	proxy := httputil.NewSingleHostReverseProxy(backendURL)

	originalDirector := proxy.Director
	proxy.Director = func(req *http.Request) {
		originalDirector(req)
		if req.Header.Get("Upgrade") != "" {
			req.Header.Set("Connection", "Upgrade")
		}
	}

	proxy.Transport = &http.Transport{
		MaxIdleConns:        100,
		MaxIdleConnsPerHost: 100,
		IdleConnTimeout:     90 * time.Second,
		DisableCompression:  true,
		WriteBufferSize:     32 << 10,
		ReadBufferSize:      32 << 10,
	}

	proxy.ErrorHandler = func(w http.ResponseWriter, r *http.Request, err error) {
		fmt.Printf("Proxy error: %v\n", err)
		http.Error(w, "Backend temporarily unavailable", http.StatusBadGateway)
	}

	// Main handler that serves SPA and proxies API
	mainHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Proxy API routes
		if strings.HasPrefix(r.URL.Path, "/api/") {
			proxy.ServeHTTP(w, r)
			return
		}

		// Proxy webhooks
		if strings.HasPrefix(r.URL.Path, "/webhooks/") {
			proxy.ServeHTTP(w, r)
			return
		}

		// OAuth routes
		if strings.HasPrefix(r.URL.Path, "/oauth/") {
			http.DefaultServeMux.ServeHTTP(w, r)
			return
		}

		// MCP routes
		if strings.HasPrefix(r.URL.Path, "/mcp") || strings.HasPrefix(r.URL.Path, "/.well-known/oauth-") {
			http.DefaultServeMux.ServeHTTP(w, r)
			return
		}

		// WebSocket routes
		if strings.HasPrefix(r.URL.Path, "/ws") || strings.HasPrefix(r.URL.Path, "/api/v1/agent/ws") {
			proxyWebSocket(w, r, c.Host, backendPort)
			return
		}

		// Serve SPA for everything else
		if spaErr == nil {
			app.SPAHandler(spaFS).ServeHTTP(w, r)
		} else {
			http.Error(w, "SPA not available - run 'cd app && pnpm build' first", http.StatusServiceUnavailable)
		}
	})

	handler := middleware.Gzip(middleware.CacheControl(mainHandler))

	httpServer := &http.Server{
		Addr:         fmt.Sprintf(":%d", serverPort),
		Handler:      handler,
		ReadTimeout:  10 * time.Second,
		WriteTimeout: 30 * time.Second,
		IdleTimeout:  120 * time.Second,
	}

	if !quiet {
		fmt.Printf("Server ready at http://localhost:%d\n", serverPort)
	}

	go func() {
		if err := httpServer.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			fmt.Printf("HTTP server error: %v\n", err)
		}
	}()

	<-ctx.Done()

	if !quiet {
		fmt.Println("\nShutting down server gracefully...")
	}
	shutdownCtx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	httpServer.Shutdown(shutdownCtx)
	return nil
}

func agentWebSocketHandler(ctx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		agentID := "gobot-agent"
		ctx.AgentHub.HandleWebSocket(w, r, agentID)
	}
}

func proxyWebSocket(w http.ResponseWriter, r *http.Request, backendHost string, backendPort int) {
	hijacker, ok := w.(http.Hijacker)
	if !ok {
		http.Error(w, "WebSocket not supported", http.StatusInternalServerError)
		return
	}

	backendAddr := fmt.Sprintf("%s:%d", backendHost, backendPort)
	backendConn, err := net.Dial("tcp", backendAddr)
	if err != nil {
		http.Error(w, "Backend unavailable", http.StatusBadGateway)
		return
	}
	defer backendConn.Close()

	clientConn, clientBuf, err := hijacker.Hijack()
	if err != nil {
		http.Error(w, "Hijack failed", http.StatusInternalServerError)
		return
	}
	defer clientConn.Close()

	if err := r.Write(backendConn); err != nil {
		return
	}

	if clientBuf.Reader.Buffered() > 0 {
		buffered := make([]byte, clientBuf.Reader.Buffered())
		clientBuf.Read(buffered)
		backendConn.Write(buffered)
	}

	done := make(chan struct{}, 2)
	go func() {
		io.Copy(backendConn, clientConn)
		done <- struct{}{}
	}()
	go func() {
		io.Copy(clientConn, backendConn)
		done <- struct{}{}
	}()
	<-done
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
