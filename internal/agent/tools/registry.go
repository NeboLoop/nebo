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

// HookDispatcher is the interface for the app hooks system.
// Defined here (not in apps/) to avoid circular imports.
type HookDispatcher interface {
	// ApplyFilter calls filter subscribers in priority order.
	// Returns modified payload and whether a filter set handled=true.
	ApplyFilter(ctx context.Context, hook string, payload []byte) ([]byte, bool)
	// DoAction calls action subscribers (fire-and-forget with timeout).
	DoAction(ctx context.Context, hook string, payload []byte)
	// HasSubscribers returns true if any app is subscribed to the given hook.
	HasSubscribers(hook string) bool
}

// Registry manages available tools
type Registry struct {
	mu              sync.RWMutex
	tools           map[string]Tool
	policy          *Policy
	processRegistry *ProcessRegistry
	listeners       []ChangeListener
	desktopQueue    DesktopQueueFunc // When set, desktop tools are serialized through this
	systemTool      *SystemTool      // cached reference for accessor
	hooks           HookDispatcher   // App hooks dispatcher (optional)
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

// SetHookDispatcher sets the app hooks dispatcher for intercepting tool execution.
func (r *Registry) SetHookDispatcher(hooks HookDispatcher) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.hooks = hooks
}

// GetHookDispatcher returns the hook dispatcher (may be nil).
func (r *Registry) GetHookDispatcher() HookDispatcher {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return r.hooks
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

	// System domain: file + shell (always) + platform resources (app, clipboard, settings, music, search, keychain)
	// Platform resources are registered via init() in system_domain_darwin/linux/windows.go
	r.systemTool = NewSystemTool(r.policy, r.processRegistry)
	r.Register(r.systemTool)

	// Web domain: fetch, search, browser
	if allowed("web") {
		r.Register(NewWebDomainToolWithConfig(WebDomainConfig{
			Headless: true,
		}))
	}

	// Bot domain: task, memory, session, profile, context, advisors, vision, ask
	// Requires DB — registered via RegisterBotTool() from agent.go

	// Loop domain: NeboLoop comms — dm, channel, group, topic
	r.Register(NewLoopTool())

	// Message domain: outbound delivery — owner, sms, notify
	r.Register(NewMsgTool())

	// Event domain: scheduling & reminders — registered via RegisterEventTool()
	// App domain: app management + store — registered via RegisterAppTool()
}

// GetProcessRegistry returns the process registry for external access
func (r *Registry) GetProcessRegistry() *ProcessRegistry {
	return r.processRegistry
}

// GetFileTool returns the file tool (via system domain tool).
func (r *Registry) GetFileTool() *FileTool {
	if r.systemTool != nil {
		return r.systemTool.GetFileTool()
	}
	return nil
}

// GetShellTool returns the shell tool (via system domain tool).
func (r *Registry) GetShellTool() *ShellTool {
	if r.systemTool != nil {
		return r.systemTool.GetShellTool()
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

// RegisterBotTool registers the bot domain tool (task, memory, session, profile, context, advisors, vision, ask)
// This must be called separately from RegisterDefaults since it requires DB and session manager.
func (r *Registry) RegisterBotTool(botTool *BotTool) {
	r.Register(botTool)
}

// GetBotTool returns the bot domain tool for external access.
func (r *Registry) GetBotTool() *BotTool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if tool, ok := r.tools["bot"]; ok {
		if botTool, ok := tool.(*BotTool); ok {
			return botTool
		}
	}
	return nil
}

// GetLoopTool returns the loop domain tool for external access.
func (r *Registry) GetLoopTool() *LoopTool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if tool, ok := r.tools["loop"]; ok {
		if loopTool, ok := tool.(*LoopTool); ok {
			return loopTool
		}
	}
	return nil
}

// GetMsgTool returns the message domain tool for external access.
func (r *Registry) GetMsgTool() *MsgTool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if tool, ok := r.tools["message"]; ok {
		if msgTool, ok := tool.(*MsgTool); ok {
			return msgTool
		}
	}
	return nil
}

// RegisterEventTool registers the event domain tool (cron, reminders).
func (r *Registry) RegisterEventTool(eventTool *EventTool) {
	r.Register(eventTool)
}

// GetEventTool returns the event domain tool for external access.
func (r *Registry) GetEventTool() *EventTool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if tool, ok := r.tools["event"]; ok {
		if eventTool, ok := tool.(*EventTool); ok {
			return eventTool
		}
	}
	return nil
}

// RegisterAppTool registers the app domain tool (list, launch, stop, browse, install).
func (r *Registry) RegisterAppTool(appTool *AppTool) {
	r.Register(appTool)
}

