//go:build linux

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

// ClipboardTool provides clipboard operations on Linux.
// Uses xclip or xsel for X11, wl-copy/wl-paste for Wayland.
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

// isWayland checks if we're running under Wayland
func isWayland() bool {
	// Check for Wayland display
	if _, err := exec.LookPath("wl-paste"); err == nil {
		// Check if WAYLAND_DISPLAY is set
		out, _ := exec.Command("sh", "-c", "echo $WAYLAND_DISPLAY").Output()
		return strings.TrimSpace(string(out)) != ""
	}
	return false
}

func (t *ClipboardTool) getClipboard() (string, error) {
	var out []byte
	var err error

	if isWayland() {
		out, err = exec.Command("wl-paste", "-n").Output()
	} else if _, xerr := exec.LookPath("xclip"); xerr == nil {
		out, err = exec.Command("xclip", "-selection", "clipboard", "-o").Output()
	} else if _, xerr := exec.LookPath("xsel"); xerr == nil {
		out, err = exec.Command("xsel", "--clipboard", "--output").Output()
	} else {
		return "", fmt.Errorf("no clipboard tool found (install xclip, xsel, or wl-clipboard)")
	}

	if err != nil {
		// Check if clipboard is just empty
		if strings.Contains(err.Error(), "exit status") {
			return "Clipboard is empty", nil
		}
		return "", fmt.Errorf("failed to get clipboard: %v", err)
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
		return "Clipboard is empty", nil
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

	var cmd *exec.Cmd

	if isWayland() {
		cmd = exec.Command("wl-copy")
	} else if _, err := exec.LookPath("xclip"); err == nil {
		cmd = exec.Command("xclip", "-selection", "clipboard")
	} else if _, err := exec.LookPath("xsel"); err == nil {
		cmd = exec.Command("xsel", "--clipboard", "--input")
	} else {
		return "", fmt.Errorf("no clipboard tool found (install xclip, xsel, or wl-clipboard)")
	}

	cmd.Stdin = strings.NewReader(content)
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("failed to set clipboard: %v", err)
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
	var cmd *exec.Cmd

	if isWayland() {
		cmd = exec.Command("wl-copy", "--clear")
	} else if _, err := exec.LookPath("xclip"); err == nil {
		cmd = exec.Command("xclip", "-selection", "clipboard")
		cmd.Stdin = strings.NewReader("")
	} else if _, err := exec.LookPath("xsel"); err == nil {
		cmd = exec.Command("xsel", "--clipboard", "--delete")
	} else {
		return "", fmt.Errorf("no clipboard tool found (install xclip, xsel, or wl-clipboard)")
	}

	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("failed to clear clipboard: %v", err)
	}
	return "Clipboard cleared", nil
}

func (t *ClipboardTool) getClipboardType() (string, error) {
	var out []byte
	var err error

	if isWayland() {
		// List available MIME types
		out, err = exec.Command("wl-paste", "--list-types").Output()
	} else if _, xerr := exec.LookPath("xclip"); xerr == nil {
		out, err = exec.Command("xclip", "-selection", "clipboard", "-t", "TARGETS", "-o").Output()
	} else {
		return "Clipboard type detection requires xclip or wl-clipboard", nil
	}

	if err != nil {
		return "Clipboard is empty or error detecting type", nil
	}

	result := strings.TrimSpace(string(out))
	if result == "" {
		return "Clipboard is empty", nil
	}

	// Detect common types
	if strings.Contains(result, "image/") {
		return fmt.Sprintf("Clipboard contains: IMAGE\nMIME types: %s", result), nil
	}
	if strings.Contains(result, "text/uri-list") || strings.Contains(result, "x-special/gnome-copied-files") {
		return fmt.Sprintf("Clipboard contains: FILE(S)\nMIME types: %s", result), nil
	}
	if strings.Contains(result, "text/") || strings.Contains(result, "UTF8_STRING") {
		return fmt.Sprintf("Clipboard contains: TEXT\nMIME types: %s", result), nil
	}

	return fmt.Sprintf("Clipboard formats:\n%s", result), nil
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

