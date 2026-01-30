// Notion Plugin - Interact with Notion API
// Build: go build -o ~/.nebo/plugins/tools/notion
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/rpc"
	"os"
	"strings"
	"time"

	"github.com/hashicorp/go-plugin"
)

var Handshake = plugin.HandshakeConfig{
	ProtocolVersion:  1,
	MagicCookieKey:   "GOBOT_PLUGIN",
	MagicCookieValue: "gobot-plugin-v1",
}

type NotionTool struct {
	token string
}

type notionInput struct {
	Action   string `json:"action"`    // search, get_page, get_database, query_database, create_page, append_blocks
	PageID   string `json:"page_id"`   // Page ID for get/update
	DBID     string `json:"db_id"`     // Database ID
	Query    string `json:"query"`     // Search query
	Title    string `json:"title"`     // Page title
	Content  string `json:"content"`   // Content to add
	ParentID string `json:"parent_id"` // Parent page ID for create
	Filter   string `json:"filter"`    // JSON filter for database query
}

type ToolResult struct {
	Content string `json:"content"`
	IsError bool   `json:"is_error"`
}

func (t *NotionTool) Name() string {
	return "notion"
}

func (t *NotionTool) Description() string {
	return "Interact with Notion: search, read pages/databases, create pages, and add content."
}

