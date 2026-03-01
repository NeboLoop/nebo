package tools

import (
	"context"
	"encoding/json"
	"fmt"
)

// pendingSystemResources collects platform-specific sub-tools registered via init().
// They are pulled into the SystemTool when NewSystemTool is called.
var pendingSystemResources = make(map[string]Tool)

// RegisterSystemResourceInit registers a platform-specific resource for the SystemTool.
// Called from platform init() functions (e.g., system_domain_darwin.go).
func RegisterSystemResourceInit(name string, tool Tool) {
	pendingSystemResources[name] = tool
}

// SystemTool consolidates OS-level operations into a single STRAP domain tool.
// Core resources (always registered): file, shell
// Platform resources (registered via init + RegisterSystemResourceInit): app, clipboard,
// settings, music, search, keychain
type SystemTool struct {
	// Core resources (cross-platform, always present)
	fileTool  *FileTool
	shellTool *ShellTool

	// Platform resources (registered via init)
	platformTools map[string]Tool // resource name → sub-tool
}

// NewSystemTool creates a system domain tool with core file+shell resources.
// Platform resources registered via RegisterSystemResourceInit() are automatically included.
func NewSystemTool(policy *Policy, processRegistry *ProcessRegistry) *SystemTool {
	t := &SystemTool{
		fileTool:      NewFileTool(),
		shellTool:     NewShellTool(policy, processRegistry),
		platformTools: make(map[string]Tool),
	}
	// Pull in platform resources registered via init()
	for name, tool := range pendingSystemResources {
		t.platformTools[name] = tool
	}
	return t
}

// RegisterSystemResource adds a platform-specific resource to the system tool.
// Called from platform init() functions (e.g., system_platform_darwin.go).
func (t *SystemTool) RegisterSystemResource(name string, tool Tool) {
	t.platformTools[name] = tool
}

// --- Tool interface ---

func (t *SystemTool) Name() string { return "system" }

func (t *SystemTool) Description() string {
	return BuildDomainDescription(t.schemaConfig())
}

func (t *SystemTool) Schema() json.RawMessage {
	return BuildDomainSchema(t.schemaConfig())
}

func (t *SystemTool) RequiresApproval() bool {
	// Shell exec needs approval; file read doesn't.
	// Handled per-action by the delegated tools.
	return false
}

// --- DomainTool interface ---

func (t *SystemTool) Domain() string { return "system" }

func (t *SystemTool) Resources() []string {
	resources := []string{"file", "shell"}
	for name := range t.platformTools {
		resources = append(resources, name)
	}
	return resources
}

func (t *SystemTool) ActionsFor(resource string) []string {
	switch resource {
	case "file":
		return []string{"read", "write", "edit", "glob", "grep"}
	case "shell":
		return []string{"exec", "list", "poll", "log", "write", "kill", "info", "status"}
	case "app":
		return []string{"list", "launch", "quit", "quit_all", "activate", "hide", "info", "frontmost"}
	case "clipboard":
		return []string{"get", "set", "clear", "type", "history"}
	case "settings":
		return []string{"volume", "brightness", "sleep", "lock", "wifi", "bluetooth", "darkmode", "info", "mute", "unmute"}
	case "music":
		return []string{"play", "pause", "next", "previous", "status", "search", "volume", "playlists", "shuffle"}
	case "search":
		return []string{"query"}
	case "keychain":
		return []string{"get", "find", "add", "delete"}
	default:
		return nil
	}
}

