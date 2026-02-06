package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

// GlobTool finds files matching patterns
type GlobTool struct{}

// NewGlobTool creates a new glob tool
func NewGlobTool() *GlobTool {
	return &GlobTool{}
}

// Name returns the tool name
func (t *GlobTool) Name() string {
	return "glob"
}

// Description returns the tool description
func (t *GlobTool) Description() string {
	return `Find files matching a glob pattern. Supports ** for recursive matching.
Examples:
- "*.go" - Go files in current directory
- "**/*.ts" - TypeScript files recursively
- "src/**/*.test.js" - Test files in src directory`
}

// Schema returns the JSON schema for the tool input
func (t *GlobTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"pattern": {
				"type": "string",
				"description": "Glob pattern to match files against"
			},
			"path": {
				"type": "string",
				"description": "Base directory to search in (default: current directory)"
			},
			"limit": {
				"type": "integer",
				"description": "Maximum number of files to return (default: 1000)"
			}
		},
		"required": ["pattern"]
	}`)
}

// GlobInput represents the tool input
type GlobInput struct {
	Pattern string `json:"pattern"`
	Path    string `json:"path"`
	Limit   int    `json:"limit"`
}

// Execute finds matching files
func (t *GlobTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in GlobInput
	if err := json.Unmarshal(input, &in); err != nil {
		return nil, fmt.Errorf("invalid input: %w", err)
	}

	if in.Pattern == "" {
		return &ToolResult{
			Content: "Error: pattern is required",
			IsError: true,
		}, nil
	}

	// Set defaults
	if in.Path == "" {
		in.Path = "."
	}
	if in.Limit <= 0 {
		in.Limit = 1000
	}

	// Expand home directory
	if strings.HasPrefix(in.Path, "~/") {
		home, _ := os.UserHomeDir()
		in.Path = filepath.Join(home, in.Path[2:])
	}

	// Check if using ** for recursive matching
	var matches []string
	var err error

	if strings.Contains(in.Pattern, "**") {
		matches, err = t.recursiveGlob(in.Path, in.Pattern, in.Limit)
	} else {
		fullPattern := filepath.Join(in.Path, in.Pattern)
		matches, err = filepath.Glob(fullPattern)
	}

	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error: %v", err),
			IsError: true,
		}, nil
	}

	// Sort by modification time (newest first)
	type fileWithTime struct {
		path    string
		modTime int64
	}

	filesWithTime := make([]fileWithTime, 0, len(matches))
	for _, m := range matches {
		info, err := os.Stat(m)
		if err == nil && !info.IsDir() {
			filesWithTime = append(filesWithTime, fileWithTime{
				path:    m,
				modTime: info.ModTime().Unix(),
			})
		}
	}

	sort.Slice(filesWithTime, func(i, j int) bool {
		return filesWithTime[i].modTime > filesWithTime[j].modTime
	})

	// Limit results
	if len(filesWithTime) > in.Limit {
		filesWithTime = filesWithTime[:in.Limit]
	}

	if len(filesWithTime) == 0 {
		return &ToolResult{
			Content: fmt.Sprintf("No files found matching pattern: %s", in.Pattern),
		}, nil
	}

	var result strings.Builder
	for _, f := range filesWithTime {
		result.WriteString(f.path)
		result.WriteString("\n")
	}

	return &ToolResult{
		Content: strings.TrimSpace(result.String()),
	}, nil
}

// recursiveGlob handles ** patterns
func (t *GlobTool) recursiveGlob(basePath, pattern string, limit int) ([]string, error) {
	var matches []string

	// Split pattern into parts
	parts := strings.Split(pattern, "**")
	if len(parts) != 2 {
		// Multiple ** not supported, fall back to simple glob
		return filepath.Glob(filepath.Join(basePath, pattern))
	}

	prefix := strings.TrimSuffix(parts[0], "/")
	suffix := strings.TrimPrefix(parts[1], "/")

	searchPath := basePath
	if prefix != "" {
		searchPath = filepath.Join(basePath, prefix)
	}

	err := filepath.Walk(searchPath, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return nil // Skip errors
		}

		if info.IsDir() {
			// Skip hidden directories
			if strings.HasPrefix(info.Name(), ".") && info.Name() != "." {
				return filepath.SkipDir
			}
			// Skip common non-source directories
			if info.Name() == "node_modules" || info.Name() == "vendor" || info.Name() == "__pycache__" {
				return filepath.SkipDir
			}
			return nil
		}

		// Check if file matches suffix pattern
		if suffix != "" {
			matched, _ := filepath.Match(suffix, info.Name())
			if !matched {
				// Also try matching the full relative path
				rel, _ := filepath.Rel(searchPath, path)
				matched, _ = filepath.Match(suffix, rel)
				if !matched {
					return nil
				}
			}
		}

		matches = append(matches, path)

		if len(matches) >= limit {
			return filepath.SkipAll
		}
		return nil
	})

	return matches, err
}

// RequiresApproval returns false - reading is safe
func (t *GlobTool) RequiresApproval() bool {
	return false
}
