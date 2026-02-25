//go:build linux

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

// SpotlightTool provides Linux file search via locate, plocate, or find.
type SpotlightTool struct {
	backend string // "plocate", "locate", "find", or ""
}

func NewSpotlightTool() *SpotlightTool {
	t := &SpotlightTool{}
	t.backend = t.detectBackend()
	return t
}

func (t *SpotlightTool) detectBackend() string {
	// plocate is the modern replacement for locate
	if _, err := exec.LookPath("plocate"); err == nil {
		return "plocate"
	}
	if _, err := exec.LookPath("locate"); err == nil {
		return "locate"
	}
	// find is always available
	if _, err := exec.LookPath("find"); err == nil {
		return "find"
	}
	return ""
}

func (t *SpotlightTool) Name() string { return "spotlight" }

func (t *SpotlightTool) Description() string {
	switch t.backend {
	case "plocate":
		return "Search Files (using plocate) - fast indexed file search. Find documents, apps, images, and more."
	case "locate":
		return "Search Files (using locate) - indexed file search. Find documents, apps, images, and more."
	case "find":
		return "Search Files (using find) - recursive directory search. Find documents, apps, images, and more."
	default:
		return "Search Files - requires locate/plocate or find to be installed."
	}
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
			"dir": {"type": "string", "description": "Directory to search in (default: home directory)"},
			"name": {"type": "boolean", "description": "Search by filename only (not path)"}
		},
		"required": ["query"]
	}`)
}

func (t *SpotlightTool) RequiresApproval() bool { return false }

type spotlightInputLinux struct {
	Query string `json:"query"`
	Kind  string `json:"kind"`
	Limit int    `json:"limit"`
	Dir   string `json:"dir"`
	Name  bool   `json:"name"`
}

func (t *SpotlightTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	if t.backend == "" {
		return &ToolResult{
			Content: "No file search backend available. Please install one of:\n" +
				"  - plocate: sudo apt install plocate (Debian/Ubuntu)\n" +
				"  - mlocate: sudo apt install mlocate (older systems)\n" +
				"After installation, run: sudo updatedb",
			IsError: true,
		}, nil
	}

	var p spotlightInputLinux
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	if p.Query == "" {
		return &ToolResult{Content: "Query is required", IsError: true}, nil
	}

	if p.Limit <= 0 {
		p.Limit = 20
	}

	switch t.backend {
	case "plocate", "locate":
		return t.searchLocate(ctx, p)
	case "find":
		return t.searchFind(ctx, p)
	default:
		return &ToolResult{Content: "Unknown backend", IsError: true}, nil
	}
}

func (t *SpotlightTool) searchLocate(ctx context.Context, p spotlightInputLinux) (*ToolResult, error) {
	args := []string{"-i", "-l", strconv.Itoa(p.Limit)}

	// Build pattern based on kind filter
	pattern := "*" + p.Query + "*"
	if p.Name {
		// For name-only search, we'll filter results after
	}

	// Add file type filter
	if p.Kind != "" {
		extensions := t.getExtensionsForKind(p.Kind)
		if len(extensions) > 0 {
			// locate doesn't support OR patterns well, so we'll do multiple searches
			var allResults []string
			for _, ext := range extensions {
				args := []string{"-i", "-l", strconv.Itoa(p.Limit)}
				searchPattern := "*" + p.Query + "*" + ext
				args = append(args, searchPattern)

				cmd := exec.CommandContext(ctx, t.backend, args...)
				out, _ := cmd.Output()
				results := strings.Split(strings.TrimSpace(string(out)), "\n")
				for _, r := range results {
					if r != "" {
						allResults = append(allResults, r)
					}
				}
				if len(allResults) >= p.Limit {
					break
				}
			}

			// Filter by directory if specified
			if p.Dir != "" {
				var filtered []string
				for _, r := range allResults {
					if strings.HasPrefix(r, p.Dir) {
						filtered = append(filtered, r)
					}
				}
				allResults = filtered
			}

			if len(allResults) == 0 {
				return &ToolResult{Content: fmt.Sprintf("No %s files found matching '%s'", p.Kind, p.Query)}, nil
			}
			if len(allResults) > p.Limit {
				allResults = allResults[:p.Limit]
			}
			return &ToolResult{Content: fmt.Sprintf("Found %d results:\n%s", len(allResults), strings.Join(allResults, "\n"))}, nil
		}
	}

	args = append(args, pattern)
	cmd := exec.CommandContext(ctx, t.backend, args...)
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

	// Filter by directory if specified
	if p.Dir != "" {
		lines := strings.Split(results, "\n")
		var filtered []string
		for _, line := range lines {
			if strings.HasPrefix(line, p.Dir) {
				filtered = append(filtered, line)
			}
		}
		if len(filtered) == 0 {
			return &ToolResult{Content: fmt.Sprintf("No files found matching '%s' in %s", p.Query, p.Dir)}, nil
		}
		results = strings.Join(filtered, "\n")
	}

	lines := strings.Split(results, "\n")
	return &ToolResult{Content: fmt.Sprintf("Found %d results:\n%s", len(lines), results)}, nil
}

func (t *SpotlightTool) searchFind(ctx context.Context, p spotlightInputLinux) (*ToolResult, error) {
	searchDir := p.Dir
	if searchDir == "" {
		searchDir = os.Getenv("HOME")
	}

	args := []string{searchDir, "-maxdepth", "10"}

	// Name pattern
	if p.Name {
		args = append(args, "-name", "*"+p.Query+"*")
	} else {
		args = append(args, "-iname", "*"+p.Query+"*")
	}

	// File type filter
	if p.Kind != "" {
		switch p.Kind {
		case "folder":
			args = append(args, "-type", "d")
		case "app":
			args = append(args, "-type", "f", "-executable")
		default:
			args = append(args, "-type", "f")
			extensions := t.getExtensionsForKind(p.Kind)
			if len(extensions) > 0 {
				// Build OR pattern for extensions
				var extArgs []string
				for i, ext := range extensions {
					if i > 0 {
						extArgs = append(extArgs, "-o")
					}
					extArgs = append(extArgs, "-iname", "*"+ext)
				}
				if len(extArgs) > 0 {
					args = append(args, "(")
					args = append(args, extArgs...)
					args = append(args, ")")
				}
			}
		}
	}

	args = append(args, "-print")

	cmd := exec.CommandContext(ctx, "find", args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		// find returns error if no matches, but that's ok
		output := strings.TrimSpace(string(out))
		if output == "" {
			return &ToolResult{Content: fmt.Sprintf("No files found matching '%s'", p.Query)}, nil
		}
	}

	results := strings.TrimSpace(string(out))
	if results == "" {
		return &ToolResult{Content: fmt.Sprintf("No files found matching '%s'", p.Query)}, nil
	}

	lines := strings.Split(results, "\n")
	if len(lines) > p.Limit {
		lines = lines[:p.Limit]
		results = strings.Join(lines, "\n")
	}

	return &ToolResult{Content: fmt.Sprintf("Found %d results:\n%s", len(lines), results)}, nil
}

func (t *SpotlightTool) getExtensionsForKind(kind string) []string {
	switch kind {
	case "app":
		return []string{".desktop", ".AppImage"}
	case "folder":
		return nil // handled separately
	case "document":
		return []string{".doc", ".docx", ".odt", ".txt", ".rtf", ".md"}
	case "image":
		return []string{".jpg", ".jpeg", ".png", ".gif", ".bmp", ".svg", ".webp"}
	case "audio":
		return []string{".mp3", ".wav", ".flac", ".ogg", ".m4a", ".aac"}
	case "video":
		return []string{".mp4", ".mkv", ".avi", ".mov", ".webm", ".flv"}
	case "pdf":
		return []string{".pdf"}
	default:
		return nil
	}
}

// launchApp tries to launch an application by name
func (t *SpotlightTool) launchApp(ctx context.Context, name string) (*ToolResult, error) {
	// Try to find .desktop file
	desktopDirs := []string{
		"/usr/share/applications",
		"/usr/local/share/applications",
		filepath.Join(os.Getenv("HOME"), ".local/share/applications"),
	}

	for _, dir := range desktopDirs {
		entries, err := os.ReadDir(dir)
		if err != nil {
			continue
		}
		for _, e := range entries {
			if strings.Contains(strings.ToLower(e.Name()), strings.ToLower(name)) && strings.HasSuffix(e.Name(), ".desktop") {
				desktopFile := filepath.Join(dir, e.Name())
				cmd := exec.CommandContext(ctx, "gtk-launch", strings.TrimSuffix(e.Name(), ".desktop"))
				if err := cmd.Start(); err == nil {
					return &ToolResult{Content: fmt.Sprintf("Launched: %s", e.Name())}, nil
				}
				// Try xdg-open as fallback
				cmd = exec.CommandContext(ctx, "xdg-open", desktopFile)
				if err := cmd.Start(); err == nil {
					return &ToolResult{Content: fmt.Sprintf("Launched: %s", e.Name())}, nil
				}
			}
		}
	}

	// Try direct command
	cmd := exec.CommandContext(ctx, name)
	if err := cmd.Start(); err == nil {
		return &ToolResult{Content: fmt.Sprintf("Launched: %s", name)}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Could not launch application: %s", name), IsError: true}, nil
}

