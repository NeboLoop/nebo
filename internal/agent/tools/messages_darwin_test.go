//go:build darwin && !ios

package tools

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"path/filepath"
	"strings"
	"testing"
	"time"

	_ "modernc.org/sqlite"
)

// =============================================================================
// recipient() parsing tests
// =============================================================================

func TestMessagesRecipientString(t *testing.T) {
	p := messagesInput{To: json.RawMessage(`"+15551234567"`)}
	if got := p.recipient(); got != "+15551234567" {
		t.Errorf("recipient() = %q, want %q", got, "+15551234567")
	}
}

func TestMessagesRecipientArray(t *testing.T) {
	p := messagesInput{To: json.RawMessage(`["+15551234567"]`)}
	if got := p.recipient(); got != "+15551234567" {
		t.Errorf("recipient() = %q, want %q", got, "+15551234567")
	}
}

func TestMessagesRecipientArrayMultiple(t *testing.T) {
	p := messagesInput{To: json.RawMessage(`["+15551234567", "+15559876543"]`)}
	if got := p.recipient(); got != "+15551234567" {
		t.Errorf("recipient() = %q, want first element %q", got, "+15551234567")
	}
}

func TestMessagesRecipientEmpty(t *testing.T) {
	p := messagesInput{To: nil}
	if got := p.recipient(); got != "" {
		t.Errorf("recipient() = %q, want empty", got)
	}
}

func TestMessagesRecipientEmptyArray(t *testing.T) {
	p := messagesInput{To: json.RawMessage(`[]`)}
	if got := p.recipient(); got != "" {
		t.Errorf("recipient() = %q, want empty for empty array", got)
	}
}

func TestMessagesRecipientEmail(t *testing.T) {
	p := messagesInput{To: json.RawMessage(`"alice@example.com"`)}
	if got := p.recipient(); got != "alice@example.com" {
		t.Errorf("recipient() = %q, want %q", got, "alice@example.com")
	}
}

// =============================================================================
// cocoaToTime() tests
// =============================================================================

func TestCocoaToTime(t *testing.T) {
	tests := []struct {
		name       string
		cocoaNanos int64
		wantYear   int
		wantMonth  time.Month
		wantDay    int
	}{
		{
			name:       "cocoa epoch zero is 2001-01-01",
			cocoaNanos: 0,
			wantYear:   2001,
			wantMonth:  time.January,
			wantDay:    1,
		},
		{
			name:       "known date 2024-06-15",
			cocoaNanos: 740102400 * 1e9, // seconds since 2001-01-01 to 2024-06-15 00:00 UTC
			wantYear:   2024,
			wantMonth:  time.June,
			wantDay:    15,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := cocoaToTime(tt.cocoaNanos).UTC()
			if got.Year() != tt.wantYear || got.Month() != tt.wantMonth || got.Day() != tt.wantDay {
				t.Errorf("cocoaToTime(%d) = %v, want %d-%02d-%02d",
					tt.cocoaNanos, got, tt.wantYear, tt.wantMonth, tt.wantDay)
			}
		})
	}
}

// =============================================================================
// Execute() routing and input validation tests
// =============================================================================

func TestMessagesExecuteUnknownAction(t *testing.T) {
	tool := NewMessagesTool()
	input, _ := json.Marshal(map[string]string{"action": "delete"})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatal(err)
	}
	if !result.IsError {
		t.Error("expected error for unknown action")
	}
	if !strings.Contains(result.Content, "Unknown action") {
		t.Errorf("expected 'Unknown action' in error, got: %s", result.Content)
	}
}

func TestMessagesExecuteBadJSON(t *testing.T) {
	tool := NewMessagesTool()
	result, err := tool.Execute(context.Background(), json.RawMessage(`{bad json`))
	if err != nil {
		t.Fatal(err)
	}
	if !result.IsError {
		t.Error("expected error for bad JSON")
	}
}

func TestMessagesSendMissingRecipient(t *testing.T) {
	tool := NewMessagesTool()
	input, _ := json.Marshal(map[string]string{"action": "send", "body": "hello"})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatal(err)
	}
	if !result.IsError {
		t.Error("expected error for missing recipient")
	}
	if !strings.Contains(result.Content, "Recipient") {
		t.Errorf("expected 'Recipient' in error, got: %s", result.Content)
	}
}

