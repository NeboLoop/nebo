//go:build windows

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

// ClipboardTool provides clipboard operations on Windows.
// Uses PowerShell for clipboard access.
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
		result, err = t.getClipboard(ctx)
	case "set":
		result, err = t.setClipboard(ctx, in.Content)
	case "clear":
		result, err = t.clearClipboard(ctx)
	case "type":
		result, err = t.getClipboardType(ctx)
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

func (t *ClipboardTool) getClipboard(ctx context.Context) (string, error) {
	// Use PowerShell to get clipboard content
	out, err := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", "Get-Clipboard").Output()
	if err != nil {
		return "", fmt.Errorf("Get-Clipboard failed: %v", err)
	}

	content := strings.TrimRight(string(out), "\r\n")

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

func (t *ClipboardTool) setClipboard(ctx context.Context, content string) (string, error) {
	if content == "" {
		return "", fmt.Errorf("content is required")
	}

	// Escape the content for PowerShell
	escaped := strings.ReplaceAll(content, "`", "``")
	escaped = strings.ReplaceAll(escaped, "$", "`$")
	escaped = strings.ReplaceAll(escaped, "\"", "`\"")

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command",
		fmt.Sprintf("Set-Clipboard -Value \"%s\"", escaped))
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("Set-Clipboard failed: %v", err)
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

func (t *ClipboardTool) clearClipboard(ctx context.Context) (string, error) {
	// Clear by setting to empty
	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", "Set-Clipboard -Value $null")
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("failed to clear clipboard: %v", err)
	}
	return "Clipboard cleared", nil
}

func (t *ClipboardTool) getClipboardType(ctx context.Context) (string, error) {
	// PowerShell script to detect clipboard content type
	script := `
$formats = @()
Add-Type -AssemblyName System.Windows.Forms
$data = [System.Windows.Forms.Clipboard]::GetDataObject()
if ($data) {
    $formats = $data.GetFormats()
    $formats -join ', '
} else {
    'Empty'
}
`
	out, err := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		return "", fmt.Errorf("failed to get clipboard type: %v", err)
	}

	result := strings.TrimSpace(string(out))
	if result == "" || result == "Empty" {
		return "Clipboard is empty", nil
	}

	// Detect common types
	if strings.Contains(result, "Bitmap") || strings.Contains(result, "PNG") || strings.Contains(result, "DeviceIndependentBitmap") {
		return fmt.Sprintf("Clipboard contains: IMAGE\nRaw formats: %s", result), nil
	}
	if strings.Contains(result, "FileDrop") || strings.Contains(result, "FileName") {
		return fmt.Sprintf("Clipboard contains: FILE(S)\nRaw formats: %s", result), nil
	}
	if strings.Contains(result, "Text") || strings.Contains(result, "UnicodeText") {
		return fmt.Sprintf("Clipboard contains: TEXT\nRaw formats: %s", result), nil
	}

	return fmt.Sprintf("Clipboard formats: %s", result), nil
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
