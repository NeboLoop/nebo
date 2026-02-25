//go:build darwin && !ios

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// SpotlightTool searches files using macOS Spotlight (mdfind).
type SpotlightTool struct{}

func NewSpotlightTool() *SpotlightTool { return &SpotlightTool{} }

func (t *SpotlightTool) Name() string { return "spotlight" }

func (t *SpotlightTool) Description() string {
	return "Search files and applications using Spotlight. Find documents, apps, images, and more."
}

func (t *SpotlightTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"query": {"type": "string", "description": "Search query"},
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

type spotlightInput struct {
	Query string `json:"query"`
	Kind  string `json:"kind"`
	Limit int    `json:"limit"`
	Dir   string `json:"dir"`
	Name  bool   `json:"name"`
}

func (t *SpotlightTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p spotlightInput
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}
	if p.Query == "" {
		return &ToolResult{Content: "Search query is required", IsError: true}, nil
	}
	return t.search(p)
}

func (t *SpotlightTool) search(p spotlightInput) (*ToolResult, error) {
	query := p.Query

	// Add kind filter
	kindMap := map[string]string{
		"app":      "kMDItemContentType == 'com.apple.application-bundle'",
		"folder":   "kMDItemContentType == 'public.folder'",
		"document": "kMDItemKind == 'Document'",
		"image":    "kMDItemContentTypeTree == 'public.image'",
		"audio":    "kMDItemContentTypeTree == 'public.audio'",
		"video":    "kMDItemContentTypeTree == 'public.movie'",
		"pdf":      "kMDItemContentType == 'com.adobe.pdf'",
	}
	if kindQuery, ok := kindMap[p.Kind]; ok {
		query = fmt.Sprintf("(%s) && (%s)", kindQuery, query)
	}

	// Search by name only
	if p.Name {
		query = fmt.Sprintf("kMDItemDisplayName == '*%s*'wc", p.Query)
	}

	args := []string{query}
	if p.Dir != "" {
		args = append(args, "-onlyin", p.Dir)
	}

	out, err := exec.Command("mdfind", args...).CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Search failed: %v\n%s", err, string(out)), IsError: true}, nil
	}

	results := strings.TrimSpace(string(out))
	if results == "" {
		return &ToolResult{Content: fmt.Sprintf("No results for '%s'", p.Query)}, nil
	}

	lines := strings.Split(results, "\n")
	limit := p.Limit
	if limit <= 0 {
		limit = 20
	}
	if len(lines) > limit {
		lines = lines[:limit]
	}

	return &ToolResult{Content: fmt.Sprintf("Search results for '%s':\n%s", p.Query, strings.Join(lines, "\n"))}, nil
}

