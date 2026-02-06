package db

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
)

// Common errors
var (
	ErrDatabaseRequired = errors.New("database connection is required")
)

// Store provides all database operations with transaction support
type Store struct {
	*Queries
	db *sql.DB
}

// NewStore creates a new Store instance
func NewStore(db *sql.DB) *Store {
	return &Store{
		Queries: New(db),
		db:      db,
	}
}

// ExecTx executes a function within a database transaction
// If the function returns an error, the transaction is rolled back
// Otherwise, the transaction is committed
func (s *Store) ExecTx(ctx context.Context, fn func(*Queries) error) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return fmt.Errorf("failed to begin transaction: %w", err)
	}

	q := New(tx)
	err = fn(q)
	if err != nil {
		if rbErr := tx.Rollback(); rbErr != nil {
			return fmt.Errorf("tx error: %v, rb error: %w", err, rbErr)
		}
		return err
	}

	if err := tx.Commit(); err != nil {
		return fmt.Errorf("failed to commit transaction: %w", err)
	}

	return nil
}

// ExecTxWithResult executes a function within a transaction and returns a result
// If the function returns an error, the transaction is rolled back
// Otherwise, the transaction is committed and the result is returned
func ExecTxWithResult[T any](s *Store, ctx context.Context, fn func(*Queries) (T, error)) (T, error) {
	var result T

	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return result, fmt.Errorf("failed to begin transaction: %w", err)
	}

	q := New(tx)
	result, err = fn(q)
	if err != nil {
		if rbErr := tx.Rollback(); rbErr != nil {
			return result, fmt.Errorf("tx error: %v, rb error: %w", err, rbErr)
		}
		return result, err
	}

	if err := tx.Commit(); err != nil {
		return result, fmt.Errorf("failed to commit transaction: %w", err)
	}

	return result, nil
}

// GetDB returns the underlying database connection
func (s *Store) GetDB() *sql.DB {
	return s.db
}

// Ping verifies the database connection is alive
func (s *Store) Ping(ctx context.Context) error {
	return s.db.PingContext(ctx)
}

// Close closes the database connection
func (s *Store) Close() error {
	return s.db.Close()
}
