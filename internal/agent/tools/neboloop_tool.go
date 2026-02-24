package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/neboloop/nebo/internal/neboloop"
)

// NeboLoopClientProvider creates a NeboLoop API client on demand.
// Implemented in agent.go where it has access to DB, plugin settings, and auth profiles.
type NeboLoopClientProvider func(ctx context.Context) (*neboloop.Client, error)

// NeboLoopTool provides agent access to the NeboLoop app store and skill catalog.
// Resources: apps, skills
type NeboLoopTool struct {
	clientProvider NeboLoopClientProvider
}

// NewNeboLoopTool creates a new NeboLoop domain tool.
func NewNeboLoopTool(provider NeboLoopClientProvider) *NeboLoopTool {
	return &NeboLoopTool{clientProvider: provider}
}

func (t *NeboLoopTool) Name() string { return "store" }

func (t *NeboLoopTool) Domain() string { return "store" }

func (t *NeboLoopTool) Resources() []string { return []string{"apps", "skills"} }

func (t *NeboLoopTool) ActionsFor(resource string) []string {
	switch resource {
	case "apps":
		return []string{"list", "get", "install", "uninstall", "featured", "popular", "reviews"}
	case "skills":
		return []string{"list", "get", "install", "uninstall"}
	default:
		return nil
	}
}

func (t *NeboLoopTool) RequiresApproval() bool { return false }

var neboloopResources = map[string]ResourceConfig{
	"apps": {
		Name:        "apps",
		Actions:     []string{"list", "get", "install", "uninstall", "featured", "popular", "reviews"},
		Description: "Browse, install, and manage apps from the NeboLoop store",
	},
	"skills": {
		Name:        "skills",
		Actions:     []string{"list", "get", "install", "uninstall"},
		Description: "Browse, install, and manage skills from the NeboLoop store",
	},
}

var neboloopSchemaConfig = DomainSchemaConfig{
	Domain: "store",
	Description: `Browse and manage apps and skills from the NeboLoop store.

Resources:
- apps: Browse, install, and manage apps (list, get, install, uninstall, featured, popular, reviews)
- skills: Browse, install, and manage skills (list, get, install, uninstall)`,
	Resources: neboloopResources,
	Fields: []FieldConfig{
		{Name: "id", Type: "string", Description: "App or skill ID (required for get, install, uninstall, reviews)"},
		{Name: "query", Type: "string", Description: "Search query (for list)"},
		{Name: "category", Type: "string", Description: "Filter by category (for list)"},
		{Name: "page", Type: "integer", Description: "Page number (default: 1)"},
		{Name: "page_size", Type: "integer", Description: "Results per page (default: 20)"},
	},
	Examples: []string{
		`store(resource: "apps", action: "list")`,
		`store(resource: "apps", action: "list", query: "calendar")`,
		`store(resource: "apps", action: "get", id: "app-uuid")`,
		`store(resource: "apps", action: "install", id: "app-uuid")`,
		`store(resource: "apps", action: "featured")`,
		`store(resource: "skills", action: "list")`,
		`store(resource: "skills", action: "get", id: "skill-uuid")`,
		`store(resource: "skills", action: "install", id: "skill-uuid")`,
	},
}

func (t *NeboLoopTool) Description() string {
	return BuildDomainDescription(neboloopSchemaConfig)
}

func (t *NeboLoopTool) Schema() json.RawMessage {
	return BuildDomainSchema(neboloopSchemaConfig)
}

// neboloopInput is the parsed input for the store tool.
type neboloopInput struct {
	Resource string `json:"resource"`
	Action   string `json:"action"`
	ID       string `json:"id,omitempty"`
	Query    string `json:"query,omitempty"`
	Category string `json:"category,omitempty"`
	Page     int    `json:"page,omitempty"`
	PageSize int    `json:"page_size,omitempty"`
}

