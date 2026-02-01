package runner

import (
	"context"
	"database/sql"
	"encoding/json"
	"testing"
	"time"

	"nebo/agent/ai"
	"nebo/agent/config"
	"nebo/agent/session"
	"nebo/agent/tools"

	_ "modernc.org/sqlite"
)

// setupTestDB creates a test database with the required schema
func setupTestDB(t *testing.T, dbPath string) {
	t.Helper()

	db, err := sql.Open("sqlite", dbPath)
	if err != nil {
		t.Fatalf("failed to open database: %v", err)
	}
	defer db.Close()

	// Create sessions table (from 0010_auth_profiles.sql)
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
			metadata TEXT,
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
			created_at INTEGER NOT NULL
		)
	`)
	if err != nil {
		t.Fatalf("failed to create session_messages table: %v", err)
	}
}

// mockProvider implements ai.Provider for testing
type mockProvider struct {
	id       string
	events   []ai.StreamEvent
	err      error
	callCount int
}

func (m *mockProvider) ID() string {
	return m.id
}

func (m *mockProvider) Stream(ctx context.Context, req *ai.ChatRequest) (<-chan ai.StreamEvent, error) {
	m.callCount++
	if m.err != nil {
		return nil, m.err
	}

	ch := make(chan ai.StreamEvent)
	go func() {
		defer close(ch)
		for _, event := range m.events {
			select {
			case <-ctx.Done():
				return
			case ch <- event:
			}
		}
	}()

	return ch, nil
}

func TestNew(t *testing.T) {
	cfg := config.DefaultConfig()

	// Create temp db
	tmpDir := t.TempDir()
	dbPath := tmpDir + "/test.db"
	setupTestDB(t, dbPath)
	sessions, err := session.New(dbPath)
	if err != nil {
		t.Fatalf("failed to create session manager: %v", err)
	}
	defer sessions.Close()

	providers := []ai.Provider{
		&mockProvider{id: "test"},
	}
	registry := tools.NewRegistry(nil)

	r := New(cfg, sessions, providers, registry)

	if r == nil {
		t.Fatal("New returned nil")
	}
	if r.config != cfg {
		t.Error("config not set correctly")
	}
	if r.sessions != sessions {
		t.Error("sessions not set correctly")
	}
}

func TestRunNoProviders(t *testing.T) {
	cfg := config.DefaultConfig()

	tmpDir := t.TempDir()
	dbPath := tmpDir + "/test.db"
	setupTestDB(t, dbPath)
	sessions, err := session.New(dbPath)
	if err != nil {
		t.Fatalf("failed to create session manager: %v", err)
	}
	defer sessions.Close()

	r := New(cfg, sessions, nil, tools.NewRegistry(nil))

	_, err = r.Run(context.Background(), &RunRequest{
		Prompt: "Hello",
	})

	if err == nil {
		t.Error("expected error for no providers")
	}
}

func TestRunSimpleResponse(t *testing.T) {
	cfg := config.DefaultConfig()
	cfg.MaxIterations = 10

	tmpDir := t.TempDir()
	dbPath := tmpDir + "/test.db"
	setupTestDB(t, dbPath)
	sessions, err := session.New(dbPath)
	if err != nil {
		t.Fatalf("failed to create session manager: %v", err)
	}
	defer sessions.Close()

	// Mock provider that returns simple text
	provider := &mockProvider{
		id: "test",
		events: []ai.StreamEvent{
			{Type: ai.EventTypeText, Text: "Hello, "},
			{Type: ai.EventTypeText, Text: "world!"},
		},
	}

	r := New(cfg, sessions, []ai.Provider{provider}, tools.NewRegistry(nil))

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	events, err := r.Run(ctx, &RunRequest{
		Prompt: "Say hello",
	})
	if err != nil {
		t.Fatalf("Run failed: %v", err)
	}

	var receivedText string
	var gotDone bool

	for event := range events {
		switch event.Type {
		case ai.EventTypeText:
			receivedText += event.Text
		case ai.EventTypeDone:
			gotDone = true
		case ai.EventTypeError:
			t.Fatalf("unexpected error: %v", event.Error)
		}
	}

	if receivedText != "Hello, world!" {
		t.Errorf("expected 'Hello, world!', got %q", receivedText)
	}
	if !gotDone {
		t.Error("expected done event")
	}
}

func TestRunWithToolCall(t *testing.T) {
	cfg := config.DefaultConfig()
	cfg.MaxIterations = 10

	tmpDir := t.TempDir()
	dbPath := tmpDir + "/test.db"
	setupTestDB(t, dbPath)
	sessions, err := session.New(dbPath)
	if err != nil {
		t.Fatalf("failed to create session manager: %v", err)
	}
	defer sessions.Close()

	// First call returns a tool call, second call returns text
	callCount := 0
	provider := &mockProvider{
		id: "test",
	}

	// Override Stream to return different results
	originalStream := provider.Stream
	_ = originalStream

	// Create a custom provider for this test
	customProvider := &toolTestProvider{
		callCount: &callCount,
	}

	registry := tools.NewRegistry(nil)
	registry.RegisterDefaults()

	r := New(cfg, sessions, []ai.Provider{customProvider}, registry)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	events, err := r.Run(ctx, &RunRequest{
		SessionKey: "test-tool-session",
		Prompt:     "List files",
	})
	if err != nil {
		t.Fatalf("Run failed: %v", err)
	}

	var gotToolCall bool
	var gotToolResult bool

	for event := range events {
		switch event.Type {
		case ai.EventTypeToolCall:
			gotToolCall = true
		case ai.EventTypeToolResult:
			gotToolResult = true
		case ai.EventTypeError:
			// May error due to tool execution, that's ok for this test
		}
	}

	if !gotToolCall {
		t.Error("expected tool call event")
	}
	if !gotToolResult {
		t.Error("expected tool result event")
	}
}

// toolTestProvider returns a tool call on first request, text on second
type toolTestProvider struct {
	callCount *int
}

func (p *toolTestProvider) ID() string {
	return "tool-test"
}

func (p *toolTestProvider) Stream(ctx context.Context, req *ai.ChatRequest) (<-chan ai.StreamEvent, error) {
	*p.callCount++
	ch := make(chan ai.StreamEvent)

	go func() {
		defer close(ch)

		if *p.callCount == 1 {
			// First call: return a tool call
			ch <- ai.StreamEvent{
				Type: ai.EventTypeToolCall,
				ToolCall: &ai.ToolCall{
					ID:    "call_1",
					Name:  "glob",
					Input: json.RawMessage(`{"pattern": "*.go"}`),
				},
			}
		} else {
			// Subsequent calls: return text and finish
			ch <- ai.StreamEvent{Type: ai.EventTypeText, Text: "Done!"}
		}
	}()

	return ch, nil
}

func TestRunDefaultSessionKey(t *testing.T) {
	cfg := config.DefaultConfig()
	cfg.MaxIterations = 5

	tmpDir := t.TempDir()
	dbPath := tmpDir + "/test.db"
	setupTestDB(t, dbPath)
	sessions, err := session.New(dbPath)
	if err != nil {
		t.Fatalf("failed to create session manager: %v", err)
	}
	defer sessions.Close()

	provider := &mockProvider{
		id:     "test",
		events: []ai.StreamEvent{{Type: ai.EventTypeText, Text: "OK"}},
	}

	r := New(cfg, sessions, []ai.Provider{provider}, tools.NewRegistry(nil))

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Empty session key should use "default"
	events, err := r.Run(ctx, &RunRequest{
		SessionKey: "",
		Prompt:     "Hello",
	})
	if err != nil {
		t.Fatalf("Run failed: %v", err)
	}

	// Drain events
	for range events {
	}

	// Verify session was created with "default" key
	sess, err := sessions.GetOrCreate("default")
	if err != nil {
		t.Fatalf("failed to get default session: %v", err)
	}
	if sess.SessionKey != "default" {
		t.Errorf("expected session key 'default', got %s", sess.SessionKey)
	}
}

func TestChat(t *testing.T) {
	cfg := config.DefaultConfig()

	tmpDir := t.TempDir()
	dbPath := tmpDir + "/test.db"
	setupTestDB(t, dbPath)
	sessions, err := session.New(dbPath)
	if err != nil {
		t.Fatalf("failed to create session manager: %v", err)
	}
	defer sessions.Close()

	provider := &mockProvider{
		id: "test",
		events: []ai.StreamEvent{
			{Type: ai.EventTypeText, Text: "Hello!"},
		},
	}

	r := New(cfg, sessions, []ai.Provider{provider}, tools.NewRegistry(nil))

	result, err := r.Chat(context.Background(), "Hi")
	if err != nil {
		t.Fatalf("Chat failed: %v", err)
	}

	if result != "Hello!" {
		t.Errorf("expected 'Hello!', got %q", result)
	}
}

func TestChatNoProviders(t *testing.T) {
	cfg := config.DefaultConfig()

	tmpDir := t.TempDir()
	dbPath := tmpDir + "/test.db"
	setupTestDB(t, dbPath)
	sessions, err := session.New(dbPath)
	if err != nil {
		t.Fatalf("failed to create session manager: %v", err)
	}
	defer sessions.Close()

	r := New(cfg, sessions, nil, tools.NewRegistry(nil))

	_, err = r.Chat(context.Background(), "Hi")
	if err == nil {
		t.Error("expected error for no providers")
	}
}

func TestChatWithError(t *testing.T) {
	cfg := config.DefaultConfig()

	tmpDir := t.TempDir()
	dbPath := tmpDir + "/test.db"
	setupTestDB(t, dbPath)
	sessions, err := session.New(dbPath)
	if err != nil {
		t.Fatalf("failed to create session manager: %v", err)
	}
	defer sessions.Close()

	provider := &mockProvider{
		id: "test",
		events: []ai.StreamEvent{
			{Type: ai.EventTypeText, Text: "Partial"},
			{Type: ai.EventTypeError, Error: &ai.ProviderError{Message: "test error"}},
		},
	}

	r := New(cfg, sessions, []ai.Provider{provider}, tools.NewRegistry(nil))

	result, err := r.Chat(context.Background(), "Hi")

	// Should return partial result and error
	if result != "Partial" {
		t.Errorf("expected 'Partial', got %q", result)
	}
	if err == nil {
		t.Error("expected error")
	}
}

func TestDefaultSystemPrompt(t *testing.T) {
	if DefaultSystemPrompt == "" {
		t.Error("DefaultSystemPrompt is empty")
	}

	// Check it mentions key tools
	if !contains(DefaultSystemPrompt, "read") {
		t.Error("system prompt should mention 'read' tool")
	}
	if !contains(DefaultSystemPrompt, "bash") {
		t.Error("system prompt should mention 'bash' tool")
	}
}

func contains(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || contains(s[1:], substr) || s[:len(substr)] == substr)
}

func TestGenerateSummary(t *testing.T) {
	cfg := config.DefaultConfig()

	tmpDir := t.TempDir()
	dbPath := tmpDir + "/test.db"
	setupTestDB(t, dbPath)
	sessions, err := session.New(dbPath)
	if err != nil {
		t.Fatalf("failed to create session manager: %v", err)
	}
	defer sessions.Close()

	r := New(cfg, sessions, nil, tools.NewRegistry(nil))

	messages := []session.Message{
		{Role: "user", Content: "Hello"},
		{Role: "assistant", Content: "Hi there!"},
		{Role: "user", Content: "How are you?"},
	}

	summary := r.generateSummary(context.Background(), messages)

	if summary == "" {
		t.Error("summary should not be empty")
	}
	if !contains(summary, "Hello") {
		t.Error("summary should contain user message")
	}
}
