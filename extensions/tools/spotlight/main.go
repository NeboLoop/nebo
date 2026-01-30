// Spotlight Plugin - macOS Spotlight search integration
// Build: go build -o ~/.nebo/plugins/tools/spotlight
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"net/rpc"
	"os/exec"
	"strings"

	"github.com/hashicorp/go-plugin"
)

var Handshake = plugin.HandshakeConfig{
	ProtocolVersion:  1,
	MagicCookieKey:   "GOBOT_PLUGIN",
	MagicCookieValue: "gobot-plugin-v1",
}

type SpotlightTool struct{}

type spotlightInput struct {
	Query   string `json:"query"`    // Search query
	Kind    string `json:"kind"`     // File kind filter (app, folder, document, image, audio, video, pdf)
	Limit   int    `json:"limit"`    // Max results
	Dir     string `json:"dir"`      // Directory to search in
	Name    bool   `json:"name"`     // Search by name only
}

type ToolResult struct {
	Content string `json:"content"`
	IsError bool   `json:"is_error"`
}

func (t *SpotlightTool) Name() string {
	return "spotlight"
}

func (t *SpotlightTool) Description() string {
	return "Search files and applications using macOS Spotlight. Find documents, apps, images, and more."
}

func (t *SpotlightTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"query": {
				"type": "string",
				"description": "Search query"
			},
			"kind": {
				"type": "string",
				"enum": ["app", "folder", "document", "image", "audio", "video", "pdf", "email", "contact", "event"],
				"description": "Filter by file type"
			},
			"limit": {
				"type": "integer",
				"description": "Maximum number of results (default: 20)"
			},
			"dir": {
				"type": "string",
				"description": "Directory to search in (default: entire system)"
			},
			"name": {
				"type": "boolean",
				"description": "Search by filename only (not content)"
			}
		},
		"required": ["query"]
	}`)
}

func (t *SpotlightTool) RequiresApproval() bool {
	return false
}

func (t *SpotlightTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params spotlightInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	if params.Query == "" {
		return &ToolResult{Content: "Search query is required", IsError: true}, nil
	}

	return t.search(params)
}

func (t *SpotlightTool) search(params spotlightInput) (*ToolResult, error) {
	args := []string{}

	// Build mdfind query
	query := params.Query

	// Add kind filter
	if params.Kind != "" {
		kindMap := map[string]string{
			"app":      "kMDItemContentType == 'com.apple.application-bundle'",
			"folder":   "kMDItemContentType == 'public.folder'",
			"document": "kMDItemKind == 'Document'",
			"image":    "kMDItemContentTypeTree == 'public.image'",
			"audio":    "kMDItemContentTypeTree == 'public.audio'",
			"video":    "kMDItemContentTypeTree == 'public.movie'",
			"pdf":      "kMDItemContentType == 'com.adobe.pdf'",
			"email":    "kMDItemContentType == 'com.apple.mail.emlx'",
			"contact":  "kMDItemContentType == 'com.apple.addressbook.person'",
			"event":    "kMDItemContentType == 'com.apple.ical.event'",
		}
		if kindQuery, ok := kindMap[params.Kind]; ok {
			query = fmt.Sprintf("(%s) && (%s)", kindQuery, query)
		}
	}

	// Search by name only
	if params.Name {
		query = fmt.Sprintf("kMDItemDisplayName == '*%s*'wc", params.Query)
	}

	args = append(args, query)

	// Add directory scope
	if params.Dir != "" {
		args = append(args, "-onlyin", params.Dir)
	}

	cmd := exec.Command("mdfind", args...)
	output, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Search failed: %v\n%s", err, string(output)), IsError: true}, nil
	}

	results := strings.TrimSpace(string(output))
	if results == "" {
		return &ToolResult{Content: fmt.Sprintf("No results found for '%s'", params.Query), IsError: false}, nil
	}

	// Limit results
	lines := strings.Split(results, "\n")
	limit := params.Limit
	if limit <= 0 {
		limit = 20
	}
	if len(lines) > limit {
		lines = lines[:limit]
	}

	return &ToolResult{Content: fmt.Sprintf("Search results for '%s':\n%s", params.Query, strings.Join(lines, "\n")), IsError: false}, nil
}

// RPC wrapper
type SpotlightToolRPC struct {
	tool *SpotlightTool
}

func (t *SpotlightToolRPC) Name(args interface{}, reply *string) error {
	*reply = t.tool.Name()
	return nil
}

func (t *SpotlightToolRPC) Description(args interface{}, reply *string) error {
	*reply = t.tool.Description()
	return nil
}

func (t *SpotlightToolRPC) Schema(args interface{}, reply *json.RawMessage) error {
	*reply = t.tool.Schema()
	return nil
}

func (t *SpotlightToolRPC) RequiresApproval(args interface{}, reply *bool) error {
	*reply = t.tool.RequiresApproval()
	return nil
}

type ExecuteArgs struct {
	Input json.RawMessage
}

func (t *SpotlightToolRPC) Execute(args *ExecuteArgs, reply *ToolResult) error {
	result, err := t.tool.Execute(context.Background(), args.Input)
	if err != nil {
		return err
	}
	*reply = *result
	return nil
}

type SpotlightPlugin struct {
	tool *SpotlightTool
}

func (p *SpotlightPlugin) Server(*plugin.MuxBroker) (interface{}, error) {
	return &SpotlightToolRPC{tool: p.tool}, nil
}

func (p *SpotlightPlugin) Client(b *plugin.MuxBroker, c *rpc.Client) (interface{}, error) {
	return nil, nil
}

func main() {
	plugin.Serve(&plugin.ServeConfig{
		HandshakeConfig: Handshake,
		Plugins: map[string]plugin.Plugin{
			"tool": &SpotlightPlugin{tool: &SpotlightTool{}},
		},
	})
}
