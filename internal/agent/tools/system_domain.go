package tools

import (
	"context"
	"encoding/json"
	"fmt"
)

// SystemDomainTool consolidates system control tools (app, notify, clipboard, settings,
// music, search, keychain) into a single STRAP domain tool with resource-based routing.
// Each resource delegates to the existing platform-specific tool implementation.
type SystemDomainTool struct {
	subTools map[string]Tool // resource name → sub-tool
}

// NewSystemDomainTool creates a system domain tool from the given sub-tools.
// Pass nil for resources not available on this platform.
func NewSystemDomainTool(opts SystemDomainOpts) *SystemDomainTool {
	t := &SystemDomainTool{
		subTools: make(map[string]Tool),
	}
	if opts.App != nil {
		t.subTools["app"] = opts.App
	}
	if opts.Notify != nil {
		t.subTools["notify"] = opts.Notify
	}
	if opts.Clipboard != nil {
		t.subTools["clipboard"] = opts.Clipboard
	}
	if opts.Settings != nil {
		t.subTools["settings"] = opts.Settings
	}
	if opts.Music != nil {
		t.subTools["music"] = opts.Music
	}
	if opts.Search != nil {
		t.subTools["search"] = opts.Search
	}
	if opts.Keychain != nil {
		t.subTools["keychain"] = opts.Keychain
	}
	return t
}

// SystemDomainOpts configures which sub-tools are available for the system domain.
type SystemDomainOpts struct {
	App       Tool // application management (was: app)
	Notify    Tool // notifications/alerts (was: notification)
	Clipboard Tool // clipboard access (was: clipboard)
	Settings  Tool // system settings/info (was: system)
	Music     Tool // music playback control (was: music)
	Search    Tool // system search/spotlight (was: spotlight)
	Keychain  Tool // secure credential storage (was: keychain)
}

func (t *SystemDomainTool) Name() string          { return "system" }
func (t *SystemDomainTool) Domain() string         { return "system" }
func (t *SystemDomainTool) RequiresApproval() bool { return true }

func (t *SystemDomainTool) Resources() []string {
	resources := make([]string, 0, len(t.subTools))
	for r := range t.subTools {
		resources = append(resources, r)
	}
	return resources
}

func (t *SystemDomainTool) ActionsFor(resource string) []string {
	switch resource {
	case "app":
		return []string{"list", "launch", "quit", "quit_all", "activate", "hide", "info", "frontmost"}
	case "notify":
		return []string{"send", "alert", "speak", "dnd_status"}
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

func (t *SystemDomainTool) Description() string {
	return BuildDomainDescription(t.schemaConfig())
}

func (t *SystemDomainTool) Schema() json.RawMessage {
	return BuildDomainSchema(t.schemaConfig())
}

func (t *SystemDomainTool) schemaConfig() DomainSchemaConfig {
	resources := make(map[string]ResourceConfig)
	for name := range t.subTools {
		resources[name] = ResourceConfig{
			Name:    name,
			Actions: t.ActionsFor(name),
		}
	}

	return DomainSchemaConfig{
		Domain:      "system",
		Description: "System control: application management, notifications, clipboard, settings, music, search, keychain.",
		Resources:   resources,
		Fields: []FieldConfig{
			{Name: "name", Type: "string", Description: "App name, shortcut name, or keychain service name"},
			{Name: "path", Type: "string", Description: "App path for launch"},
			{Name: "force", Type: "boolean", Description: "Force quit without saving"},
			{Name: "title", Type: "string", Description: "Notification title"},
			{Name: "text", Type: "string", Description: "Notification body, clipboard text, or speech text"},
			{Name: "sound", Type: "string", Description: "Notification sound name"},
			{Name: "content", Type: "string", Description: "Clipboard content to set"},
			{Name: "level", Type: "integer", Description: "Volume/brightness level (0-100)"},
			{Name: "query", Type: "string", Description: "Search query or music search"},
			{Name: "track", Type: "string", Description: "Track name for music play"},
			{Name: "playlist", Type: "string", Description: "Playlist name"},
			{Name: "account", Type: "string", Description: "Keychain account name"},
			{Name: "password", Type: "string", Description: "Keychain password to store"},
			{Name: "service", Type: "string", Description: "Keychain service identifier"},
		},
		Examples: []string{
			`system(resource: "app", action: "list")`,
			`system(resource: "app", action: "launch", name: "Safari")`,
			`system(resource: "notify", action: "send", title: "Done!", text: "Task completed")`,
			`system(resource: "clipboard", action: "get")`,
			`system(resource: "settings", action: "volume", level: 50)`,
			`system(resource: "music", action: "play")`,
			`system(resource: "search", action: "query", query: "project files")`,
			`system(resource: "keychain", action: "get", service: "github", account: "user")`,
		},
	}
}

// inferResource guesses the resource from the action name when resource is omitted.
func (t *SystemDomainTool) inferResource(action string) string {
	switch action {
	case "launch", "quit", "quit_all", "activate", "hide", "frontmost":
		return "app"
	case "send", "alert", "speak", "dnd_status":
		return "notify"
	case "get", "set", "clear":
		// ambiguous: could be clipboard or keychain — require explicit resource
		return ""
	case "type", "history":
		return "clipboard"
	case "volume", "brightness", "sleep", "lock", "wifi", "bluetooth", "darkmode", "mute", "unmute":
		return "settings"
	case "play", "pause", "next", "previous", "status", "playlists", "shuffle":
		return "music"
	case "find", "add", "delete":
		return "" // ambiguous between keychain and others
	default:
		return ""
	}
}

func (t *SystemDomainTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
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

	sub, ok := t.subTools[resource]
	if !ok {
		return &ToolResult{
			Content: fmt.Sprintf("Resource %q not available on this platform. Available: %v", resource, t.Resources()),
			IsError: true,
		}, nil
	}

	return sub.Execute(ctx, input)
}
