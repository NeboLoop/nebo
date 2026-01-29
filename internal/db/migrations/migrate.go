package migrations

import (
	"database/sql"
	"embed"
	"fmt"
	"log"

	"github.com/pressly/goose/v3"
)

//go:embed *.sql
var embedMigrations embed.FS

// QuietMode suppresses migration logging for clean CLI output
var QuietMode bool

// Run applies all pending database migrations.
// This is safe to call on every startup - it only applies migrations
// that haven't been applied yet (tracked in goose_db_version table).
func Run(db *sql.DB) error {
	goose.SetBaseFS(embedMigrations)

	if QuietMode {
		goose.SetLogger(log.New(nopWriter{}, "", 0))
	}

	if err := goose.SetDialect("sqlite3"); err != nil {
		return fmt.Errorf("failed to set dialect: %w", err)
	}

	if err := goose.Up(db, "."); err != nil {
		return fmt.Errorf("failed to run migrations: %w", err)
	}

	return nil
}

// nopWriter discards all writes
type nopWriter struct{}

func (nopWriter) Write(p []byte) (int, error) { return len(p), nil }

// Status prints the current migration status to stdout.
func Status(db *sql.DB) error {
	goose.SetBaseFS(embedMigrations)

	if err := goose.SetDialect("sqlite3"); err != nil {
		return fmt.Errorf("failed to set dialect: %w", err)
	}

	return goose.Status(db, ".")
}

// Down rolls back the last migration.
func Down(db *sql.DB) error {
	goose.SetBaseFS(embedMigrations)

	if err := goose.SetDialect("sqlite3"); err != nil {
		return fmt.Errorf("failed to set dialect: %w", err)
	}

	return goose.Down(db, ".")
}
