package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"slices"
	"strings"
	"time"

	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/mcp/mcpctx"

	"github.com/modelcontextprotocol/go-sdk/mcp"
)

// notificationActions defines valid actions for notification resource.
var notificationActions = map[string][]string{
	"notification": {"list", "get", "mark_read", "mark_all_read", "count_unread"},
}

// NotificationInput defines input for the unified notification tool.
type NotificationInput struct {
	Resource string `json:"resource" jsonschema:"required,Resource type: notification"`
	Action   string `json:"action" jsonschema:"required,Action to perform"`

	// Common
	ID string `json:"id,omitempty" jsonschema:"Notification ID (for get, mark_read)"`

	// List-specific
	Limit  int  `json:"limit,omitempty" jsonschema:"Max notifications to return (default: 20). For notification.list."`
	Offset int  `json:"offset,omitempty" jsonschema:"Offset for pagination. For notification.list."`
	Unread bool `json:"unread,omitempty" jsonschema:"Only return unread notifications. For notification.list."`
}

// RegisterNotificationTool registers the unified notification tool.
func RegisterNotificationTool(server *mcp.Server, toolCtx *mcpctx.ToolContext) {
	mcp.AddTool(server, &mcp.Tool{
		Name:  "notification",
		Title: "Notification Management",
		Description: `Manage user notifications.

Resources:
- notification: User notifications

NOTIFICATION RESOURCE:
- notification.list: List notifications (optional: limit, offset, unread)
- notification.get: Get a specific notification (requires: id)
- notification.mark_read: Mark notification as read (requires: id)
- notification.mark_all_read: Mark all notifications as read
- notification.count_unread: Get count of unread notifications

Examples:
  notification(resource: notification, action: list)
  notification(resource: notification, action: list, unread: true, limit: 10)
  notification(resource: notification, action: get, id: "uuid")
  notification(resource: notification, action: mark_read, id: "uuid")
  notification(resource: notification, action: mark_all_read)
  notification(resource: notification, action: count_unread)`,
	}, notificationHandler(toolCtx))
}

func notificationHandler(toolCtx *mcpctx.ToolContext) func(ctx context.Context, req *mcp.CallToolRequest, input NotificationInput) (*mcp.CallToolResult, any, error) {
	return func(ctx context.Context, req *mcp.CallToolRequest, input NotificationInput) (*mcp.CallToolResult, any, error) {
		fmt.Printf("[MCP notification] Handler called - Resource: %q, Action: %q, ID: %q\n",
			input.Resource, input.Action, input.ID)

		// Validate resource
		validActions, ok := notificationActions[input.Resource]
		if !ok {
			return nil, nil, mcpctx.NewValidationError(
				fmt.Sprintf("invalid resource '%s', must be: notification", input.Resource),
				"resource")
		}

		// Validate action
		if !slices.Contains(validActions, input.Action) {
			return nil, nil, mcpctx.NewValidationError(
				fmt.Sprintf("invalid action '%s' for resource '%s', must be: %s",
					input.Action, input.Resource, strings.Join(validActions, ", ")),
				"action")
		}

		switch input.Action {
		case "list":
			return handleNotificationList(ctx, toolCtx, input)
		case "get":
			return handleNotificationGet(ctx, toolCtx, input)
		case "mark_read":
			return handleNotificationMarkRead(ctx, toolCtx, input)
		case "mark_all_read":
			return handleNotificationMarkAllRead(ctx, toolCtx, input)
		case "count_unread":
			return handleNotificationCountUnread(ctx, toolCtx, input)
		}
		return nil, nil, nil
	}
}

// ============================================================================
// NOTIFICATION HANDLERS
// ============================================================================

// NotificationItem represents a notification in the list.
type NotificationItem struct {
	ID        string  `json:"id"`
	Type      string  `json:"type"`
	Title     string  `json:"title"`
	Body      string  `json:"body,omitempty"`
	ActionURL string  `json:"action_url,omitempty"`
	Read      bool    `json:"read"`
	CreatedAt string  `json:"created_at"`
}

// NotificationListOutput defines output for notification.list.
type NotificationListOutput struct {
	Notifications []NotificationItem `json:"notifications"`
	Total         int                `json:"total"`
}

// NotificationGetOutput defines output for notification.get.
type NotificationGetOutput struct {
	ID        string `json:"id"`
	Type      string `json:"type"`
	Title     string `json:"title"`
	Body      string `json:"body,omitempty"`
	ActionURL string `json:"action_url,omitempty"`
	Read      bool   `json:"read"`
	CreatedAt string `json:"created_at"`
}

// NotificationMarkReadOutput defines output for notification.mark_read.
type NotificationMarkReadOutput struct {
	ID      string `json:"id"`
	Read    bool   `json:"read"`
	Success bool   `json:"success"`
}

// NotificationMarkAllReadOutput defines output for notification.mark_all_read.
type NotificationMarkAllReadOutput struct {
	Success bool   `json:"success"`
	Message string `json:"message"`
}

// NotificationCountOutput defines output for notification.count_unread.
type NotificationCountOutput struct {
	Count int64 `json:"count"`
}

