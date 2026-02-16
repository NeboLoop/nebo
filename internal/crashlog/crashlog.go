package crashlog

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"runtime"
	"sync"

	"github.com/neboloop/nebo/internal/db"
)

// Logger persists errors and panics to the error_logs table.
// Safe for concurrent use from multiple goroutines.
type Logger struct {
	queries *db.Queries
	mu      sync.Mutex
}

var (
	global   *Logger
	globalMu sync.Mutex
)

// Init sets up the global crash logger. Call once at startup.
func Init(sqlDB *sql.DB) {
	globalMu.Lock()
	defer globalMu.Unlock()
	global = &Logger{queries: db.New(sqlDB)}
}

// LogPanic records a recovered panic with a full stack trace.
// Safe to call even if Init() was never called (prints to stdout as fallback).
func LogPanic(module string, r any, ctx map[string]string) {
	msg := fmt.Sprintf("%v", r)
	stack := make([]byte, 4096)
	n := runtime.Stack(stack, false)
	stackStr := string(stack[:n])

	// Always print to stdout for immediate visibility
	fmt.Printf("[PANIC] %s: %s\n%s\n", module, msg, stackStr)

	globalMu.Lock()
	l := global
	globalMu.Unlock()

	if l == nil {
		return
	}

	l.insert("panic", module, msg, stackStr, ctx)
}

// LogError records an error with optional context.
func LogError(module string, err error, ctx map[string]string) {
	if err == nil {
		return
	}

	globalMu.Lock()
	l := global
	globalMu.Unlock()

	if l == nil {
		fmt.Printf("[ERROR] %s: %v\n", module, err)
		return
	}

	l.insert("error", module, err.Error(), "", ctx)
}

// LogWarn records a warning.
func LogWarn(module string, msg string, ctx map[string]string) {
	globalMu.Lock()
	l := global
	globalMu.Unlock()

	if l == nil {
		fmt.Printf("[WARN] %s: %s\n", module, msg)
		return
	}

	l.insert("warn", module, msg, "", ctx)
}

func (l *Logger) insert(level, module, message, stacktrace string, ctx map[string]string) {
	l.mu.Lock()
	defer l.mu.Unlock()

	var ctxJSON sql.NullString
	if len(ctx) > 0 {
		if b, err := json.Marshal(ctx); err == nil {
			ctxJSON = sql.NullString{String: string(b), Valid: true}
		}
	}

	var stackNull sql.NullString
	if stacktrace != "" {
		stackNull = sql.NullString{String: stacktrace, Valid: true}
	}

	_ = l.queries.InsertErrorLog(context.Background(), db.InsertErrorLogParams{
		Level:      level,
		Module:     module,
		Message:    message,
		Stacktrace: stackNull,
		Context:    ctxJSON,
	})
}
