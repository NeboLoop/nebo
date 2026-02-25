//go:build darwin && !ios

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
	"sync"
	"time"
)

// ClipboardTool provides clipboard operations on macOS.
// Uses pbcopy/pbpaste for text and AppleScript for type detection.
type ClipboardTool struct {
	mu      sync.Mutex
	history []clipboardEntry
	maxHist int
}

type clipboardEntry struct {
	Content   string    `json:"content"`
	Timestamp time.Time `json:"timestamp"`
	Type      string    `json:"type"`
}

// NewClipboardTool creates a new clipboard tool
func NewClipboardTool() *ClipboardTool {
	return &ClipboardTool{
		history: make([]clipboardEntry, 0),
		maxHist: 20,
	}
}

func (t *ClipboardTool) Name() string {
	return "clipboard"
}

func (t *ClipboardTool) Description() string {
	return "Manage clipboard: get current content, set new content, clear, view history, and detect content type (text/image/file)."
}

func (t *ClipboardTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"description": "Action to perform",
				"enum": ["get", "set", "clear", "type", "history"]
			},
			"content": {
				"type": "string",
				"description": "Content to set (for set action)"
			},
			"limit": {
				"type": "integer",
				"description": "Number of history entries to return (default: 10)"
			}
		},
		"required": ["action"]
	}`)
}

func (t *ClipboardTool) RequiresApproval() bool {
	return false
}

type clipboardInput struct {
	Action  string `json:"action"`
	Content string `json:"content"`
	Limit   int    `json:"limit"`
}

func (t *ClipboardTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in clipboardInput
	if err := json.Unmarshal(input, &in); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	var result string
	var err error

	switch in.Action {
	case "get":
		result, err = t.getClipboard()
	case "set":
		result, err = t.setClipboard(in.Content)
	case "clear":
		result, err = t.clearClipboard()
	case "type":
		result, err = t.getClipboardType()
	case "history":
		limit := in.Limit
		if limit <= 0 {
			limit = 10
		}
		result, err = t.getHistory(limit)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", in.Action), IsError: true}, nil
	}

	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Action failed: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: result, IsError: false}, nil
}

func (t *ClipboardTool) getClipboard() (string, error) {
	out, err := exec.Command("pbpaste").Output()
	if err != nil {
		return "", fmt.Errorf("pbpaste failed: %v", err)
	}

	content := string(out)

	// Store in history
	t.mu.Lock()
	t.history = append([]clipboardEntry{{
		Content:   content,
		Timestamp: time.Now(),
		Type:      "text",
	}}, t.history...)
	if len(t.history) > t.maxHist {
		t.history = t.history[:t.maxHist]
	}
	t.mu.Unlock()

	if content == "" {
		return "Clipboard is empty (or contains non-text content)", nil
	}

	// Truncate if very long
	if len(content) > 5000 {
		return fmt.Sprintf("%s\n\n... (truncated, total %d characters)", content[:5000], len(content)), nil
	}

	return content, nil
}

func (t *ClipboardTool) setClipboard(content string) (string, error) {
	if content == "" {
		return "", fmt.Errorf("content is required")
	}

	cmd := exec.Command("pbcopy")
	cmd.Stdin = strings.NewReader(content)
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("pbcopy failed: %v", err)
	}

	// Store in history
	t.mu.Lock()
	t.history = append([]clipboardEntry{{
		Content:   content,
		Timestamp: time.Now(),
		Type:      "text",
	}}, t.history...)
	if len(t.history) > t.maxHist {
		t.history = t.history[:t.maxHist]
	}
	t.mu.Unlock()

	preview := content
	if len(preview) > 100 {
		preview = preview[:100] + "..."
	}
	return fmt.Sprintf("Clipboard set to: %q", preview), nil
}

func (t *ClipboardTool) clearClipboard() (string, error) {
	cmd := exec.Command("pbcopy")
	cmd.Stdin = strings.NewReader("")
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("failed to clear clipboard: %v", err)
	}
	return "Clipboard cleared", nil
}

func (t *ClipboardTool) getClipboardType() (string, error) {
	script := `
		set clipTypes to ""
		try
			set clipInfo to (clipboard info)
			repeat with clipItem in clipInfo
				set clipTypes to clipTypes & (item 1 of clipItem as string) & ", "
			end repeat
		end try
		return clipTypes
	`

	out, err := exec.Command("osascript", "-e", script).Output()
	if err != nil {
		return "", fmt.Errorf("failed to get clipboard type: %v", err)
	}

	result := strings.TrimSpace(string(out))
	if result == "" {
		return "Clipboard is empty", nil
	}

	types := strings.TrimSuffix(result, ", ")
	if strings.Contains(types, "«class PNGf»") || strings.Contains(types, "TIFF") {
		return fmt.Sprintf("Clipboard contains: IMAGE\nRaw types: %s", types), nil
	}
	if strings.Contains(types, "furl") || strings.Contains(types, "«class furl»") {
		return fmt.Sprintf("Clipboard contains: FILE(S)\nRaw types: %s", types), nil
	}
	if strings.Contains(types, "utxt") || strings.Contains(types, "«class utf8»") {
		return fmt.Sprintf("Clipboard contains: TEXT\nRaw types: %s", types), nil
	}

	return fmt.Sprintf("Clipboard types: %s", types), nil
}

func (t *ClipboardTool) getHistory(limit int) (string, error) {
	t.mu.Lock()
	defer t.mu.Unlock()

	if len(t.history) == 0 {
		return "No clipboard history available (history is session-only and starts empty)", nil
	}

	var sb strings.Builder
	count := limit
	if count > len(t.history) {
		count = len(t.history)
	}
	sb.WriteString(fmt.Sprintf("Clipboard History (last %d entries):\n\n", count))

	for i := 0; i < count; i++ {
		entry := t.history[i]
		preview := entry.Content
		if len(preview) > 80 {
			preview = preview[:80] + "..."
		}
		preview = strings.ReplaceAll(preview, "\n", "\\n")
		sb.WriteString(fmt.Sprintf("%d. [%s] %s: %q\n",
			i+1,
			entry.Timestamp.Format("15:04:05"),
			entry.Type,
			preview,
		))
	}

	return sb.String(), nil
}