func TestMessagesSendMissingBody(t *testing.T) {
	tool := NewMessagesTool()
	input, _ := json.Marshal(map[string]string{"action": "send", "to": "+15551234567"})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatal(err)
	}
	if !result.IsError {
		t.Error("expected error for missing body")
	}
	if !strings.Contains(result.Content, "body") {
		t.Errorf("expected 'body' in error, got: %s", result.Content)
	}
}

func TestMessagesReadMissingChatID(t *testing.T) {
	tool := NewMessagesTool()
	input, _ := json.Marshal(map[string]string{"action": "read"})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatal(err)
	}
	if !result.IsError {
		t.Error("expected error for missing chat_id")
	}
	if !strings.Contains(result.Content, "chat_id") {
		t.Errorf("expected 'chat_id' in error, got: %s", result.Content)
	}
}

func TestMessagesSearchMissingQuery(t *testing.T) {
	tool := NewMessagesTool()
	input, _ := json.Marshal(map[string]string{"action": "search"})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatal(err)
	}
	if !result.IsError {
		t.Error("expected error for missing query")
	}
	if !strings.Contains(result.Content, "query") {
		t.Errorf("expected 'query' in error, got: %s", result.Content)
	}
}

// =============================================================================
// SQLite-based tests with a temporary chat.db
// =============================================================================

// createTestChatDB creates a temporary SQLite database mimicking ~/Library/Messages/chat.db
// with the minimum schema needed for our queries.
func createTestChatDB(t *testing.T) string {
	t.Helper()
	dir := t.TempDir()
	dbPath := filepath.Join(dir, "chat.db")

	db, err := sql.Open("sqlite", dbPath)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()

	// Create the tables matching Apple's Messages schema
	schema := `
		CREATE TABLE handle (
			ROWID INTEGER PRIMARY KEY,
			id TEXT NOT NULL
		);
		CREATE TABLE chat (
			ROWID INTEGER PRIMARY KEY,
			chat_identifier TEXT NOT NULL,
			display_name TEXT DEFAULT ''
		);
		CREATE TABLE message (
			ROWID INTEGER PRIMARY KEY,
			handle_id INTEGER,
			text TEXT,
			date INTEGER NOT NULL,
			is_from_me INTEGER DEFAULT 0
		);
		CREATE TABLE chat_message_join (
			chat_id INTEGER NOT NULL,
			message_id INTEGER NOT NULL
		);
	`
	if _, err := db.Exec(schema); err != nil {
		t.Fatal(err)
	}

	// Insert test data
	// Handle: the contact
	db.Exec(`INSERT INTO handle (ROWID, id) VALUES (1, '+15551234567')`)
	db.Exec(`INSERT INTO handle (ROWID, id) VALUES (2, 'alice@example.com')`)

	// Chats
	db.Exec(`INSERT INTO chat (ROWID, chat_identifier, display_name) VALUES (1, '+15551234567', 'John Doe')`)
	db.Exec(`INSERT INTO chat (ROWID, chat_identifier, display_name) VALUES (2, 'alice@example.com', 'Alice')`)

	// Messages — use Cocoa timestamps (nanoseconds since 2001-01-01)
	// 2024-01-15 10:00:00 UTC → 726829200 seconds since 2001-01-01
	baseNanos := int64(726829200) * 1e9

	db.Exec(`INSERT INTO message (ROWID, handle_id, text, date, is_from_me) VALUES (1, 1, 'Hey, how are you?', ?, 0)`, baseNanos)
	db.Exec(`INSERT INTO message (ROWID, handle_id, text, date, is_from_me) VALUES (2, NULL, 'Doing great!', ?, 1)`, baseNanos+int64(60*1e9))
	db.Exec(`INSERT INTO message (ROWID, handle_id, text, date, is_from_me) VALUES (3, 1, 'Want to grab lunch?', ?, 0)`, baseNanos+int64(120*1e9))
	db.Exec(`INSERT INTO message (ROWID, handle_id, text, date, is_from_me) VALUES (4, 2, 'Meeting at 3pm', ?, 0)`, baseNanos+int64(180*1e9))
	db.Exec(`INSERT INTO message (ROWID, handle_id, text, date, is_from_me) VALUES (5, NULL, 'Sounds good', ?, 1)`, baseNanos+int64(240*1e9))

	// Join table
	db.Exec(`INSERT INTO chat_message_join (chat_id, message_id) VALUES (1, 1)`)
	db.Exec(`INSERT INTO chat_message_join (chat_id, message_id) VALUES (1, 2)`)
	db.Exec(`INSERT INTO chat_message_join (chat_id, message_id) VALUES (1, 3)`)
	db.Exec(`INSERT INTO chat_message_join (chat_id, message_id) VALUES (2, 4)`)
	db.Exec(`INSERT INTO chat_message_join (chat_id, message_id) VALUES (2, 5)`)

	return dbPath
}

