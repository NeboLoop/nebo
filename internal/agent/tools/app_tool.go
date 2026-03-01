package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/neboloop/nebo/internal/neboloop"
)

// AppManager provides local app lifecycle operations.
// Implemented by apps.AppRegistry — defined here to avoid import cycles.
type AppManager interface {
	ListInstalled() []AppInfo
	LaunchApp(ctx context.Context, appID string) error
	StopApp(ctx context.Context, appID string) error
}

// AppInfo describes an installed app.
type AppInfo struct {
	ID      string `json:"id"`
	Name    string `json:"name"`
	Version string `json:"version"`
	Status  string `json:"status"` // running, stopped, error
}

// AppTool provides app management: list installed, launch, stop, settings,
// and browse/install from the NeboLoop store.
type AppTool struct {
	clientProvider NeboLoopClientProvider
	appManager     AppManager
}

// NewAppTool creates a new app domain tool.
func NewAppTool(clientProvider NeboLoopClientProvider) *AppTool {
	return &AppTool{clientProvider: clientProvider}
}

// SetAppManager sets the local app manager.
func (t *AppTool) SetAppManager(mgr AppManager) {
	t.appManager = mgr
}

func (t *AppTool) Name() string   { return "app" }
func (t *AppTool) Domain() string { return "app" }

func (t *AppTool) Resources() []string { return nil } // flat domain

func (t *AppTool) ActionsFor(_ string) []string {
	return []string{"list", "launch", "stop", "settings", "browse", "install", "uninstall"}
}

func (t *AppTool) RequiresApproval() bool { return false }

var appResources = map[string]ResourceConfig{
	"": {Name: "", Actions: []string{"list", "launch", "stop", "settings", "browse", "install", "uninstall"}, Description: "Manage installed apps and browse the NeboLoop app store"},
}

func (t *AppTool) Description() string {
	return `Manage apps — list installed, launch, stop, and browse/install from the NeboLoop store.

Actions:
- list: List installed apps and their status
- launch: Launch an installed app by ID
- stop: Stop a running app by ID
- settings: View/manage app settings
- browse: Browse apps from the NeboLoop store (supports query, category, page)
- install: Install an app from the store by ID
- uninstall: Uninstall an app by ID

Examples:
  app(action: list)
  app(action: launch, id: "app-uuid")
  app(action: stop, id: "app-uuid")
  app(action: browse)
  app(action: browse, query: "calendar")
  app(action: install, id: "app-uuid")
  app(action: uninstall, id: "app-uuid")`
}

func (t *AppTool) Schema() json.RawMessage {
	return BuildDomainSchema(DomainSchemaConfig{
		Domain:      "app",
		Description: t.Description(),
		Resources:   appResources,
		Fields: []FieldConfig{
			{Name: "id", Type: "string", Description: "App ID (required for launch, stop, install, uninstall)"},
			{Name: "query", Type: "string", Description: "Search query (for browse)"},
			{Name: "category", Type: "string", Description: "Filter by category (for browse)"},
			{Name: "page", Type: "integer", Description: "Page number (default: 1)"},
			{Name: "page_size", Type: "integer", Description: "Results per page (default: 20)"},
		},
	})
}

// AppInput defines the input for the app domain tool.
type AppInput struct {
	Resource string `json:"resource,omitempty"` // ignored (flat domain)
	Action   string `json:"action"`
	ID       string `json:"id,omitempty"`
	Query    string `json:"query,omitempty"`
	Category string `json:"category,omitempty"`
	Page     int    `json:"page,omitempty"`
	PageSize int    `json:"page_size,omitempty"`
}

func (t *AppTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in AppInput
	if err := json.Unmarshal(input, &in); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch in.Action {
	case "list":
		return t.handleList(ctx)
	case "launch":
		return t.handleLaunch(ctx, in)
	case "stop":
		return t.handleStop(ctx, in)
	case "settings":
		return t.handleSettings(ctx, in)
	case "browse":
		return t.handleBrowse(ctx, in)
	case "install":
		return t.handleInstall(ctx, in)
	case "uninstall":
		return t.handleUninstall(ctx, in)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown app action: %s", in.Action), IsError: true}, nil
	}
}

// =============================================================================
// Local app management
// =============================================================================

func (t *AppTool) handleList(_ context.Context) (*ToolResult, error) {
	if t.appManager == nil {
		return &ToolResult{Content: "App manager not configured", IsError: true}, nil
	}
	apps := t.appManager.ListInstalled()
	if len(apps) == 0 {
		return &ToolResult{Content: "No apps installed."}, nil
	}
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Installed apps (%d):\n\n", len(apps)))
	for _, app := range apps {
		sb.WriteString(fmt.Sprintf("  - %s v%s [%s] (ID: %s)\n", app.Name, app.Version, app.Status, app.ID))
	}
	return &ToolResult{Content: sb.String()}, nil
}

