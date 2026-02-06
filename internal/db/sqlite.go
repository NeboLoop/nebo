package db

import (
	"database/sql"
	"fmt"
	"os"
	"path/filepath"

	"github.com/nebolabs/nebo/internal/db/migrations"

	_ "modernc.org/sqlite" // Pure Go SQLite driver (no CGO)

	"github.com/nebolabs/nebo/internal/logging"
)

// NewSQLite creates a new SQLite database connection, runs migrations, and returns a Store
func NewSQLite(path string) (*Store, error) {
	// Ensure directory exists
	dir := filepath.Dir(path)
	if dir != "" && dir != "." {
		if err := os.MkdirAll(dir, 0755); err != nil {
			return nil, fmt.Errorf("failed to create database directory: %w", err)
		}
	}

	// Open database with WAL mode and single connection (no concurrency)
	db, err := sql.Open("sqlite", path+"?_pragma=journal_mode(WAL)&_pragma=synchronous(NORMAL)&_pragma=cache_size(1000000000)&_pragma=foreign_keys(1)")
	if err != nil {
		return nil, fmt.Errorf("failed to open database: %w", err)
	}

	// CRITICAL: Force single connection - SQLite doesn't handle concurrent writers well
	// All DB access must be serialized through this single connection
	db.SetMaxOpenConns(1)
	db.SetMaxIdleConns(1)

	// Test connection
	if err := db.Ping(); err != nil {
		return nil, fmt.Errorf("failed to ping database: %w", err)
	}

	// Run goose migrations
	if err := migrations.Run(db); err != nil {
		return nil, fmt.Errorf("failed to run migrations: %w", err)
	}

	logging.Infof("SQLite database initialized at %s", path)
	return NewStore(db), nil
}