func handleNotificationList(ctx context.Context, toolCtx *mcpctx.ToolContext, input NotificationInput) (*mcp.CallToolResult, any, error) {
	limit := input.Limit
	if limit <= 0 {
		limit = 20
	}

	var notifications []NotificationItem

	if input.Unread {
		notifs, err := toolCtx.DB().ListUnreadNotifications(ctx, db.ListUnreadNotificationsParams{
			UserID:   toolCtx.UserID(),
			PageSize: int64(limit),
		})
		if err != nil {
			return nil, nil, fmt.Errorf("failed to list notifications: %w", err)
		}
		for _, n := range notifs {
			notifications = append(notifications, NotificationItem{
				ID:        n.ID,
				Type:      n.Type,
				Title:     n.Title,
				Body:      n.Body.String,
				ActionURL: n.ActionUrl.String,
				Read:      n.ReadAt.Valid,
				CreatedAt: time.Unix(n.CreatedAt, 0).Format(time.RFC3339),
			})
		}
	} else {
		notifs, err := toolCtx.DB().ListUserNotifications(ctx, db.ListUserNotificationsParams{
			UserID:     toolCtx.UserID(),
			PageOffset: int64(input.Offset),
			PageSize:   int64(limit),
		})
		if err != nil {
			return nil, nil, fmt.Errorf("failed to list notifications: %w", err)
		}
		for _, n := range notifs {
			notifications = append(notifications, NotificationItem{
				ID:        n.ID,
				Type:      n.Type,
				Title:     n.Title,
				Body:      n.Body.String,
				ActionURL: n.ActionUrl.String,
				Read:      n.ReadAt.Valid,
				CreatedAt: time.Unix(n.CreatedAt, 0).Format(time.RFC3339),
			})
		}
	}

	return nil, NotificationListOutput{
		Notifications: notifications,
		Total:         len(notifications),
	}, nil
}

func handleNotificationGet(ctx context.Context, toolCtx *mcpctx.ToolContext, input NotificationInput) (*mcp.CallToolResult, any, error) {
	if input.ID == "" {
		return nil, nil, mcpctx.NewValidationError("id is required", "id")
	}

	n, err := toolCtx.DB().GetNotification(ctx, db.GetNotificationParams{
		ID:     input.ID,
		UserID: toolCtx.UserID(),
	})
	if err != nil {
		return nil, nil, mcpctx.NewNotFoundError(fmt.Sprintf("notification %s not found", input.ID))
	}

	return nil, NotificationGetOutput{
		ID:        n.ID,
		Type:      n.Type,
		Title:     n.Title,
		Body:      n.Body.String,
		ActionURL: n.ActionUrl.String,
		Read:      n.ReadAt.Valid,
		CreatedAt: time.Unix(n.CreatedAt, 0).Format(time.RFC3339),
	}, nil
}

func handleNotificationMarkRead(ctx context.Context, toolCtx *mcpctx.ToolContext, input NotificationInput) (*mcp.CallToolResult, any, error) {
	if input.ID == "" {
		return nil, nil, mcpctx.NewValidationError("id is required", "id")
	}

	// First verify the notification exists and belongs to user
	_, err := toolCtx.DB().GetNotification(ctx, db.GetNotificationParams{
		ID:     input.ID,
		UserID: toolCtx.UserID(),
	})
	if err != nil {
		return nil, nil, mcpctx.NewNotFoundError(fmt.Sprintf("notification %s not found", input.ID))
	}

	err = toolCtx.DB().MarkNotificationRead(ctx, db.MarkNotificationReadParams{
		ID:     input.ID,
		UserID: toolCtx.UserID(),
	})
	if err != nil {
		return nil, nil, fmt.Errorf("failed to mark notification as read: %w", err)
	}

	return nil, NotificationMarkReadOutput{
		ID:      input.ID,
		Read:    true,
		Success: true,
	}, nil
}

func handleNotificationMarkAllRead(ctx context.Context, toolCtx *mcpctx.ToolContext, input NotificationInput) (*mcp.CallToolResult, any, error) {
	err := toolCtx.DB().MarkAllNotificationsRead(ctx, toolCtx.UserID())
	if err != nil {
		return nil, nil, fmt.Errorf("failed to mark all notifications as read: %w", err)
	}

	return nil, NotificationMarkAllReadOutput{
		Success: true,
		Message: "All notifications marked as read",
	}, nil
}

func handleNotificationCountUnread(ctx context.Context, toolCtx *mcpctx.ToolContext, input NotificationInput) (*mcp.CallToolResult, any, error) {
	count, err := toolCtx.DB().CountUnreadNotifications(ctx, toolCtx.UserID())
	if err != nil {
		return nil, nil, fmt.Errorf("failed to count unread notifications: %w", err)
	}

	return nil, NotificationCountOutput{
		Count: count,
	}, nil
}

// registerNotificationToolToRegistry registers notification tool to the direct-call registry.
func registerNotificationToolToRegistry(registry *ToolRegistry, toolCtx *mcpctx.ToolContext) {
	registry.Register("notification", func(ctx context.Context, args json.RawMessage) (interface{}, error) {
		var input NotificationInput
		if err := json.Unmarshal(args, &input); err != nil {
			return nil, fmt.Errorf("invalid input: %w", err)
		}
		handler := notificationHandler(toolCtx)
		_, output, err := handler(ctx, nil, input)
		return output, err
	})
}
