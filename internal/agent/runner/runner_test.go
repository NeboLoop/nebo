package runner

import (
	"context"
	"database/sql"
	"encoding/json"
	"path/filepath"
	"testing"
	"time"

	"github.com/neboloop/nebo/internal/agent/ai"
	"github.com/neboloop/nebo/internal/agent/config"
	"github.com/neboloop/nebo/internal/agent/session"
	"github.com/neboloop/nebo/internal/agent/tools"
	"github.com/neboloop/nebo/internal/provider"

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

// mockProvider implements ai.Provider for testing
type mockProvider struct {
	id        string
	events    []ai.StreamEvent
	err       error
	callCount int
}

func (m *mockProvider) ID() string {
	return m.id
}

func (m *mockProvider) ProfileID() string {
	return ""
}

func (m *mockProvider) HandlesTools() bool {
	return false
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

	db := openTestDB(t)
	defer db.Close()

	sessions, err := session.New(db)
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

	db := openTestDB(t)
	defer db.Close()

	sessions, err := session.New(db)
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

	db := openTestDB(t)
	defer db.Close()

	sessions, err := session.New(db)
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

	db := openTestDB(t)
	defer db.Close()

	sessions, err := session.New(db)
	if err != nil {
		t.Fatalf("failed to create session manager: %v", err)
	}
	defer sessions.Close()

	// First call returns a tool call, second call returns text
	callCount := 0

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

func (p *toolTestProvider) ProfileID() string {
	return ""
}

func (p *toolTestProvider) HandlesTools() bool {
	return false
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

	db := openTestDB(t)
	defer db.Close()

	sessions, err := session.New(db)
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
	sess, err := sessions.GetOrCreate("default", "")
	if err != nil {
		t.Fatalf("failed to get default session: %v", err)
	}
	if sess.SessionKey != "default" {
		t.Errorf("expected session key 'default', got %s", sess.SessionKey)
	}
}

func TestChat(t *testing.T) {
	cfg := config.DefaultConfig()

	db := openTestDB(t)
	defer db.Close()

	sessions, err := session.New(db)
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

	db := openTestDB(t)
	defer db.Close()

	sessions, err := session.New(db)
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

	db := openTestDB(t)
	defer db.Close()

	sessions, err := session.New(db)
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

	// Check it mentions key STRAP pattern tools
	if !contains(DefaultSystemPrompt, "file") {
		t.Error("system prompt should mention 'file' tool")
	}
	if !contains(DefaultSystemPrompt, "shell") {
		t.Error("system prompt should mention 'shell' tool")
	}
	if !contains(DefaultSystemPrompt, "web") {
		t.Error("system prompt should mention 'web' tool")
	}
}

func contains(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || contains(s[1:], substr) || s[:len(substr)] == substr)
}

func TestContextTokenLimit(t *testing.T) {
	cfg := config.DefaultConfig()

	db := openTestDB(t)
	defer db.Close()

	sessions, err := session.New(db)
	if err != nil {
		t.Fatalf("failed to create session manager: %v", err)
	}
	defer sessions.Close()

	t.Run("no selector uses default", func(t *testing.T) {
		r := New(cfg, sessions, nil, tools.NewRegistry(nil))
		limit := r.contextTokenLimit()
		if limit != DefaultContextTokenLimit {
			t.Errorf("expected %d without selector, got %d", DefaultContextTokenLimit, limit)
		}
	})

	t.Run("selector with 200k model", func(t *testing.T) {
		r := New(cfg, sessions, nil, tools.NewRegistry(nil))
		modelsConfig := &provider.ModelsConfig{
			Providers: map[string][]provider.ModelInfo{
				"anthropic": {
					{ID: "claude-sonnet-4-5", ContextWindow: 200000, Capabilities: []string{"general"}},
				},
			},
			Credentials: map[string]provider.ProviderCredentials{
				"anthropic": {APIKey: "test"},
			},
			Defaults: &provider.Defaults{Primary: "anthropic/claude-sonnet-4-5"},
		}
		r.selector = ai.NewModelSelector(modelsConfig)

		limit := r.contextTokenLimit()
		// (200000 - 20000) * 0.8 = 144000
		if limit != 144000 {
			t.Errorf("expected 144000 for 200k model, got %d", limit)
		}
	})

	t.Run("selector with small model falls back to default", func(t *testing.T) {
		r := New(cfg, sessions, nil, tools.NewRegistry(nil))
		modelsConfig := &provider.ModelsConfig{
			Providers: map[string][]provider.ModelInfo{
				"ollama": {
					{ID: "tiny", ContextWindow: 8000},
				},
			},
			Credentials: map[string]provider.ProviderCredentials{
				"ollama": {BaseURL: "http://localhost:11434"},
			},
			Defaults: &provider.Defaults{Primary: "ollama/tiny"},
		}
		r.selector = ai.NewModelSelector(modelsConfig)

		limit := r.contextTokenLimit()
		if limit != DefaultContextTokenLimit {
			t.Errorf("expected default %d for small model, got %d", DefaultContextTokenLimit, limit)
		}
	})

	t.Run("flush threshold is 75 percent of context limit", func(t *testing.T) {
		r := New(cfg, sessions, nil, tools.NewRegistry(nil))
		modelsConfig := &provider.ModelsConfig{
			Providers: map[string][]provider.ModelInfo{
				"anthropic": {
					{ID: "claude-sonnet-4-5", ContextWindow: 200000},
				},
			},
			Credentials: map[string]provider.ProviderCredentials{
				"anthropic": {APIKey: "test"},
			},
			Defaults: &provider.Defaults{Primary: "anthropic/claude-sonnet-4-5"},
		}
		r.selector = ai.NewModelSelector(modelsConfig)

		limit := r.contextTokenLimit()
		flush := r.memoryFlushThreshold()
		expected := limit * 75 / 100
		if flush != expected {
			t.Errorf("expected flush %d (75%% of %d), got %d", expected, limit, flush)
		}
	})

	t.Run("caps at 500k", func(t *testing.T) {
		r := New(cfg, sessions, nil, tools.NewRegistry(nil))
		modelsConfig := &provider.ModelsConfig{
			Providers: map[string][]provider.ModelInfo{
				"anthropic": {
					{ID: "claude-opus-4-6", ContextWindow: 1000000},
				},
			},
			Credentials: map[string]provider.ProviderCredentials{
				"anthropic": {APIKey: "test"},
			},
			Defaults: &provider.Defaults{Primary: "anthropic/claude-opus-4-6"},
		}
		r.selector = ai.NewModelSelector(modelsConfig)

		limit := r.contextTokenLimit()
		if limit != 500000 {
			t.Errorf("expected 500000 cap for 1M model, got %d", limit)
		}
	})
}

func TestGenerateSummary(t *testing.T) {
	cfg := config.DefaultConfig()

	db := openTestDB(t)
	defer db.Close()

	sessions, err := session.New(db)
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
