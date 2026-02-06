package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"sync"

	"github.com/nebolabs/nebo/internal/agent/ai"
)

// ToolResult represents the result of a tool execution
type ToolResult struct {
	Content string `json:"content"`
	IsError bool   `json:"is_error,omitempty"`
}

// Tool interface that all tools must implement
type Tool interface {
	// Name returns the tool's unique name
	Name() string

	// Description returns a description for the AI
	Description() string

	// Schema returns the JSON schema for the tool's input
	Schema() json.RawMessage

	// Execute runs the tool with the given input
	Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error)

	// RequiresApproval returns true if this tool needs user approval
	RequiresApproval() bool
}

// ChangeListener is called when tools are added or removed from the registry.
// added contains names of new/replaced tools, removed contains names of deleted tools.
type ChangeListener func(added []string, removed []string)

// Registry manages available tools
type Registry struct {
	mu              sync.RWMutex
	tools           map[string]Tool
	policy          *Policy
	processRegistry *ProcessRegistry
	listeners       []ChangeListener
}

// NewRegistry creates a new tool registry with optional process registry for background bash support
func NewRegistry(policy *Policy) *Registry {
	return &Registry{
		tools:           make(map[string]Tool),
		policy:          policy,
		processRegistry: NewProcessRegistry(),
	}
}

// OnChange registers a listener that is called when tools are added or removed.
func (r *Registry) OnChange(fn ChangeListener) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.listeners = append(r.listeners, fn)
}

// notifyListeners calls all change listeners (must NOT hold lock).
func (r *Registry) notifyListeners(added, removed []string) {
	r.mu.RLock()
	listeners := make([]ChangeListener, len(r.listeners))
	copy(listeners, r.listeners)
	r.mu.RUnlock()

	for _, fn := range listeners {
		fn(added, removed)
	}
}

// Register adds a tool to the registry
func (r *Registry) Register(tool Tool) {
	r.mu.Lock()
	r.tools[tool.Name()] = tool
	r.mu.Unlock()

	r.notifyListeners([]string{tool.Name()}, nil)
}

// Unregister removes a tool from the registry by name
func (r *Registry) Unregister(name string) {
	r.mu.Lock()
	_, existed := r.tools[name]
	delete(r.tools, name)
	r.mu.Unlock()

	if existed {
		r.notifyListeners(nil, []string{name})
	}
}

// Get returns a tool by name
func (r *Registry) Get(name string) (Tool, bool) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	tool, ok := r.tools[name]
	return tool, ok
}

// List returns all tools as AI tool definitions
func (r *Registry) List() []ai.ToolDefinition {
	r.mu.RLock()
	defer r.mu.RUnlock()

	defs := make([]ai.ToolDefinition, 0, len(r.tools))
	for _, tool := range r.tools {
		defs = append(defs, ai.ToolDefinition{
			Name:        tool.Name(),
			Description: tool.Description(),
			InputSchema: tool.Schema(),
		})
	}
	return defs
}

// Execute runs a tool and returns the result
func (r *Registry) Execute(ctx context.Context, toolCall *ai.ToolCall) *ToolResult {
	fmt.Printf("[Registry] Executing tool: %s\n", toolCall.Name)

	r.mu.RLock()
	tool, ok := r.tools[toolCall.Name]
	r.mu.RUnlock()

	if !ok {
		fmt.Printf("[Registry] Unknown tool: %s\n", toolCall.Name)
		// Build list of available tool names so the LLM can self-correct
		r.mu.RLock()
		available := make([]string, 0, len(r.tools))
		for name := range r.tools {
			available = append(available, name)
		}
		r.mu.RUnlock()

		// Provide specific correction for known hallucinated tool names
		correction := toolCorrection(toolCall.Name)

		return &ToolResult{
			Content: fmt.Sprintf(
				"TOOL ERROR: %q does not exist. You do NOT have that tool. Do NOT call it again.\n\n%s\nYour available tools are: %s",
				toolCall.Name, correction, strings.Join(available, ", ")),
			IsError: true,
		}
	}

	// Check if approval is required
	if tool.RequiresApproval() && r.policy != nil {
		fmt.Printf("[Registry] Tool requires approval, policy level=%s\n", r.policy.Level)
		approved, err := r.policy.RequestApproval(ctx, tool.Name(), toolCall.Input)
		fmt.Printf("[Registry] Approval result: approved=%v err=%v\n", approved, err)
		if err != nil {
			return &ToolResult{
				Content: fmt.Sprintf("Approval error: %v", err),
				IsError: true,
			}
		}
		if !approved {
			return &ToolResult{
				Content: "Tool execution denied by user",
				IsError: true,
			}
		}
	} else {
		fmt.Printf("[Registry] No approval needed (requiresApproval=%v, hasPolicy=%v)\n",
			tool.RequiresApproval(), r.policy != nil)
	}

	fmt.Printf("[Registry] Calling tool.Execute...\n")
	result, err := tool.Execute(ctx, toolCall.Input)
	fmt.Printf("[Registry] tool.Execute returned: err=%v\n", err)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Tool error: %v", err),
			IsError: true,
		}
	}

	fmt.Printf("[Registry] Tool completed, content_len=%d, isError=%v\n", len(result.Content), result.IsError)
	return result
}

