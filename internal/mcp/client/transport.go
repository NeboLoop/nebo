package client

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"sync"
	"time"

	"github.com/modelcontextprotocol/go-sdk/mcp"

	"github.com/neboloop/nebo/internal/logging"
)

// AuthenticatedTransport wraps http.RoundTripper to add OAuth/API key authentication
type AuthenticatedTransport struct {
	Base          http.RoundTripper
	MCPClient     *Client
	IntegrationID string
}

// RoundTrip adds the Bearer token to requests
func (t *AuthenticatedTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	// Get access token (will refresh if needed)
	token, err := t.MCPClient.GetAccessToken(req.Context(), t.IntegrationID)
	if err != nil {
		return nil, fmt.Errorf("failed to get access token: %w", err)
	}

	// Clone the request and add authorization header
	req2 := req.Clone(req.Context())
	req2.Header.Set("Authorization", "Bearer "+token)

	return t.Base.RoundTrip(req2)
}

// sessionEntry holds a cached MCP client session for an integration.
type sessionEntry struct {
	session *mcp.ClientSession
}

// sessions caches live MCP client sessions per integration ID.
var (
	sessions   sync.Map // map[string]*sessionEntry
	sessionsMu sync.Mutex
)

// getOrCreateSession returns a cached session or creates a new one.
func (c *Client) getOrCreateSession(ctx context.Context, integrationID, serverURL, authType string) (*mcp.ClientSession, error) {
	// Fast path: check cache
	if entry, ok := sessions.Load(integrationID); ok {
		return entry.(*sessionEntry).session, nil
	}

	sessionsMu.Lock()
	defer sessionsMu.Unlock()

	// Double-check after acquiring lock
	if entry, ok := sessions.Load(integrationID); ok {
		return entry.(*sessionEntry).session, nil
	}

	// Only wrap with auth transport when credentials are expected
	var rt http.RoundTripper = http.DefaultTransport
	if authType != "" && authType != "none" {
		rt = &AuthenticatedTransport{
			Base:          http.DefaultTransport,
			MCPClient:     c,
			IntegrationID: integrationID,
		}
	}

	httpClient := &http.Client{
		Timeout:   60 * time.Second,
		Transport: rt,
	}

	transport := &mcp.StreamableClientTransport{
		Endpoint:   serverURL,
		HTTPClient: httpClient,
	}

	client := mcp.NewClient(&mcp.Implementation{
		Name:    "nebo",
		Version: "1.0.0",
	}, nil)

	session, err := client.Connect(ctx, transport, nil)
	if err != nil {
		return nil, fmt.Errorf("failed to connect to MCP server: %w", err)
	}

	sessions.Store(integrationID, &sessionEntry{session: session})
	logging.Infof("MCP session established for integration %s at %s", integrationID, serverURL)

	return session, nil
}

// CloseSession closes and removes a cached session for an integration.
func (c *Client) CloseSession(integrationID string) {
	if entry, ok := sessions.LoadAndDelete(integrationID); ok {
		se := entry.(*sessionEntry)
		se.session.Close()
		logging.Infof("MCP session closed for integration %s", integrationID)
	}
}

// CloseAllSessions closes all cached sessions.
func (c *Client) CloseAllSessions() {
	sessions.Range(func(key, value any) bool {
		se := value.(*sessionEntry)
		se.session.Close()
		sessions.Delete(key)
		return true
	})
}

// ListTools fetches available tools from an external MCP server via the SDK.
func (c *Client) ListTools(ctx context.Context, integrationID string) ([]*mcp.Tool, error) {
	integration, err := c.db.GetMCPIntegration(ctx, integrationID)
	if err != nil {
		return nil, fmt.Errorf("failed to get integration: %w", err)
	}

	serverURL := integration.ServerUrl.String
	if serverURL == "" {
		return nil, fmt.Errorf("no server URL configured")
	}

	session, err := c.getOrCreateSession(ctx, integrationID, serverURL, integration.AuthType)
	if err != nil {
		// Clear stale session on connect failure
		c.CloseSession(integrationID)
		return nil, err
	}

	result, err := session.ListTools(ctx, nil)
	if err != nil {
		// Session may be stale — close and retry once
		c.CloseSession(integrationID)
		session, err = c.getOrCreateSession(ctx, integrationID, serverURL, integration.AuthType)
		if err != nil {
			return nil, err
		}
		result, err = session.ListTools(ctx, nil)
		if err != nil {
			return nil, fmt.Errorf("failed to list tools: %w", err)
		}
	}

	return result.Tools, nil
}

// CallTool executes a tool on an external MCP server via the SDK.
func (c *Client) CallTool(ctx context.Context, integrationID, toolName string, input json.RawMessage) (*mcp.CallToolResult, error) {
	integration, err := c.db.GetMCPIntegration(ctx, integrationID)
	if err != nil {
		return nil, fmt.Errorf("failed to get integration: %w", err)
	}

	serverURL := integration.ServerUrl.String
	if serverURL == "" {
		return nil, fmt.Errorf("no server URL configured")
	}

	session, err := c.getOrCreateSession(ctx, integrationID, serverURL, integration.AuthType)
	if err != nil {
		return nil, err
	}

	// Convert json.RawMessage to map[string]any for the SDK
	var args map[string]any
	if len(input) > 0 {
		if err := json.Unmarshal(input, &args); err != nil {
			return nil, fmt.Errorf("failed to parse tool arguments: %w", err)
		}
	}

	logging.Infof("Calling MCP tool %s on %s", toolName, serverURL)

	result, err := session.CallTool(ctx, &mcp.CallToolParams{
		Name:      toolName,
		Arguments: args,
	})
	if err != nil {
		return nil, fmt.Errorf("failed to call tool %s: %w", toolName, err)
	}

	return result, nil
}
