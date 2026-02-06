package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// EditTool performs find-and-replace edits on files
type EditTool struct{}

// NewEditTool creates a new edit tool
func NewEditTool() *EditTool {
	return &EditTool{}
}

// Name returns the tool name
func (t *EditTool) Name() string {
	return "edit"
}

// Description returns the tool description
func (t *EditTool) Description() string {
	return `Edit a file by replacing an exact string with a new string.
The old_string must match exactly (including whitespace and indentation).
Use this for making targeted changes to existing files.
For creating new files, use the 'write' tool instead.`
}

// Schema returns the JSON schema for the tool input
func (t *EditTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"path": {
				"type": "string",
				"description": "Path to the file to edit"
			},
			"old_string": {
				"type": "string",
				"description": "The exact string to find and replace"
			},
			"new_string": {
				"type": "string",
				"description": "The string to replace it with"
			},
			"replace_all": {
				"type": "boolean",
				"description": "Replace all occurrences (default: false, replaces first only)"
			}
		},
		"required": ["path", "old_string", "new_string"]
	}`)
}

// EditInput represents the tool input
type EditInput struct {
	Path       string `json:"path"`
	OldString  string `json:"old_string"`
	NewString  string `json:"new_string"`
	ReplaceAll bool   `json:"replace_all"`
}

// Execute performs the edit
func (t *EditTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in EditInput
	if err := json.Unmarshal(input, &in); err != nil {
		return nil, fmt.Errorf("invalid input: %w", err)
	}

	if in.Path == "" {
		return &ToolResult{
			Content: "Error: path is required",
			IsError: true,
		}, nil
	}

	if in.OldString == "" {
		return &ToolResult{
			Content: "Error: old_string is required",
			IsError: true,
		}, nil
	}

	if in.OldString == in.NewString {
		return &ToolResult{
			Content: "Error: old_string and new_string are identical",
			IsError: true,
		}, nil
	}

	// Expand home directory
	if strings.HasPrefix(in.Path, "~/") {
		home, _ := os.UserHomeDir()
		in.Path = filepath.Join(home, in.Path[2:])
	}

	// Read current content
	content, err := os.ReadFile(in.Path)
	if err != nil {
		if os.IsNotExist(err) {
			return &ToolResult{
				Content: fmt.Sprintf("File not found: %s", in.Path),
				IsError: true,
			}, nil
		}
		return &ToolResult{
			Content: fmt.Sprintf("Error reading file: %v", err),
			IsError: true,
		}, nil
	}

	contentStr := string(content)

	// Check if old_string exists
	if !strings.Contains(contentStr, in.OldString) {
		// Try to provide helpful feedback
		return &ToolResult{
			Content: fmt.Sprintf("Error: old_string not found in file.\n\nSearched for:\n```\n%s\n```\n\nMake sure the string matches exactly, including whitespace and indentation.", in.OldString),
			IsError: true,
		}, nil
	}

	// Count occurrences
	count := strings.Count(contentStr, in.OldString)
	if count > 1 && !in.ReplaceAll {
		return &ToolResult{
			Content: fmt.Sprintf("Error: old_string appears %d times in file. Use replace_all=true to replace all, or make the search string more specific.", count),
			IsError: true,
		}, nil
	}

	// Perform replacement
	var newContent string
	if in.ReplaceAll {
		newContent = strings.ReplaceAll(contentStr, in.OldString, in.NewString)
	} else {
		newContent = strings.Replace(contentStr, in.OldString, in.NewString, 1)
	}

	// Write back
	if err := os.WriteFile(in.Path, []byte(newContent), 0644); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error writing file: %v", err),
			IsError: true,
		}, nil
	}

	if in.ReplaceAll && count > 1 {
		return &ToolResult{
			Content: fmt.Sprintf("Replaced %d occurrences in %s", count, in.Path),
		}, nil
	}

	return &ToolResult{
		Content: fmt.Sprintf("Edited %s", in.Path),
	}, nil
}

// RequiresApproval returns true - editing files needs approval
func (t *EditTool) RequiresApproval() bool {
	return true
}
