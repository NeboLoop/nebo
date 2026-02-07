package tools

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/nebolabs/nebo/internal/browser"
)

// WebDomainTool provides web operations: fetch, search, browser automation
type WebDomainTool struct {
	client       *http.Client
	searchAPIKey string
	searchCX     string
	headless     bool
}

// WebDomainInput represents the consolidated input for all web operations
type WebDomainInput struct {
	// STRAP fields
	Action string `json:"action"` // fetch, search, browser (navigate, click, type, screenshot, etc.)

	// Fetch fields
	URL     string            `json:"url,omitempty"`
	Method  string            `json:"method,omitempty"` // GET, POST, etc.
	Headers map[string]string `json:"headers,omitempty"`
	Body    string            `json:"body,omitempty"`

	// Search fields
	Query  string `json:"query,omitempty"`
	Engine string `json:"engine,omitempty"` // duckduckgo, google
	Limit  int    `json:"limit,omitempty"`

	// Browser fields
	Profile  string `json:"profile,omitempty"`  // "nebo" (managed) or "chrome" (extension relay)
	Selector string `json:"selector,omitempty"` // CSS selector
	Text     string `json:"text,omitempty"`     // Text to type or JS to evaluate
	Value    string `json:"value,omitempty"`    // Value for fill action
	Output   string `json:"output,omitempty"`   // Output path for screenshot
	Timeout  int    `json:"timeout,omitempty"`  // Action timeout in seconds
	Ref      string `json:"ref,omitempty"`      // Element ref from snapshot (e.g., "e1", "e5")
	TargetID string `json:"target_id,omitempty"` // Page/tab ID for multi-tab control
}

// WebDomainConfig configures the web domain tool
type WebDomainConfig struct {
	SearchAPIKey string // For Google Custom Search (optional)
	SearchCX     string // Google Custom Search Engine ID (optional)
	Headless     bool   // Browser headless mode
}

// NewWebDomainTool creates a new web domain tool with SSRF-safe HTTP client
func NewWebDomainTool() *WebDomainTool {
	return &WebDomainTool{
		client: &http.Client{
			Timeout:       30 * time.Second,
			Transport:     ssrfSafeTransport(),
			CheckRedirect: ssrfSafeRedirectCheck(),
		},
		headless: true,
	}
}

// NewWebDomainToolWithConfig creates a web tool with full configuration
func NewWebDomainToolWithConfig(cfg WebDomainConfig) *WebDomainTool {
	t := NewWebDomainTool()
	t.searchAPIKey = cfg.SearchAPIKey
	t.searchCX = cfg.SearchCX
	t.headless = cfg.Headless
	return t
}

// Name returns the tool name
func (t *WebDomainTool) Name() string {
	return "web"
}

// Domain returns the domain name
func (t *WebDomainTool) Domain() string {
	return "web"
}

// Resources returns available resources
func (t *WebDomainTool) Resources() []string {
	return []string{"fetch", "search", "browser"}
}

// ActionsFor returns available actions
func (t *WebDomainTool) ActionsFor(resource string) []string {
	switch resource {
	case "fetch":
		return []string{"get", "post", "put", "delete"}
	case "search":
		return []string{"search"}
	case "browser":
		return []string{"navigate", "click", "type", "screenshot", "text", "html", "evaluate", "wait", "snapshot", "click_ref", "type_ref"}
	default:
		return []string{}
	}
}

