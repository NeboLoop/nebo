package bridge

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"strings"
	"sync"

	"github.com/modelcontextprotocol/go-sdk/mcp"

	"github.com/nebolabs/nebo/internal/agent/tools"
	"github.com/nebolabs/nebo/internal/db"
	mcpclient "github.com/nebolabs/nebo/internal/mcp/client"
)

// Bridge manages connections to external MCP servers and registers their tools
// as proxy tools in the agent's tool registry.
type Bridge struct {
	mu          sync.Mutex
	connections map[string]*connection // integrationID → live connection
	registry    *tools.Registry
	queries     *db.Queries
	mcpClient   *mcpclient.Client
}

type connection struct {
	IntegrationID string
	ServerType    string
	ToolNames     []string // namespaced names registered in Registry
}

// New creates a new MCP Bridge.
func New(registry *tools.Registry, queries *db.Queries, mcpClient *mcpclient.Client) *Bridge {
	return &Bridge{
		connections: make(map[string]*connection),
		registry:    registry,
		queries:     queries,
		mcpClient:   mcpClient,
	}
}

// SyncAll loads all enabled MCP integrations and connects to each.
// Safe to call multiple times; it disconnects stale connections and connects new ones.
func (b *Bridge) SyncAll(ctx context.Context) error {
	integrations, err := b.queries.ListEnabledMCPIntegrations(ctx)
	if err != nil {
		return fmt.Errorf("list integrations: %w", err)
	}

	// Build set of enabled integration IDs
	enabled := make(map[string]bool, len(integrations))
	for _, ig := range integrations {
		enabled[ig.ID] = true
	}

	// Disconnect integrations that are no longer enabled
	b.mu.Lock()
	for id := range b.connections {
		if !enabled[id] {
			b.disconnectLocked(id)
		}
	}
	b.mu.Unlock()

	// Connect new/updated integrations
	var lastErr error
	for _, ig := range integrations {
		if !ig.ServerUrl.Valid || ig.ServerUrl.String == "" {
			continue
		}
		// Skip OAuth integrations that haven't completed authentication yet
		if ig.AuthType == "oauth" && (!ig.ConnectionStatus.Valid || ig.ConnectionStatus.String != "connected") {
			continue
		}
		if err := b.Connect(ctx, ig.ID, ig.ServerType); err != nil {
			fmt.Printf("[MCP Bridge] Failed to connect %s (%s): %v\n", ig.Name, ig.ID, err)
			lastErr = err
		}
	}
	return lastErr
}

// Connect connects to a single MCP integration, lists its tools, and registers
// them as proxy tools in the agent registry.
func (b *Bridge) Connect(ctx context.Context, integrationID, serverType string) error {
	// Disconnect existing connection first
	b.Disconnect(integrationID)

	// List tools from the external server
	mcpTools, err := b.mcpClient.ListTools(ctx, integrationID)
	if err != nil {
		// Update status to error
		b.queries.UpdateMCPIntegrationConnectionStatus(ctx, db.UpdateMCPIntegrationConnectionStatusParams{
			ConnectionStatus: sql.NullString{String: "error", Valid: true},
			Column2:          "error",
			LastError:        sql.NullString{String: err.Error(), Valid: true},
			ID:               integrationID,
		})
		return fmt.Errorf("list tools: %w", err)
	}

	// Register each tool as a proxy in the agent registry
	conn := &connection{
		IntegrationID: integrationID,
		ServerType:    serverType,
		ToolNames:     make([]string, 0, len(mcpTools)),
	}

	for _, mt := range mcpTools {
		proxyName := toolName(serverType, mt.Name)

		// Marshal InputSchema (any) to json.RawMessage for the tool registry
		var schema json.RawMessage
		if mt.InputSchema != nil {
			schema, _ = json.Marshal(mt.InputSchema)
		}

		proxy := &proxyTool{
			name:          proxyName,
			originalName:  mt.Name,
			description:   mt.Description,
			inputSchema:   schema,
			integrationID: integrationID,
			mcpClient:     b.mcpClient,
		}
		b.registry.Register(proxy)
		conn.ToolNames = append(conn.ToolNames, proxyName)
	}

	b.mu.Lock()
	b.connections[integrationID] = conn
	b.mu.Unlock()

	// Update tool count in DB
	b.queries.UpdateMCPIntegrationToolCount(ctx, db.UpdateMCPIntegrationToolCountParams{
		ToolCount: sql.NullInt64{Int64: int64(len(mcpTools)), Valid: true},
		ID:        integrationID,
	})

	fmt.Printf("[MCP Bridge] Connected %s: %d tools registered\n", serverType, len(mcpTools))
	return nil
}

// Disconnect removes all proxy tools for an integration.
func (b *Bridge) Disconnect(integrationID string) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.disconnectLocked(integrationID)
}

func (b *Bridge) disconnectLocked(integrationID string) {
	conn, ok := b.connections[integrationID]
	if !ok {
		return
	}
	for _, name := range conn.ToolNames {
		b.registry.Unregister(name)
	}
	b.mcpClient.CloseSession(integrationID)
	delete(b.connections, integrationID)
	fmt.Printf("[MCP Bridge] Disconnected %s: %d tools unregistered\n", integrationID, len(conn.ToolNames))
}

// Close disconnects all integrations.
func (b *Bridge) Close() {
	b.mu.Lock()
	defer b.mu.Unlock()
	for id := range b.connections {
		b.disconnectLocked(id)
	}
}

// toolName generates a namespaced tool name: mcp__{serverType}__{toolName}
func toolName(serverType, original string) string {
	st := strings.ReplaceAll(strings.ToLower(serverType), " ", "_")
	return fmt.Sprintf("mcp__%s__%s", st, original)
}

// proxyTool implements tools.Tool by forwarding calls to an external MCP server.
type proxyTool struct {
	name          string
	originalName  string
	description   string
	inputSchema   json.RawMessage
	integrationID string
	mcpClient     *mcpclient.Client
}

func (t *proxyTool) Name() string        { return t.name }
func (t *proxyTool) Description() string  { return t.description }
func (t *proxyTool) RequiresApproval() bool { return true }

func (t *proxyTool) Schema() json.RawMessage {
	if len(t.inputSchema) > 0 {
		return t.inputSchema
	}
	return json.RawMessage(`{"type":"object"}`)
}

func (t *proxyTool) Execute(ctx context.Context, input json.RawMessage) (*tools.ToolResult, error) {
	result, err := t.mcpClient.CallTool(ctx, t.integrationID, t.originalName, input)
	if err != nil {
		return nil, fmt.Errorf("MCP tool %s: %w", t.originalName, err)
	}

	// Extract text content from MCP result (Content is []mcp.Content interface)
	var sb strings.Builder
	for _, c := range result.Content {
		if tc, ok := c.(*mcp.TextContent); ok && tc.Text != "" {
			if sb.Len() > 0 {
				sb.WriteString("\n")
			}
			sb.WriteString(tc.Text)
		}
	}

	return &tools.ToolResult{
		Content: sb.String(),
		IsError: result.IsError,
	}, nil
}
