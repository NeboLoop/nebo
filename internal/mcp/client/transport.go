package client

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"

	"github.com/nebolabs/nebo/internal/logging"
)

// AuthenticatedTransport wraps http.RoundTripper to add OAuth authentication
type AuthenticatedTransport struct {
	Base          http.RoundTripper
	MCPClient     *Client
	IntegrationID string
}

// RoundTrip adds the OAuth Bearer token to requests
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

// MCPTool represents a tool exposed by an external MCP server
type MCPTool struct {
	Name        string          `json:"name"`
	Description string          `json:"description,omitempty"`
	InputSchema json.RawMessage `json:"inputSchema,omitempty"`
}

// MCPToolResult represents the result of a tool execution
type MCPToolResult struct {
	Content []MCPContent `json:"content"`
	IsError bool         `json:"isError,omitempty"`
}

// MCPContent represents content in an MCP response
type MCPContent struct {
	Type string `json:"type"`
	Text string `json:"text,omitempty"`
}

// ListTools fetches available tools from an external MCP server
func (c *Client) ListTools(ctx context.Context, integrationID string) ([]MCPTool, error) {
	integration, err := c.db.GetMCPIntegration(ctx, integrationID)
	if err != nil {
		return nil, fmt.Errorf("failed to get integration: %w", err)
	}

	serverURL := integration.ServerUrl.String
	if serverURL == "" {
		return nil, fmt.Errorf("no server URL configured")
	}

	// Create authenticated HTTP client
	httpClient := &http.Client{
		Timeout: 30 * time.Second,
		Transport: &AuthenticatedTransport{
			Base:          http.DefaultTransport,
			MCPClient:     c,
			IntegrationID: integrationID,
		},
	}

	// MCP servers typically expose tools/list endpoint
	toolsURL := strings.TrimSuffix(serverURL, "/") + "/tools/list"

	req, err := http.NewRequestWithContext(ctx, "POST", toolsURL, strings.NewReader("{}"))
	if err != nil {
		return nil, fmt.Errorf("failed to create request: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json")

	resp, err := httpClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("failed to list tools: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		return nil, fmt.Errorf("list tools failed with status %d: %s", resp.StatusCode, string(body))
	}

	var result struct {
		Tools []MCPTool `json:"tools"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return nil, fmt.Errorf("failed to decode response: %w", err)
	}

	return result.Tools, nil
}

// CallTool makes an authenticated JSON-RPC call to an external MCP server
func (c *Client) CallTool(ctx context.Context, integrationID, toolName string, input json.RawMessage) (*MCPToolResult, error) {
	integration, err := c.db.GetMCPIntegration(ctx, integrationID)
	if err != nil {
		return nil, fmt.Errorf("failed to get integration: %w", err)
	}

	serverURL := integration.ServerUrl.String
	if serverURL == "" {
		return nil, fmt.Errorf("no server URL configured")
	}

	// Create authenticated HTTP client
	httpClient := &http.Client{
		Timeout: 60 * time.Second, // Longer timeout for tool execution
		Transport: &AuthenticatedTransport{
			Base:          http.DefaultTransport,
			MCPClient:     c,
			IntegrationID: integrationID,
		},
	}

	// MCP servers typically expose tools/call endpoint
	callURL := strings.TrimSuffix(serverURL, "/") + "/tools/call"

	// Build JSON-RPC request
	rpcRequest := map[string]interface{}{
		"name":      toolName,
		"arguments": json.RawMessage(input),
	}

	body, err := json.Marshal(rpcRequest)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal request: %w", err)
	}

	req, err := http.NewRequestWithContext(ctx, "POST", callURL, strings.NewReader(string(body)))
	if err != nil {
		return nil, fmt.Errorf("failed to create request: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json")

	logging.Infof("Calling MCP tool %s on %s", toolName, serverURL)

	resp, err := httpClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("failed to call tool: %w", err)
	}
	defer resp.Body.Close()

	respBody, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("failed to read response: %w", err)
	}

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("tool call failed with status %d: %s", resp.StatusCode, string(respBody))
	}

	var result MCPToolResult
	if err := json.Unmarshal(respBody, &result); err != nil {
		// If the response is not a structured result, wrap it as text content
		result = MCPToolResult{
			Content: []MCPContent{{Type: "text", Text: string(respBody)}},
		}
	}

	return &result, nil
}