// Description returns the tool description
func (t *WebDomainTool) Description() string {
	return `Web operations: fetch URLs, search the web, browser automation with profile support.

Profiles:
- nebo (default): Managed browser instance with isolated profile
- chrome: Connect to user's Chrome via extension relay (access to logged-in sessions)

Actions:
- fetch: HTTP requests (GET, POST, etc.)
- search: Web search using DuckDuckGo or Google
- navigate: Browser navigation to URL
- click: Click element by ref (from snapshot) or CSS selector
- fill: Fill input field (clears first, then types)
- type: Type text (character by character, for complex inputs)
- screenshot: Capture page screenshot
- snapshot: Get accessibility tree with element refs [e1], [e2], etc.
- text: Extract text content
- evaluate: Run JavaScript
- wait: Wait for element
- scroll: Scroll page (up, down, or to element)
- hover: Hover over element
- select: Select option from dropdown
- back/forward/reload: Navigation controls

Examples:
  web(action: fetch, url: "https://api.example.com/data")
  web(action: navigate, url: "https://gmail.com", profile: "chrome")
  web(action: snapshot)
  web(action: click, ref: "e5")
  web(action: fill, ref: "e3", value: "search query")
  web(action: type, ref: "e3", text: "hello")
  web(action: screenshot, output: "page.png")`
}