// GetAppTool returns the app domain tool for external access.
func (r *Registry) GetAppTool() *AppTool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if tool, ok := r.tools["app"]; ok {
		if appTool, ok := tool.(*AppTool); ok {
			return appTool
		}
	}
	return nil
}

// GetSystemTool returns the system domain tool for external access.
func (r *Registry) GetSystemTool() *SystemTool {
	return r.systemTool
}

// GetSkillTool returns the skill domain tool for external access.
func (r *Registry) GetSkillTool() *SkillDomainTool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if tool, ok := r.tools["skill"]; ok {
		if skillTool, ok := tool.(*SkillDomainTool); ok {
			return skillTool
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
		return "INSTEAD USE: system(resource: \"file\", action: \"read\", path: \"/path/to/file\")"
	case "write":
		return "INSTEAD USE: system(resource: \"file\", action: \"write\", path: \"/path\", content: \"...\")"
	case "edit":
		return "INSTEAD USE: system(resource: \"file\", action: \"edit\", path: \"/path\", old_string: \"...\", new_string: \"...\")"
	case "grep":
		return "INSTEAD USE: system(resource: \"file\", action: \"grep\", pattern: \"...\", path: \"/dir\")"
	case "glob":
		return "INSTEAD USE: system(resource: \"file\", action: \"glob\", pattern: \"**/*.go\")"
	case "bash":
		return "INSTEAD USE: system(resource: \"shell\", action: \"exec\", command: \"...\")"
	case "file":
		return "INSTEAD USE: system(resource: \"file\", action: \"read\", path: \"...\") — file operations are under the system tool"
	case "shell":
		return "INSTEAD USE: system(resource: \"shell\", action: \"exec\", command: \"...\") — shell operations are under the system tool"
	case "agent":
		return "INSTEAD USE: bot(resource: \"memory\", action: \"store\", ...) or bot(resource: \"task\", action: \"spawn\", ...) or loop(resource: \"channel\", action: \"send\", ...) or event(action: \"create\", ...)"
	case "apps", "application", "applications":
		return "INSTEAD USE: app(action: \"list\") or app(action: \"launch\", id: \"...\") or app(action: \"browse\")"
	case "mail", "email":
		return "INSTEAD USE: organizer(resource: \"mail\", action: \"unread\") or organizer(resource: \"mail\", action: \"send\", ...)"
	case "contacts":
		return "INSTEAD USE: organizer(resource: \"contacts\", action: \"search\", query: \"...\")"
	case "calendar":
		return "INSTEAD USE: organizer(resource: \"calendar\", action: \"today\") or organizer(resource: \"calendar\", action: \"create\", ...)"
	case "reminders":
		return "INSTEAD USE: organizer(resource: \"reminders\", action: \"list\") or organizer(resource: \"reminders\", action: \"create\", ...)"
	case "pim":
		return "INSTEAD USE: organizer(resource: \"mail\", action: \"...\") — pim has been renamed to organizer"
	case "clipboard":
		return "INSTEAD USE: system(resource: \"clipboard\", action: \"get\") or system(resource: \"clipboard\", action: \"set\", content: \"...\")"
	case "notification", "notify":
		return "INSTEAD USE: message(resource: \"notify\", action: \"send\", title: \"...\", text: \"...\")"
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
		return "INSTEAD USE: message(resource: \"sms\", action: \"send\", to: \"+15551234567\", body: \"Hello!\") or message(resource: \"sms\", action: \"conversations\")"
	case "notify_owner":
		return "INSTEAD USE: message(resource: \"owner\", action: \"notify\", text: \"...\")"
	case "query_sessions":
		return "INSTEAD USE: bot(resource: \"session\", action: \"query\", query: \"...\")"
	case "screenshot":
		return "INSTEAD USE: desktop(resource: \"screenshot\", action: \"capture\") or desktop(resource: \"screenshot\", action: \"see\", app: \"...\")"
	case "tts":
		return "INSTEAD USE: desktop(resource: \"tts\", action: \"speak\", text: \"...\")"
	case "vision":
		return "INSTEAD USE: bot(resource: \"vision\", action: \"analyze\", image: \"...\", text: \"...\")"
	case "advisors":
		return "INSTEAD USE: bot(resource: \"advisors\", action: \"deliberate\", task: \"...\")"
	case "store":
		return "INSTEAD USE: skill(action: \"browse\") or app(action: \"browse\")"
	case "cron":
		return "INSTEAD USE: event(action: \"create\", ...) or event(action: \"list\")"
	case "task":
		return "INSTEAD USE: bot(resource: \"task\", action: \"spawn\", description: \"...\") or bot(resource: \"task\", action: \"list\")"
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