// SetPolicy updates the registry's policy
func (r *Registry) SetPolicy(policy *Policy) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.policy = policy
}

// RegisterDefaults registers the default set of tools using the STRAP domain pattern
// This consolidates 35+ tools into ~5 domain tools for reduced context window overhead
func (r *Registry) RegisterDefaults() {
	// STRAP Domain Tools (consolidate multiple tools into domain-based routing)
	r.registerDomainTools()

	// Platform-specific capabilities (clipboard, notification, system, tts, etc.)
	// These are auto-registered via init() in platform-specific files
	// and filtered by the current platform
	RegisterPlatformCapabilities(r)
}

// registerDomainTools registers STRAP domain tools
func (r *Registry) registerDomainTools() {
	// File domain: read, write, edit, glob, grep
	r.Register(NewFileTool())

	// Shell domain: bash, process, sessions
	r.Register(NewShellTool(r.policy, r.processRegistry))

	// Web domain: fetch, search, browser, screenshot
	r.Register(NewWebDomainToolWithConfig(WebDomainConfig{
		Headless: true,
	}))

	// Standalone screenshot tool (kept separate for direct capture)
	r.Register(NewScreenshotTool())

	// Vision (image analysis) - requires ANTHROPIC_API_KEY
	// Kept as standalone since it has different config requirements
	if apiKey := os.Getenv("ANTHROPIC_API_KEY"); apiKey != "" {
		r.Register(NewVisionTool(VisionConfig{APIKey: apiKey}))
	}

	// Agent domain: task, cron, memory, message, session
	// Note: AgentDomainTool requires DB and is registered via RegisterAgentDomainTool()
}

// RegisterLegacyDefaults registers individual tools (non-domain) for backward compatibility
// Deprecated: Use RegisterDefaults() with domain tools instead
func (r *Registry) RegisterLegacyDefaults() {
	// Core file tools
	r.Register(NewBashTool(r.policy, r.processRegistry))
	r.Register(NewReadTool())
	r.Register(NewWriteTool())
	r.Register(NewEditTool())
	r.Register(NewGlobTool())
	r.Register(NewGrepTool())

	// Web tools
	r.Register(NewWebTool())
	r.Register(NewSearchTool())

	// Browser automation (headless by default)
	r.Register(NewBrowserTool(BrowserConfig{Headless: true}))

	// Screenshot (desktop capture)
	r.Register(NewScreenshotTool())

	// Vision (image analysis) - requires ANTHROPIC_API_KEY
	if apiKey := os.Getenv("ANTHROPIC_API_KEY"); apiKey != "" {
		r.Register(NewVisionTool(VisionConfig{APIKey: apiKey}))
	}

	// Process management
	r.Register(NewProcessTool())

	// Bash session management for background processes
	r.Register(NewBashSessionsTool(r.processRegistry))

	// Task/sub-agent spawning
	r.Register(NewTaskTool())

	// Platform-specific capabilities
	RegisterPlatformCapabilities(r)
}

// GetProcessRegistry returns the process registry for external access
func (r *Registry) GetProcessRegistry() *ProcessRegistry {
	return r.processRegistry
}