// Schema returns the JSON schema
func (t *WebDomainTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"description": "Web action: fetch, search, navigate, click, fill, type, screenshot, snapshot, text, evaluate, wait, scroll, hover, select, back, forward, reload",
				"enum": ["fetch", "search", "navigate", "click", "fill", "type", "screenshot", "snapshot", "text", "evaluate", "wait", "scroll", "hover", "select", "back", "forward", "reload"]
			},
			"profile": {
				"type": "string",
				"description": "Browser profile: nebo (managed, default) or chrome (extension relay with authenticated sessions)",
				"enum": ["nebo", "chrome"]
			},
			"url": {
				"type": "string",
				"description": "URL for fetch or navigate actions"
			},
			"method": {
				"type": "string",
				"description": "HTTP method for fetch (default: GET)",
				"enum": ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"]
			},
			"headers": {
				"type": "object",
				"description": "HTTP headers for fetch",
				"additionalProperties": { "type": "string" }
			},
			"body": {
				"type": "string",
				"description": "Request body for fetch (POST/PUT/PATCH)"
			},
			"query": {
				"type": "string",
				"description": "Search query (for search action)"
			},
			"engine": {
				"type": "string",
				"description": "Search engine: duckduckgo (default), google",
				"enum": ["duckduckgo", "google"]
			},
			"limit": {
				"type": "integer",
				"description": "Max search results (default: 10)"
			},
			"ref": {
				"type": "string",
				"description": "Element ref from snapshot (e.g., 'e1', 'e5') for click, fill, type, hover, select"
			},
			"selector": {
				"type": "string",
				"description": "CSS selector for browser element actions (alternative to ref)"
			},
			"value": {
				"type": "string",
				"description": "Value for fill action (clears field first then enters value)"
			},
			"text": {
				"type": "string",
				"description": "Text for type action (types character by character) or JavaScript for evaluate"
			},
			"output": {
				"type": "string",
				"description": "Output path for screenshot (returns base64 if empty)"
			},
			"timeout": {
				"type": "integer",
				"description": "Action timeout in seconds (default: 30)"
			},
			"target_id": {
				"type": "string",
				"description": "Page/tab ID for multi-tab control (use list_pages to see available)"
			}
		},
		"required": ["action"]
	}`)
}

// RequiresApproval returns true for browser actions
func (t *WebDomainTool) RequiresApproval() bool {
	return true // Browser operations can be dangerous
}

// Execute routes to the appropriate handler
func (t *WebDomainTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in WebDomainInput
	if err := json.Unmarshal(input, &in); err != nil {
		return nil, fmt.Errorf("invalid input: %w", err)
	}

	switch in.Action {
	case "fetch":
		return t.handleFetch(ctx, in)
	case "search":
		return t.handleSearch(ctx, in)
	case "navigate", "click", "fill", "type", "screenshot", "snapshot", "text", "evaluate", "wait", "scroll", "hover", "select", "back", "forward", "reload":
		return t.handleBrowser(ctx, in)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown action: %s (valid: fetch, search, navigate, click, fill, type, screenshot, snapshot, text, evaluate, wait, scroll, hover, select, back, forward, reload)", in.Action),
			IsError: true,
		}, nil
	}
}

// handleFetch performs HTTP requests
func (t *WebDomainTool) handleFetch(ctx context.Context, in WebDomainInput) (*ToolResult, error) {
	if in.URL == "" {
		return &ToolResult{Content: "Error: url is required", IsError: true}, nil
	}

	// SSRF pre-flight: validate URL before making request
	if err := validateFetchURL(in.URL); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error: %v", err), IsError: true}, nil
	}

	method := in.Method
	if method == "" {
		method = "GET"
	}

	var body io.Reader
	if in.Body != "" {
		body = strings.NewReader(in.Body)
	}

	req, err := http.NewRequestWithContext(ctx, method, in.URL, body)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error creating request: %v", err), IsError: true}, nil
	}

	req.Header.Set("User-Agent", "Nebo/1.0")

	for k, v := range in.Headers {
		req.Header.Set(k, v)
	}

	resp, err := t.client.Do(req)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error fetching URL: %v", err), IsError: true}, nil
	}
	defer resp.Body.Close()

	content, err := io.ReadAll(resp.Body)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error reading response: %v", err), IsError: true}, nil
	}

	// Truncate very long responses
	const maxContent = 100000
	result := string(content)
	if len(result) > maxContent {
		result = result[:maxContent] + "\n... (content truncated)"
	}

	header := fmt.Sprintf("HTTP %d %s\nContent-Type: %s\nContent-Length: %d\n\n",
		resp.StatusCode,
		resp.Status,
		resp.Header.Get("Content-Type"),
		len(content),
	)

	return &ToolResult{
		Content: header + result,
		IsError: resp.StatusCode >= 400,
	}, nil
}

// handleSearch performs web searches
func (t *WebDomainTool) handleSearch(ctx context.Context, in WebDomainInput) (*ToolResult, error) {
	if in.Query == "" {
		return &ToolResult{Content: "Error: query is required", IsError: true}, nil
	}

	if in.Limit <= 0 {
		in.Limit = 10
	}

	if in.Engine == "" {
		in.Engine = "duckduckgo"
	}

	var results []webSearchResult
	var err error

	switch in.Engine {
	case "google":
		if t.searchAPIKey != "" && t.searchCX != "" {
			results, err = t.searchGoogle(ctx, in.Query, in.Limit)
		} else {
			return &ToolResult{
				Content: "Google search requires API key configuration. Using DuckDuckGo instead.",
			}, nil
		}
	default:
		results, err = t.searchDuckDuckGo(ctx, in.Query, in.Limit)
	}

	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Search error: %v", err), IsError: true}, nil
	}

	if len(results) == 0 {
		return &ToolResult{Content: "No results found for: " + in.Query}, nil
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Search results for: %s\n\n", in.Query))

	for i, r := range results {
		sb.WriteString(fmt.Sprintf("%d. %s\n", i+1, r.Title))
		sb.WriteString(fmt.Sprintf("   URL: %s\n", r.URL))
		if r.Snippet != "" {
			sb.WriteString(fmt.Sprintf("   %s\n", r.Snippet))
		}
		sb.WriteString("\n")
	}

	return &ToolResult{Content: sb.String()}, nil
}

type webSearchResult struct {
	Title   string
	URL     string
	Snippet string
}

func (t *WebDomainTool) searchDuckDuckGo(ctx context.Context, query string, limit int) ([]webSearchResult, error) {
	searchURL := fmt.Sprintf("https://html.duckduckgo.com/html/?q=%s", url.QueryEscape(query))

	req, err := http.NewRequestWithContext(ctx, "GET", searchURL, nil)
	if err != nil {
		return nil, err
	}

	req.Header.Set("User-Agent", "Mozilla/5.0 (compatible; Nebo/1.0)")

	resp, err := t.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, err
	}

	return parseWebDuckDuckGoHTML(string(body), limit), nil
}

func parseWebDuckDuckGoHTML(html string, limit int) []webSearchResult {
	var results []webSearchResult

	parts := strings.Split(html, `class="result__body"`)

	for i, part := range parts[1:] {
		if i >= limit {
			break
		}

		result := webSearchResult{}

		if idx := strings.Index(part, `class="result__a"`); idx != -1 {
			hrefStart := strings.Index(part[idx:], `href="`)
			if hrefStart != -1 {
				hrefStart += idx + 6
				hrefEnd := strings.Index(part[hrefStart:], `"`)
				if hrefEnd != -1 {
					rawURL := part[hrefStart : hrefStart+hrefEnd]
					if u, err := url.Parse(rawURL); err == nil {
						if uddg := u.Query().Get("uddg"); uddg != "" {
							result.URL = uddg
						} else {
							result.URL = rawURL
						}
					}
				}
			}

			titleStart := strings.Index(part[idx:], ">")
			if titleStart != -1 {
				titleStart += idx + 1
				titleEnd := strings.Index(part[titleStart:], "</a>")
				if titleEnd != -1 {
					result.Title = strings.TrimSpace(stripWebHTMLTags(part[titleStart : titleStart+titleEnd]))
				}
			}
		}

		if idx := strings.Index(part, `class="result__snippet"`); idx != -1 {
			snippetStart := strings.Index(part[idx:], ">")
			if snippetStart != -1 {
				snippetStart += idx + 1
				snippetEnd := strings.Index(part[snippetStart:], "</a>")
				if snippetEnd == -1 {
					snippetEnd = strings.Index(part[snippetStart:], "</span>")
				}
				if snippetEnd != -1 {
					result.Snippet = strings.TrimSpace(stripWebHTMLTags(part[snippetStart : snippetStart+snippetEnd]))
				}
			}
		}

		if result.Title != "" && result.URL != "" {
			results = append(results, result)
		}
	}

	return results
}

