//go:build darwin && !ios

package tools

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	_ "modernc.org/sqlite"
)

// cocoaEpochOffset is the number of seconds between Unix epoch (1970-01-01)
// and Apple's Cocoa epoch (2001-01-01).
const cocoaEpochOffset = 978307200

// MessagesTool provides macOS Messages/iMessage integration.
// Send uses AppleScript; read operations use SQLite on ~/Library/Messages/chat.db.
type MessagesTool struct{}

func NewMessagesTool() *MessagesTool { return &MessagesTool{} }

func (t *MessagesTool) Name() string        { return "messages" }
func (t *MessagesTool) RequiresApproval() bool { return true }

func (t *MessagesTool) Description() string {
	return "Send and read iMessages — send texts, list conversations, read chat history, search messages."
}

func (t *MessagesTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["send", "conversations", "read", "search"],
				"description": "Action: send (iMessage), conversations (list recent), read (chat history), search (message content)"
			},
			"to": {"type": "string", "description": "Recipient phone number or email (for send)"},
			"body": {"type": "string", "description": "Message text (for send)"},
			"chat_id": {"type": "string", "description": "Chat identifier — phone number, email, or chat ID from conversations (for read)"},
			"query": {"type": "string", "description": "Search query (for search)"},
			"count": {"type": "integer", "description": "Number of items to return (default: 20)"}
		},
		"required": ["action"]
	}`)
}

type messagesInput struct {
	Action string          `json:"action"`
	To     json.RawMessage `json:"to"`
	Body   string          `json:"body"`
	ChatID string          `json:"chat_id"`
	Query  string          `json:"query"`
	Count  int             `json:"count"`
}

// recipient extracts the To field as a string, handling both "phone" and ["phone"] forms.
func (p *messagesInput) recipient() string {
	if len(p.To) == 0 {
		return ""
	}
	var s string
	if json.Unmarshal(p.To, &s) == nil {
		return s
	}
	var arr []string
	if json.Unmarshal(p.To, &arr) == nil {
		if len(arr) > 0 {
			return arr[0]
		}
		return ""
	}
	return strings.Trim(string(p.To), `"`)
}

func (t *MessagesTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p messagesInput
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch p.Action {
	case "send":
		return t.sendMessage(p)
	case "conversations":
		return t.listConversations(p.Count)
	case "read":
		return t.readMessages(p.ChatID, p.Count)
	case "search":
		return t.searchMessages(p.Query, p.Count)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s. Use: send, conversations, read, search", p.Action), IsError: true}, nil
	}
}

// sendMessage sends an iMessage via AppleScript.
func (t *MessagesTool) sendMessage(p messagesInput) (*ToolResult, error) {
	to := p.recipient()
	if to == "" {
		return &ToolResult{Content: "Recipient (to) is required — phone number or email", IsError: true}, nil
	}
	if p.Body == "" {
		return &ToolResult{Content: "Message body is required", IsError: true}, nil
	}

	script := fmt.Sprintf(`tell application "Messages"
	set targetService to 1st account whose service type = iMessage
	set targetBuddy to participant "%s" of targetService
	send "%s" to targetBuddy
end tell
return "Message sent successfully"`, escapeAS(to), escapeAS(p.Body))

	out, err := execAppleScript(script)
	if err != nil {
		errStr := strings.ToLower(fmt.Sprintf("%v %s", err, out))
		if strings.Contains(errStr, "not authorized") || strings.Contains(errStr, "assistive") {
			return &ToolResult{
				Content: "macOS denied Automation access for Messages.app. Grant permission in System Settings > Privacy & Security > Automation.",
				IsError: true,
			}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Failed to send message: %s", out), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Message sent to %s", to)}, nil
}

// fdaError is the standard error returned when Full Disk Access is needed.
const fdaError = "Cannot read Messages database. Grant Full Disk Access to Nebo (or Terminal, if running via CLI) in System Settings > Privacy & Security > Full Disk Access."

// isFDAError checks if an error indicates a macOS Full Disk Access denial.
// The SQLite driver surfaces this in various ways depending on the driver and OS version.
func isFDAError(err error) bool {
	if err == nil {
		return false
	}
	s := strings.ToLower(err.Error())
	return strings.Contains(s, "operation not permitted") ||
		strings.Contains(s, "authorization denied") ||
		strings.Contains(s, "unable to open database file") ||
		strings.Contains(s, "cantopen")
}

