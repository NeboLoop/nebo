package tools

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// ReadTool reads file contents
type ReadTool struct{}

// NewReadTool creates a new read tool
func NewReadTool() *ReadTool {
	return &ReadTool{}
}

// Name returns the tool name
func (t *ReadTool) Name() string {
	return "read"
}

// Description returns the tool description
func (t *ReadTool) Description() string {
	return `Read the contents of a file. Returns the file content with line numbers.
Use this instead of cat/head/tail commands for better output formatting.
Supports reading specific line ranges for large files.`
}

// Schema returns the JSON schema for the tool input
func (t *ReadTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"path": {
				"type": "string",
				"description": "Path to the file to read"
			},
			"offset": {
				"type": "integer",
				"description": "Line number to start from (1-based, default: 1)"
			},
			"limit": {
				"type": "integer",
				"description": "Maximum number of lines to read (default: 2000)"
			}
		},
		"required": ["path"]
	}`)
}

// ReadInput represents the tool input
type ReadInput struct {
	Path   string `json:"path"`
	Offset int    `json:"offset"`
	Limit  int    `json:"limit"`
}

// Execute reads the file
func (t *ReadTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in ReadInput
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

	// Set defaults
	if in.Offset <= 0 {
		in.Offset = 1
	}
	if in.Limit <= 0 {
		in.Limit = 2000
	}

	// Check if file exists
	info, err := os.Stat(in.Path)
	if err != nil {
		if os.IsNotExist(err) {
			return &ToolResult{
				Content: fmt.Sprintf("File not found: %s", in.Path),
				IsError: true,
			}, nil
		}
		return &ToolResult{
			Content: fmt.Sprintf("Error accessing file: %v", err),
			IsError: true,
		}, nil
	}

	if info.IsDir() {
		return &ToolResult{
			Content: fmt.Sprintf("Path is a directory: %s\nUse 'ls' command to list directory contents", in.Path),
			IsError: true,
		}, nil
	}

	// Read file
	file, err := os.Open(in.Path)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error opening file: %v", err),
			IsError: true,
		}, nil
	}
	defer file.Close()

	// Read lines
	var result strings.Builder
	scanner := bufio.NewScanner(file)
	scanner.Buffer(make([]byte, 1024*1024), 1024*1024) // 1MB line buffer

	lineNum := 0
	linesRead := 0

	for scanner.Scan() {
		lineNum++

		// Skip until offset
		if lineNum < in.Offset {
			continue
		}

		// Check limit
		if linesRead >= in.Limit {
			result.WriteString(fmt.Sprintf("\n... (showing lines %d-%d of %d+)", in.Offset, lineNum-1, lineNum))
			break
		}

		line := scanner.Text()

		// Truncate very long lines
		const maxLineLen = 2000
		if len(line) > maxLineLen {
			line = line[:maxLineLen] + "..."
		}

		result.WriteString(fmt.Sprintf("%6d\t%s\n", lineNum, line))
		linesRead++
	}

	if err := scanner.Err(); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error reading file: %v", err),
			IsError: true,
		}, nil
	}

	content := result.String()
	if content == "" {
		if in.Offset > 1 {
			content = fmt.Sprintf("(file has fewer than %d lines)", in.Offset)
		} else {
			content = "(file is empty)"
		}
	}

	return &ToolResult{
		Content: content,
	}, nil
}

// RequiresApproval returns false - reading is safe
func (t *ReadTool) RequiresApproval() bool {
	return false
}