// GetTaskTool returns the task tool for external access (e.g., to set recovery manager)
func (r *Registry) GetTaskTool() *TaskTool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if tool, ok := r.tools["task"]; ok {
		if taskTool, ok := tool.(*TaskTool); ok {
			return taskTool
		}
	}
	return nil
}

// RegisterMemoryTool registers the memory tool with the given database connection
// This must be called separately from RegisterDefaults since it requires a DB
// Deprecated: Use RegisterAgentDomainTool for STRAP pattern instead
func (r *Registry) RegisterMemoryTool(memTool *MemoryTool) {
	r.Register(memTool)
}

// RegisterAgentDomainTool registers the agent domain tool (task, cron, memory, message, session)
// This must be called separately from RegisterDefaults since it requires DB and session manager
func (r *Registry) RegisterAgentDomainTool(agentTool *AgentDomainTool) {
	r.Register(agentTool)
}

// GetAgentDomainTool returns the agent domain tool for external access
// (e.g., to set orchestrator, recovery manager, channel manager)
func (r *Registry) GetAgentDomainTool() *AgentDomainTool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if tool, ok := r.tools["agent"]; ok {
		if agentTool, ok := tool.(*AgentDomainTool); ok {
			return agentTool
		}
	}
	return nil
}

// GetFileTool returns the file domain tool for external access
func (r *Registry) GetFileTool() *FileTool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if tool, ok := r.tools["file"]; ok {
		if fileTool, ok := tool.(*FileTool); ok {
			return fileTool
		}
	}
	return nil
}

// GetShellTool returns the shell domain tool for external access
func (r *Registry) GetShellTool() *ShellTool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if tool, ok := r.tools["shell"]; ok {
		if shellTool, ok := tool.(*ShellTool); ok {
			return shellTool
		}
	}
	return nil
}

// GetWebDomainTool returns the web domain tool for external access
func (r *Registry) GetWebDomainTool() *WebDomainTool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if tool, ok := r.tools["web"]; ok {
		if webTool, ok := tool.(*WebDomainTool); ok {
			return webTool
		}
	}
	return nil
}

// RegisterAdvisorsTool registers the advisors tool for internal deliberation
// This must be called separately from RegisterDefaults since it requires a loader and provider
func (r *Registry) RegisterAdvisorsTool(advisorsTool *AdvisorsTool) {
	r.Register(advisorsTool)
}

// GetAdvisorsTool returns the advisors tool for external access
func (r *Registry) GetAdvisorsTool() *AdvisorsTool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if tool, ok := r.tools["advisors"]; ok {
		if advisorsTool, ok := tool.(*AdvisorsTool); ok {
			return advisorsTool
		}
	}
	return nil
}

// toolCorrection returns a specific "use this instead" message for known
// hallucinated tool names. If the name isn't recognized, returns a generic hint.
func toolCorrection(name string) string {
	switch strings.ToLower(name) {
	case "websearch", "web_search":
		return "INSTEAD USE: web(action: \"search\", query: \"your search query\")"
	case "webfetch", "web_fetch":
		return "INSTEAD USE: web(action: \"fetch\", url: \"https://...\")"
	case "read":
		return "INSTEAD USE: file(action: \"read\", path: \"/path/to/file\")"
	case "write":
		return "INSTEAD USE: file(action: \"write\", path: \"/path\", content: \"...\")"
	case "edit":
		return "INSTEAD USE: file(action: \"edit\", path: \"/path\", old_string: \"...\", new_string: \"...\")"
	case "grep":
		return "INSTEAD USE: file(action: \"grep\", pattern: \"...\", path: \"/dir\")"
	case "glob":
		return "INSTEAD USE: file(action: \"glob\", pattern: \"**/*.go\")"
	case "bash":
		return "INSTEAD USE: shell(resource: \"bash\", action: \"exec\", command: \"...\")"
	default:
		return "Check your available tools and use the correct name."
	}
}

// Close cleans up registry resources (e.g., process registry sweeper)
func (r *Registry) Close() {
	if r.processRegistry != nil {
		r.processRegistry.Close()
	}
}
