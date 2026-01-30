package tools

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"slices"
	"strings"
	"time"

	"nebo/internal/db"
	"nebo/internal/mcp/mcpctx"

	"github.com/modelcontextprotocol/go-sdk/mcp"
)

// userActions defines valid actions for each user resource.
var userActions = map[string][]string{
	"user":        {"get", "update"},
	"preferences": {"get", "update"},
}

// UserInput defines input for the unified user tool.
type UserInput struct {
	Resource string `json:"resource" jsonschema:"required,Resource type: user or preferences"`
	Action   string `json:"action" jsonschema:"required,Action to perform"`

	// Update-specific (user)
	Name string `json:"name,omitempty" jsonschema:"User's display name. For user.update."`

	// Preferences-specific
	Theme              string `json:"theme,omitempty" jsonschema:"UI theme: light, dark, or system. For preferences.update."`
	Language           string `json:"language,omitempty" jsonschema:"Preferred language code. For preferences.update."`
	Timezone           string `json:"timezone,omitempty" jsonschema:"User timezone. For preferences.update."`
	EmailNotifications *bool  `json:"email_notifications,omitempty" jsonschema:"Enable email notifications. For preferences.update."`
	MarketingEmails    *bool  `json:"marketing_emails,omitempty" jsonschema:"Opt in to marketing emails. For preferences.update."`
}

// RegisterUserTool registers the unified user tool.
func RegisterUserTool(server *mcp.Server, toolCtx *mcpctx.ToolContext) {
	mcp.AddTool(server, &mcp.Tool{
		Name:  "user",
		Title: "User Management",
		Description: `Manage user profile and preferences.

Resources:
- user: User profile management
- preferences: User preferences

USER RESOURCE:
- user.get: Get current user profile
- user.update: Update user profile (optional: name)

PREFERENCES RESOURCE:
- preferences.get: Get user preferences
- preferences.update: Update preferences (optional: theme, language, timezone, email_notifications, marketing_emails)

Examples:
  user(resource: user, action: get)
  user(resource: user, action: update, name: "John Doe")
  user(resource: preferences, action: get)
  user(resource: preferences, action: update, theme: "dark", email_notifications: false)`,
	}, userHandler(toolCtx))
}

func userHandler(toolCtx *mcpctx.ToolContext) func(ctx context.Context, req *mcp.CallToolRequest, input UserInput) (*mcp.CallToolResult, any, error) {
	return func(ctx context.Context, req *mcp.CallToolRequest, input UserInput) (*mcp.CallToolResult, any, error) {
		fmt.Printf("[MCP user] Handler called - Resource: %q, Action: %q\n", input.Resource, input.Action)

		// Validate resource
		validActions, ok := userActions[input.Resource]
		if !ok {
			return nil, nil, mcpctx.NewValidationError(
				fmt.Sprintf("invalid resource '%s', must be: user or preferences", input.Resource),
				"resource")
		}

		// Validate action
		if !slices.Contains(validActions, input.Action) {
			return nil, nil, mcpctx.NewValidationError(
				fmt.Sprintf("invalid action '%s' for resource '%s', must be: %s",
					input.Action, input.Resource, strings.Join(validActions, ", ")),
				"action")
		}

		switch input.Resource {
		case "user":
			return handleUser(ctx, toolCtx, input)
		case "preferences":
			return handleUserPreferences(ctx, toolCtx, input)
		}
		return nil, nil, nil
	}
}

// ============================================================================
// USER HANDLERS
// ============================================================================

// UserGetOutput defines output for user.get.
type UserGetOutput struct {
	ID            string `json:"id"`
	Email         string `json:"email"`
	Name          string `json:"name"`
	EmailVerified bool   `json:"email_verified"`
	CreatedAt     string `json:"created_at"`
}

// UserUpdateOutput defines output for user.update.
type UserUpdateOutput struct {
	ID      string `json:"id"`
	Email   string `json:"email"`
	Name    string `json:"name"`
	Updated bool   `json:"updated"`
}

func handleUser(ctx context.Context, toolCtx *mcpctx.ToolContext, input UserInput) (*mcp.CallToolResult, any, error) {
	switch input.Action {
	case "get":
		return handleUserGet(ctx, toolCtx, input)
	case "update":
		return handleUserUpdate(ctx, toolCtx, input)
	}
	return nil, nil, nil
}

func handleUserGet(ctx context.Context, toolCtx *mcpctx.ToolContext, input UserInput) (*mcp.CallToolResult, any, error) {
	user := toolCtx.User()
	if user == nil {
		return nil, nil, mcpctx.NewUnauthorizedError("not authenticated")
	}

	return nil, UserGetOutput{
		ID:            user.ID,
		Email:         user.Email,
		Name:          user.Name,
		EmailVerified: user.EmailVerified == 1,
		CreatedAt:     time.Unix(user.CreatedAt, 0).Format(time.RFC3339),
	}, nil
}

