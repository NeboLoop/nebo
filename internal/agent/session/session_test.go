package session

import (
	"database/sql"
	"path/filepath"
	"testing"

	_ "modernc.org/sqlite"
)

// openTestDB creates and returns an open test database with the required schema
func openTestDB(t *testing.T) *sql.DB {
	t.Helper()

	tmpDir := t.TempDir()
	dbPath := filepath.Join(tmpDir, "test.db")

	db, err := sql.Open("sqlite", dbPath)
	if err != nil {
		t.Fatalf("failed to open database: %v", err)
	}

	// Create sessions table (matches current schema with migration 0024)
	_, err = db.Exec(`
		CREATE TABLE IF NOT EXISTS sessions (
			id TEXT PRIMARY KEY,
			name TEXT,
			scope TEXT DEFAULT 'global',
			scope_id TEXT,
			summary TEXT,
			token_count INTEGER DEFAULT 0,
			message_count INTEGER DEFAULT 0,
			last_compacted_at INTEGER,
			compaction_count INTEGER DEFAULT 0,
			memory_flush_at INTEGER,
			memory_flush_compaction_count INTEGER,
			metadata TEXT,
			send_policy TEXT DEFAULT 'allow',
			model_override TEXT,
			provider_override TEXT,
			auth_profile_override TEXT,
			auth_profile_override_source TEXT,
			verbose_level TEXT,
			custom_label TEXT,
			last_embedded_message_id INTEGER DEFAULT 0,
				active_task TEXT,
				last_summarized_count INTEGER DEFAULT 0,
				created_at INTEGER NOT NULL,
			updated_at INTEGER NOT NULL
		)
	`)
	if err != nil {
		t.Fatalf("failed to create sessions table: %v", err)
	}

	// Create session_messages table
	_, err = db.Exec(`
		CREATE TABLE IF NOT EXISTS session_messages (
			id INTEGER PRIMARY KEY AUTOINCREMENT,
			session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
			role TEXT NOT NULL,
			content TEXT,
			tool_calls TEXT,
			tool_results TEXT,
			token_estimate INTEGER DEFAULT 0,
			is_compacted INTEGER DEFAULT 0,
			created_at INTEGER NOT NULL DEFAULT (unixepoch())
		)
	`)
	if err != nil {
		t.Fatalf("failed to create session_messages table: %v", err)
	}

	return db
}

func TestSessionManager(t *testing.T) {
	db := openTestDB(t)
	defer db.Close()

	// Create manager
	manager, err := New(db)
	if err != nil {
		t.Fatalf("failed to create manager: %v", err)
	}
	defer manager.Close()

	// Test GetOrCreate
	sess, err := manager.GetOrCreate("test-session", "")
	if err != nil {
		t.Fatalf("failed to create session: %v", err)
	}

	if sess.SessionKey != "test-session" {
		t.Errorf("expected session key 'test-session', got %q", sess.SessionKey)
	}

	// Test getting the same session
	sess2, err := manager.GetOrCreate("test-session", "")
	if err != nil {
		t.Fatalf("failed to get session: %v", err)
	}

	if sess.ID != sess2.ID {
		t.Error("expected same session ID")
	}

	// Test AppendMessage
	err = manager.AppendMessage(sess.ID, Message{
		SessionID: sess.ID,
		Role:      "user",
		Content:   "hello",
	})
	if err != nil {
		t.Fatalf("failed to append message: %v", err)
	}

	// Test GetMessages
	messages, err := manager.GetMessages(sess.ID, 0)
	if err != nil {
		t.Fatalf("failed to get messages: %v", err)
	}

	if len(messages) != 1 {
		t.Errorf("expected 1 message, got %d", len(messages))
	}

	if messages[0].Content != "hello" {
		t.Errorf("expected content 'hello', got %q", messages[0].Content)
	}

	// Test Reset
	err = manager.Reset(sess.ID)
	if err != nil {
		t.Fatalf("failed to reset session: %v", err)
	}

	messages, _ = manager.GetMessages(sess.ID, 0)
	if len(messages) != 0 {
		t.Errorf("expected 0 messages after reset, got %d", len(messages))
	}
}

func TestSessionManagerWithLimit(t *testing.T) {
	db := openTestDB(t)
	defer db.Close()

	manager, err := New(db)
	if err != nil {
		t.Fatalf("failed to create manager: %v", err)
	}
	defer manager.Close()

	sess, _ := manager.GetOrCreate("limit-test", "")

	// Add 10 messages
	for i := 0; i < 10; i++ {
		manager.AppendMessage(sess.ID, Message{
			SessionID: sess.ID,
			Role:      "user",
			Content:   "message",
		})
	}

	// Get with limit of 5
	messages, _ := manager.GetMessages(sess.ID, 5)
	if len(messages) != 5 {
		t.Errorf("expected 5 messages with limit, got %d", len(messages))
	}
}

func TestListSessions(t *testing.T) {
	db := openTestDB(t)
	defer db.Close()

	manager, err := New(db)
	if err != nil {
		t.Fatalf("failed to create manager: %v", err)
	}
	defer manager.Close()

	// Create some sessions
	manager.GetOrCreate("session-1", "")
	manager.GetOrCreate("session-2", "")
	manager.GetOrCreate("session-3", "")

	sessions, err := manager.ListSessions("")
	if err != nil {
		t.Fatalf("failed to list sessions: %v", err)
	}

	if len(sessions) != 3 {
		t.Errorf("expected 3 sessions, got %d", len(sessions))
	}
}