func (t *WebDomainTool) searchGoogle(ctx context.Context, query string, limit int) ([]webSearchResult, error) {
	if limit > 10 {
		limit = 10
	}

	searchURL := fmt.Sprintf(
		"https://www.googleapis.com/customsearch/v1?key=%s&cx=%s&q=%s&num=%d",
		t.searchAPIKey, t.searchCX, url.QueryEscape(query), limit,
	)

	req, err := http.NewRequestWithContext(ctx, "GET", searchURL, nil)
	if err != nil {
		return nil, err
	}

	resp, err := t.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		return nil, fmt.Errorf("Google API error: %s - %s", resp.Status, string(body))
	}

	var data struct {
		Items []struct {
			Title   string `json:"title"`
			Link    string `json:"link"`
			Snippet string `json:"snippet"`
		} `json:"items"`
	}

	if err := json.NewDecoder(resp.Body).Decode(&data); err != nil {
		return nil, err
	}

	results := make([]webSearchResult, 0, len(data.Items))
	for _, item := range data.Items {
		results = append(results, webSearchResult{
			Title:   item.Title,
			URL:     item.Link,
			Snippet: item.Snippet,
		})
	}

	return results, nil
}

func stripWebHTMLTags(s string) string {
	var result strings.Builder
	inTag := false

	for _, r := range s {
		if r == '<' {
			inTag = true
		} else if r == '>' {
			inTag = false
		} else if !inTag {
			result.WriteRune(r)
		}
	}

	text := result.String()
	text = strings.ReplaceAll(text, "&amp;", "&")
	text = strings.ReplaceAll(text, "&lt;", "<")
	text = strings.ReplaceAll(text, "&gt;", ">")
	text = strings.ReplaceAll(text, "&quot;", "\"")
	text = strings.ReplaceAll(text, "&#x27;", "'")
	text = strings.ReplaceAll(text, "&nbsp;", " ")

	return text
}