func handleUserUpdate(ctx context.Context, toolCtx *mcpctx.ToolContext, input UserInput) (*mcp.CallToolResult, any, error) {
	user := toolCtx.User()
	if user == nil {
		return nil, nil, mcpctx.NewUnauthorizedError("not authenticated")
	}

	if input.Name == "" {
		return nil, nil, mcpctx.NewValidationError("name is required for update", "name")
	}

	err := toolCtx.DB().UpdateUser(ctx, db.UpdateUserParams{
		ID:   user.ID,
		Name: sql.NullString{String: input.Name, Valid: true},
	})
	if err != nil {
		return nil, nil, fmt.Errorf("failed to update user: %w", err)
	}

	return nil, UserUpdateOutput{
		ID:      user.ID,
		Email:   user.Email,
		Name:    input.Name,
		Updated: true,
	}, nil
}

// ============================================================================
// PREFERENCES HANDLERS
// ============================================================================

// PreferencesGetOutput defines output for preferences.get.
type PreferencesGetOutput struct {
	Theme              string `json:"theme"`
	Language           string `json:"language"`
	Timezone           string `json:"timezone"`
	EmailNotifications bool   `json:"email_notifications"`
	MarketingEmails    bool   `json:"marketing_emails"`
}

// PreferencesUpdateOutput defines output for preferences.update.
type PreferencesUpdateOutput struct {
	Theme              string `json:"theme"`
	Language           string `json:"language"`
	Timezone           string `json:"timezone"`
	EmailNotifications bool   `json:"email_notifications"`
	MarketingEmails    bool   `json:"marketing_emails"`
	Updated            bool   `json:"updated"`
}

func handleUserPreferences(ctx context.Context, toolCtx *mcpctx.ToolContext, input UserInput) (*mcp.CallToolResult, any, error) {
	switch input.Action {
	case "get":
		return handlePreferencesGet(ctx, toolCtx, input)
	case "update":
		return handlePreferencesUpdate(ctx, toolCtx, input)
	}
	return nil, nil, nil
}

func handlePreferencesGet(ctx context.Context, toolCtx *mcpctx.ToolContext, input UserInput) (*mcp.CallToolResult, any, error) {
	prefs, err := toolCtx.DB().GetUserPreferences(ctx, toolCtx.UserID())
	if err != nil {
		// Return defaults if no preferences exist
		return nil, PreferencesGetOutput{
			Theme:              "system",
			Language:           "en",
			Timezone:           "UTC",
			EmailNotifications: true,
			MarketingEmails:    false,
		}, nil
	}

	return nil, PreferencesGetOutput{
		Theme:              prefs.Theme,
		Language:           prefs.Language,
		Timezone:           prefs.Timezone,
		EmailNotifications: prefs.EmailNotifications == 1,
		MarketingEmails:    prefs.MarketingEmails == 1,
	}, nil
}

func handlePreferencesUpdate(ctx context.Context, toolCtx *mcpctx.ToolContext, input UserInput) (*mcp.CallToolResult, any, error) {
	// Get current preferences first to check if they exist
	_, err := toolCtx.DB().GetUserPreferences(ctx, toolCtx.UserID())
	if err != nil {
		// Create new preferences if they don't exist
		_, err = toolCtx.DB().CreateUserPreferences(ctx, toolCtx.UserID())
		if err != nil {
			return nil, nil, fmt.Errorf("failed to create preferences: %w", err)
		}
	}

	// Build update params
	params := db.UpdateUserPreferencesParams{
		UserID: toolCtx.UserID(),
	}

	if input.Theme != "" {
		params.Theme = sql.NullString{String: input.Theme, Valid: true}
	}
	if input.Language != "" {
		params.Language = sql.NullString{String: input.Language, Valid: true}
	}
	if input.Timezone != "" {
		params.Timezone = sql.NullString{String: input.Timezone, Valid: true}
	}
	if input.EmailNotifications != nil {
		val := int64(0)
		if *input.EmailNotifications {
			val = 1
		}
		params.EmailNotifications = sql.NullInt64{Int64: val, Valid: true}
	}
	if input.MarketingEmails != nil {
		val := int64(0)
		if *input.MarketingEmails {
			val = 1
		}
		params.MarketingEmails = sql.NullInt64{Int64: val, Valid: true}
	}

	err = toolCtx.DB().UpdateUserPreferences(ctx, params)
	if err != nil {
		return nil, nil, fmt.Errorf("failed to update preferences: %w", err)
	}

	// Get updated values
	updatedPrefs, _ := toolCtx.DB().GetUserPreferences(ctx, toolCtx.UserID())

	return nil, PreferencesUpdateOutput{
		Theme:              updatedPrefs.Theme,
		Language:           updatedPrefs.Language,
		Timezone:           updatedPrefs.Timezone,
		EmailNotifications: updatedPrefs.EmailNotifications == 1,
		MarketingEmails:    updatedPrefs.MarketingEmails == 1,
		Updated:            true,
	}, nil
}

// registerUserToolToRegistry registers user tool to the direct-call registry.
func registerUserToolToRegistry(registry *ToolRegistry, toolCtx *mcpctx.ToolContext) {
	registry.Register("user", func(ctx context.Context, args json.RawMessage) (interface{}, error) {
		var input UserInput
		if err := json.Unmarshal(args, &input); err != nil {
			return nil, fmt.Errorf("invalid input: %w", err)
		}
		handler := userHandler(toolCtx)
		_, output, err := handler(ctx, nil, input)
		return output, err
	})
}