// testOpenChatDB opens a test database instead of the real one.
func testOpenChatDB(dbPath string) (*sql.DB, error) {
	db, err := sql.Open("sqlite", dbPath+"?_pragma=query_only(1)")
	if err != nil {
		return nil, fmt.Errorf("cannot open test chat.db: %w", err)
	}
	return db, nil
}

// newTestMessagesTool creates a MessagesTool pre-wired to a test chat.db
// so Execute() tests go through the real code path without touching ~/Library.
func newTestMessagesTool(t *testing.T) *MessagesTool {
	t.Helper()
	dbPath := createTestChatDB(t)
	db, err := sql.Open("sqlite", dbPath+"?_pragma=query_only(1)")
	if err != nil {
		t.Fatal(err)
	}
	db.SetMaxOpenConns(1)
	tool := &MessagesTool{db: db}
	// Mark sync.Once as done so chatDB() returns our test DB.
	tool.dbOnce.Do(func() {})
	return tool
}

func TestMessagesListConversationsWithTestDB(t *testing.T) {
	dbPath := createTestChatDB(t)

	db, err := testOpenChatDB(dbPath)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()

	// Run the same query the tool uses
	rows, err := db.Query(`
		SELECT
			c.chat_identifier,
			c.display_name,
			COALESCE(MAX(m.date), 0) as last_date,
			COUNT(m.ROWID) as msg_count
		FROM chat c
		LEFT JOIN chat_message_join cmj ON cmj.chat_id = c.ROWID
		LEFT JOIN message m ON m.ROWID = cmj.message_id
		GROUP BY c.ROWID
		ORDER BY last_date DESC
		LIMIT 20
	`)
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()

	type conv struct {
		chatID, displayName string
		lastDate            int64
		msgCount            int
	}
	var convs []conv
	for rows.Next() {
		var c conv
		if err := rows.Scan(&c.chatID, &c.displayName, &c.lastDate, &c.msgCount); err != nil {
			t.Fatal(err)
		}
		convs = append(convs, c)
	}

	if len(convs) != 2 {
		t.Fatalf("expected 2 conversations, got %d", len(convs))
	}

	// Alice's chat has the latest message, should be first
	if convs[0].chatID != "alice@example.com" {
		t.Errorf("expected alice@example.com first (latest message), got %s", convs[0].chatID)
	}
	if convs[0].msgCount != 2 {
		t.Errorf("expected 2 messages for Alice, got %d", convs[0].msgCount)
	}
	if convs[1].chatID != "+15551234567" {
		t.Errorf("expected +15551234567 second, got %s", convs[1].chatID)
	}
	if convs[1].msgCount != 3 {
		t.Errorf("expected 3 messages for John, got %d", convs[1].msgCount)
	}
}

func TestMessagesReadWithTestDB(t *testing.T) {
	dbPath := createTestChatDB(t)

	db, err := testOpenChatDB(dbPath)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()

	rows, err := db.Query(`
		SELECT
			m.is_from_me,
			COALESCE(m.text, '') as text,
			m.date,
			COALESCE(h.id, '') as sender
		FROM message m
		JOIN chat_message_join cmj ON cmj.message_id = m.ROWID
		JOIN chat c ON c.ROWID = cmj.chat_id
		LEFT JOIN handle h ON h.ROWID = m.handle_id
		WHERE c.chat_identifier = ?
		ORDER BY m.date DESC
		LIMIT 20
	`, "+15551234567")
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()

	var count int
	var foundLunch, foundGreat bool
	for rows.Next() {
		var isFromMe int
		var text, sender string
		var date int64
		if err := rows.Scan(&isFromMe, &text, &date, &sender); err != nil {
			t.Fatal(err)
		}
		count++
		if strings.Contains(text, "lunch") {
			foundLunch = true
		}
		if strings.Contains(text, "great") {
			foundGreat = true
			if isFromMe != 1 {
				t.Error("'Doing great!' should be is_from_me=1")
			}
		}
	}

	if count != 3 {
		t.Errorf("expected 3 messages for +15551234567, got %d", count)
	}
	if !foundLunch {
		t.Error("expected to find 'lunch' message")
	}
	if !foundGreat {
		t.Error("expected to find 'great' message")
	}
}

