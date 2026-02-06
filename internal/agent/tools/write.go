package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// WriteTool writes content to files
type WriteTool struct{}

// NewWriteTool creates a new write tool
func NewWriteTool() *WriteTool {
	return &WriteTool{}
}

// Name returns the tool name
func (t *WriteTool) Name() string {
	return "write"
}

// Description returns the tool description
func (t *WriteTool) Description() string {
	return `Write content to a file. Creates the file and any necessary directories if they don't exist.
For editing existing files, prefer the 'edit' tool which supports find-and-replace operations.
This tool will overwrite the entire file content.`
}

// Schema returns the JSON schema for the tool input
func (t *WriteTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"path": {
				"type": "string",
				"description": "Path to the file to write"
			},
			"content": {
				"type": "string",
				"description": "Content to write to the file"
			},
			"append": {
				"type": "boolean",
				"description": "If true, append to file instead of overwriting (default: false)"
			}
		},
		"required": ["path", "content"]
	}`)
}

// WriteInput represents the tool input
type WriteInput struct {
	Path    string `json:"path"`
	Content string `json:"content"`
	Append  bool   `json:"append"`
}

// Execute writes the file
func (t *WriteTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in WriteInput
	if err := json.Unmarshal(input, &in); err != nil {
		return nil, fmt.Errorf("invalid input: %w", err)
	}

	if in.Path == "" {
		return &ToolResult{
			Content: "Error: path is required",
			IsError: true,
		}, nil
	}

	// Expand home directory
	if strings.HasPrefix(in.Path, "~/") {
		home, _ := os.UserHomeDir()
		in.Path = filepath.Join(home, in.Path[2:])
	}

	// Create parent directories if needed
	dir := filepath.Dir(in.Path)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error creating directories: %v", err),
			IsError: true,
		}, nil
	}

	// Determine file flags
	flags := os.O_WRONLY | os.O_CREATE
	if in.Append {
		flags |= os.O_APPEND
	} else {
		flags |= os.O_TRUNC
	}

	// Write file
	file, err := os.OpenFile(in.Path, flags, 0644)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error opening file: %v", err),
			IsError: true,
		}, nil
	}
	defer file.Close()

	n, err := file.WriteString(in.Content)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error writing file: %v", err),
			IsError: true,
		}, nil
	}

	action := "Wrote"
	if in.Append {
		action = "Appended"
	}

	return &ToolResult{
		Content: fmt.Sprintf("%s %d bytes to %s", action, n, in.Path),
	}, nil
}

// RequiresApproval returns true - writing files needs approval
func (t *WriteTool) RequiresApproval() bool {
	return true
}
