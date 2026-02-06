package mcp

import (
	"fmt"
	"net/http"

	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/mcp/mcpauth"
	"github.com/nebolabs/nebo/internal/mcp/mcpctx"
	"github.com/nebolabs/nebo/internal/mcp/tools"
	"github.com/nebolabs/nebo/internal/svc"

	"github.com/google/uuid"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

// NewServer creates a new user-scoped MCP server with all tools registered.
// This is a convenience wrapper around NewServerWithContext that discards the toolCtx.
func NewServer(svc *svc.ServiceContext, r *http.Request) *mcp.Server {
	server, _ := NewServerWithContext(svc, r)
	return server
}

// NewServerWithContext creates a new MCP server and returns both the server and the ToolContext.
// The ToolContext is returned so the caller can cache it for session persistence.
func NewServerWithContext(svc *svc.ServiceContext, r *http.Request) (*mcp.Server, *mcpctx.ToolContext) {
	server := mcp.NewServer(&mcp.Implementation{
		Name:    "nebo",
		Version: "1.0.0",
	}, nil)

	// Get token info from our custom auth middleware
	tokenInfo := mcpauth.TokenInfoFromContext(r.Context())
	if tokenInfo == nil {
		// No auth context - return server without tools
		fmt.Println("[MCP] No token info in context - returning server without tools")
		return server, nil
	}

	// Extract user info from token
	userInfo, ok := tokenInfo.Extra["user_info"].(*mcpauth.UserInfo)
	if !ok {
		fmt.Println("[MCP] Failed to extract user_info from token")
		return server, nil
	}

	user, ok := tokenInfo.Extra["user"].(*db.User)
	if !ok {
		fmt.Println("[MCP] Failed to extract user from token")
		return server, nil
	}

	sessionID := r.Header.Get("Mcp-Session-Id")
	fmt.Printf("[MCP] Creating server for user: %s (ID: %s, Session: %s)\n", user.Email, user.ID, sessionID)

	// Generate request ID for tracing
	requestID := uuid.New().String()
	userAgent := r.Header.Get("User-Agent")

	// Create user-scoped tool context
	toolCtx := mcpctx.NewToolContext(svc, *user, requestID, userAgent, sessionID)

	// Store additional user info in context for tools that need it
	_ = userInfo // Available via toolCtx.User() methods

	// Register all tools (unified resource/action pattern)
	tools.RegisterUserTool(server, toolCtx)
	tools.RegisterNotificationTool(server, toolCtx)
	tools.RegisterMemoryTool(server, toolCtx)

	return server, toolCtx
}