func (t *NeboLoopTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params neboloopInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Invalid input: %v", err), IsError: true}, nil
	}

	if err := ValidateResourceAction(params.Resource, params.Action, neboloopResources); err != nil {
		return &ToolResult{Content: err.Error(), IsError: true}, nil
	}

	client, err := t.clientProvider(ctx)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Not connected to NeboLoop: %v. Connect via Settings → NeboLoop first.", err),
			IsError: true,
		}, nil
	}

	switch params.Resource {
	case "apps":
		return t.executeApps(ctx, client, params)
	case "skills":
		return t.executeSkills(ctx, client, params)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown resource: %s", params.Resource), IsError: true}, nil
	}
}

// --------------------------------------------------------------------------
// Apps
// --------------------------------------------------------------------------

func (t *NeboLoopTool) executeApps(ctx context.Context, client *neboloop.Client, params neboloopInput) (*ToolResult, error) {
	switch params.Action {
	case "list":
		return t.listApps(ctx, client, params)
	case "get":
		return t.getApp(ctx, client, params)
	case "install":
		return t.installApp(ctx, client, params)
	case "uninstall":
		return t.uninstallApp(ctx, client, params)
	case "featured":
		return t.featuredApps(ctx, client, params)
	case "popular":
		return t.popularApps(ctx, client, params)
	case "reviews":
		return t.appReviews(ctx, client, params)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown apps action: %s", params.Action), IsError: true}, nil
	}
}

func (t *NeboLoopTool) listApps(ctx context.Context, client *neboloop.Client, params neboloopInput) (*ToolResult, error) {
	resp, err := client.ListApps(ctx, params.Query, params.Category, params.Page, params.PageSize)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to list apps: %v", err), IsError: true}, nil
	}
	return t.formatResult(resp)
}

func (t *NeboLoopTool) getApp(ctx context.Context, client *neboloop.Client, params neboloopInput) (*ToolResult, error) {
	if params.ID == "" {
		return &ToolResult{Content: "id is required for get action", IsError: true}, nil
	}
	resp, err := client.GetApp(ctx, params.ID)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get app: %v", err), IsError: true}, nil
	}
	return t.formatResult(resp)
}

func (t *NeboLoopTool) installApp(ctx context.Context, client *neboloop.Client, params neboloopInput) (*ToolResult, error) {
	if params.ID == "" {
		return &ToolResult{Content: "id is required for install action", IsError: true}, nil
	}
	resp, err := client.InstallApp(ctx, params.ID)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to install app: %v", err), IsError: true}, nil
	}
	return t.formatResult(resp)
}

func (t *NeboLoopTool) uninstallApp(ctx context.Context, client *neboloop.Client, params neboloopInput) (*ToolResult, error) {
	if params.ID == "" {
		return &ToolResult{Content: "id is required for uninstall action", IsError: true}, nil
	}
	if err := client.UninstallApp(ctx, params.ID); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to uninstall app: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("App %s uninstalled successfully", params.ID)}, nil
}

func (t *NeboLoopTool) featuredApps(ctx context.Context, client *neboloop.Client, _ neboloopInput) (*ToolResult, error) {
	// ListApps with empty query hits the main endpoint; NeboLoop has a dedicated featured endpoint
	// but the client doesn't expose it separately — use ListApps with category workaround
	// Actually, check if GetFeaturedApps exists...
	// The client has ListApps which calls /apps with query params. For featured, we call /apps/featured.
	// Since the client doesn't have a ListFeaturedApps method, we'll add a simple fetch.
	resp, err := client.ListApps(ctx, "", "featured", 0, 0)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to list featured apps: %v", err), IsError: true}, nil
	}
	return t.formatResult(resp)
}

func (t *NeboLoopTool) popularApps(ctx context.Context, client *neboloop.Client, _ neboloopInput) (*ToolResult, error) {
	resp, err := client.ListApps(ctx, "", "popular", 0, 0)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to list popular apps: %v", err), IsError: true}, nil
	}
	return t.formatResult(resp)
}

