package tools

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
)

// FileTool provides file operations: read, write, edit, glob, grep
type FileTool struct {
	hasRipgrep bool
	OnFileRead func(path string) // Called after a successful file read (for access tracking)
}

// FileInput represents the consolidated input for all file operations
type FileInput struct {
	// STRAP fields
	Action string `json:"action"` // read, write, edit, glob, grep

	// Common fields
	Path string `json:"path,omitempty"` // File or directory path

	// Read fields
	Offset int `json:"offset,omitempty"` // Line number to start from (1-based)
	Limit  int `json:"limit,omitempty"`  // Maximum number of lines/files/matches

	// Write fields
	Content string `json:"content,omitempty"` // Content to write
	Append  bool   `json:"append,omitempty"`  // Append instead of overwrite

	// Edit fields
	OldString  string `json:"old_string,omitempty"`  // String to find
	NewString  string `json:"new_string,omitempty"`  // String to replace with
	ReplaceAll bool   `json:"replace_all,omitempty"` // Replace all occurrences

	// Glob fields
	Pattern string `json:"pattern,omitempty"` // Glob pattern

	// Grep fields
	Regex           string `json:"regex,omitempty"`            // Search pattern (regex)
	Glob            string `json:"glob,omitempty"`             // File filter pattern
	CaseInsensitive bool   `json:"case_insensitive,omitempty"` // Case-insensitive search
	Context         int    `json:"context,omitempty"`          // Lines of context
}

// NewFileTool creates a new file domain tool
func NewFileTool() *FileTool {
	// Check if ripgrep is available
	_, err := exec.LookPath("rg")
	return &FileTool{hasRipgrep: err == nil}
}

// Name returns the tool name
func (t *FileTool) Name() string {
	return "file"
}

// Domain returns the domain name
func (t *FileTool) Domain() string {
	return "file"
}

// Resources returns available resources
func (t *FileTool) Resources() []string {
	return []string{"file"}
}

// ActionsFor returns available actions
func (t *FileTool) ActionsFor(resource string) []string {
	return []string{"read", "write", "edit", "glob", "grep"}
}

// Description returns the tool description
func (t *FileTool) Description() string {
	return `File operations: read, write, edit, search.

Actions:
- read: Read file contents with optional line range
- write: Write content to a file (creates directories if needed)
- edit: Find-and-replace text in a file
- glob: Find files matching a pattern (supports **)
- grep: Search for regex patterns in files

Examples:
  file(action: read, path: "src/main.go")
  file(action: read, path: "large.log", offset: 100, limit: 50)
  file(action: write, path: "out.txt", content: "hello world")
  file(action: edit, path: "config.yaml", old_string: "port: 8080", new_string: "port: 3000")
  file(action: glob, pattern: "**/*.go")
  file(action: grep, regex: "TODO", path: "src/", glob: "*.go")`
}

