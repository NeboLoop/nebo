package client

import (
	"context"
	"encoding/json"
	"fmt"
	"math/rand/v2"
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
	session       *mcp.ClientSession
	createdAt     time.Time
	lastHeartbeat time.Time
}

// sessions caches live MCP client sessions per integration ID.
var (
	sessions   sync.Map // map[string]*sessionEntry
	sessionsMu sync.Mutex
)

// healthCheckInterval defines how often we verify session health
const healthCheckInterval = 5 * time.Minute

// maxSessionAge defines the maximum lifetime of a session before forcing reconnect
const maxSessionAge = 30 * time.Minute

// isSessionHealthy checks if a cached session is still alive and valid.
// Returns true if the session should be reused, false if it needs reconnection.
func isSessionHealthy(entry *sessionEntry) bool {
	if entry == nil || entry.session == nil {
		return false
	}
	
	// Close session if it exceeded max age
	if time.Since(entry.createdAt) > maxSessionAge {
		return false
	}
	
	// Close session if it hasn't been used in 10 minutes (server may have dropped it)
	if time.Since(entry.lastHeartbeat) > 10*time.Minute {
		return false
	}
	
	return true
}

// getOrCreateSession returns a cached session or creates a new one.
func (c *Client) getOrCreateSession(ctx context.Context, integrationID, serverURL, authType string) (*mcp.ClientSession, error) {
	// Fast path: check cache and health
	if entry, ok := sessions.Load(integrationID); ok {
		se := entry.(*sessionEntry)
		if isSessionHealthy(se) {
			se.lastHeartbeat = time.Now()
			return se.session, nil
		}
		// Session is stale, close and remove it
		se.session.Close()
		sessions.Delete(integrationID)
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

	now := time.Now()
	sessions.Store(integrationID, &sessionEntry{
		session:       session,
		createdAt:     now,
		lastHeartbeat: now,
	})
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

// StartHealthChecker starts a background goroutine that periodically validates
// and reconnects MCP sessions. It checks every healthCheckInterval.
// This helps detect stale connections and reconnect before they fail user operations.
func (c *Client) StartHealthChecker(ctx context.Context) {
	go func() {
		ticker := time.NewTicker(healthCheckInterval)
		defer ticker.Stop()

		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				c.performHealthCheck(ctx)
			}
		}
	}()
	logging.Infof("MCP health checker started (interval: %v)", healthCheckInterval)
}

// performHealthCheck validates all cached sessions and reconnects stale ones.
func (c *Client) performHealthCheck(ctx context.Context) {
	sessions.Range(func(key, value any) bool {
		integrationID := key.(string)
		entry := value.(*sessionEntry)

		// Skip if session is still healthy
		if isSessionHealthy(entry) {
			return true
		}

		// Close the stale session
		entry.session.Close()
		sessions.Delete(integrationID)

		// Try to reconnect
		integration, err := c.db.GetMCPIntegration(ctx, integrationID)
		if err != nil {
			logging.Warnf("Health check: failed to get integration %s: %v", integrationID, err)
			return true
		}

		if !integration.ServerUrl.Valid || integration.ServerUrl.String == "" {
			return true
		}

		// Attempt reconnect
		_, err = c.getOrCreateSession(ctx, integrationID, integration.ServerUrl.String, integration.AuthType)
		if err != nil {
			logging.Warnf("Health check: failed to reconnect %s (%s): %v", integration.Name, integrationID, err)
		} else {
			logging.Infof("Health check: successfully reconnected %s (%s)", integration.Name, integrationID)
		}

		return true
	})
}

// ListTools fetches available tools from an external MCP server via the SDK.
// It implements automatic reconnection on failure.
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
		return nil, fmt.Errorf("failed to connect to MCP server: %w", err)
	}

	result, err := session.ListTools(ctx, nil)
	if err != nil {
		// Session may be stale or broken — close and retry once
		logging.Warnf("ListTools failed for %s, attempting reconnect: %v", integrationID, err)
		c.CloseSession(integrationID)
		
		session, err = c.getOrCreateSession(ctx, integrationID, serverURL, integration.AuthType)
		if err != nil {
			return nil, fmt.Errorf("failed to reconnect to MCP server: %w", err)
		}
		
		result, err = session.ListTools(ctx, nil)
		if err != nil {
			// Second attempt failed, mark as error
			return nil, fmt.Errorf("failed to list tools after reconnect: %w", err)
		}
	}

	return result.Tools, nil
}

// CallTool executes a tool on an external MCP server via the SDK.
// It implements automatic reconnection on failure.
func (c *Client) CallTool(ctx context.Context, integrationID, toolName string, input json.RawMessage) (*mcp.CallToolResult, error) {
	integration, err := c.db.GetMCPIntegration(ctx, integrationID)
	if err != nil {
		return nil, fmt.Errorf("failed to get integration: %w", err)
	}

	serverURL := integration.ServerUrl.String
	if serverURL == "" {
		return nil, fmt.Errorf("no server URL configured")
	}

	// Convert json.RawMessage to map[string]any for the SDK once
	var args map[string]any
	if len(input) > 0 {
		if err := json.Unmarshal(input, &args); err != nil {
			return nil, fmt.Errorf("failed to parse tool arguments: %w", err)
		}
	}

	logging.Infof("Calling MCP tool %s on %s", toolName, serverURL)

	// Retry with exponential backoff: never give up on transient errors
	// Only stop on context cancellation or if explicitly disabled
	// At scale (1M+ users), 60s would create thundering herd.
	// 10min allows graceful stagger across all clients.
	base := 100 * time.Millisecond
	maxDelay := 10 * time.Minute
	attempt := 0

	for {
		session, err := c.getOrCreateSession(ctx, integrationID, serverURL, integration.AuthType)
		if err != nil {
			// Connection error — close session and retry
			c.CloseSession(integrationID)
			logging.Warnf("Failed to get MCP session for %s, will retry: %v", integrationID, err)
		} else {
			// Got a session, try calling the tool
			result, err := session.CallTool(ctx, &mcp.CallToolParams{
				Name:      toolName,
				Arguments: args,
			})
			if err == nil {
				// Success
				return result, nil
			}
			// Tool call failed — session may be stale, close it and retry
			c.CloseSession(integrationID)
			logging.Warnf("CallTool failed for %s on %s, will retry: %v", toolName, integrationID, err)
		}

		// Calculate next retry delay with exponential backoff
		attempt++
		delay := base * time.Duration(1<<uint(min(attempt, 9))) // 2^9 = 512, max ~50s before cap
		if delay > maxDelay {
			delay = maxDelay
		}
		// Add jitter: ±25% of delay
		jitter := time.Duration(rand.Int64N(int64(delay) / 2))
		delay = delay - delay/4 + jitter

		logging.Infof("Retrying CallTool for %s, attempt %d, delay %v", toolName, attempt, delay)

		// Wait before retrying, respecting context cancellation
		select {
		case <-ctx.Done():
			return nil, fmt.Errorf("context cancelled while retrying tool call: %w", ctx.Err())
		case <-time.After(delay):
			// Continue to next attempt
		}
	}
}