func TestMessagesSearchWithTestDB(t *testing.T) {
	dbPath := createTestChatDB(t)

	db, err := testOpenChatDB(dbPath)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()

	rows, err := db.Query(`
		SELECT
			m.is_from_me,
			COALESCE(m.text, '') as text,
			m.date,
			COALESCE(h.id, '') as sender,
			c.chat_identifier
		FROM message m
		JOIN chat_message_join cmj ON cmj.message_id = m.ROWID
		JOIN chat c ON c.ROWID = cmj.chat_id
		LEFT JOIN handle h ON h.ROWID = m.handle_id
		WHERE m.text LIKE ?
		ORDER BY m.date DESC
		LIMIT 20
	`, "%lunch%")
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()

	var count int
	for rows.Next() {
		var isFromMe int
		var text, sender, chatID string
		var date int64
		if err := rows.Scan(&isFromMe, &text, &date, &sender, &chatID); err != nil {
			t.Fatal(err)
		}
		count++
		if chatID != "+15551234567" {
			t.Errorf("expected lunch message in +15551234567 chat, got %s", chatID)
		}
	}

	if count != 1 {
		t.Errorf("expected 1 search result for 'lunch', got %d", count)
	}
}

func TestMessagesSearchNoResultsWithTestDB(t *testing.T) {
	dbPath := createTestChatDB(t)

	db, err := testOpenChatDB(dbPath)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()

	rows, err := db.Query(`
		SELECT COUNT(*) FROM message m
		JOIN chat_message_join cmj ON cmj.message_id = m.ROWID
		WHERE m.text LIKE ?
	`, "%nonexistent_xyz%")
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()

	var count int
	if rows.Next() {
		rows.Scan(&count)
	}
	if count != 0 {
		t.Errorf("expected 0 results for nonexistent search, got %d", count)
	}
}

func TestMessagesReadOnlyDB(t *testing.T) {
	dbPath := createTestChatDB(t)

	db, err := testOpenChatDB(dbPath)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()

	// Attempting to write should fail since we open with query_only(1)
	_, err = db.Exec(`INSERT INTO message (ROWID, handle_id, text, date) VALUES (99, 1, 'injected', 0)`)
	if err == nil {
		t.Error("expected write to fail on read-only database")
	}
}

// =============================================================================
// isFDAError tests
// =============================================================================

func TestIsFDAError(t *testing.T) {
	tests := []struct {
		name string
		err  error
		want bool
	}{
		{"nil error", nil, false},
		{"operation not permitted", fmt.Errorf("operation not permitted"), true},
		{"unable to open database file", fmt.Errorf("unable to open database file: out of memory (14)"), true},
		{"cantopen", fmt.Errorf("sqlite: CANTOPEN"), true},
		{"authorization denied", fmt.Errorf("authorization denied"), true},
		{"unrelated error", fmt.Errorf("table not found"), false},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := isFDAError(tt.err); got != tt.want {
				t.Errorf("isFDAError(%v) = %v, want %v", tt.err, got, tt.want)
			}
		})
	}
}

// =============================================================================
// PIM domain integration tests
// =============================================================================

func TestPIMDomainIncludesMessages(t *testing.T) {
	tool := NewPIMDomainTool(nil, nil, nil, nil, NewMessagesTool())

	resources := tool.Resources()
	found := false
	for _, r := range resources {
		if r == "messages" {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("expected 'messages' in resources, got: %v", resources)
	}
}

func TestPIMDomainMessagesActions(t *testing.T) {
	tool := NewPIMDomainTool(nil, nil, nil, nil, NewMessagesTool())

	actions := tool.ActionsFor("messages")
	expected := []string{"send", "conversations", "read", "search"}
	if len(actions) != len(expected) {
		t.Fatalf("expected %d actions, got %d: %v", len(expected), len(actions), actions)
	}
	for i, a := range expected {
		if actions[i] != a {
			t.Errorf("action[%d] = %q, want %q", i, actions[i], a)
		}
	}
}

func TestPIMDomainInferResourceConversations(t *testing.T) {
	tool := NewPIMDomainTool(nil, nil, nil, nil, NewMessagesTool())
	if got := tool.inferResource("conversations"); got != "messages" {
		t.Errorf("inferResource(conversations) = %q, want 'messages'", got)
	}
}

func TestPIMDomainNilMessages(t *testing.T) {
	tool := NewPIMDomainTool(nil, nil, nil, nil, nil)

	resources := tool.Resources()
	for _, r := range resources {
		if r == "messages" {
			t.Error("messages should not be in resources when nil")
		}
	}
}

func TestPIMDomainMessagesRouting(t *testing.T) {
	tool := NewPIMDomainTool(nil, nil, nil, nil, NewMessagesTool())

	// Should route to messages and return validation error (missing chat_id)
	input, _ := json.Marshal(map[string]string{
		"resource": "messages",
		"action":   "read",
	})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatal(err)
	}
	if !result.IsError {
		t.Error("expected error for missing chat_id")
	}
	if !strings.Contains(result.Content, "chat_id") {
		t.Errorf("expected 'chat_id' in error, got: %s", result.Content)
	}
}

