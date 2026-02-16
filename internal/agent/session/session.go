// Package session provides backward compatibility aliases for the session manager.
// The canonical implementation is in internal/db/session_manager.go
package session

import (
	"database/sql"

	"github.com/neboloop/nebo/internal/db"
)

// Type aliases for backward compatibility
type (
	Manager     = db.SessionManager
	Session     = db.AgentSession
	Message     = db.AgentMessage
	ToolCall    = db.AgentToolCall
	ToolResult  = db.AgentToolResult
)

// New creates a session manager from a raw database connection.
// This is the only approved way to create a session manager in the agent.
func New(sqlDB *sql.DB) (*Manager, error) {
	if sqlDB == nil {
		return nil, db.ErrDatabaseRequired
	}
	return db.NewSessionManagerFromDB(sqlDB), nil
}

// NewFromStore creates a session manager from a db.Store.
func NewFromStore(store *db.Store) *Manager {
	return db.NewSessionManager(store)
}
