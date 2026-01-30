package mcp

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"

	"nebo/agent/ai"
	"nebo/agent/tools"

	"github.com/modelcontextprotocol/go-sdk/mcp"
)

// Server wraps a tool registry to expose tools via MCP
type Server struct {
	registry *tools.Registry
	server   *mcp.Server
}

// ToolInput is a generic input type for tool calls
type ToolInput map[string]any

// NewServer creates a new MCP server for the agent
func NewServer(registry *tools.Registry) *Server {
	s := &Server{
		registry: registry,
	}

	s.server = mcp.NewServer(&mcp.Implementation{
		Name:    "gobot-agent",
		Version: "1.0.0",
	}, nil)

	// Register tools from registry
	s.registerTools()

	return s
}

// registerTools registers all tools from the registry with the MCP server
func (s *Server) registerTools() {
	toolDefs := s.registry.List()

	for _, def := range toolDefs {
		// Capture for closure
		toolDef := def

		// Parse schema
		var schemaMap map[string]any
		if err := json.Unmarshal(toolDef.InputSchema, &schemaMap); err != nil {
			fmt.Printf("[AgentMCP] Failed to parse schema for %s: %v\n", toolDef.Name, err)
			continue
		}

		// Register tool with MCP server using the SDK helper
		mcp.AddTool(s.server, &mcp.Tool{
			Name:        toolDef.Name,
			Description: toolDef.Description,
			InputSchema: schemaMap,
		}, s.createToolHandler(toolDef.Name))
	}
}

// createToolHandler creates an MCP tool handler for a specific tool
func (s *Server) createToolHandler(toolName string) func(ctx context.Context, req *mcp.CallToolRequest, input ToolInput) (*mcp.CallToolResult, any, error) {
	return func(ctx context.Context, req *mcp.CallToolRequest, input ToolInput) (*mcp.CallToolResult, any, error) {
		// Marshal arguments to JSON for the registry
		inputJSON, err := json.Marshal(input)
		if err != nil {
			return &mcp.CallToolResult{
				IsError: true,
			}, map[string]string{"error": fmt.Sprintf("Failed to marshal arguments: %v", err)}, nil
		}

		// Execute via registry
		result := s.registry.Execute(ctx, &ai.ToolCall{
			ID:    toolName,
			Name:  toolName,
			Input: inputJSON,
		})

		if result.IsError {
			return &mcp.CallToolResult{
				IsError: true,
			}, map[string]string{"error": result.Content}, nil
		}

		return nil, map[string]string{"result": result.Content}, nil
	}
}

// Handler returns an HTTP handler for the MCP server
func (s *Server) Handler() http.Handler {
	return mcp.NewStreamableHTTPHandler(
		func(r *http.Request) *mcp.Server {
			return s.server
		},
		nil,
	)
}

// GetServer returns the underlying MCP server
func (s *Server) GetServer() *mcp.Server {
	return s.server
}
