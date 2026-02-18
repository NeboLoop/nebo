package tools

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
)

// GrepTool searches for patterns in files
type GrepTool struct {
	hasRipgrep bool
}

// NewGrepTool creates a new grep tool
func NewGrepTool() *GrepTool {
	// Check if ripgrep is available
	_, err := exec.LookPath("rg")
	return &GrepTool{hasRipgrep: err == nil}
}

// Name returns the tool name
func (t *GrepTool) Name() string {
	return "grep"
}

// Description returns the tool description
func (t *GrepTool) Description() string {
	return `Search for patterns in files using regular expressions.
Returns matching lines with file paths and line numbers.
Use glob parameter to filter which files to search.`
}

// Schema returns the JSON schema for the tool input
func (t *GrepTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"pattern": {
				"type": "string",
				"description": "Regular expression pattern to search for"
			},
			"path": {
				"type": "string",
				"description": "File or directory to search in (default: current directory)"
			},
			"glob": {
				"type": "string",
				"description": "Glob pattern to filter files (e.g., '*.go', '**/*.ts')"
			},
			"case_insensitive": {
				"type": "boolean",
				"description": "Make search case-insensitive (default: false)"
			},
			"context": {
				"type": "integer",
				"description": "Number of lines of context around matches (default: 0)"
			},
			"limit": {
				"type": "integer",
				"description": "Maximum number of matches to return (default: 100)"
			}
		},
		"required": ["pattern"]
	}`)
}

// GrepInput represents the tool input
type GrepInput struct {
	Pattern         string `json:"pattern"`
	Path            string `json:"path"`
	Glob            string `json:"glob"`
	CaseInsensitive bool   `json:"case_insensitive"`
	Context         int    `json:"context"`
	Limit           int    `json:"limit"`
}

// GrepMatch represents a single match
type GrepMatch struct {
	File    string
	Line    int
	Content string
}

// Execute searches for the pattern
func (t *GrepTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in GrepInput
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
		in.Limit = 100
	}

	// Expand home directory
	if strings.HasPrefix(in.Path, "~/") {
		home, _ := os.UserHomeDir()
		in.Path = filepath.Join(home, in.Path[2:])
	}

	// Block dangerous root paths that would search the entire filesystem
	absPath, _ := filepath.Abs(in.Path)
	dangerousPaths := []string{"/", "/usr", "/var", "/etc", "/System", "/Library", "/Applications", "/bin", "/sbin", "/opt"}
	for _, dangerous := range dangerousPaths {
		if absPath == dangerous {
			return &ToolResult{
				Content: fmt.Sprintf("Error: Cannot search '%s' - path is too broad. Please specify a more specific directory.", in.Path),
				IsError: true,
			}, nil
		}
	}

	// Use ripgrep if available
	if t.hasRipgrep {
		return t.executeWithRipgrep(ctx, &in)
	}

	return t.executeWithGo(ctx, &in)
}

// executeWithRipgrep uses the rg command for fast searching
func (t *GrepTool) executeWithRipgrep(ctx context.Context, in *GrepInput) (*ToolResult, error) {
	args := []string{
		"--line-number",
		"--no-heading",
		"--color=never",
		fmt.Sprintf("--max-count=%d", in.Limit),
	}

	if in.CaseInsensitive {
		args = append(args, "-i")
	}

	if in.Context > 0 {
		args = append(args, fmt.Sprintf("-C%d", in.Context))
	}

	if in.Glob != "" {
		args = append(args, "--glob", in.Glob)
	}

	args = append(args, in.Pattern, in.Path)

	cmd := exec.CommandContext(ctx, "rg", args...)
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr

	err := cmd.Run()
	if err != nil {
		// rg returns exit code 1 when no matches found - that's not an error
		if exitErr, ok := err.(*exec.ExitError); ok && exitErr.ExitCode() == 1 {
			return &ToolResult{
				Content: fmt.Sprintf("No matches found for pattern: %s", in.Pattern),
			}, nil
		}
		if ctx.Err() != nil {
			return &ToolResult{
				Content: "Error: search timed out or was cancelled",
				IsError: true,
			}, nil
		}
		// Return stderr if there's an error message
		if stderr.Len() > 0 {
			return &ToolResult{
				Content: fmt.Sprintf("Error: %s", strings.TrimSpace(stderr.String())),
				IsError: true,
			}, nil
		}
		return &ToolResult{
			Content: fmt.Sprintf("Error running search: %v", err),
			IsError: true,
		}, nil
	}

	output := strings.TrimSpace(stdout.String())
	if output == "" {
		return &ToolResult{
			Content: fmt.Sprintf("No matches found for pattern: %s", in.Pattern),
		}, nil
	}

	// Truncate if too long
	lines := strings.Split(output, "\n")
	if len(lines) > in.Limit {
		output = strings.Join(lines[:in.Limit], "\n")
		output += fmt.Sprintf("\n... (showing first %d matches)", in.Limit)
	}

	return &ToolResult{
		Content: output,
	}, nil
}

// executeWithGo uses pure Go implementation with parallel workers as fallback
func (t *GrepTool) executeWithGo(ctx context.Context, in *GrepInput) (*ToolResult, error) {
	// Compile regex
	flags := ""
	if in.CaseInsensitive {
		flags = "(?i)"
	}
	re, err := regexp.Compile(flags + in.Pattern)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Invalid regex pattern: %v", err),
			IsError: true,
		}, nil
	}

	// Get files to search
	var files []string
	info, err := os.Stat(in.Path)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error: %v", err),
			IsError: true,
		}, nil
	}

	if info.IsDir() {
		files, err = t.findFiles(ctx, in.Path, in.Glob)
		if err != nil {
			if ctx.Err() != nil {
				return &ToolResult{
					Content: "Error: search timed out or was cancelled",
					IsError: true,
				}, nil
			}
			return &ToolResult{
				Content: fmt.Sprintf("Error finding files: %v", err),
				IsError: true,
			}, nil
		}
	} else {
		files = []string{in.Path}
	}

	// Parallel search with worker pool
	numWorkers := runtime.NumCPU()
	if numWorkers > 8 {
		numWorkers = 8
	}
	if len(files) < numWorkers {
		numWorkers = len(files)
	}
	if numWorkers < 1 {
		numWorkers = 1
	}

	type indexedMatch struct {
		GrepMatch
		fileIdx int
	}

	var (
		matchCount atomic.Int64
		mu         sync.Mutex
		allMatches []indexedMatch
	)

	type fileItem struct {
		path string
		idx  int
	}
	fileCh := make(chan fileItem, numWorkers*2)

	var wg sync.WaitGroup
	for range numWorkers {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for item := range fileCh {
				if matchCount.Load() >= int64(in.Limit) {
					continue
				}
				remaining := in.Limit - int(matchCount.Load())
				if remaining <= 0 {
					continue
				}
				fileMatches, err := t.searchFile(item.path, re, remaining)
				if err != nil || len(fileMatches) == 0 {
					continue
				}
				batch := make([]indexedMatch, len(fileMatches))
				for i, m := range fileMatches {
					batch[i] = indexedMatch{GrepMatch: m, fileIdx: item.idx}
				}
				matchCount.Add(int64(len(batch)))
				mu.Lock()
				allMatches = append(allMatches, batch...)
				mu.Unlock()
			}
		}()
	}

	for i, f := range files {
		if ctx.Err() != nil || matchCount.Load() >= int64(in.Limit) {
			break
		}
		fileCh <- fileItem{f, i}
	}
	close(fileCh)
	wg.Wait()

	if len(allMatches) == 0 {
		return &ToolResult{
			Content: fmt.Sprintf("No matches found for pattern: %s", in.Pattern),
		}, nil
	}

	// Sort by file discovery order then line number for stable output
	sort.Slice(allMatches, func(i, j int) bool {
		if allMatches[i].fileIdx != allMatches[j].fileIdx {
			return allMatches[i].fileIdx < allMatches[j].fileIdx
		}
		return allMatches[i].Line < allMatches[j].Line
	})

	if len(allMatches) > in.Limit {
		allMatches = allMatches[:in.Limit]
	}

	var result strings.Builder
	for _, m := range allMatches {
		result.WriteString(fmt.Sprintf("%s:%d: %s\n", m.File, m.Line, m.Content))
	}

	if len(allMatches) >= in.Limit {
		result.WriteString(fmt.Sprintf("\n... (showing first %d matches)", in.Limit))
	}

	return &ToolResult{
		Content: strings.TrimSpace(result.String()),
	}, nil
}

// findFiles finds all files matching the glob in the directory
func (t *GrepTool) findFiles(ctx context.Context, dir, glob string) ([]string, error) {
	var files []string

	err := filepath.Walk(dir, func(path string, info os.FileInfo, err error) error {
		// Check for context cancellation
		select {
		case <-ctx.Done():
			return ctx.Err()
		default:
		}

		if err != nil {
			return nil
		}

		if info.IsDir() {
			// Skip hidden and common non-source directories
			name := info.Name()
			if strings.HasPrefix(name, ".") && name != "." {
				return filepath.SkipDir
			}
			if name == "node_modules" || name == "vendor" || name == "__pycache__" {
				return filepath.SkipDir
			}
			return nil
		}

		// Skip binary files by extension
		ext := filepath.Ext(path)
		binaryExts := map[string]bool{
			".exe": true, ".bin": true, ".so": true, ".dylib": true,
			".png": true, ".jpg": true, ".gif": true, ".ico": true,
			".zip": true, ".tar": true, ".gz": true,
		}
		if binaryExts[ext] {
			return nil
		}

		// Check glob pattern if specified
		if glob != "" {
			matched, _ := filepath.Match(glob, info.Name())
			if !matched {
				return nil
			}
		}

		files = append(files, path)

		// Limit files to search
		if len(files) >= 10000 {
			return filepath.SkipAll
		}
		return nil
	})

	return files, err
}

// searchFile searches a single file for the pattern
func (t *GrepTool) searchFile(path string, re *regexp.Regexp, maxMatches int) ([]GrepMatch, error) {
	file, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer file.Close()

	var matches []GrepMatch
	scanner := bufio.NewScanner(file)
	lineNum := 0

	for scanner.Scan() {
		lineNum++
		if len(matches) >= maxMatches {
			break
		}

		line := scanner.Text()
		if re.MatchString(line) {
			// Truncate long lines
			content := line
			if len(content) > 500 {
				content = content[:500] + "..."
			}

			matches = append(matches, GrepMatch{
				File:    path,
				Line:    lineNum,
				Content: content,
			})
		}
	}

	return matches, scanner.Err()
}

// RequiresApproval returns false - searching is safe
func (t *GrepTool) RequiresApproval() bool {
	return false
}