func (t *AppTool) handleLaunch(ctx context.Context, in AppInput) (*ToolResult, error) {
	if in.ID == "" {
		return &ToolResult{Content: "Error: 'id' is required for launch", IsError: true}, nil
	}
	if t.appManager == nil {
		return &ToolResult{Content: "App manager not configured", IsError: true}, nil
	}
	if err := t.appManager.LaunchApp(ctx, in.ID); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to launch app: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("App %s launched", in.ID)}, nil
}

func (t *AppTool) handleStop(ctx context.Context, in AppInput) (*ToolResult, error) {
	if in.ID == "" {
		return &ToolResult{Content: "Error: 'id' is required for stop", IsError: true}, nil
	}
	if t.appManager == nil {
		return &ToolResult{Content: "App manager not configured", IsError: true}, nil
	}
	if err := t.appManager.StopApp(ctx, in.ID); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to stop app: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("App %s stopped", in.ID)}, nil
}

func (t *AppTool) handleSettings(_ context.Context, in AppInput) (*ToolResult, error) {
	return &ToolResult{Content: "App settings not yet implemented"}, nil
}

// =============================================================================
// NeboLoop store operations (browse, install, uninstall)
// =============================================================================

func (t *AppTool) handleBrowse(ctx context.Context, in AppInput) (*ToolResult, error) {
	client, errResult := t.getClient(ctx)
	if errResult != nil {
		return errResult, nil
	}

	// If an ID is provided, get details for that app
	if in.ID != "" {
		resp, fetchErr := client.GetApp(ctx, in.ID)
		if fetchErr != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to get app: %v", fetchErr), IsError: true}, nil
		}
		return &ToolResult{Content: formatAppDetail(resp)}, nil
	}

	// Handle category shortcuts
	switch in.Category {
	case "featured":
		resp, fetchErr := client.ListApps(ctx, "", "featured", 0, 0)
		if fetchErr != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to list featured apps: %v", fetchErr), IsError: true}, nil
		}
		return &ToolResult{Content: formatAppsResponse(resp)}, nil
	case "popular":
		resp, fetchErr := client.ListApps(ctx, "", "popular", 0, 0)
		if fetchErr != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to list popular apps: %v", fetchErr), IsError: true}, nil
		}
		return &ToolResult{Content: formatAppsResponse(resp)}, nil
	}

	resp, fetchErr := client.ListApps(ctx, in.Query, in.Category, in.Page, in.PageSize)
	if fetchErr != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to list apps: %v", fetchErr), IsError: true}, nil
	}
	return &ToolResult{Content: formatAppsResponse(resp)}, nil
}

func (t *AppTool) handleInstall(ctx context.Context, in AppInput) (*ToolResult, error) {
	if in.ID == "" {
		return &ToolResult{Content: "Error: 'id' is required for install", IsError: true}, nil
	}
	client, errResult := t.getClient(ctx)
	if errResult != nil {
		return errResult, nil
	}
	resp, installErr := client.InstallApp(ctx, in.ID)
	if installErr != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to install app: %v", installErr), IsError: true}, nil
	}
	return &ToolResult{Content: formatInstallResponse(resp)}, nil
}

func (t *AppTool) handleUninstall(ctx context.Context, in AppInput) (*ToolResult, error) {
	if in.ID == "" {
		return &ToolResult{Content: "Error: 'id' is required for uninstall", IsError: true}, nil
	}
	client, errResult := t.getClient(ctx)
	if errResult != nil {
		return errResult, nil
	}
	if uninstallErr := client.UninstallApp(ctx, in.ID); uninstallErr != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to uninstall app: %v", uninstallErr), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("App %s uninstalled successfully", in.ID)}, nil
}

// getClient creates a NeboLoop client or returns an error ToolResult.
func (t *AppTool) getClient(ctx context.Context) (*neboloop.Client, *ToolResult) {
	if t.clientProvider == nil {
		return nil, &ToolResult{
			Content: "Not connected to NeboLoop. Connect via Settings first.",
			IsError: true,
		}
	}
	client, err := t.clientProvider(ctx)
	if err != nil {
		return nil, &ToolResult{
			Content: fmt.Sprintf("Not connected to NeboLoop: %v. Connect via Settings first.", err),
			IsError: true,
		}
	}
	return client, nil
}