func (t *SystemTool) schemaConfig() DomainSchemaConfig {
	resources := map[string]ResourceConfig{
		"file": {
			Name:        "file",
			Actions:     t.ActionsFor("file"),
			Description: "File operations: read, write, edit, glob search, grep search",
		},
		"shell": {
			Name:        "shell",
			Actions:     t.ActionsFor("shell"),
			Description: "Shell operations: exec commands, manage sessions and processes",
		},
	}
	for name := range t.platformTools {
		resources[name] = ResourceConfig{
			Name:    name,
			Actions: t.ActionsFor(name),
		}
	}

	return DomainSchemaConfig{
		Domain:      "system",
		Description: "OS operations — files, commands, apps, clipboard, settings, music, search, keychain.",
		Resources:   resources,
		Fields: []FieldConfig{
			// File fields
			{Name: "path", Type: "string", Description: "File path, directory path, or app path"},
			{Name: "content", Type: "string", Description: "File content to write, or clipboard content to set"},
			{Name: "pattern", Type: "string", Description: "Glob pattern or grep regex pattern"},
			{Name: "old_string", Type: "string", Description: "String to find in file (for edit)"},
			{Name: "new_string", Type: "string", Description: "Replacement string (for edit)"},
			{Name: "replace_all", Type: "boolean", Description: "Replace all occurrences (for edit)"},
			{Name: "offset", Type: "integer", Description: "Line offset for reading"},
			{Name: "limit", Type: "integer", Description: "Max lines/results to return"},
			{Name: "append", Type: "boolean", Description: "Append to file instead of overwrite"},
			// Shell fields
			{Name: "command", Type: "string", Description: "Shell command to execute"},
			{Name: "timeout", Type: "integer", Description: "Command timeout in seconds"},
			{Name: "session_id", Type: "string", Description: "Background session ID"},
			{Name: "pid", Type: "integer", Description: "Process ID"},
			{Name: "signal", Type: "string", Description: "Signal to send: SIGTERM, SIGKILL, SIGINT"},
			{Name: "input", Type: "string", Description: "Input/stdin to write to session, or shortcut input"},
			// Platform fields
			{Name: "name", Type: "string", Description: "App name, shortcut name, or keychain service name"},
			{Name: "force", Type: "boolean", Description: "Force quit without saving"},
			{Name: "title", Type: "string", Description: "Notification title"},
			{Name: "text", Type: "string", Description: "Notification body, clipboard text, or speech text"},
			{Name: "sound", Type: "string", Description: "Notification sound name"},
			{Name: "level", Type: "integer", Description: "Volume/brightness level (0-100)"},
			{Name: "query", Type: "string", Description: "Search query or music search"},
			{Name: "track", Type: "string", Description: "Track name for music play"},
			{Name: "playlist", Type: "string", Description: "Playlist name"},
			{Name: "account", Type: "string", Description: "Keychain account name"},
			{Name: "password", Type: "string", Description: "Keychain password to store"},
			{Name: "service", Type: "string", Description: "Keychain service identifier"},
		},
		Examples: []string{
			`system(resource: "file", action: "read", path: "/path/to/file.txt")`,
			`system(resource: "file", action: "write", path: "/path/to/file.txt", content: "hello")`,
			`system(resource: "file", action: "edit", path: "/path/to/file.txt", old_string: "foo", new_string: "bar")`,
			`system(resource: "file", action: "glob", pattern: "**/*.go")`,
			`system(resource: "file", action: "grep", pattern: "TODO", path: "/project")`,
			`system(resource: "shell", action: "exec", command: "ls -la")`,
			`system(resource: "app", action: "list")`,
			`system(resource: "app", action: "launch", name: "Safari")`,
			`system(resource: "clipboard", action: "get")`,
			`system(resource: "settings", action: "volume", level: 50)`,
			`system(resource: "music", action: "play")`,
			`system(resource: "search", action: "query", query: "project files")`,
			`system(resource: "keychain", action: "get", service: "github", account: "user")`,
		},
	}
}

// inferResource guesses the resource from the action name when resource is omitted.
func (t *SystemTool) inferResource(action string) string {
	switch action {
	// File-only actions
	case "read", "write", "edit", "glob", "grep":
		return "file"
	// Shell-only actions
	case "exec", "poll", "log":
		return "shell"
	// Platform actions
	case "launch", "quit", "quit_all", "activate", "hide", "frontmost":
		return "app"
	case "type", "history":
		return "clipboard"
	case "volume", "brightness", "sleep", "lock", "wifi", "bluetooth", "darkmode", "mute", "unmute":
		return "settings"
	case "play", "pause", "next", "previous", "playlists", "shuffle":
		return "music"
	default:
		return "" // ambiguous — require explicit resource
	}
}

func (t *SystemTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p DomainInput
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	resource := p.Resource
	if resource == "" {
		resource = t.inferResource(p.Action)
	}
	if resource == "" {
		return &ToolResult{
			Content: fmt.Sprintf("Resource is required. Available: %v", t.Resources()),
			IsError: true,
		}, nil
	}

	switch resource {
	case "file":
		return t.fileTool.Execute(ctx, input)
	case "shell":
		return t.shellTool.Execute(ctx, input)
	default:
		sub, ok := t.platformTools[resource]
		if !ok {
			return &ToolResult{
				Content: fmt.Sprintf("Resource %q not available on this platform. Available: %v", resource, t.Resources()),
				IsError: true,
			}, nil
		}
		return sub.Execute(ctx, input)
	}
}

// --- Accessor methods for late binding ---

// GetFileTool returns the underlying file tool for external access.
func (t *SystemTool) GetFileTool() *FileTool {
	return t.fileTool
}

// GetShellTool returns the underlying shell tool for external access.
func (t *SystemTool) GetShellTool() *ShellTool {
	return t.shellTool
}