func (t *NeboLoopTool) appReviews(ctx context.Context, client *neboloop.Client, params neboloopInput) (*ToolResult, error) {
	if params.ID == "" {
		return &ToolResult{Content: "id is required for reviews action", IsError: true}, nil
	}
	resp, err := client.GetAppReviews(ctx, params.ID, params.Page, params.PageSize)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get reviews: %v", err), IsError: true}, nil
	}
	return t.formatResult(resp)
}

// --------------------------------------------------------------------------
// Skills
// --------------------------------------------------------------------------

func (t *NeboLoopTool) executeSkills(ctx context.Context, client *neboloop.Client, params neboloopInput) (*ToolResult, error) {
	switch params.Action {
	case "list":
		return t.listSkills(ctx, client, params)
	case "get":
		return t.getSkill(ctx, client, params)
	case "install":
		return t.installSkill(ctx, client, params)
	case "uninstall":
		return t.uninstallSkill(ctx, client, params)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown skills action: %s", params.Action), IsError: true}, nil
	}
}

func (t *NeboLoopTool) listSkills(ctx context.Context, client *neboloop.Client, params neboloopInput) (*ToolResult, error) {
	resp, err := client.ListSkills(ctx, params.Query, params.Category, params.Page, params.PageSize)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to list skills: %v", err), IsError: true}, nil
	}
	return t.formatResult(resp)
}

func (t *NeboLoopTool) getSkill(ctx context.Context, client *neboloop.Client, params neboloopInput) (*ToolResult, error) {
	if params.ID == "" {
		return &ToolResult{Content: "id is required for get action", IsError: true}, nil
	}
	resp, err := client.GetSkill(ctx, params.ID)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get skill: %v", err), IsError: true}, nil
	}
	return t.formatResult(resp)
}

func (t *NeboLoopTool) installSkill(ctx context.Context, client *neboloop.Client, params neboloopInput) (*ToolResult, error) {
	if params.ID == "" {
		return &ToolResult{Content: "id is required for install action", IsError: true}, nil
	}
	resp, err := client.InstallSkill(ctx, params.ID)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to install skill: %v", err), IsError: true}, nil
	}
	return t.formatResult(resp)
}

func (t *NeboLoopTool) uninstallSkill(ctx context.Context, client *neboloop.Client, params neboloopInput) (*ToolResult, error) {
	if params.ID == "" {
		return &ToolResult{Content: "id is required for uninstall action", IsError: true}, nil
	}
	if err := client.UninstallSkill(ctx, params.ID); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to uninstall skill: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Skill %s uninstalled successfully", params.ID)}, nil
}

// --------------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------------

func (t *NeboLoopTool) formatResult(v any) (*ToolResult, error) {
	// Format as human-readable text for the agent, not raw JSON
	switch val := v.(type) {
	case *neboloop.AppsResponse:
		return &ToolResult{Content: formatAppsResponse(val)}, nil
	case *neboloop.AppDetail:
		return &ToolResult{Content: formatAppDetail(val)}, nil
	case *neboloop.SkillsResponse:
		return &ToolResult{Content: formatSkillsResponse(val)}, nil
	case *neboloop.SkillDetail:
		return &ToolResult{Content: formatSkillDetail(val)}, nil
	case *neboloop.InstallResponse:
		return &ToolResult{Content: formatInstallResponse(val)}, nil
	case *neboloop.ReviewsResponse:
		return &ToolResult{Content: formatReviewsResponse(val)}, nil
	default:
		data, err := json.MarshalIndent(v, "", "  ")
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("%v", v)}, nil
		}
		return &ToolResult{Content: string(data)}, nil
	}
}