// openChatDB opens ~/Library/Messages/chat.db in read-only mode.
func openChatDB() (*sql.DB, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return nil, fmt.Errorf("cannot determine home directory: %w", err)
	}
	dbPath := filepath.Join(home, "Library", "Messages", "chat.db")

	if _, err := os.Stat(dbPath); os.IsNotExist(err) {
		return nil, fmt.Errorf("chat.db not found at %s — Messages may not be set up", dbPath)
	}

	db, err := sql.Open("sqlite", dbPath+"?_pragma=query_only(1)")
	if err != nil {
		return nil, fmt.Errorf("cannot open chat.db: %w", err)
	}

	// sql.Open is lazy — ping to verify we can actually read the file.
	if err := db.Ping(); err != nil {
		db.Close()
		if isFDAError(err) {
			return nil, fmt.Errorf(fdaError)
		}
		return nil, fmt.Errorf("cannot access chat.db: %w", err)
	}

	return db, nil
}

// cocoaToTime converts Apple's CoreData timestamp (nanoseconds since 2001-01-01) to time.Time.
func cocoaToTime(cocoaNanos int64) time.Time {
	unixSeconds := cocoaNanos/1e9 + cocoaEpochOffset
	return time.Unix(unixSeconds, 0)
}

// listConversations returns recent conversations from chat.db.
func (t *MessagesTool) listConversations(count int) (*ToolResult, error) {
	if count <= 0 {
		count = 20
	}

	db, err := openChatDB()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	defer db.Close()

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
		LIMIT ?
	`, count)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to query conversations: %v", err), IsError: true}, nil
	}
	defer rows.Close()

	var sb strings.Builder
	sb.WriteString("Recent conversations:\n\n")
	i := 0
	for rows.Next() {
		var chatID, displayName string
		var lastDate int64
		var msgCount int
		if err := rows.Scan(&chatID, &displayName, &lastDate, &msgCount); err != nil {
			continue
		}
		i++
		label := displayName
		if label == "" {
			label = chatID
		}
		lastTime := "never"
		if lastDate > 0 {
			lastTime = cocoaToTime(lastDate).Format("Jan 2, 2006 3:04 PM")
		}
		sb.WriteString(fmt.Sprintf("%d. %s (chat_id: %s) — %d messages, last: %s\n", i, label, chatID, msgCount, lastTime))
	}
	if i == 0 {
		return &ToolResult{Content: "No conversations found."}, nil
	}
	return &ToolResult{Content: sb.String()}, nil
}

// readMessages returns recent messages from a specific chat.
func (t *MessagesTool) readMessages(chatID string, count int) (*ToolResult, error) {
	if chatID == "" {
		return &ToolResult{Content: "chat_id is required — use conversations action to find chat IDs", IsError: true}, nil
	}
	if count <= 0 {
		count = 20
	}

	db, err := openChatDB()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
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
		LIMIT ?
	`, chatID, count)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to query messages: %v", err), IsError: true}, nil
	}
	defer rows.Close()

	type msg struct {
		from string
		text string
		time string
	}
	var msgs []msg
	for rows.Next() {
		var isFromMe int
		var text, sender string
		var date int64
		if err := rows.Scan(&isFromMe, &text, &date, &sender); err != nil {
			continue
		}
		from := sender
		if isFromMe == 1 {
			from = "Me"
		}
		msgs = append(msgs, msg{
			from: from,
			text: text,
			time: cocoaToTime(date).Format("Jan 2, 2006 3:04 PM"),
		})
	}

	if len(msgs) == 0 {
		return &ToolResult{Content: fmt.Sprintf("No messages found for chat %s", chatID)}, nil
	}

	// Reverse so oldest is first (we queried DESC for LIMIT)
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Messages with %s (newest first):\n\n", chatID))
	for _, m := range msgs {
		sb.WriteString(fmt.Sprintf("[%s] %s: %s\n", m.time, m.from, m.text))
	}
	return &ToolResult{Content: sb.String()}, nil
}

// searchMessages searches message content across all chats.
func (t *MessagesTool) searchMessages(query string, count int) (*ToolResult, error) {
	if query == "" {
		return &ToolResult{Content: "Search query is required", IsError: true}, nil
	}
	if count <= 0 {
		count = 20
	}

	db, err := openChatDB()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
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
		LIMIT ?
	`, "%"+query+"%", count)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to search messages: %v", err), IsError: true}, nil
	}
	defer rows.Close()

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Messages matching \"%s\":\n\n", query))
	i := 0
	for rows.Next() {
		var isFromMe int
		var text, sender, chatID string
		var date int64
		if err := rows.Scan(&isFromMe, &text, &date, &sender, &chatID); err != nil {
			continue
		}
		i++
		from := sender
		if isFromMe == 1 {
			from = "Me"
		}
		sb.WriteString(fmt.Sprintf("[%s] %s in %s: %s\n",
			cocoaToTime(date).Format("Jan 2, 2006 3:04 PM"),
			from, chatID, text))
	}
	if i == 0 {
		return &ToolResult{Content: fmt.Sprintf("No messages found matching \"%s\"", query)}, nil
	}
	return &ToolResult{Content: sb.String()}, nil
}