func TestPIMDomainMessagesInferredRouting(t *testing.T) {
	tool := NewPIMDomainTool(nil, nil, nil, nil, NewMessagesTool())

	// Action "conversations" should infer resource "messages"
	input, _ := json.Marshal(map[string]string{
		"action": "conversations",
	})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatal(err)
	}
	// It will try to open the real chat.db and likely fail in CI/test,
	// but we're testing routing, not DB access. The error should NOT be
	// about missing resource.
	if result.IsError && strings.Contains(result.Content, "Resource is required") {
		t.Error("conversations should infer messages resource, but got 'Resource is required'")
	}
}

// =============================================================================
// Execute integration tests (pre-wired test DB)
// =============================================================================

func TestMessagesExecuteConversationsWithTestDB(t *testing.T) {
	tool := newTestMessagesTool(t)
	input, _ := json.Marshal(map[string]string{"action": "conversations"})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatal(err)
	}
	if result.IsError {
		t.Fatalf("unexpected error: %s", result.Content)
	}
	if !strings.Contains(result.Content, "+15551234567") {
		t.Errorf("expected +15551234567 in output, got: %s", result.Content)
	}
	if !strings.Contains(result.Content, "Alice") {
		t.Errorf("expected Alice in output, got: %s", result.Content)
	}
}

func TestMessagesExecuteReadWithTestDB(t *testing.T) {
	tool := newTestMessagesTool(t)
	input, _ := json.Marshal(map[string]string{"action": "read", "chat_id": "+15551234567"})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatal(err)
	}
	if result.IsError {
		t.Fatalf("unexpected error: %s", result.Content)
	}
	if !strings.Contains(result.Content, "lunch") {
		t.Errorf("expected 'lunch' in output, got: %s", result.Content)
	}
	if !strings.Contains(result.Content, "Me:") {
		t.Errorf("expected 'Me:' for is_from_me messages, got: %s", result.Content)
	}
}

func TestMessagesExecuteSearchWithTestDB(t *testing.T) {
	tool := newTestMessagesTool(t)
	input, _ := json.Marshal(map[string]string{"action": "search", "query": "Meeting"})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatal(err)
	}
	if result.IsError {
		t.Fatalf("unexpected error: %s", result.Content)
	}
	if !strings.Contains(result.Content, "Meeting at 3pm") {
		t.Errorf("expected 'Meeting at 3pm' in output, got: %s", result.Content)
	}
	if !strings.Contains(result.Content, "alice@example.com") {
		t.Errorf("expected alice@example.com chat ID in output, got: %s", result.Content)
	}
}

func TestMessagesExecuteSearchNoResultsWithTestDB(t *testing.T) {
	tool := newTestMessagesTool(t)
	input, _ := json.Marshal(map[string]string{"action": "search", "query": "nonexistent_xyz_999"})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatal(err)
	}
	if result.IsError {
		t.Fatal("search with no results should not be an error")
	}
	if !strings.Contains(result.Content, "No messages found") {
		t.Errorf("expected 'No messages found' in output, got: %s", result.Content)
	}
}

// =============================================================================
// toolCorrection tests
// =============================================================================

func TestToolCorrectionMessages(t *testing.T) {
	names := []string{"messages", "imessage", "sms", "text", "message"}
	for _, name := range names {
		t.Run(name, func(t *testing.T) {
			got := toolCorrection(name)
			if !strings.Contains(got, "pim") || !strings.Contains(got, "messages") {
				t.Errorf("toolCorrection(%q) = %q, expected pim messages redirect", name, got)
			}
		})
	}
}
