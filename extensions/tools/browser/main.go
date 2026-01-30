// Browser Plugin - macOS browser control (Safari/Chrome)
// Build: go build -o ~/.gobot/plugins/tools/browser
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

type BrowserTool struct{}

type browserInput struct {
	Action  string `json:"action"`  // open, tabs, close, focus, search, current, back, forward, reload
	URL     string `json:"url"`     // URL to open
	Browser string `json:"browser"` // safari, chrome, firefox, arc, brave (default: safari)
	Query   string `json:"query"`   // Search query
	TabID   int    `json:"tab_id"`  // Tab index (1-based)
}

type ToolResult struct {
	Content string `json:"content"`
	IsError bool   `json:"is_error"`
}

func (t *BrowserTool) Name() string {
	return "browser_control"
}

func (t *BrowserTool) Description() string {
	return "Control web browsers - open URLs, manage tabs, navigate, search. Supports Safari, Chrome, Firefox, Arc, and Brave."
}

func (t *BrowserTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["open", "tabs", "close", "focus", "search", "current", "back", "forward", "reload"],
				"description": "Action: open (URL), tabs (list), close (tab), focus (tab), search (Google), current (active URL), back, forward, reload"
			},
			"url": {
				"type": "string",
				"description": "URL to open"
			},
			"browser": {
				"type": "string",
				"enum": ["safari", "chrome", "firefox", "arc", "brave"],
				"description": "Browser to control (default: safari)"
			},
			"query": {
				"type": "string",
				"description": "Search query for Google search"
			},
			"tab_id": {
				"type": "integer",
				"description": "Tab index (1-based) for close/focus actions"
			}
		},
		"required": ["action"]
	}`)
}

func (t *BrowserTool) RequiresApproval() bool {
	return false
}

func (t *BrowserTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params browserInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	browser := params.Browser
	if browser == "" {
		browser = "safari"
	}

	switch params.Action {
	case "open":
		return t.openURL(browser, params.URL)
	case "tabs":
		return t.listTabs(browser)
	case "close":
		return t.closeTab(browser, params.TabID)
	case "focus":
		return t.focusTab(browser, params.TabID)
	case "search":
		return t.search(browser, params.Query)
	case "current":
		return t.getCurrentURL(browser)
	case "back":
		return t.navigate(browser, "back")
	case "forward":
		return t.navigate(browser, "forward")
	case "reload":
		return t.navigate(browser, "reload")
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", params.Action), IsError: true}, nil
	}
}

func (t *BrowserTool) getBrowserAppName(browser string) string {
	switch browser {
	case "chrome":
		return "Google Chrome"
	case "firefox":
		return "Firefox"
	case "arc":
		return "Arc"
	case "brave":
		return "Brave Browser"
	default:
		return "Safari"
	}
}

func (t *BrowserTool) openURL(browser, url string) (*ToolResult, error) {
	if url == "" {
		return &ToolResult{Content: "URL is required", IsError: true}, nil
	}

	if !strings.HasPrefix(url, "http://") && !strings.HasPrefix(url, "https://") {
		url = "https://" + url
	}

	appName := t.getBrowserAppName(browser)

	var script string
	if browser == "safari" {
		script = fmt.Sprintf(`
			tell application "Safari"
				activate
				open location "%s"
			end tell
		`, escapeAppleScript(url))
	} else {
		script = fmt.Sprintf(`
			tell application "%s"
				activate
				open location "%s"
			end tell
		`, appName, escapeAppleScript(url))
	}

	_, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to open URL: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Opened %s in %s", url, appName), IsError: false}, nil
}

func (t *BrowserTool) listTabs(browser string) (*ToolResult, error) {
	appName := t.getBrowserAppName(browser)

	var script string
	if browser == "safari" {
		script = `
			tell application "Safari"
				set tabList to {}
				set windowCount to count of windows
				repeat with w from 1 to windowCount
					set tabCount to count of tabs of window w
					repeat with t from 1 to tabCount
						set theTab to tab t of window w
						set tabInfo to "W" & w & " T" & t & ": " & name of theTab & " - " & URL of theTab
						set end of tabList to tabInfo
					end repeat
				end repeat
				return tabList
			end tell
		`
	} else if browser == "chrome" || browser == "brave" || browser == "arc" {
		script = fmt.Sprintf(`
			tell application "%s"
				set tabList to {}
				set windowCount to count of windows
				repeat with w from 1 to windowCount
					set tabCount to count of tabs of window w
					repeat with t from 1 to tabCount
						set theTab to tab t of window w
						set tabInfo to "W" & w & " T" & t & ": " & title of theTab & " - " & URL of theTab
						set end of tabList to tabInfo
					end repeat
				end repeat
				return tabList
			end tell
		`, appName)
	} else {
		return &ToolResult{Content: fmt.Sprintf("Tab listing not supported for %s", browser), IsError: true}, nil
	}

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to list tabs: %v", err), IsError: true}, nil
	}
	if output == "" || output == "{}" {
		return &ToolResult{Content: "No tabs open", IsError: false}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Open tabs:\n%s", output), IsError: false}, nil
}

func (t *BrowserTool) closeTab(browser string, tabID int) (*ToolResult, error) {
	if tabID <= 0 {
		return &ToolResult{Content: "Tab ID is required (1-based index)", IsError: true}, nil
	}

	appName := t.getBrowserAppName(browser)

	var script string
	if browser == "safari" {
		script = fmt.Sprintf(`
			tell application "Safari"
				close tab %d of window 1
			end tell
			return "Tab closed"
		`, tabID)
	} else {
		script = fmt.Sprintf(`
			tell application "%s"
				close tab %d of window 1
			end tell
			return "Tab closed"
		`, appName, tabID)
	}

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to close tab: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func (t *BrowserTool) focusTab(browser string, tabID int) (*ToolResult, error) {
	if tabID <= 0 {
		return &ToolResult{Content: "Tab ID is required (1-based index)", IsError: true}, nil
	}

	appName := t.getBrowserAppName(browser)

	var script string
	if browser == "safari" {
		script = fmt.Sprintf(`
			tell application "Safari"
				activate
				set current tab of window 1 to tab %d of window 1
			end tell
			return "Tab focused"
		`, tabID)
	} else {
		script = fmt.Sprintf(`
			tell application "%s"
				activate
				set active tab index of window 1 to %d
			end tell
			return "Tab focused"
		`, appName, tabID)
	}

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to focus tab: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func (t *BrowserTool) search(browser, query string) (*ToolResult, error) {
	if query == "" {
		return &ToolResult{Content: "Search query is required", IsError: true}, nil
	}

	searchURL := fmt.Sprintf("https://www.google.com/search?q=%s", strings.ReplaceAll(query, " ", "+"))
	return t.openURL(browser, searchURL)
}

func (t *BrowserTool) getCurrentURL(browser string) (*ToolResult, error) {
	appName := t.getBrowserAppName(browser)

	var script string
	if browser == "safari" {
		script = `
			tell application "Safari"
				set currentURL to URL of current tab of window 1
				set currentTitle to name of current tab of window 1
				return currentTitle & return & currentURL
			end tell
		`
	} else {
		script = fmt.Sprintf(`
			tell application "%s"
				set currentURL to URL of active tab of window 1
				set currentTitle to title of active tab of window 1
				return currentTitle & return & currentURL
			end tell
		`, appName)
	}

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get current URL: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Current page:\n%s", output), IsError: false}, nil
}

func (t *BrowserTool) navigate(browser, direction string) (*ToolResult, error) {
	appName := t.getBrowserAppName(browser)

	var script string
	var action string

	switch direction {
	case "back":
		action = "go back"
	case "forward":
		action = "go forward"
	case "reload":
		action = "reload"
	}

	if browser == "safari" {
		script = fmt.Sprintf(`
			tell application "Safari"
				tell document 1
					do JavaScript "history.%s()"
				end tell
			end tell
		`, direction)
		if direction == "reload" {
			script = `
				tell application "Safari"
					tell document 1
						do JavaScript "location.reload()"
					end tell
				end tell
			`
		}
	} else {
		script = fmt.Sprintf(`
			tell application "%s"
				tell active tab of window 1
					%s
				end tell
			end tell
		`, appName, action)
	}

	_, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to %s: %v", direction, err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Navigated %s", direction), IsError: false}, nil
}

func escapeAppleScript(s string) string {
	s = strings.ReplaceAll(s, "\\", "\\\\")
	s = strings.ReplaceAll(s, "\"", "\\\"")
	return s
}

func runAppleScript(script string) (string, error) {
	cmd := exec.Command("osascript", "-e", script)
	output, err := cmd.CombinedOutput()
	return strings.TrimSpace(string(output)), err
}

// RPC wrapper
type BrowserToolRPC struct {
	tool *BrowserTool
}

func (t *BrowserToolRPC) Name(args interface{}, reply *string) error {
	*reply = t.tool.Name()
	return nil
}

func (t *BrowserToolRPC) Description(args interface{}, reply *string) error {
	*reply = t.tool.Description()
	return nil
}

func (t *BrowserToolRPC) Schema(args interface{}, reply *json.RawMessage) error {
	*reply = t.tool.Schema()
	return nil
}

func (t *BrowserToolRPC) RequiresApproval(args interface{}, reply *bool) error {
	*reply = t.tool.RequiresApproval()
	return nil
}

type ExecuteArgs struct {
	Input json.RawMessage
}

func (t *BrowserToolRPC) Execute(args *ExecuteArgs, reply *ToolResult) error {
	result, err := t.tool.Execute(context.Background(), args.Input)
	if err != nil {
		return err
	}
	*reply = *result
	return nil
}

type BrowserPlugin struct {
	tool *BrowserTool
}

func (p *BrowserPlugin) Server(*plugin.MuxBroker) (interface{}, error) {
	return &BrowserToolRPC{tool: p.tool}, nil
}

func (p *BrowserPlugin) Client(b *plugin.MuxBroker, c *rpc.Client) (interface{}, error) {
	return nil, nil
}

func main() {
	plugin.Serve(&plugin.ServeConfig{
		HandshakeConfig: Handshake,
		Plugins: map[string]plugin.Plugin{
			"tool": &BrowserPlugin{tool: &BrowserTool{}},
		},
	})
}
