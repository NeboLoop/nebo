package mcp

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/neboloop/nebo/internal/agent/advisors"
	"github.com/neboloop/nebo/internal/agent/ai"
	"github.com/neboloop/nebo/internal/agent/session"
	"github.com/neboloop/nebo/internal/agent/tools"

	"github.com/modelcontextprotocol/go-sdk/mcp"
)

// Option configures the MCP server
type Option func(*Server)

// WithAdvisors enables the advisors tool using a direct AI provider
func WithAdvisors(loader *advisors.Loader, provider ai.Provider) Option {
	return func(s *Server) {
		s.advisorLoader = loader
		s.advisorProvider = provider
	}
}

// Server wraps a tool registry to expose tools via MCP
type Server struct {
	registry        *tools.Registry
	server          *mcp.Server
	advisorLoader   *advisors.Loader
	advisorProvider ai.Provider
	mu              sync.Mutex
	registeredTools map[string]bool // tracks which tools are in the MCP server

	// Context values injected by the runner before each agentic loop.
	// CLI providers (claude-code, gemini-cli) call tools via HTTP, which
	// creates a fresh request context that loses the runner's context.Values.
	// We store them here so createToolHandler can re-inject them.
	sessionKey string
	origin     tools.Origin
}

// SetContext sets the session key and origin for MCP tool calls.
// Called by the runner before each agentic loop so that tools invoked
// via CLI providers (which cross an HTTP boundary) get the correct context.
func (s *Server) SetContext(sessionKey string, origin tools.Origin) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.sessionKey = sessionKey
	s.origin = origin
}

// NewServer creates a new MCP server for the agent
func NewServer(registry *tools.Registry, opts ...Option) *Server {
	s := &Server{
		registry:        registry,
		registeredTools: make(map[string]bool),
	}

	// Apply options
	for _, opt := range opts {
		opt(s)
	}

	s.server = mcp.NewServer(&mcp.Implementation{
		Name:    "nebo-agent",
		Version: "1.0.0",
	}, nil)

	// Register tools from registry
	s.registerTools()

	// Register advisors tool if loader and provider are available
	if s.advisorLoader != nil && s.advisorProvider != nil {
		s.registerAdvisorsTool()
	}

	// Subscribe to registry changes so new tools (e.g. plugins) are exposed dynamically.
	// The go-sdk's AddTool/RemoveTools automatically sends notifications/tools/list_changed
	// to connected clients, so Claude CLI will re-fetch the tool list.
	registry.OnChange(func(added, removed []string) {
		s.syncTools(added, removed)
	})

	return s
}

// registerTools registers all tools from the registry with the MCP server
func (s *Server) registerTools() {
	toolDefs := s.registry.List()

	s.mu.Lock()
	defer s.mu.Unlock()

	for _, def := range toolDefs {
		toolDef := def
		s.addToolToServer(toolDef.Name, toolDef.Description, toolDef.InputSchema)
	}
}

// addToolToServer adds a single tool to the MCP server (caller must hold s.mu)
func (s *Server) addToolToServer(name, description string, inputSchema json.RawMessage) {
	var schemaMap map[string]any
	if err := json.Unmarshal(inputSchema, &schemaMap); err != nil {
		fmt.Printf("[AgentMCP] Failed to parse schema for %s: %v\n", name, err)
		return
	}

	// AddTool replaces if already exists and auto-notifies connected clients
	s.server.AddTool(&mcp.Tool{
		Name:        name,
		Description: description,
		InputSchema: schemaMap,
	}, s.createToolHandler(name))
	s.registeredTools[name] = true
}

// syncTools handles dynamic tool changes from the registry.
// Called when tools are added/removed (e.g. plugin install/uninstall).
func (s *Server) syncTools(added, removed []string) {
	s.mu.Lock()
	defer s.mu.Unlock()

	// Remove tools that were unregistered from the registry
	if len(removed) > 0 {
		s.server.RemoveTools(removed...)
		for _, name := range removed {
			delete(s.registeredTools, name)
		}
		fmt.Printf("[AgentMCP] Removed tools: %s\n", strings.Join(removed, ", "))
	}

	// Add/replace tools that were registered
	for _, name := range added {
		tool, ok := s.registry.Get(name)
		if !ok {
			continue
		}
		s.addToolToServer(tool.Name(), tool.Description(), tool.Schema())
		fmt.Printf("[AgentMCP] Added/updated tool: %s\n", name)
	}
}