// writeScreenshotFile writes screenshot data to a file, creating directories as needed
func writeScreenshotFile(path string, data []byte) error {
	// Expand ~ to home directory
	if strings.HasPrefix(path, "~/") {
		homeDir, err := os.UserHomeDir()
		if err != nil {
			return fmt.Errorf("failed to get home directory: %w", err)
		}
		path = homeDir + path[1:]
	}

	// Ensure parent directory exists
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return fmt.Errorf("failed to create directory: %w", err)
	}

	return os.WriteFile(path, data, 0644)
}

// handleBrowser uses the new browser package with profile support
func (t *WebDomainTool) handleBrowser(ctx context.Context, in WebDomainInput) (*ToolResult, error) {
	// Get browser manager
	mgr := browser.GetManager()

	// Default to "nebo" profile if not specified
	profile := in.Profile
	if profile == "" {
		profile = browser.DefaultProfileName
	}

	// Get session for this profile
	session, err := mgr.GetSession(ctx, profile)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to get browser session: %v", err),
			IsError: true,
		}, nil
	}

	// Get page (create new if needed)
	page, err := session.GetPage(in.TargetID)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to get page: %v", err),
			IsError: true,
		}, nil
	}

	// Set timeout if specified
	timeout := 30 * time.Second
	if in.Timeout > 0 {
		timeout = time.Duration(in.Timeout) * time.Second
	}
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	switch in.Action {
	case "navigate":
		if in.URL == "" {
			return &ToolResult{Content: "Error: url is required for navigate", IsError: true}, nil
		}
		result, err := page.Navigate(ctx, browser.NavigateOptions{URL: in.URL})
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Navigate failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Navigated to: %s\nTitle: %s", in.URL, result.Title)}, nil

	case "click":
		result, err := page.Click(ctx, browser.ClickOptions{Ref: in.Ref, Selector: in.Selector})
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Click failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: result.Message}, nil

	case "fill":
		if in.Value == "" && in.Text == "" {
			return &ToolResult{Content: "Error: value or text is required for fill", IsError: true}, nil
		}
		value := in.Value
		if value == "" {
			value = in.Text
		}
		result, err := page.Fill(ctx, browser.FillOptions{Ref: in.Ref, Selector: in.Selector, Value: value})
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Fill failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: result.Message}, nil

	case "type":
		if in.Text == "" {
			return &ToolResult{Content: "Error: text is required for type", IsError: true}, nil
		}
		result, err := page.Type(ctx, browser.TypeOptions{Ref: in.Ref, Selector: in.Selector, Text: in.Text})
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Type failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: result.Message}, nil

	case "screenshot":
		b64, err := page.Screenshot(ctx, browser.ScreenshotOptions{FullPage: true})
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Screenshot failed: %v", err), IsError: true}, nil
		}
		if in.Output != "" {
			// Decode base64 and save to file
			data, err := base64.StdEncoding.DecodeString(b64)
			if err != nil {
				return &ToolResult{Content: fmt.Sprintf("Failed to decode screenshot: %v", err), IsError: true}, nil
			}
			if err := writeScreenshotFile(in.Output, data); err != nil {
				return &ToolResult{Content: fmt.Sprintf("Failed to save screenshot: %v", err), IsError: true}, nil
			}
			return &ToolResult{Content: fmt.Sprintf("Screenshot saved to: %s (%d bytes)", in.Output, len(data))}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Screenshot captured\ndata:image/png;base64,%s", b64)}, nil

	case "snapshot":
		snapshot, err := page.Snapshot(ctx, browser.SnapshotOptions{IncludeRefs: true})
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Snapshot failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: snapshot}, nil

	case "text":
		text, err := page.GetText(ctx, in.Ref, in.Selector)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Get text failed: %v", err), IsError: true}, nil
		}
		// Truncate if too long
		if len(text) > 10000 {
			text = text[:10000] + "\n... (truncated)"
		}
		return &ToolResult{Content: text}, nil

	case "evaluate":
		if in.Text == "" {
			return &ToolResult{Content: "Error: text (JavaScript code) is required for evaluate", IsError: true}, nil
		}
		result, err := page.Evaluate(ctx, in.Text)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Evaluate failed: %v", err), IsError: true}, nil
		}
		// Convert result to string
		switch v := result.(type) {
		case string:
			return &ToolResult{Content: v}, nil
		case nil:
			return &ToolResult{Content: "undefined"}, nil
		default:
			jsonResult, err := json.MarshalIndent(result, "", "  ")
			if err != nil {
				return &ToolResult{Content: fmt.Sprintf("%v", result)}, nil
			}
			return &ToolResult{Content: string(jsonResult)}, nil
		}

	case "wait":
		if in.Selector == "" && in.Ref == "" {
			return &ToolResult{Content: "Error: selector or ref is required for wait", IsError: true}, nil
		}
		result, err := page.Wait(ctx, browser.WaitOptions{Ref: in.Ref, Selector: in.Selector})
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Wait failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: result.Message}, nil

	case "scroll":
		direction := in.Text // Use text field for direction: up, down, left, right
		if direction == "" {
			direction = "down"
		}
		result, err := page.Scroll(ctx, browser.ScrollOptions{Direction: direction, Ref: in.Ref, Selector: in.Selector})
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Scroll failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: result.Message}, nil

	case "hover":
		result, err := page.Hover(ctx, browser.HoverOptions{Ref: in.Ref, Selector: in.Selector})
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Hover failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: result.Message}, nil

	case "select":
		if in.Value == "" {
			return &ToolResult{Content: "Error: value is required for select", IsError: true}, nil
		}
		result, err := page.Select(ctx, browser.SelectOptions{Ref: in.Ref, Selector: in.Selector, Values: []string{in.Value}})
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Select failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: result.Message}, nil

	case "back":
		result, err := page.GoBack(ctx)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Back failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: result.Message}, nil

	case "forward":
		result, err := page.GoForward(ctx)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Forward failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: result.Message}, nil

	case "reload":
		result, err := page.Reload(ctx)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Reload failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: result.Message}, nil

	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown browser action: %s", in.Action), IsError: true}, nil
	}
}