func (t *NotionTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["search", "get_page", "get_database", "query_database", "create_page", "append_blocks"],
				"description": "Action: search (find pages/databases), get_page, get_database, query_database, create_page, append_blocks"
			},
			"page_id": {
				"type": "string",
				"description": "Page ID for get_page or append_blocks"
			},
			"db_id": {
				"type": "string",
				"description": "Database ID for query_database"
			},
			"query": {
				"type": "string",
				"description": "Search query"
			},
			"title": {
				"type": "string",
				"description": "Page title for create_page"
			},
			"content": {
				"type": "string",
				"description": "Content to add (markdown-style text)"
			},
			"parent_id": {
				"type": "string",
				"description": "Parent page or database ID for create_page"
			},
			"filter": {
				"type": "string",
				"description": "JSON filter for query_database"
			}
		},
		"required": ["action"]
	}`)
}

func (t *NotionTool) RequiresApproval() bool {
	return false
}

func (t *NotionTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params notionInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	// Get token
	token := t.token
	if token == "" {
		token = os.Getenv("NOTION_API_KEY")
	}
	if token == "" {
		return &ToolResult{Content: "NOTION_API_KEY not set", IsError: true}, nil
	}

	var result string
	var err error

	switch params.Action {
	case "search":
		result, err = t.search(ctx, token, params)
	case "get_page":
		result, err = t.getPage(ctx, token, params)
	case "get_database":
		result, err = t.getDatabase(ctx, token, params)
	case "query_database":
		result, err = t.queryDatabase(ctx, token, params)
	case "create_page":
		result, err = t.createPage(ctx, token, params)
	case "append_blocks":
		result, err = t.appendBlocks(ctx, token, params)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", params.Action), IsError: true}, nil
	}

	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Notion API error: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: result, IsError: false}, nil
}

func (t *NotionTool) apiRequest(ctx context.Context, token, method, endpoint string, body io.Reader) ([]byte, error) {
	url := "https://api.notion.com/v1" + endpoint
	req, err := http.NewRequestWithContext(ctx, method, url, body)
	if err != nil {
		return nil, err
	}

	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Notion-Version", "2022-06-28")
	req.Header.Set("Content-Type", "application/json")

	client := &http.Client{Timeout: 30 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	respBody, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, err
	}

	if resp.StatusCode >= 400 {
		return nil, fmt.Errorf("HTTP %d: %s", resp.StatusCode, string(respBody))
	}

	return respBody, nil
}

func (t *NotionTool) search(ctx context.Context, token string, params notionInput) (string, error) {
	body := map[string]interface{}{}
	if params.Query != "" {
		body["query"] = params.Query
	}
	body["page_size"] = 10

	jsonBody, _ := json.Marshal(body)
	data, err := t.apiRequest(ctx, token, "POST", "/search", strings.NewReader(string(jsonBody)))
	if err != nil {
		return "", err
	}

	var result struct {
		Results []struct {
			Object     string `json:"object"`
			ID         string `json:"id"`
			CreatedTime string `json:"created_time"`
			Properties map[string]interface{} `json:"properties"`
			Parent     struct {
				Type string `json:"type"`
			} `json:"parent"`
		} `json:"results"`
	}
	if err := json.Unmarshal(data, &result); err != nil {
		return "", err
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Search results (%d):\n\n", len(result.Results)))
	for _, r := range result.Results {
		title := t.extractTitle(r.Properties)
		sb.WriteString(fmt.Sprintf("- [%s] %s\n  ID: %s\n", r.Object, title, r.ID))
	}

	return sb.String(), nil
}

func (t *NotionTool) extractTitle(properties map[string]interface{}) string {
	// Try common title property names
	for _, key := range []string{"title", "Title", "Name", "name"} {
		if prop, ok := properties[key]; ok {
			if propMap, ok := prop.(map[string]interface{}); ok {
				if title, ok := propMap["title"].([]interface{}); ok && len(title) > 0 {
					if textObj, ok := title[0].(map[string]interface{}); ok {
						if text, ok := textObj["plain_text"].(string); ok {
							return text
						}
					}
				}
			}
		}
	}
	return "(untitled)"
}

func (t *NotionTool) getPage(ctx context.Context, token string, params notionInput) (string, error) {
	if params.PageID == "" {
		return "", fmt.Errorf("page_id is required")
	}

	// Get page metadata
	pageData, err := t.apiRequest(ctx, token, "GET", "/pages/"+params.PageID, nil)
	if err != nil {
		return "", err
	}

	var page struct {
		ID         string                 `json:"id"`
		CreatedTime string                `json:"created_time"`
		Properties map[string]interface{} `json:"properties"`
		URL        string                 `json:"url"`
	}
	if err := json.Unmarshal(pageData, &page); err != nil {
		return "", err
	}

	// Get page content (blocks)
	blocksData, err := t.apiRequest(ctx, token, "GET", "/blocks/"+params.PageID+"/children?page_size=100", nil)
	if err != nil {
		return "", err
	}

	var blocks struct {
		Results []map[string]interface{} `json:"results"`
	}
	if err := json.Unmarshal(blocksData, &blocks); err != nil {
		return "", err
	}

	var sb strings.Builder
	title := t.extractTitle(page.Properties)
	sb.WriteString(fmt.Sprintf("Page: %s\n", title))
	sb.WriteString(fmt.Sprintf("ID: %s\n", page.ID))
	sb.WriteString(fmt.Sprintf("URL: %s\n\n", page.URL))
	sb.WriteString("Content:\n")

	for _, block := range blocks.Results {
		blockType := block["type"].(string)
		content := t.extractBlockContent(block, blockType)
		if content != "" {
			sb.WriteString(content + "\n")
		}
	}

	return sb.String(), nil
}

func (t *NotionTool) extractBlockContent(block map[string]interface{}, blockType string) string {
	if typeData, ok := block[blockType].(map[string]interface{}); ok {
		if richText, ok := typeData["rich_text"].([]interface{}); ok {
			var texts []string
			for _, rt := range richText {
				if rtMap, ok := rt.(map[string]interface{}); ok {
					if text, ok := rtMap["plain_text"].(string); ok {
						texts = append(texts, text)
					}
				}
			}
			prefix := ""
			switch blockType {
			case "heading_1":
				prefix = "# "
			case "heading_2":
				prefix = "## "
			case "heading_3":
				prefix = "### "
			case "bulleted_list_item":
				prefix = "- "
			case "numbered_list_item":
				prefix = "1. "
			case "to_do":
				checked := false
				if c, ok := typeData["checked"].(bool); ok {
					checked = c
				}
				if checked {
					prefix = "[x] "
				} else {
					prefix = "[ ] "
				}
			case "code":
				lang := ""
				if l, ok := typeData["language"].(string); ok {
					lang = l
				}
				return fmt.Sprintf("```%s\n%s\n```", lang, strings.Join(texts, ""))
			}
			return prefix + strings.Join(texts, "")
		}
	}
	return ""
}

func (t *NotionTool) getDatabase(ctx context.Context, token string, params notionInput) (string, error) {
	if params.DBID == "" {
		return "", fmt.Errorf("db_id is required")
	}

	data, err := t.apiRequest(ctx, token, "GET", "/databases/"+params.DBID, nil)
	if err != nil {
		return "", err
	}

	var db struct {
		ID          string                 `json:"id"`
		Title       []map[string]interface{} `json:"title"`
		Properties  map[string]interface{} `json:"properties"`
		URL         string                 `json:"url"`
	}
	if err := json.Unmarshal(data, &db); err != nil {
		return "", err
	}

	var sb strings.Builder
	title := "(untitled)"
	if len(db.Title) > 0 {
		if text, ok := db.Title[0]["plain_text"].(string); ok {
			title = text
		}
	}

	sb.WriteString(fmt.Sprintf("Database: %s\n", title))
	sb.WriteString(fmt.Sprintf("ID: %s\n", db.ID))
	sb.WriteString(fmt.Sprintf("URL: %s\n\n", db.URL))
	sb.WriteString("Properties:\n")

	for name, prop := range db.Properties {
		if propMap, ok := prop.(map[string]interface{}); ok {
			propType := propMap["type"].(string)
			sb.WriteString(fmt.Sprintf("- %s (%s)\n", name, propType))
		}
	}

	return sb.String(), nil
}

func (t *NotionTool) queryDatabase(ctx context.Context, token string, params notionInput) (string, error) {
	if params.DBID == "" {
		return "", fmt.Errorf("db_id is required")
	}

	body := map[string]interface{}{
		"page_size": 20,
	}
	if params.Filter != "" {
		var filter interface{}
		if err := json.Unmarshal([]byte(params.Filter), &filter); err == nil {
			body["filter"] = filter
		}
	}

	jsonBody, _ := json.Marshal(body)
	data, err := t.apiRequest(ctx, token, "POST", "/databases/"+params.DBID+"/query", strings.NewReader(string(jsonBody)))
	if err != nil {
		return "", err
	}

	var result struct {
		Results []struct {
			ID         string                 `json:"id"`
			Properties map[string]interface{} `json:"properties"`
			URL        string                 `json:"url"`
		} `json:"results"`
	}
	if err := json.Unmarshal(data, &result); err != nil {
		return "", err
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Query results (%d):\n\n", len(result.Results)))
	for _, r := range result.Results {
		title := t.extractTitle(r.Properties)
		sb.WriteString(fmt.Sprintf("- %s\n  ID: %s\n", title, r.ID))
	}

	return sb.String(), nil
}

func (t *NotionTool) createPage(ctx context.Context, token string, params notionInput) (string, error) {
	if params.ParentID == "" {
		return "", fmt.Errorf("parent_id is required")
	}
	if params.Title == "" {
		return "", fmt.Errorf("title is required")
	}

	body := map[string]interface{}{
		"parent": map[string]string{
			"page_id": params.ParentID,
		},
		"properties": map[string]interface{}{
			"title": map[string]interface{}{
				"title": []map[string]interface{}{
					{
						"text": map[string]string{
							"content": params.Title,
						},
					},
				},
			},
		},
	}

	// Add content if provided
	if params.Content != "" {
		body["children"] = t.textToBlocks(params.Content)
	}

	jsonBody, _ := json.Marshal(body)
	data, err := t.apiRequest(ctx, token, "POST", "/pages", strings.NewReader(string(jsonBody)))
	if err != nil {
		return "", err
	}

	var page struct {
		ID  string `json:"id"`
		URL string `json:"url"`
	}
	if err := json.Unmarshal(data, &page); err != nil {
		return "", err
	}

	return fmt.Sprintf("Created page: %s\nURL: %s", page.ID, page.URL), nil
}

func (t *NotionTool) appendBlocks(ctx context.Context, token string, params notionInput) (string, error) {
	if params.PageID == "" {
		return "", fmt.Errorf("page_id is required")
	}
	if params.Content == "" {
		return "", fmt.Errorf("content is required")
	}

	body := map[string]interface{}{
		"children": t.textToBlocks(params.Content),
	}

	jsonBody, _ := json.Marshal(body)
	_, err := t.apiRequest(ctx, token, "PATCH", "/blocks/"+params.PageID+"/children", strings.NewReader(string(jsonBody)))
	if err != nil {
		return "", err
	}

	return "Content added successfully", nil
}

func (t *NotionTool) textToBlocks(content string) []map[string]interface{} {
	lines := strings.Split(content, "\n")
	var blocks []map[string]interface{}

	for _, line := range lines {
		if line == "" {
			continue
		}

		blockType := "paragraph"
		text := line

		if strings.HasPrefix(line, "# ") {
			blockType = "heading_1"
			text = strings.TrimPrefix(line, "# ")
		} else if strings.HasPrefix(line, "## ") {
			blockType = "heading_2"
			text = strings.TrimPrefix(line, "## ")
		} else if strings.HasPrefix(line, "### ") {
			blockType = "heading_3"
			text = strings.TrimPrefix(line, "### ")
		} else if strings.HasPrefix(line, "- ") {
			blockType = "bulleted_list_item"
			text = strings.TrimPrefix(line, "- ")
		} else if strings.HasPrefix(line, "* ") {
			blockType = "bulleted_list_item"
			text = strings.TrimPrefix(line, "* ")
		}

		blocks = append(blocks, map[string]interface{}{
			"object": "block",
			"type":   blockType,
			blockType: map[string]interface{}{
				"rich_text": []map[string]interface{}{
					{
						"type": "text",
						"text": map[string]string{
							"content": text,
						},
					},
				},
			},
		})
	}

	return blocks
}

// RPC wrapper
type NotionToolRPC struct {
	tool *NotionTool
}

func (t *NotionToolRPC) Name(args interface{}, reply *string) error {
	*reply = t.tool.Name()
	return nil
}

func (t *NotionToolRPC) Description(args interface{}, reply *string) error {
	*reply = t.tool.Description()
	return nil
}

func (t *NotionToolRPC) Schema(args interface{}, reply *json.RawMessage) error {
	*reply = t.tool.Schema()
	return nil
}

func (t *NotionToolRPC) RequiresApproval(args interface{}, reply *bool) error {
	*reply = t.tool.RequiresApproval()
	return nil
}

type ExecuteArgs struct {
	Input json.RawMessage
}

func (t *NotionToolRPC) Execute(args *ExecuteArgs, reply *ToolResult) error {
	result, err := t.tool.Execute(context.Background(), args.Input)
	if err != nil {
		return err
	}
	*reply = *result
	return nil
}

type NotionPlugin struct {
	tool *NotionTool
}

func (p *NotionPlugin) Server(*plugin.MuxBroker) (interface{}, error) {
	return &NotionToolRPC{tool: p.tool}, nil
}

func (p *NotionPlugin) Client(b *plugin.MuxBroker, c *rpc.Client) (interface{}, error) {
	return &NotionToolRPCClient{client: c}, nil
}

type NotionToolRPCClient struct {
	client *rpc.Client
}

func (c *NotionToolRPCClient) Name() string {
	var reply string
	c.client.Call("Plugin.Name", new(interface{}), &reply)
	return reply
}

func (c *NotionToolRPCClient) Description() string {
	var reply string
	c.client.Call("Plugin.Description", new(interface{}), &reply)
	return reply
}

func (c *NotionToolRPCClient) Schema() json.RawMessage {
	var reply json.RawMessage
	c.client.Call("Plugin.Schema", new(interface{}), &reply)
	return reply
}

func (c *NotionToolRPCClient) RequiresApproval() bool {
	var reply bool
	c.client.Call("Plugin.RequiresApproval", new(interface{}), &reply)
	return reply
}

func (c *NotionToolRPCClient) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var reply ToolResult
	err := c.client.Call("Plugin.Execute", &ExecuteArgs{Input: input}, &reply)
	return &reply, err
}

func main() {
	plugin.Serve(&plugin.ServeConfig{
		HandshakeConfig: Handshake,
		Plugins: map[string]plugin.Plugin{
			"tool": &NotionPlugin{tool: &NotionTool{}},
		},
	})
}