// createToolHandler creates an MCP tool handler that returns proper TextContent
func (s *Server) createToolHandler(toolName string) mcp.ToolHandler {
	return func(ctx context.Context, req *mcp.CallToolRequest) (retResult *mcp.CallToolResult, retErr error) {
		// Recover from panics to prevent EOF
		defer func() {
			if r := recover(); r != nil {
				fmt.Printf("[AgentMCP] PANIC in tool %s: %v\n", toolName, r)
				retResult = &mcp.CallToolResult{
					Content: []mcp.Content{&mcp.TextContent{Text: fmt.Sprintf("tool panicked: %v", r)}},
					IsError: true,
				}
				retErr = nil
			}
		}()

		// Re-inject session key and origin that were lost at the HTTP boundary.
		// CLI providers (claude-code) call tools via HTTP POST to /agent/mcp,
		// which creates a fresh context without the runner's context.Values.
		s.mu.Lock()
		if s.sessionKey != "" {
			ctx = tools.WithSessionKey(ctx, s.sessionKey)
		}
		if s.origin != "" {
			ctx = tools.WithOrigin(ctx, s.origin)
		}
		s.mu.Unlock()

		// Arguments come as json.RawMessage from the request
		inputJSON := json.RawMessage(req.Params.Arguments)
		fmt.Printf("[AgentMCP] Tool call received: %s input=%s\n", toolName, truncate(string(inputJSON), 200))

		// Execute via registry
		result := s.registry.Execute(ctx, &ai.ToolCall{
			ID:    toolName,
			Name:  toolName,
			Input: inputJSON,
		})

		fmt.Printf("[AgentMCP] Tool %s result: isError=%v content_len=%d\n", toolName, result.IsError, len(result.Content))

		return &mcp.CallToolResult{
			Content: []mcp.Content{&mcp.TextContent{Text: result.Content}},
			IsError: result.IsError,
		}, nil
	}
}

// registerAdvisorsTool registers the advisors tool that uses a direct AI provider
func (s *Server) registerAdvisorsTool() {
	// Build dynamic description listing available advisors
	advisorList := s.advisorLoader.List()
	var names []string
	for _, a := range advisorList {
		names = append(names, fmt.Sprintf("%s (%s)", a.Name, a.Role))
	}

	description := "Consult Nebo's internal advisors for deliberation on a task. " +
		"Each advisor is a persona that provides an independent critique with assessment, confidence score, risks, and suggestions. " +
		"Advisors run in parallel."
	if len(names) > 0 {
		description += "\n\nAvailable advisors: " + strings.Join(names, ", ")
	}

	s.server.AddTool(&mcp.Tool{
		Name:        "advisors",
		Description: description,
		InputSchema: map[string]any{
			"type": "object",
			"properties": map[string]any{
				"task": map[string]any{
					"type":        "string",
					"description": "The task or question to deliberate on. Be specific about what you want advice on.",
				},
				"advisors": map[string]any{
					"type":        "array",
					"description": "Optional list of specific advisor names to consult. If omitted, all enabled advisors are consulted.",
					"items": map[string]any{
						"type": "string",
					},
				},
			},
			"required": []string{"task"},
		},
	}, s.advisorsHandler())
}