// Close cleans up browser resources
func (t *WebDomainTool) Close() {
	// Browser cleanup is handled by the manager
}

// HandleVision analyzes images (placeholder - requires API key)
func (t *WebDomainTool) HandleVision(ctx context.Context, imagePath, imageBase64, prompt string) (*ToolResult, error) {
	// Encode image if path provided
	var b64 string
	if imagePath != "" {
		data, err := io.ReadAll(strings.NewReader(imagePath))
		if err != nil {
			return &ToolResult{
				Content: fmt.Sprintf("Error reading image: %v", err),
				IsError: true,
			}, nil
		}
		b64 = base64.StdEncoding.EncodeToString(data)
	} else {
		b64 = imageBase64
	}

	if b64 == "" {
		return &ToolResult{
			Content: "Error: image_path or image_base64 is required",
			IsError: true,
		}, nil
	}

	return &ToolResult{
		Content: "Vision analysis requires ANTHROPIC_API_KEY configuration",
		IsError: true,
	}, nil
}

// --- SSRF Protection ---

// ssrfBlockedNets contains CIDR ranges that must never be reached by the web fetch tool.
// This blocks private networks, link-local, loopback, and cloud metadata endpoints.
var ssrfBlockedNets = func() []*net.IPNet {
	cidrs := []string{
		"127.0.0.0/8",    // IPv4 loopback
		"10.0.0.0/8",     // RFC 1918 private
		"172.16.0.0/12",  // RFC 1918 private
		"192.168.0.0/16", // RFC 1918 private
		"169.254.0.0/16", // Link-local / AWS metadata
		"0.0.0.0/8",      // Current network
		"100.64.0.0/10",  // Shared address space (CGNAT)
		"192.0.0.0/24",   // IETF protocol assignments
		"198.18.0.0/15",  // Benchmarking
		"::1/128",        // IPv6 loopback
		"fc00::/7",       // IPv6 unique local
		"fe80::/10",      // IPv6 link-local
	}
	nets := make([]*net.IPNet, 0, len(cidrs))
	for _, cidr := range cidrs {
		_, n, err := net.ParseCIDR(cidr)
		if err == nil {
			nets = append(nets, n)
		}
	}
	return nets
}()

