//go:build windows

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
)

// SpotlightTool provides Windows file search via Windows Search or Everything.
type SpotlightTool struct {
	hasEverything bool
}

func NewSpotlightTool() *SpotlightTool {
	t := &SpotlightTool{}
	t.hasEverything = t.checkEverything()
	return t
}

func (t *SpotlightTool) checkEverything() bool {
	// Check if Everything CLI (es.exe) is available
	_, err := exec.LookPath("es.exe")
	if err == nil {
		return true
	}
	// Check common install location
	paths := []string{
		filepath.Join(os.Getenv("ProgramFiles"), "Everything", "es.exe"),
		filepath.Join(os.Getenv("ProgramFiles(x86)"), "Everything", "es.exe"),
		filepath.Join(os.Getenv("LOCALAPPDATA"), "Everything", "es.exe"),
	}
	for _, p := range paths {
		if _, err := os.Stat(p); err == nil {
			return true
		}
	}
	return false
}

func (t *SpotlightTool) Name() string { return "spotlight" }

func (t *SpotlightTool) Description() string {
	if t.hasEverything {
		return "Search Files (using Everything) - instant indexed file search. Find documents, apps, images, and more."
	}
	return "Search Files (using Windows Search) - find documents, apps, images, and more via PowerShell."
}

func (t *SpotlightTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"query": {"type": "string", "description": "Search query (filename pattern)"},
			"kind": {
				"type": "string",
				"enum": ["app", "folder", "document", "image", "audio", "video", "pdf"],
				"description": "Filter by file type"
			},
			"limit": {"type": "integer", "description": "Max results (default: 20)"},
			"dir": {"type": "string", "description": "Directory to search in"},
			"name": {"type": "boolean", "description": "Search by filename only"}
		},
		"required": ["query"]
	}`)
}

func (t *SpotlightTool) RequiresApproval() bool { return false }

type spotlightInputWin struct {
	Query string `json:"query"`
	Kind  string `json:"kind"`
	Limit int    `json:"limit"`
	Dir   string `json:"dir"`
	Name  bool   `json:"name"`
}

func (t *SpotlightTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p spotlightInputWin
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	if p.Query == "" {
		return &ToolResult{Content: "Query is required", IsError: true}, nil
	}

	if p.Limit <= 0 {
		p.Limit = 20
	}

	if t.hasEverything {
		return t.searchEverything(ctx, p)
	}
	return t.searchWindowsSearch(ctx, p)
}

func (t *SpotlightTool) searchEverything(ctx context.Context, p spotlightInputWin) (*ToolResult, error) {
	// Find es.exe path
	esPath := "es.exe"
	if _, err := exec.LookPath("es.exe"); err != nil {
		paths := []string{
			filepath.Join(os.Getenv("ProgramFiles"), "Everything", "es.exe"),
			filepath.Join(os.Getenv("ProgramFiles(x86)"), "Everything", "es.exe"),
			filepath.Join(os.Getenv("LOCALAPPDATA"), "Everything", "es.exe"),
		}
		for _, p := range paths {
			if _, err := os.Stat(p); err == nil {
				esPath = p
				break
			}
		}
	}

	args := []string{"-n", strconv.Itoa(p.Limit)}

	// Build search query
	query := p.Query

	// Add directory filter
	if p.Dir != "" {
		query = fmt.Sprintf(`"%s" %s`, p.Dir, query)
	}

	// Add file type filter
	if p.Kind != "" {
		extensions := t.getExtensionsForKind(p.Kind)
		if len(extensions) > 0 {
			extFilter := "ext:" + strings.Join(extensions, ";")
			query = extFilter + " " + query
		} else if p.Kind == "folder" {
			query = "folder: " + query
		}
	}

	args = append(args, query)

	cmd := exec.CommandContext(ctx, esPath, args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		output := strings.TrimSpace(string(out))
		if strings.Contains(output, "Everything IPC") || strings.Contains(output, "not running") {
			return &ToolResult{Content: "Everything search engine is not running. Please start Everything first."}, nil
		}
		if output == "" {
			return &ToolResult{Content: fmt.Sprintf("No files found matching '%s'", p.Query)}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Search failed: %v\nOutput: %s", err, output), IsError: true}, nil
	}

	results := strings.TrimSpace(string(out))
	if results == "" {
		return &ToolResult{Content: fmt.Sprintf("No files found matching '%s'", p.Query)}, nil
	}

	lines := strings.Split(results, "\n")
	return &ToolResult{Content: fmt.Sprintf("Found %d results:\n%s", len(lines), results)}, nil
}

func (t *SpotlightTool) searchWindowsSearch(ctx context.Context, p spotlightInputWin) (*ToolResult, error) {
	// Use PowerShell with Windows Search indexer
	searchDir := p.Dir
	if searchDir == "" {
		searchDir = os.Getenv("USERPROFILE")
	}

	// Build the PowerShell script for Windows Search
	script := fmt.Sprintf(`
$query = "%s"
$searchPath = "%s"
$limit = %d

# Use Get-ChildItem for recursive search
$results = Get-ChildItem -Path $searchPath -Recurse -ErrorAction SilentlyContinue | Where-Object { $_.Name -like "*$query*" }
`, escapeSpotlightPS(p.Query), escapeSpotlightPS(searchDir), p.Limit)

	// Add type filter
	if p.Kind != "" {
		extensions := t.getExtensionsForKind(p.Kind)
		if p.Kind == "folder" {
			script += `$results = $results | Where-Object { $_.PSIsContainer }
`
		} else if len(extensions) > 0 {
			extPattern := strings.Join(extensions, "|")
			script += fmt.Sprintf(`$results = $results | Where-Object { -not $_.PSIsContainer -and $_.Extension -match "%s" }
`, extPattern)
		}
	}

	script += fmt.Sprintf(`
$results | Select-Object -First %d | ForEach-Object { $_.FullName }
`, p.Limit)

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		output := strings.TrimSpace(string(out))
		if output == "" {
			return &ToolResult{Content: fmt.Sprintf("No files found matching '%s'", p.Query)}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Search failed: %v\nOutput: %s", err, output), IsError: true}, nil
	}

	results := strings.TrimSpace(string(out))
	if results == "" {
		return &ToolResult{Content: fmt.Sprintf("No files found matching '%s'", p.Query)}, nil
	}

	lines := strings.Split(results, "\n")
	return &ToolResult{Content: fmt.Sprintf("Found %d results:\n%s", len(lines), results)}, nil
}

func (t *SpotlightTool) getExtensionsForKind(kind string) []string {
	switch kind {
	case "app":
		return []string{".exe", ".msi", ".bat", ".cmd", ".ps1"}
	case "folder":
		return nil // handled separately
	case "document":
		return []string{".doc", ".docx", ".odt", ".txt", ".rtf", ".md", ".xls", ".xlsx", ".ppt", ".pptx"}
	case "image":
		return []string{".jpg", ".jpeg", ".png", ".gif", ".bmp", ".svg", ".webp", ".ico"}
	case "audio":
		return []string{".mp3", ".wav", ".flac", ".wma", ".m4a", ".aac"}
	case "video":
		return []string{".mp4", ".mkv", ".avi", ".mov", ".wmv", ".flv"}
	case "pdf":
		return []string{".pdf"}
	default:
		return nil
	}
}

func escapeSpotlightPS(s string) string {
	s = strings.ReplaceAll(s, "`", "``")
	s = strings.ReplaceAll(s, `"`, "`\"")
	s = strings.ReplaceAll(s, "$", "`$")
	return s
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewSpotlightTool(),
		Platforms: []string{PlatformWindows},
		Category:  "search",
	})
}
