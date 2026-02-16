package mcp

import (
	"fmt"
	"net/http"
	"strings"
	"sync"

	"github.com/neboloop/nebo/internal/mcp/mcpauth"
	"github.com/neboloop/nebo/internal/mcp/mcpctx"
	"github.com/neboloop/nebo/internal/svc"

	"github.com/google/uuid"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

// Handler handles MCP HTTP requests with Bearer token authentication.
type Handler struct {
	svc             *svc.ServiceContext
	authenticator   *mcpauth.Authenticator
	httpHandler     http.Handler
	resourceMetaURL string

	// sessionCache stores MCP servers + ToolContext by session ID (in-memory).
	sessionCache sync.Map // map[sessionID]*sessionData
}

// sessionData holds cached session data.
type sessionData struct {
	server  *mcp.Server
	toolCtx *mcpctx.ToolContext
}

// NewHandler creates a new MCP handler with authentication.
// baseURL is used to construct the resource metadata URL for OAuth discovery.
func NewHandler(svc *svc.ServiceContext, baseURL string) *Handler {
	baseURL = strings.TrimSuffix(baseURL, "/")

	h := &Handler{
		svc:             svc,
		authenticator:   mcpauth.NewAuthenticator(svc),
		resourceMetaURL: baseURL + "/.well-known/oauth-protected-resource",
	}

	// Create the streamable HTTP handler in STATELESS mode.
	// Stateless mode means the SDK doesn't validate session IDs - we handle it ourselves.
	streamHandler := mcp.NewStreamableHTTPHandler(
		h.getServerForRequest,
		&mcp.StreamableHTTPOptions{
			Stateless: true, // Don't track sessions in SDK - we do it ourselves
		},
	)

	// Wrap with our custom auth middleware that properly formats WWW-Authenticate
	h.httpHandler = h.authMiddleware(streamHandler)

	return h
}

// authMiddleware validates Bearer tokens and returns proper OAuth challenge on 401.
// This custom implementation ensures WWW-Authenticate header is RFC 9728 compliant
// with quoted resource_metadata value.
// It also ensures session IDs are generated and communicated back to the client.
func (h *Handler) authMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Check for session ID - generate one if not provided
		sessionID := r.Header.Get("Mcp-Session-Id")
		newSession := false
		if sessionID == "" {
			sessionID = uuid.New().String()
			newSession = true
			// Add session ID to request so getServerForRequest can use it
			r.Header.Set("Mcp-Session-Id", sessionID)
		}

		fmt.Printf("[MCP DEBUG] %s %s | Session: %q (new: %v) | Accept: %s\n",
			r.Method, r.URL.Path, sessionID, newSession, r.Header.Get("Accept"))

		// Extract Bearer token
		authHeader := r.Header.Get("Authorization")
		if authHeader == "" || !strings.HasPrefix(authHeader, "Bearer ") {
			h.writeUnauthorized(w, "missing bearer token")
			return
		}

		token := strings.TrimPrefix(authHeader, "Bearer ")
		if token == "" {
			h.writeUnauthorized(w, "empty bearer token")
			return
		}

		// Verify token using our authenticator (TokenVerifier returns a func)
		tokenInfo, err := h.authenticator.TokenVerifier()(r.Context(), token, r)
		if err != nil {
			h.writeUnauthorized(w, "invalid token")
			return
		}

		// Always set session ID in response header so client can use it for subsequent requests
		// This is critical for MCP session continuity - the client needs to know the session ID
		w.Header().Set("Mcp-Session-Id", sessionID)

		// Add token info to request context and continue
		ctx := mcpauth.ContextWithTokenInfo(r.Context(), tokenInfo)
		next.ServeHTTP(w, r.WithContext(ctx))
	})
}

// writeUnauthorized sends a 401 response with WWW-Authenticate header for OAuth discovery.
// Uses simple format matching working MCP OAuth implementations.
func (h *Handler) writeUnauthorized(w http.ResponseWriter, msg string) {
	// Bearer challenge with resource_metadata URL and scope
	wwwAuth := fmt.Sprintf(`Bearer resource_metadata="%s", scope="mcp:full"`, h.resourceMetaURL)
	w.Header().Set("WWW-Authenticate", wwwAuth)
	http.Error(w, "Unauthorized", http.StatusUnauthorized)
}

// getServerForRequest returns a cached server for the session, or creates a new one.
// We cache by SESSION ID to support multiple clients per user.
func (h *Handler) getServerForRequest(r *http.Request) *mcp.Server {
	// Session ID is always set by authMiddleware (generated if not provided by client)
	sessionID := r.Header.Get("Mcp-Session-Id")

	// Get token info from context (set by authMiddleware)
	tokenInfo := mcpauth.TokenInfoFromContext(r.Context())
	if tokenInfo == nil {
		fmt.Println("[MCP] ERROR: No token info in context for getServerForRequest")
		return NewServer(h.svc, r)
	}

	// Check cache first - if we have this session, reuse it
	if cached, ok := h.sessionCache.Load(sessionID); ok {
		data := cached.(*sessionData)
		fmt.Printf("[MCP] Using cached session: %s\n", sessionID)
		return data.server
	}

	// Session not in cache - create a new server
	fmt.Printf("[MCP] Session %s not in cache - creating new server\n", sessionID)
	server, toolCtx := NewServerWithContext(h.svc, r)

	// Cache the session
	h.sessionCache.Store(sessionID, &sessionData{server: server, toolCtx: toolCtx})

	return server
}

// ServeHTTP handles all MCP HTTP requests.
func (h *Handler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	h.httpHandler.ServeHTTP(w, r)
}

// Authenticator returns the authenticator for cache invalidation.
func (h *Handler) Authenticator() *mcpauth.Authenticator {
	return h.authenticator
}
