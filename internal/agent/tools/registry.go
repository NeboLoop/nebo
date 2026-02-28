package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"sync"

	"github.com/neboloop/nebo/internal/agent/ai"
)

// ToolResult represents the result of a tool execution
type ToolResult struct {
	Content  string `json:"content"`
	IsError  bool   `json:"is_error,omitempty"`
	ImageURL string `json:"image_url,omitempty"` // URL of an image produced by the tool (e.g., screenshot)
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
	desktopQueue    DesktopQueueFunc // When set, desktop tools are serialized through this
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
	if existing, ok := r.tools[tool.Name()]; ok {
		fmt.Printf("[Registry] WARNING: tool %q already registered (%T), overwritten by %T\n",
			tool.Name(), existing, tool)
	}
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

// stripMCPPrefix removes the MCP namespace prefix from tool names.
// Claude CLI exposes tools via MCP as "mcp__{server}__{tool}" (e.g., "mcp__nebo-agent__web").
// When these prefixed names leak into session history, non-CLI providers may repeat them.
// This strips the prefix so the registry can find the actual tool.
func stripMCPPrefix(name string) string {
	if !strings.HasPrefix(name, "mcp__") {
		return name
	}
	// mcp__{server}__{tool} → tool
	parts := strings.SplitN(name, "__", 3)
	if len(parts) == 3 {
		return parts[2]
	}
	return name
}

// Execute runs a tool and returns the result
func (r *Registry) Execute(ctx context.Context, toolCall *ai.ToolCall) *ToolResult {
	// If the tool name has an MCP prefix (e.g., "mcp__nebo-agent__web"), check if it
	// exists as-is first (external MCP proxy tools). Only strip the prefix as a fallback
	// for Nebo's own tools exposed via MCP to the CLI provider.
	if strings.HasPrefix(toolCall.Name, "mcp__") {
		r.mu.RLock()
		_, exists := r.tools[toolCall.Name]
		r.mu.RUnlock()
		if !exists {
			toolCall.Name = stripMCPPrefix(toolCall.Name)
		}
	}

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

	// Hard safety guard — unconditional, cannot be overridden by any setting
	if err := CheckSafeguard(toolCall.Name, toolCall.Input); err != nil {
		fmt.Printf("[Registry] SAFEGUARD BLOCKED: %s — %v\n", toolCall.Name, err)
		return &ToolResult{
			Content: err.Error(),
			IsError: true,
		}
	}

	// Delegate execution — routes desktop tools through LaneDesktop queue if configured,
	// otherwise runs inline. Origin checks and approval happen inside executeTool().
	return r.executeWithDesktopQueue(ctx, tool, toolCall)
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
	r.RegisterDefaultsWithPermissions(nil)
}

// RegisterDefaultsWithPermissions registers tools filtered by the given permission map.
// Keys are category names: "chat", "file", "shell", "web", "contacts", "desktop", "media", "system".
// A nil map registers all tools (no filtering). A missing key defaults to false (denied).
func (r *Registry) RegisterDefaultsWithPermissions(permissions map[string]bool) {
	// STRAP Domain Tools (consolidate multiple tools into domain-based routing)
	r.registerDomainToolsWithPermissions(permissions)

	// Platform-specific capabilities (clipboard, notification, system, tts, etc.)
	// These are auto-registered via init() in platform-specific files
	// and filtered by the current platform + permissions
	RegisterPlatformCapabilitiesWithPermissions(r, permissions)
}

// registerDomainTools registers STRAP domain tools (no permission filtering)
func (r *Registry) registerDomainTools() {
	r.registerDomainToolsWithPermissions(nil)
}

// registerDomainToolsWithPermissions registers STRAP domain tools filtered by permissions
func (r *Registry) registerDomainToolsWithPermissions(permissions map[string]bool) {
	allowed := func(category string) bool {
		if permissions == nil {
			return true // No permissions = allow all
		}
		return permissions[category]
	}

	// File domain: read, write, edit, glob, grep
	if allowed("file") {
		r.Register(NewFileTool())
	}

	// Shell domain: bash, process, sessions
	if allowed("shell") {
		r.Register(NewShellTool(r.policy, r.processRegistry))
	}

	// Web domain: fetch, search, browser, screenshot
	if allowed("web") {
		r.Register(NewWebDomainToolWithConfig(WebDomainConfig{
			Headless: true,
		}))
	}

	// Standalone screenshot tool (kept separate for direct capture)
	if allowed("media") {
		r.Register(NewScreenshotTool())

		// Vision (image analysis) — always registered, AnalyzeFunc wired after provider loading
		r.Register(NewVisionTool(VisionConfig{}))
	}

	// Agent domain: task, cron, memory, message, session
	// Note: AgentDomainTool requires DB and is registered via RegisterAgentDomainTool()
	// The agent tool is always registered (it covers chat/memory which is always on)

	// notify_owner: cross-lane owner notification (always registered, wired via setter)
	r.Register(&NotifyOwnerTool{})

	// query_sessions: cross-session awareness for Main Chat (always registered, wired via setter)
	r.Register(&QuerySessionsTool{})
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

// GetNotifyOwnerTool returns the notify_owner tool for external wiring.
func (r *Registry) GetNotifyOwnerTool() *NotifyOwnerTool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if tool, ok := r.tools["notify_owner"]; ok {
		if t, ok := tool.(*NotifyOwnerTool); ok {
			return t
		}
	}
	return nil
}

// GetQuerySessionsTool returns the query_sessions tool for external wiring.
func (r *Registry) GetQuerySessionsTool() *QuerySessionsTool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if tool, ok := r.tools["query_sessions"]; ok {
		if t, ok := tool.(*QuerySessionsTool); ok {
			return t
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

// GetVisionTool returns the vision tool for external access (e.g., to set AnalyzeFunc)
func (r *Registry) GetVisionTool() *VisionTool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if tool, ok := r.tools["vision"]; ok {
		if visionTool, ok := tool.(*VisionTool); ok {
			return visionTool
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
	case "apps", "application", "applications", "app":
		return "INSTEAD USE: system(resource: \"app\", action: \"list\") or system(resource: \"app\", action: \"launch\", name: \"AppName\")"
	case "mail", "email":
		return "INSTEAD USE: pim(resource: \"mail\", action: \"unread\") or pim(resource: \"mail\", action: \"send\", ...)"
	case "contacts":
		return "INSTEAD USE: pim(resource: \"contacts\", action: \"search\", query: \"...\")"
	case "calendar":
		return "INSTEAD USE: pim(resource: \"calendar\", action: \"today\") or pim(resource: \"calendar\", action: \"create\", ...)"
	case "reminders":
		return "INSTEAD USE: pim(resource: \"reminders\", action: \"list\") or pim(resource: \"reminders\", action: \"create\", ...)"
	case "clipboard":
		return "INSTEAD USE: system(resource: \"clipboard\", action: \"get\") or system(resource: \"clipboard\", action: \"set\", content: \"...\")"
	case "notification", "notify":
		return "INSTEAD USE: system(resource: \"notify\", action: \"send\", title: \"...\", text: \"...\")"
	case "music":
		return "INSTEAD USE: system(resource: \"music\", action: \"play\") or system(resource: \"music\", action: \"status\")"
	case "spotlight", "search":
		return "INSTEAD USE: system(resource: \"search\", action: \"query\", query: \"...\")"
	case "keychain":
		return "INSTEAD USE: system(resource: \"keychain\", action: \"get\", service: \"...\", account: \"...\")"
	case "window":
		return "INSTEAD USE: desktop(resource: \"window\", action: \"list\") or desktop(resource: \"window\", action: \"focus\", name: \"...\")"
	case "accessibility":
		return "INSTEAD USE: desktop(resource: \"ui\", action: \"tree\", app: \"...\") or desktop(resource: \"ui\", action: \"find\", ...)"
	case "menubar":
		return "INSTEAD USE: desktop(resource: \"menu\", action: \"list\", app: \"...\") or desktop(resource: \"menu\", action: \"click\", ...)"
	case "dialog":
		return "INSTEAD USE: desktop(resource: \"dialog\", action: \"detect\") or desktop(resource: \"dialog\", action: \"click\", ...)"
	case "spaces":
		return "INSTEAD USE: desktop(resource: \"space\", action: \"list\") or desktop(resource: \"space\", action: \"switch\", space: 2)"
	case "shortcuts":
		return "INSTEAD USE: desktop(resource: \"shortcut\", action: \"list\") or desktop(resource: \"shortcut\", action: \"run\", name: \"...\")"
	case "dock":
		return "INSTEAD USE: system(resource: \"app\", action: \"list\") — dock tool has been consolidated"
	case "messages", "imessage", "sms", "text", "message":
		return "INSTEAD USE: pim(resource: \"messages\", action: \"send\", to: \"+15551234567\", body: \"Hello!\") or pim(resource: \"messages\", action: \"conversations\")"
	case "devtools", "dev_tools", "browser_devtools":
		return "INSTEAD USE: web(resource: \"devtools\", action: \"console\") or web(resource: \"devtools\", action: \"source\")"
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
