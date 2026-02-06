package mcpctx

import (
	"context"

	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/svc"
)

// AuthMode indicates how the MCP session was authenticated.
type AuthMode int

const (
	// AuthModeJWT means authenticated via JWT Bearer token (user-scoped).
	AuthModeJWT AuthMode = iota
)

// ToolContext carries context for all MCP tools.
type ToolContext struct {
	svc       *svc.ServiceContext
	requestID string
	userAgent string
	sessionID string

	// Auth mode
	authMode AuthMode

	// User is always set after authentication
	user *db.User
}

// NewToolContext creates a new user-scoped tool context.
func NewToolContext(svc *svc.ServiceContext, user db.User, requestID, userAgent, sessionID string) *ToolContext {
	return &ToolContext{
		svc:       svc,
		user:      &user,
		requestID: requestID,
		userAgent: userAgent,
		sessionID: sessionID,
		authMode:  AuthModeJWT,
	}
}

// SessionID returns the MCP session ID.
func (t *ToolContext) SessionID() string {
	return t.sessionID
}

// AuthMode returns the authentication mode.
func (t *ToolContext) AuthMode() AuthMode {
	return t.authMode
}

// User returns the authenticated user.
func (t *ToolContext) User() *db.User {
	return t.user
}

// UserID returns the authenticated user's ID.
func (t *ToolContext) UserID() string {
	if t.user == nil {
		return ""
	}
	return t.user.ID
}

// DB returns the database store for queries.
func (t *ToolContext) DB() *db.Store {
	return t.svc.DB
}

// Svc returns the full service context for advanced operations.
func (t *ToolContext) Svc() *svc.ServiceContext {
	return t.svc
}

// RequestID returns the request ID for tracing.
func (t *ToolContext) RequestID() string {
	return t.requestID
}

// UserAgent returns the client's user agent string.
func (t *ToolContext) UserAgent() string {
	return t.userAgent
}

// ToolError represents a structured error for MCP tool responses.
type ToolError struct {
	Code    string `json:"code"`    // "not_found", "validation", "conflict", "unauthorized"
	Message string `json:"message"` // Human-readable description
	Field   string `json:"field"`   // For validation errors
}

func (e *ToolError) Error() string {
	if e.Field != "" {
		return e.Code + ": " + e.Message + " (field: " + e.Field + ")"
	}
	return e.Code + ": " + e.Message
}

// NewValidationError creates a validation error for a specific field.
func NewValidationError(message, field string) *ToolError {
	return &ToolError{Code: "validation", Message: message, Field: field}
}

// NewNotFoundError creates a not found error.
func NewNotFoundError(message string) *ToolError {
	return &ToolError{Code: "not_found", Message: message}
}

// NewConflictError creates a conflict error (duplicate, already exists).
func NewConflictError(message string) *ToolError {
	return &ToolError{Code: "conflict", Message: message}
}

// NewUnauthorizedError creates an unauthorized error.
func NewUnauthorizedError(message string) *ToolError {
	return &ToolError{Code: "unauthorized", Message: message}
}

// toolContextKey is used to store ToolContext in context.Context
type toolContextKey struct{}

// WithToolContext adds ToolContext to a context.
func WithToolContext(ctx context.Context, tc *ToolContext) context.Context {
	return context.WithValue(ctx, toolContextKey{}, tc)
}

// ToolContextFromContext retrieves ToolContext from a context.
func ToolContextFromContext(ctx context.Context) *ToolContext {
	if tc, ok := ctx.Value(toolContextKey{}).(*ToolContext); ok {
		return tc
	}
	return nil
}