func formatAppsResponse(resp *neboloop.AppsResponse) string {
	if len(resp.Apps) == 0 {
		return "No apps found."
	}
	var b strings.Builder
	fmt.Fprintf(&b, "Found %d apps (page %d):\n\n", resp.TotalCount, resp.Page)
	for _, app := range resp.Apps {
		installed := ""
		if app.IsInstalled {
			installed = " ✅ installed"
		}
		fmt.Fprintf(&b, "• **%s** (%s) — %s\n  ID: %s | ⭐ %.1f (%d reviews) | %d installs%s\n\n",
			app.Name, app.Category, app.Description,
			app.ID, app.Rating, app.ReviewCount, app.InstallCount, installed)
	}
	return b.String()
}

func formatAppDetail(app *neboloop.AppDetail) string {
	var b strings.Builder
	installed := ""
	if app.IsInstalled {
		installed = " ✅ installed"
	}
	fmt.Fprintf(&b, "**%s** v%s%s\n", app.Name, app.Version, installed)
	fmt.Fprintf(&b, "Category: %s | By: %s\n", app.Category, app.Author.Name)
	fmt.Fprintf(&b, "⭐ %.1f (%d reviews) | %d installs\n\n", app.Rating, app.ReviewCount, app.InstallCount)
	fmt.Fprintf(&b, "%s\n", app.Description)
	if len(app.Platforms) > 0 {
		fmt.Fprintf(&b, "\nPlatforms: %s\n", strings.Join(app.Platforms, ", "))
	}
	fmt.Fprintf(&b, "\nID: %s\n", app.ID)
	return b.String()
}

func formatSkillsResponse(resp *neboloop.SkillsResponse) string {
	if len(resp.Skills) == 0 {
		return "No skills found."
	}
	var b strings.Builder
	fmt.Fprintf(&b, "Found %d skills (page %d):\n\n", resp.TotalCount, resp.Page)
	for _, skill := range resp.Skills {
		installed := ""
		if skill.IsInstalled {
			installed = " ✅ installed"
		}
		fmt.Fprintf(&b, "• **%s** (%s) — %s\n  ID: %s | ⭐ %.1f | %d installs%s\n\n",
			skill.Name, skill.Category, skill.Description,
			skill.ID, skill.Rating, skill.InstallCount, installed)
	}
	return b.String()
}

func formatSkillDetail(skill *neboloop.SkillDetail) string {
	var b strings.Builder
	installed := ""
	if skill.IsInstalled {
		installed = " ✅ installed"
	}
	fmt.Fprintf(&b, "**%s** v%s%s\n", skill.Name, skill.Version, installed)
	fmt.Fprintf(&b, "Category: %s | By: %s\n", skill.Category, skill.Author.Name)
	fmt.Fprintf(&b, "⭐ %.1f | %d installs\n\n", skill.Rating, skill.InstallCount)
	fmt.Fprintf(&b, "%s\n", skill.Description)
	fmt.Fprintf(&b, "\nID: %s\n", skill.ID)
	return b.String()
}

func formatInstallResponse(resp *neboloop.InstallResponse) string {
	if resp.App != nil {
		return fmt.Sprintf("✅ Installed **%s** v%s", resp.App.Name, resp.App.Version)
	}
	if resp.Skill != nil {
		return fmt.Sprintf("✅ Installed **%s** v%s", resp.Skill.Name, resp.Skill.Version)
	}
	return fmt.Sprintf("✅ Installed (ID: %s)", resp.ID)
}

func formatReviewsResponse(resp *neboloop.ReviewsResponse) string {
	if len(resp.Reviews) == 0 {
		return "No reviews yet."
	}
	var b strings.Builder
	fmt.Fprintf(&b, "⭐ %.1f average (%d reviews)\n\n", resp.Average, resp.TotalCount)
	for _, r := range resp.Reviews {
		stars := strings.Repeat("⭐", r.Rating)
		fmt.Fprintf(&b, "%s **%s** — %s\n%s\n\n", stars, r.Title, r.UserName, r.Body)
	}
	return b.String()
}