// advisorsHandler returns the MCP tool handler for the advisors tool
func (s *Server) advisorsHandler() mcp.ToolHandler {
	return func(ctx context.Context, req *mcp.CallToolRequest) (*mcp.CallToolResult, error) {
		// Parse input
		var input struct {
			Task     string   `json:"task"`
			Advisors []string `json:"advisors"`
		}
		argsJSON, err := json.Marshal(req.Params.Arguments)
		if err != nil {
			return &mcp.CallToolResult{
				Content: []mcp.Content{&mcp.TextContent{Text: fmt.Sprintf("failed to marshal arguments: %v", err)}},
				IsError: true,
			}, nil
		}
		if err := json.Unmarshal(argsJSON, &input); err != nil {
			return &mcp.CallToolResult{
				Content: []mcp.Content{&mcp.TextContent{Text: fmt.Sprintf("invalid input: %v", err)}},
				IsError: true,
			}, nil
		}

		if input.Task == "" {
			return &mcp.CallToolResult{
				Content: []mcp.Content{&mcp.TextContent{Text: "task is required"}},
				IsError: true,
			}, nil
		}

		// Get advisors to consult
		allAdvisors := s.advisorLoader.List()
		if len(allAdvisors) == 0 {
			return &mcp.CallToolResult{
				Content: []mcp.Content{&mcp.TextContent{Text: "No advisors configured. Add ADVISOR.md files to the advisors/ directory in your Nebo data folder"}},
			}, nil
		}

		// Filter to requested advisors if specified
		var selected []*advisors.Advisor
		if len(input.Advisors) > 0 {
			nameSet := make(map[string]bool, len(input.Advisors))
			for _, n := range input.Advisors {
				nameSet[strings.ToLower(n)] = true
			}
			for _, a := range allAdvisors {
				if nameSet[strings.ToLower(a.Name)] {
					selected = append(selected, a)
				}
			}
			if len(selected) == 0 {
				var available []string
				for _, a := range allAdvisors {
					available = append(available, a.Name)
				}
				return &mcp.CallToolResult{
					Content: []mcp.Content{&mcp.TextContent{Text: fmt.Sprintf("None of the requested advisors found. Available: %s", strings.Join(available, ", "))}},
					IsError: true,
				}, nil
			}
		} else {
			selected = allAdvisors
		}

		// Cap at MaxAdvisors
		if len(selected) > advisors.MaxAdvisors {
			selected = selected[:advisors.MaxAdvisors]
		}

		fmt.Printf("[AgentMCP] Advisors tool: consulting %d advisors on: %s\n", len(selected), truncate(input.Task, 100))

		// Run advisors in parallel using direct AI provider
		// Use 60s timeout (longer than internal runner's 30s) since MCP calls
		// go over HTTP to external APIs and we want all advisors to finish
		ctx, cancel := context.WithTimeout(ctx, 60*time.Second)
		defer cancel()

		type advisorResult struct {
			response advisors.Response
			err      error
		}

		var wg sync.WaitGroup
		results := make([]advisorResult, len(selected))

		for i, adv := range selected {
			wg.Add(1)
			go func(idx int, advisor *advisors.Advisor) {
				defer wg.Done()

				systemPrompt := advisor.BuildSystemPrompt(input.Task)

				// Call AI provider directly (same pattern as advisors.Runner.runAdvisor)
				events, err := s.advisorProvider.Stream(ctx, &ai.ChatRequest{
					System: systemPrompt,
					Messages: []session.Message{
						{Role: "user", Content: "Provide your assessment of the task above."},
					},
					MaxTokens: 1024,
				})
				if err != nil {
					results[idx] = advisorResult{
						response: advisors.Response{AdvisorName: advisor.Name, Role: advisor.Role},
						err:      err,
					}
					fmt.Printf("[AgentMCP] Advisor %s failed: %v\n", advisor.Name, err)
					return
				}

				// Collect streamed response
				var content strings.Builder
				for event := range events {
					switch event.Type {
					case ai.EventTypeText:
						content.WriteString(event.Text)
					case ai.EventTypeError:
						results[idx] = advisorResult{
							response: advisors.Response{AdvisorName: advisor.Name, Role: advisor.Role},
							err:      event.Error,
						}
						fmt.Printf("[AgentMCP] Advisor %s stream error: %v\n", advisor.Name, event.Error)
						return
					}
				}

				results[idx] = advisorResult{
					response: advisors.Response{
						AdvisorName: advisor.Name,
						Role:        advisor.Role,
						Critique:    content.String(),
					},
				}
				fmt.Printf("[AgentMCP] Advisor %s responded\n", advisor.Name)
			}(i, adv)
		}

		// Wait with timeout
		done := make(chan struct{})
		go func() {
			wg.Wait()
			close(done)
		}()

		select {
		case <-done:
		case <-ctx.Done():
			fmt.Printf("[AgentMCP] Advisors timed out after 60s\n")
		}

		// Collect successful responses
		var responses []advisors.Response
		var errors []string
		for _, r := range results {
			if r.err != nil {
				errors = append(errors, fmt.Sprintf("%s: %v", r.response.AdvisorName, r.err))
			} else if r.response.Critique != "" {
				responses = append(responses, r.response)
			}
		}

		// Format output as markdown
		var sb strings.Builder
		sb.WriteString(fmt.Sprintf("# Advisor Deliberation\n\n**Task:** %s\n\n", input.Task))
		sb.WriteString(fmt.Sprintf("**Advisors consulted:** %d/%d responded\n\n", len(responses), len(selected)))

		if len(responses) > 0 {
			sb.WriteString("---\n\n")
			for _, resp := range responses {
				sb.WriteString(fmt.Sprintf("## %s (%s)\n\n", resp.AdvisorName, resp.Role))
				sb.WriteString(resp.Critique)
				sb.WriteString("\n\n")
			}
		}

		if len(errors) > 0 {
			sb.WriteString("---\n\n**Errors:**\n")
			for _, e := range errors {
				sb.WriteString(fmt.Sprintf("- %s\n", e))
			}
		}

		if len(responses) > 0 {
			sb.WriteString("\n---\n\n")
			sb.WriteString("*Use the perspectives above to inform your decision. You are the authority.*\n")
		}

		return &mcp.CallToolResult{
			Content: []mcp.Content{&mcp.TextContent{Text: sb.String()}},
		}, nil
	}
}

func truncate(s string, maxLen int) string {
	if len(s) <= maxLen {
		return s
	}
	return s[:maxLen] + "..."
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