// Schema returns the JSON schema
func (t *FileTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"description": "File action: read, write, edit, glob, grep",
				"enum": ["read", "write", "edit", "glob", "grep"]
			},
			"path": {
				"type": "string",
				"description": "File or directory path (required for read, write, edit; optional for glob, grep)"
			},
			"offset": {
				"type": "integer",
				"description": "Line number to start from for read (1-based, default: 1)"
			},
			"limit": {
				"type": "integer",
				"description": "Maximum lines (read: 2000), files (glob: 1000), or matches (grep: 100)"
			},
			"content": {
				"type": "string",
				"description": "Content to write (required for write action)"
			},
			"append": {
				"type": "boolean",
				"description": "Append to file instead of overwriting (for write action)"
			},
			"old_string": {
				"type": "string",
				"description": "Exact string to find (required for edit action)"
			},
			"new_string": {
				"type": "string",
				"description": "Replacement string (required for edit action)"
			},
			"replace_all": {
				"type": "boolean",
				"description": "Replace all occurrences (for edit action, default: false)"
			},
			"pattern": {
				"type": "string",
				"description": "Glob pattern for file matching (required for glob action)"
			},
			"regex": {
				"type": "string",
				"description": "Regular expression pattern (required for grep action)"
			},
			"glob": {
				"type": "string",
				"description": "File filter pattern for grep (e.g., '*.go')"
			},
			"case_insensitive": {
				"type": "boolean",
				"description": "Case-insensitive search (for grep action)"
			},
			"context": {
				"type": "integer",
				"description": "Lines of context around matches (for grep action)"
			}
		},
		"required": ["action"]
	}`)
}

// RequiresApproval returns true for write/edit operations
func (t *FileTool) RequiresApproval() bool {
	return true // Actual check is done per-action in Execute
}

// Execute routes to the appropriate action handler
func (t *FileTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in FileInput
	if err := json.Unmarshal(input, &in); err != nil {
		return nil, fmt.Errorf("invalid input: %w", err)
	}

	switch in.Action {
	case "read":
		return t.handleRead(ctx, in)
	case "write":
		return t.handleWrite(ctx, in)
	case "edit":
		return t.handleEdit(ctx, in)
	case "glob":
		return t.handleGlob(ctx, in)
	case "grep":
		return t.handleGrep(ctx, in)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown action: %s (valid: read, write, edit, glob, grep)", in.Action),
			IsError: true,
		}, nil
	}
}

// handleRead reads file contents with optional line range
func (t *FileTool) handleRead(ctx context.Context, in FileInput) (*ToolResult, error) {
	if in.Path == "" {
		return &ToolResult{Content: "Error: path is required", IsError: true}, nil
	}

	// Validate path is not sensitive (SSH keys, credentials, etc.)
	if err := validateFilePath(in.Path, "read"); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error: %v", err), IsError: true}, nil
	}

	// Expand home directory
	path := expandPath(in.Path)

	// Set defaults
	if in.Offset <= 0 {
		in.Offset = 1
	}
	if in.Limit <= 0 {
		in.Limit = 2000
	}

	// Check if file exists
	info, err := os.Stat(path)
	if err != nil {
		if os.IsNotExist(err) {
			return &ToolResult{Content: fmt.Sprintf("File not found: %s", path), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Error accessing file: %v", err), IsError: true}, nil
	}

	if info.IsDir() {
		return &ToolResult{
			Content: fmt.Sprintf("Path is a directory: %s\nUse glob action to list directory contents", path),
			IsError: true,
		}, nil
	}

	// Read file
	file, err := os.Open(path)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error opening file: %v", err), IsError: true}, nil
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

		if lineNum < in.Offset {
			continue
		}

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
		return &ToolResult{Content: fmt.Sprintf("Error reading file: %v", err), IsError: true}, nil
	}

	content := result.String()
	if content == "" {
		if in.Offset > 1 {
			content = fmt.Sprintf("(file has fewer than %d lines)", in.Offset)
		} else {
			content = "(file is empty)"
		}
	}

	// Track file access for post-compaction re-injection
	if t.OnFileRead != nil {
		t.OnFileRead(path)
	}

	return &ToolResult{Content: content}, nil
}

// handleWrite writes content to a file
func (t *FileTool) handleWrite(ctx context.Context, in FileInput) (*ToolResult, error) {
	if in.Path == "" {
		return &ToolResult{Content: "Error: path is required", IsError: true}, nil
	}

	// Validate path is not sensitive (SSH keys, credentials, shell rc files, etc.)
	if err := validateFilePath(in.Path, "write"); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error: %v", err), IsError: true}, nil
	}

	path := expandPath(in.Path)

	// Create parent directories if needed
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error creating directories: %v", err), IsError: true}, nil
	}

	// Determine file flags
	flags := os.O_WRONLY | os.O_CREATE
	if in.Append {
		flags |= os.O_APPEND
	} else {
		flags |= os.O_TRUNC
	}

	// Write file
	file, err := os.OpenFile(path, flags, 0644)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error opening file: %v", err), IsError: true}, nil
	}
	defer file.Close()

	n, err := file.WriteString(in.Content)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error writing file: %v", err), IsError: true}, nil
	}

	action := "Wrote"
	if in.Append {
		action = "Appended"
	}

	return &ToolResult{Content: fmt.Sprintf("%s %d bytes to %s", action, n, path)}, nil
}

// handleEdit performs find-and-replace
func (t *FileTool) handleEdit(ctx context.Context, in FileInput) (*ToolResult, error) {
	if in.Path == "" {
		return &ToolResult{Content: "Error: path is required", IsError: true}, nil
	}
	if in.OldString == "" {
		return &ToolResult{Content: "Error: old_string is required", IsError: true}, nil
	}
	if in.OldString == in.NewString {
		return &ToolResult{Content: "Error: old_string and new_string are identical", IsError: true}, nil
	}

	// Validate path is not sensitive (SSH keys, credentials, shell rc files, etc.)
	if err := validateFilePath(in.Path, "edit"); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error: %v", err), IsError: true}, nil
	}

	path := expandPath(in.Path)

	// Read current content
	content, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return &ToolResult{Content: fmt.Sprintf("File not found: %s", path), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Error reading file: %v", err), IsError: true}, nil
	}

	contentStr := string(content)

	// Check if old_string exists
	if !strings.Contains(contentStr, in.OldString) {
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
	if err := os.WriteFile(path, []byte(newContent), 0644); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error writing file: %v", err), IsError: true}, nil
	}

	if in.ReplaceAll && count > 1 {
		return &ToolResult{Content: fmt.Sprintf("Replaced %d occurrences in %s", count, path)}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Edited %s", path)}, nil
}

// handleGlob finds files matching a pattern
func (t *FileTool) handleGlob(ctx context.Context, in FileInput) (*ToolResult, error) {
	if in.Pattern == "" {
		return &ToolResult{Content: "Error: pattern is required", IsError: true}, nil
	}

	// Set defaults
	basePath := in.Path
	if basePath == "" {
		basePath = "."
	}
	basePath = expandPath(basePath)

	limit := in.Limit
	if limit <= 0 {
		limit = 1000
	}

	// Check if using ** for recursive matching
	var matches []string
	var err error

	if strings.Contains(in.Pattern, "**") {
		matches, err = t.recursiveGlob(basePath, in.Pattern, limit)
	} else {
		fullPattern := filepath.Join(basePath, in.Pattern)
		matches, err = filepath.Glob(fullPattern)
	}

	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error: %v", err), IsError: true}, nil
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
	if len(filesWithTime) > limit {
		filesWithTime = filesWithTime[:limit]
	}

	if len(filesWithTime) == 0 {
		return &ToolResult{Content: fmt.Sprintf("No files found matching pattern: %s", in.Pattern)}, nil
	}

	var result strings.Builder
	for _, f := range filesWithTime {
		result.WriteString(f.path)
		result.WriteString("\n")
	}

	return &ToolResult{Content: strings.TrimSpace(result.String())}, nil
}

// recursiveGlob handles ** patterns
func (t *FileTool) recursiveGlob(basePath, pattern string, limit int) ([]string, error) {
	var matches []string

	// Split pattern into parts
	parts := strings.Split(pattern, "**")
	if len(parts) != 2 {
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
			return nil
		}

		if info.IsDir() {
			if strings.HasPrefix(info.Name(), ".") && info.Name() != "." {
				return filepath.SkipDir
			}
			if info.Name() == "node_modules" || info.Name() == "vendor" || info.Name() == "__pycache__" {
				return filepath.SkipDir
			}
			return nil
		}

		if suffix != "" {
			matched, _ := filepath.Match(suffix, info.Name())
			if !matched {
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

// handleGrep delegates to GrepTool for all grep operations
func (t *FileTool) handleGrep(ctx context.Context, in FileInput) (*ToolResult, error) {
	if in.Regex == "" {
		return &ToolResult{Content: "Error: regex is required", IsError: true}, nil
	}

	path := in.Path
	if path == "" {
		path = "."
	}
	path = expandPath(path)

	limit := in.Limit
	if limit <= 0 {
		limit = 100
	}

	grepInput, _ := json.Marshal(GrepInput{
		Pattern:         in.Regex,
		Path:            path,
		Glob:            in.Glob,
		CaseInsensitive: in.CaseInsensitive,
		Context:         in.Context,
		Limit:           limit,
	})

	grep := &GrepTool{hasRipgrep: t.hasRipgrep}
	return grep.Execute(ctx, grepInput)
}

// sensitivePaths contains paths that the agent should never read or write.
// These are resolved to absolute paths at init time for reliable matching.
var sensitivePaths = func() []string {
	home, _ := os.UserHomeDir()
	paths := []string{
		// SSH keys and config
		filepath.Join(home, ".ssh"),
		// AWS credentials
		filepath.Join(home, ".aws"),
		// GCP credentials
		filepath.Join(home, ".config", "gcloud"),
		// Azure credentials
		filepath.Join(home, ".azure"),
		// GPG keys
		filepath.Join(home, ".gnupg"),
		// Docker credentials
		filepath.Join(home, ".docker", "config.json"),
		// Kubernetes config
		filepath.Join(home, ".kube", "config"),
		// NPM tokens
		filepath.Join(home, ".npmrc"),
		// Password databases
		filepath.Join(home, ".password-store"),
		// Keychain (macOS)
		filepath.Join(home, "Library", "Keychains"),
		// Browser profiles (cookies, saved passwords)
		filepath.Join(home, "Library", "Application Support", "Google", "Chrome"),
		filepath.Join(home, "Library", "Application Support", "Firefox"),
		filepath.Join(home, ".config", "google-chrome"),
		filepath.Join(home, ".mozilla"),
		// Shell init files (write protection — prevent backdoors)
		filepath.Join(home, ".bashrc"),
		filepath.Join(home, ".bash_profile"),
		filepath.Join(home, ".zshrc"),
		filepath.Join(home, ".zprofile"),
		filepath.Join(home, ".profile"),
		// System paths
		"/etc/shadow",
		"/etc/passwd",
		"/etc/sudoers",
	}
	return paths
}()

// validateFilePath checks that a path is safe for the agent to access.
// It blocks sensitive paths (SSH keys, credentials, shell rc files) and
// resolves symlinks to prevent symlink-based traversal attacks.
func validateFilePath(rawPath string, action string) error {
	// Expand and resolve to absolute path
	expanded := expandPath(rawPath)
	absPath, err := filepath.Abs(expanded)
	if err != nil {
		return fmt.Errorf("invalid path: %w", err)
	}

	// Resolve symlinks to get the real path (prevents symlink traversal)
	realPath := absPath
	if resolved, err := filepath.EvalSymlinks(absPath); err == nil {
		realPath = resolved
	}
	// If EvalSymlinks fails (file doesn't exist yet for write), use absPath

	// Check both the requested path and the resolved path against sensitive paths
	for _, sensitive := range sensitivePaths {
		// Check if the path IS the sensitive path or is inside it
		if pathMatchesOrIsInside(absPath, sensitive) || pathMatchesOrIsInside(realPath, sensitive) {
			return fmt.Errorf("blocked: %s access to %q is restricted (sensitive path)", action, rawPath)
		}
	}

	return nil
}

// pathMatchesOrIsInside returns true if path equals target or is inside target directory.
func pathMatchesOrIsInside(path, target string) bool {
	if path == target {
		return true
	}
	// Check if path is inside target directory
	targetWithSep := target + string(filepath.Separator)
	return strings.HasPrefix(path, targetWithSep)
}

// expandPath expands ~ to home directory
func expandPath(path string) string {
	if strings.HasPrefix(path, "~/") {
		home, _ := os.UserHomeDir()
		return filepath.Join(home, path[2:])
	}
	return path
}