// isBlockedIP returns true if the IP falls within any blocked CIDR range.
func isBlockedIP(ip net.IP) bool {
	if ip == nil {
		return true // Block unresolvable addresses
	}
	for _, n := range ssrfBlockedNets {
		if n.Contains(ip) {
			return true
		}
	}
	return false
}

// validateFetchURL performs pre-flight validation on a URL before fetching.
// It blocks non-HTTP schemes, private/internal IPs, and cloud metadata endpoints.
func validateFetchURL(rawURL string) error {
	u, err := url.Parse(rawURL)
	if err != nil {
		return fmt.Errorf("invalid URL: %w", err)
	}

	// Only allow http and https
	if u.Scheme != "http" && u.Scheme != "https" {
		return fmt.Errorf("blocked: scheme %q not allowed (only http/https)", u.Scheme)
	}

	hostname := u.Hostname()
	if hostname == "" {
		return fmt.Errorf("blocked: empty hostname")
	}

	// Block cloud metadata hostnames
	metadataHosts := []string{
		"metadata.google.internal",
		"metadata.google.com",
	}
	lowerHost := strings.ToLower(hostname)
	for _, mh := range metadataHosts {
		if lowerHost == mh {
			return fmt.Errorf("blocked: cloud metadata endpoint %q", hostname)
		}
	}

	// Resolve hostname and check all IPs
	ips, err := net.LookupIP(hostname)
	if err != nil {
		return fmt.Errorf("DNS resolution failed for %q: %w", hostname, err)
	}

	for _, ip := range ips {
		if isBlockedIP(ip) {
			return fmt.Errorf("blocked: %q resolves to private/internal IP %s", hostname, ip)
		}
	}

	return nil
}

// ssrfSafeTransport returns an http.Transport with a custom dialer that
// re-validates resolved IPs at connection time. This catches DNS rebinding
// attacks where a hostname resolves to a public IP during pre-flight but
// to a private IP when the actual connection is made.
func ssrfSafeTransport() *http.Transport {
	return &http.Transport{
		DialContext: func(ctx context.Context, network, addr string) (net.Conn, error) {
			host, port, err := net.SplitHostPort(addr)
			if err != nil {
				return nil, fmt.Errorf("invalid address %q: %w", addr, err)
			}

			ips, err := net.DefaultResolver.LookupIPAddr(ctx, host)
			if err != nil {
				return nil, fmt.Errorf("DNS resolution failed: %w", err)
			}

			for _, ipAddr := range ips {
				if isBlockedIP(ipAddr.IP) {
					return nil, fmt.Errorf("SSRF blocked: %q resolved to private IP %s at connect time", host, ipAddr.IP)
				}
			}

			// Connect to the first allowed IP
			dialer := &net.Dialer{Timeout: 10 * time.Second}
			for _, ipAddr := range ips {
				target := net.JoinHostPort(ipAddr.IP.String(), port)
				conn, err := dialer.DialContext(ctx, network, target)
				if err == nil {
					return conn, nil
				}
			}
			return nil, fmt.Errorf("failed to connect to any resolved IP for %q", host)
		},
	}
}

// ssrfSafeRedirectCheck returns a CheckRedirect function that validates
// each redirect target against the SSRF blocklist.
func ssrfSafeRedirectCheck() func(req *http.Request, via []*http.Request) error {
	return func(req *http.Request, via []*http.Request) error {
		if len(via) >= 10 {
			return fmt.Errorf("too many redirects")
		}
		if err := validateFetchURL(req.URL.String()); err != nil {
			return fmt.Errorf("redirect blocked: %w", err)
		}
		return nil
	}
}
