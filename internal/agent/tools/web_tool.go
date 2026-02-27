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

	"github.com/neboloop/nebo/internal/browser"
	"github.com/neboloop/nebo/internal/webview"
)

// WebDomainTool provides web operations: HTTP requests, search, and browser automation.
type WebDomainTool struct {
	client       *http.Client
	searchAPIKey string
	searchCX     string
	headless     bool
}

// WebDomainInput represents the consolidated input for all web operations.
type WebDomainInput struct {
	// STRAP fields
	Resource string `json:"resource"` // http, search, browser
	Action   string `json:"action"`

	// HTTP fields
	URL     string            `json:"url,omitempty"`
	Method  string            `json:"method,omitempty"` // GET, POST, etc.
	Headers map[string]string `json:"headers,omitempty"`
	Body    string            `json:"body,omitempty"`

	// Search fields
	Query  string `json:"query,omitempty"`
	Engine string `json:"engine,omitempty"` // duckduckgo, google
	Limit  int    `json:"limit,omitempty"`

	// Pagination fields (for fetch chunking)
	Offset int `json:"offset,omitempty"` // Chunk offset (0-based) for paginating large responses

	// Browser fields
	Profile  string `json:"profile,omitempty"`  // "nebo" (managed) or "chrome" (extension relay)
	Selector string `json:"selector,omitempty"` // CSS selector
	Text     string `json:"text,omitempty"`     // Text to type or JS to evaluate
	Value    string `json:"value,omitempty"`    // Value for fill action
	Output   string `json:"output,omitempty"`   // Output path for screenshot
	Timeout  int    `json:"timeout,omitempty"`  // Action timeout in seconds
	Ref      string `json:"ref,omitempty"`      // Element ref from snapshot (e.g., "e1", "e5")
	TargetID string `json:"target_id,omitempty"` // Page/tab ID for multi-tab control

	// Devtools fields
	Level       string `json:"level,omitempty"`        // Console message level filter (log, warn, error, info, debug)
	Clear       bool   `json:"clear,omitempty"`        // Clear data after reading
	Key         string `json:"key,omitempty"`          // Storage key for targeted lookup
	StorageType string `json:"storage_type,omitempty"` // "local", "session", or "cookies"
}

// WebDomainConfig configures the web domain tool.
type WebDomainConfig struct {
	SearchAPIKey string // For Google Custom Search (optional)
	SearchCX     string // Google Custom Search Engine ID (optional)
	Headless     bool   // Browser headless mode
}

// NewWebDomainTool creates a new web domain tool with SSRF-safe HTTP client.
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

// NewWebDomainToolWithConfig creates a web tool with full configuration.
func NewWebDomainToolWithConfig(cfg WebDomainConfig) *WebDomainTool {
	t := NewWebDomainTool()
	t.searchAPIKey = cfg.SearchAPIKey
	t.searchCX = cfg.SearchCX
	t.headless = cfg.Headless
	return t
}

func (t *WebDomainTool) Name() string   { return "web" }
func (t *WebDomainTool) Domain() string { return "web" }

func (t *WebDomainTool) Resources() []string {
	return []string{"http", "search", "browser", "devtools"}
}

func (t *WebDomainTool) ActionsFor(resource string) []string {
	switch resource {
	case "http":
		return []string{"fetch"}
	case "search":
		return []string{"query"}
	case "browser":
		return []string{
			"navigate", "snapshot", "click", "fill", "type",
			"screenshot", "text", "evaluate", "wait", "scroll",
			"hover", "select", "back", "forward", "reload",
			"status", "launch", "close", "list_pages",
		}
	case "devtools":
		return []string{"console", "source", "storage", "dom", "cookies", "performance"}
	default:
		return nil
	}
}

func (t *WebDomainTool) Description() string {
	return BuildDomainDescription(t.schemaConfig())
}

func (t *WebDomainTool) Schema() json.RawMessage {
	return BuildDomainSchema(t.schemaConfig())
}

func (t *WebDomainTool) schemaConfig() DomainSchemaConfig {
	return DomainSchemaConfig{
		Domain:      "web",
		Description: "Web operations: HTTP requests, web search, and full browser automation with profile support.",
		Resources: map[string]ResourceConfig{
			"http": {
				Name:        "http",
				Actions:     []string{"fetch"},
				Description: "HTTP requests (GET, POST, PUT, DELETE, etc.) — no JavaScript rendering",
			},
			"search": {
				Name:        "search",
				Actions:     []string{"query"},
				Description: "Web search via DuckDuckGo or Google",
			},
			"browser": {
				Name: "browser",
				Actions: []string{
					"navigate", "snapshot", "click", "fill", "type",
					"screenshot", "text", "evaluate", "wait", "scroll",
					"hover", "select", "back", "forward", "reload",
					"status", "launch", "close", "list_pages",
				},
				Description: "Full browser automation with lifecycle control",
			},
			"devtools": {
				Name:        "devtools",
				Actions:     []string{"console", "source", "storage", "dom", "cookies", "performance"},
				Description: "Browser devtools introspection: console output, page source, storage, DOM inspection, cookies, and performance metrics",
			},
		},
		Fields: []FieldConfig{
			{Name: "url", Type: "string", Description: "URL for fetch or navigate"},
			{Name: "method", Type: "string", Description: "HTTP method for fetch (default: GET)", Enum: []string{"GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"}},
			{Name: "headers", Type: "object", Description: "HTTP headers for fetch"},
			{Name: "body", Type: "string", Description: "Request body for fetch (POST/PUT/PATCH)"},
			{Name: "query", Type: "string", Description: "Search query (for search/query)"},
			{Name: "engine", Type: "string", Description: "Search engine: duckduckgo (default), google", Enum: []string{"duckduckgo", "google"}},
			{Name: "limit", Type: "integer", Description: "Max search results (default: 10)"},
			{Name: "profile", Type: "string", Description: "Browser profile: native (Nebo's own window — fast, undetectable), nebo (managed Playwright), or chrome (extension relay with authenticated sessions)", Enum: []string{"native", "nebo", "chrome"}},
			{Name: "ref", Type: "string", Description: "Element ref from snapshot (e.g., 'e1', 'e5') for click, fill, type, hover, select"},
			{Name: "selector", Type: "string", Description: "CSS selector for browser element actions (alternative to ref)"},
			{Name: "value", Type: "string", Description: "Value for fill action (clears field first then enters value)"},
			{Name: "text", Type: "string", Description: "Text for type action (types character by character) or JavaScript for evaluate"},
			{Name: "output", Type: "string", Description: "Output path for screenshot (returns base64 if empty)"},
			{Name: "timeout", Type: "integer", Description: "Action timeout in seconds (default: 30)"},
			{Name: "target_id", Type: "string", Description: "Page/tab ID for multi-tab control (use list_pages to see available)"},
			{Name: "offset", Type: "integer", Description: "Chunk offset (0-based) for paginating large fetch/text responses"},
			{Name: "level", Type: "string", Description: "Console message level filter (for devtools/console)", Enum: []string{"log", "warn", "error", "info", "debug"}},
			{Name: "clear", Type: "boolean", Description: "Clear data after reading (for devtools/console, devtools/storage, devtools/cookies)"},
			{Name: "key", Type: "string", Description: "Storage key for targeted lookup (for devtools/storage)"},
			{Name: "storage_type", Type: "string", Description: "Storage type (for devtools/storage)", Enum: []string{"local", "session", "cookies"}},
		},
		Examples: []string{
			`web(resource: http, action: fetch, url: "https://api.example.com/data")`,
			`web(resource: search, action: query, query: "golang tutorials")`,
			`web(resource: browser, action: navigate, url: "https://example.com", profile: "native")`,
			`web(resource: browser, action: snapshot, profile: "native")`,
			`web(resource: browser, action: status)`,
			`web(resource: browser, action: launch, profile: "nebo")`,
			`web(resource: browser, action: navigate, url: "https://gmail.com", profile: "chrome")`,
			`web(resource: browser, action: click, ref: "e5")`,
			`web(resource: browser, action: fill, ref: "e3", value: "search query")`,
			`web(resource: browser, action: list_pages, profile: "native")`,
			`web(resource: browser, action: close, target_id: "win-12345", profile: "native")`,
			`web(resource: devtools, action: console)`,
			`web(resource: devtools, action: console, level: "error", clear: true)`,
			`web(resource: devtools, action: source)`,
			`web(resource: devtools, action: storage, storage_type: "local")`,
			`web(resource: devtools, action: cookies)`,
			`web(resource: devtools, action: performance)`,
		},
	}
}

func (t *WebDomainTool) RequiresApproval() bool {
	return true
}

// Execute routes to the appropriate handler based on resource.
func (t *WebDomainTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in WebDomainInput
	if err := json.Unmarshal(input, &in); err != nil {
		return nil, fmt.Errorf("invalid input: %w", err)
	}

	// Normalize: infer resource from action when resource is omitted.
	// The system prompt teaches shorthand like web(action: "search", query: "...")
	// which omits the resource field. Map action→resource automatically.
	if in.Resource == "" {
		switch in.Action {
		case "search", "query":
			in.Resource = "search"
			in.Action = "query"
		case "fetch":
			in.Resource = "http"
		case "navigate", "snapshot", "click", "fill", "type", "screenshot",
			"text", "evaluate", "wait", "scroll", "back", "forward", "reload",
			"hover", "select", "list_pages", "close", "launch", "status":
			in.Resource = "browser"
		case "console", "source", "storage", "dom", "cookies", "performance":
			in.Resource = "devtools"
		}
	}

	switch in.Resource {
	case "http":
		return t.executeHTTP(ctx, in)
	case "search":
		return t.executeSearch(ctx, in)
	case "browser":
		return t.executeBrowser(ctx, in)
	case "devtools":
		return t.executeDevtools(ctx, in)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown resource: %q (valid: http, search, browser, devtools)", in.Resource),
			IsError: true,
		}, nil
	}
}

// --- HTTP Resource ---

func (t *WebDomainTool) executeHTTP(ctx context.Context, in WebDomainInput) (*ToolResult, error) {
	switch in.Action {
	case "fetch":
		return t.handleFetch(ctx, in)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown action %q for resource 'http' (valid: fetch)", in.Action),
			IsError: true,
		}, nil
	}
}

func (t *WebDomainTool) handleFetch(ctx context.Context, in WebDomainInput) (*ToolResult, error) {
	if in.URL == "" {
		return &ToolResult{Content: "Error: url is required", IsError: true}, nil
	}

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

	contentType := resp.Header.Get("Content-Type")

	// Extract visible text from HTML, pass through other content types unchanged.
	text := ExtractVisibleText(content, contentType)

	// Format with chunking — no truncation, full content accessible via offset.
	result := FormatFetchResult(resp.StatusCode, resp.Status, contentType, len(content), text, defaultChunkSize, in.Offset)

	return &ToolResult{
		Content: result,
		IsError: resp.StatusCode >= 400,
	}, nil
}

// --- Search Resource ---

func (t *WebDomainTool) executeSearch(ctx context.Context, in WebDomainInput) (*ToolResult, error) {
	switch in.Action {
	case "query":
		return t.handleSearch(ctx, in)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown action %q for resource 'search' (valid: query)", in.Action),
			IsError: true,
		}, nil
	}
}

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
		// Try native browser first (renders JavaScript, always up-to-date)
		results, err = t.searchViaNativeBrowser(ctx, in.Query, in.Limit)
		if err != nil {
			// Fall back to HTTP scraper
			results, err = t.searchDuckDuckGo(ctx, in.Query, in.Limit)
		}
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

// --- Browser Resource ---

func (t *WebDomainTool) executeBrowser(ctx context.Context, in WebDomainInput) (*ToolResult, error) {
	// Native profile — agent-controlled Wails webview windows
	if in.Profile == "native" {
		return t.handleNativeBrowser(ctx, in)
	}

	// Lifecycle actions don't need a session
	switch in.Action {
	case "status":
		return t.handleBrowserStatus(ctx, in)
	case "launch":
		return t.handleBrowserLaunch(ctx, in)
	case "close":
		return t.handleBrowserClose(ctx, in)
	case "list_pages":
		return t.handleBrowserListPages(ctx, in)
	}

	// All other browser actions need a session + page
	return t.handleBrowserAction(ctx, in)
}

func (t *WebDomainTool) handleBrowserStatus(_ context.Context, in WebDomainInput) (*ToolResult, error) {
	mgr := browser.GetManager()

	if in.Profile != "" {
		status, err := mgr.GetProfileStatus(in.Profile)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Error: %v", err), IsError: true}, nil
		}
		data, _ := json.MarshalIndent(status, "", "  ")
		return &ToolResult{Content: string(data)}, nil
	}

	// Return all profiles
	statuses := mgr.GetAllProfileStatuses()
	if len(statuses) == 0 {
		return &ToolResult{Content: "No browser profiles configured"}, nil
	}

	data, _ := json.MarshalIndent(map[string]any{"profiles": statuses}, "", "  ")
	return &ToolResult{Content: string(data)}, nil
}

func (t *WebDomainTool) handleBrowserLaunch(ctx context.Context, in WebDomainInput) (*ToolResult, error) {
	mgr := browser.GetManager()

	profile := in.Profile
	if profile == "" {
		profile = browser.DefaultProfileName
	}

	// Chrome extension profile can't be launched by us
	p := mgr.GetProfile(profile)
	if p == nil {
		return &ToolResult{
			Content: fmt.Sprintf("Unknown profile: %q", profile),
			IsError: true,
		}, nil
	}

	if p.Driver == browser.DriverExtension {
		return &ToolResult{
			Content: fmt.Sprintf("Cannot launch the %q profile — it connects via the Chrome extension. Ensure the Nebo extension is active in Chrome.", profile),
			IsError: true,
		}, nil
	}

	// GetSession triggers ensureBrowserRunning for managed profiles
	_, err := mgr.GetSession(ctx, profile)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to launch browser for profile %q: %v", profile, err),
			IsError: true,
		}, nil
	}

	status, _ := mgr.GetProfileStatus(profile)
	data, _ := json.MarshalIndent(status, "", "  ")
	return &ToolResult{Content: fmt.Sprintf("Browser launched for profile %q\n%s", profile, string(data))}, nil
}

func (t *WebDomainTool) handleBrowserClose(_ context.Context, in WebDomainInput) (*ToolResult, error) {
	mgr := browser.GetManager()

	profile := in.Profile
	if profile == "" {
		profile = browser.DefaultProfileName
	}

	p := mgr.GetProfile(profile)
	if p == nil {
		return &ToolResult{
			Content: fmt.Sprintf("Unknown profile: %q", profile),
			IsError: true,
		}, nil
	}

	if p.Driver == browser.DriverExtension {
		// For extension profiles, close the Playwright session (disconnect) but don't close Chrome
		_ = browser.CloseSession(profile)
		return &ToolResult{Content: fmt.Sprintf("Disconnected from Chrome extension profile %q (Chrome itself remains open)", profile)}, nil
	}

	// For managed profiles, stop the browser
	if err := mgr.StopBrowser(profile); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to stop browser for profile %q: %v", profile, err),
			IsError: true,
		}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Browser stopped for profile %q", profile)}, nil
}

func (t *WebDomainTool) handleBrowserListPages(_ context.Context, in WebDomainInput) (*ToolResult, error) {
	profile := in.Profile
	if profile == "" {
		profile = browser.DefaultProfileName
	}

	// Check if session exists without creating one
	session := browser.GetSessionIfExists(profile)
	if session == nil {
		return &ToolResult{Content: fmt.Sprintf("No active session for profile %q. Use launch or navigate first.", profile)}, nil
	}

	pages := session.ListPages()
	if len(pages) == 0 {
		return &ToolResult{Content: fmt.Sprintf("No pages open in profile %q", profile)}, nil
	}

	type pageInfo struct {
		TargetID string `json:"target_id"`
		URL      string `json:"url"`
		Title    string `json:"title"`
	}

	infos := make([]pageInfo, 0, len(pages))
	for _, p := range pages {
		_ = p.UpdateState()
		s := p.State()
		infos = append(infos, pageInfo{
			TargetID: p.TargetID(),
			URL:      s.URL,
			Title:    s.Title,
		})
	}

	data, _ := json.MarshalIndent(infos, "", "  ")
	return &ToolResult{Content: string(data)}, nil
}

func (t *WebDomainTool) handleBrowserAction(ctx context.Context, in WebDomainInput) (*ToolResult, error) {
	mgr := browser.GetManager()

	profile := in.Profile
	if profile == "" {
		profile = browser.DefaultProfileName
	}

	// Get session (creates browser if needed for managed profiles)
	session, err := mgr.GetSession(ctx, profile)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Browser not available for profile %q: %v\n\nUse web(resource: browser, action: status) to check availability, or web(resource: browser, action: launch) to start the managed browser.", profile, err),
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

	// Set timeout
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
		chunk, totalChunks := ChunkText(text, defaultChunkSize, in.Offset)
		if totalChunks > 1 {
			chunk = fmt.Sprintf("Chunk: %d/%d (use offset parameter to read more)\n\n%s", in.Offset+1, totalChunks, chunk)
		}
		return &ToolResult{Content: chunk}, nil

	case "evaluate":
		if in.Text == "" {
			return &ToolResult{Content: "Error: text (JavaScript code) is required for evaluate", IsError: true}, nil
		}
		result, err := page.Evaluate(ctx, in.Text)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Evaluate failed: %v", err), IsError: true}, nil
		}
		switch v := result.(type) {
		case string:
			return &ToolResult{Content: v}, nil
		case nil:
			return &ToolResult{Content: "undefined"}, nil
		default:
			jsonResult, err := json.MarshalIndent(result, "", "  ")
			if err != nil {
				return &ToolResult{Content: fmt.Sprintf("(non-serializable %T)", result)}, nil
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
		direction := in.Text
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
		return &ToolResult{
			Content: fmt.Sprintf("Unknown action %q for resource 'browser' (valid: navigate, snapshot, click, fill, type, screenshot, text, evaluate, wait, scroll, hover, select, back, forward, reload, status, launch, close, list_pages)", in.Action),
			IsError: true,
		}, nil
	}
}

// handleNativeBrowser routes actions to the native webview manager.
func (t *WebDomainTool) handleNativeBrowser(ctx context.Context, in WebDomainInput) (*ToolResult, error) {
	mgr := webview.GetManager()

	if !mgr.IsAvailable() {
		return &ToolResult{
			Content: "Native browser requires desktop mode. Use profile \"nebo\" (managed Playwright) or \"chrome\" (extension) in headless mode.",
			IsError: true,
		}, nil
	}

	timeout := 15 * time.Second
	if in.Timeout > 0 {
		timeout = time.Duration(in.Timeout) * time.Second
	}

	switch in.Action {
	case "status":
		count := mgr.WindowCount()
		windows := mgr.ListWindows()
		type winInfo struct {
			ID    string `json:"id"`
			URL   string `json:"url"`
			Title string `json:"title"`
		}
		infos := make([]winInfo, 0, len(windows))
		for _, w := range windows {
			infos = append(infos, winInfo{ID: w.ID, URL: w.URL, Title: w.Title})
		}
		data, _ := json.MarshalIndent(map[string]any{
			"profile":      "native",
			"available":    true,
			"window_count": count,
			"windows":      infos,
		}, "", "  ")
		return &ToolResult{Content: string(data)}, nil

	case "launch", "navigate":
		if in.URL == "" && in.Action == "navigate" {
			return &ToolResult{Content: "Error: url is required for navigate", IsError: true}, nil
		}
		// If target_id specified, navigate existing window
		if in.TargetID != "" {
			result, err := webview.Navigate(ctx, mgr, in.TargetID, in.URL, timeout)
			if err != nil {
				return &ToolResult{Content: fmt.Sprintf("Navigate failed: %v", err), IsError: true}, nil
			}
			return &ToolResult{Content: fmt.Sprintf("Navigated window %s to %s\n%s", in.TargetID, in.URL, string(result))}, nil
		}
		// Create new window
		url := in.URL
		if url == "" {
			url = "about:blank"
		}
		win, err := mgr.CreateWindow(url, "Nebo Browser", GetSessionKey(ctx))
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to create window: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Opened native browser window\nWindow ID: %s\nURL: %s\n\nUse target_id: %q for subsequent actions on this window.", win.ID, url, win.ID)}, nil

	case "close":
		if in.TargetID != "" {
			if err := mgr.CloseWindow(in.TargetID); err != nil {
				return &ToolResult{Content: fmt.Sprintf("Close failed: %v", err), IsError: true}, nil
			}
			return &ToolResult{Content: fmt.Sprintf("Closed window %s", in.TargetID)}, nil
		}
		// Close most recent
		win, err := mgr.GetWindow("")
		if err != nil {
			return &ToolResult{Content: "No windows to close"}, nil
		}
		id := win.ID
		_ = mgr.CloseWindow(id)
		return &ToolResult{Content: fmt.Sprintf("Closed window %s", id)}, nil

	case "list_pages":
		windows := mgr.ListWindows()
		if len(windows) == 0 {
			return &ToolResult{Content: "No native browser windows open"}, nil
		}
		type winInfo struct {
			TargetID string `json:"target_id"`
			URL      string `json:"url"`
			Title    string `json:"title"`
		}
		infos := make([]winInfo, 0, len(windows))
		for _, w := range windows {
			infos = append(infos, winInfo{TargetID: w.ID, URL: w.URL, Title: w.Title})
		}
		data, _ := json.MarshalIndent(infos, "", "  ")
		return &ToolResult{Content: string(data)}, nil

	case "snapshot":
		result, err := webview.Snapshot(ctx, mgr, in.TargetID, timeout)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Snapshot failed: %v", err), IsError: true}, nil
		}
		// Snapshot returns a string (the DOM tree text)
		var text string
		if err := json.Unmarshal(result, &text); err != nil {
			text = string(result)
		}
		return &ToolResult{Content: text}, nil

	case "click":
		result, err := webview.Click(ctx, mgr, in.TargetID, in.Ref, in.Selector, timeout)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Click failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: formatJSONResult(result, "Clicked")}, nil

	case "fill":
		value := in.Value
		if value == "" {
			value = in.Text
		}
		if value == "" {
			return &ToolResult{Content: "Error: value is required for fill", IsError: true}, nil
		}
		result, err := webview.Fill(ctx, mgr, in.TargetID, in.Ref, in.Selector, value, timeout)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Fill failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: formatJSONResult(result, "Filled")}, nil

	case "type":
		if in.Text == "" {
			return &ToolResult{Content: "Error: text is required for type", IsError: true}, nil
		}
		result, err := webview.Type(ctx, mgr, in.TargetID, in.Ref, in.Selector, in.Text, timeout)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Type failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: formatJSONResult(result, "Typed")}, nil

	case "text":
		result, err := webview.GetText(ctx, mgr, in.TargetID, in.Selector, timeout)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Get text failed: %v", err), IsError: true}, nil
		}
		var text string
		if err := json.Unmarshal(result, &text); err != nil {
			text = string(result)
		}
		chunk, totalChunks := ChunkText(text, defaultChunkSize, in.Offset)
		if totalChunks > 1 {
			chunk = fmt.Sprintf("Chunk: %d/%d (use offset parameter to read more)\n\n%s", in.Offset+1, totalChunks, chunk)
		}
		return &ToolResult{Content: chunk}, nil

	case "evaluate":
		if in.Text == "" {
			return &ToolResult{Content: "Error: text (JavaScript code) is required for evaluate", IsError: true}, nil
		}
		result, err := webview.Evaluate(ctx, mgr, in.TargetID, in.Text, timeout)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Evaluate failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: string(result)}, nil

	case "scroll":
		direction := in.Text
		if direction == "" {
			direction = "down"
		}
		result, err := webview.Scroll(ctx, mgr, in.TargetID, direction, timeout)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Scroll failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: formatJSONResult(result, "Scrolled")}, nil

	case "wait":
		if in.Selector == "" && in.Ref == "" {
			return &ToolResult{Content: "Error: selector is required for wait", IsError: true}, nil
		}
		sel := in.Selector
		if sel == "" && in.Ref != "" {
			sel = fmt.Sprintf("[data-nebo-ref=%q]", in.Ref)
		}
		result, err := webview.Wait(ctx, mgr, in.TargetID, sel, timeout)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Wait failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: formatJSONResult(result, "Wait complete")}, nil

	case "hover":
		result, err := webview.Hover(ctx, mgr, in.TargetID, in.Ref, in.Selector, timeout)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Hover failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: formatJSONResult(result, "Hovered")}, nil

	case "select":
		if in.Value == "" {
			return &ToolResult{Content: "Error: value is required for select", IsError: true}, nil
		}
		result, err := webview.Select(ctx, mgr, in.TargetID, in.Ref, in.Selector, in.Value, timeout)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Select failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: formatJSONResult(result, "Selected")}, nil

	case "back":
		result, err := webview.Back(ctx, mgr, in.TargetID, timeout)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Back failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: formatJSONResult(result, "Navigated back")}, nil

	case "forward":
		result, err := webview.Forward(ctx, mgr, in.TargetID, timeout)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Forward failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: formatJSONResult(result, "Navigated forward")}, nil

	case "reload":
		if err := webview.Reload(ctx, mgr, in.TargetID); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Reload failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: "Page reloaded"}, nil

	case "screenshot":
		base64Data, err := webview.Screenshot(ctx, mgr, in.TargetID)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Screenshot failed: %v", err), IsError: true}, nil
		}

		// Save to file if output path provided, otherwise return base64
		if in.Output != "" {
			// Decode base64 and save
			data := strings.TrimPrefix(base64Data, "data:image/png;base64,")
			decoded, err := base64.StdEncoding.DecodeString(data)
			if err != nil {
				return &ToolResult{Content: fmt.Sprintf("Failed to decode screenshot: %v", err), IsError: true}, nil
			}
			if err := os.WriteFile(in.Output, decoded, 0644); err != nil {
				return &ToolResult{Content: fmt.Sprintf("Failed to save screenshot: %v", err), IsError: true}, nil
			}
			return &ToolResult{Content: fmt.Sprintf("Screenshot saved to %s", in.Output)}, nil
		}

		// Return base64 data
		return &ToolResult{Content: base64Data}, nil

	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown action %q for native browser (valid: navigate, snapshot, click, fill, type, text, evaluate, scroll, wait, hover, select, back, forward, reload, status, launch, close, list_pages)", in.Action),
			IsError: true,
		}, nil
	}
}

// --- Devtools Resource ---

func (t *WebDomainTool) executeDevtools(ctx context.Context, in WebDomainInput) (*ToolResult, error) {
	// Native webview doesn't support CDP-based devtools introspection
	if in.Profile == "native" {
		return &ToolResult{
			Content: "Devtools requires a managed browser profile (\"nebo\" or \"chrome\"). The native webview does not support CDP-based introspection.\n\nUse: web(resource: \"browser\", action: \"launch\") to start a managed browser, then retry.",
			IsError: true,
		}, nil
	}

	mgr := browser.GetManager()

	profile := in.Profile
	if profile == "" {
		profile = browser.DefaultProfileName
	}

	// Get session (creates browser if needed for managed profiles)
	session, err := mgr.GetSession(ctx, profile)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Browser not available for profile %q: %v\n\nUse web(resource: browser, action: launch) to start a browser first.", profile, err),
			IsError: true,
		}, nil
	}

	// Get page
	page, err := session.GetPage(in.TargetID)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to get page: %v", err),
			IsError: true,
		}, nil
	}

	// Set timeout
	timeout := 30 * time.Second
	if in.Timeout > 0 {
		timeout = time.Duration(in.Timeout) * time.Second
	}
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	switch in.Action {
	case "console":
		return t.handleDevtoolsConsole(ctx, in, page)
	case "source":
		return t.handleDevtoolsSource(ctx, in, page)
	case "storage":
		return t.handleDevtoolsStorage(ctx, in, page)
	case "dom":
		return t.handleDevtoolsDom(ctx, in, page)
	case "cookies":
		return t.handleDevtoolsCookies(ctx, in, page)
	case "performance":
		return t.handleDevtoolsPerformance(ctx, in, page)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown action %q for resource 'devtools' (valid: console, source, storage, dom, cookies, performance)", in.Action),
			IsError: true,
		}, nil
	}
}

func (t *WebDomainTool) handleDevtoolsConsole(ctx context.Context, in WebDomainInput, page *browser.Page) (*ToolResult, error) {
	result, err := page.GetConsoleMessages(ctx, in.Level, in.Clear)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Get console messages failed: %v", err), IsError: true}, nil
	}

	if len(result.Messages) == 0 && len(result.Errors) == 0 {
		return &ToolResult{Content: "No console messages or errors captured"}, nil
	}

	data, _ := json.MarshalIndent(result, "", "  ")
	content := string(data)

	chunk, totalChunks := ChunkText(content, defaultChunkSize, in.Offset)
	if totalChunks > 1 {
		chunk = fmt.Sprintf("Chunk: %d/%d (use offset parameter to read more)\n\n%s", in.Offset+1, totalChunks, chunk)
	}
	return &ToolResult{Content: chunk}, nil
}

func (t *WebDomainTool) handleDevtoolsSource(ctx context.Context, in WebDomainInput, page *browser.Page) (*ToolResult, error) {
	source, err := page.GetSource(ctx)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Get source failed: %v", err), IsError: true}, nil
	}

	if source == "" {
		return &ToolResult{Content: "Page has no source content"}, nil
	}

	chunk, totalChunks := ChunkText(source, defaultChunkSize, in.Offset)
	if totalChunks > 1 {
		chunk = fmt.Sprintf("Chunk: %d/%d (use offset parameter to read more)\n\n%s", in.Offset+1, totalChunks, chunk)
	}
	return &ToolResult{Content: chunk}, nil
}

func (t *WebDomainTool) handleDevtoolsStorage(ctx context.Context, in WebDomainInput, page *browser.Page) (*ToolResult, error) {
	storageType := in.StorageType
	if storageType == "" {
		storageType = "local"
	}

	kind := browser.StorageLocal
	if storageType == "session" {
		kind = browser.StorageSession
	}

	values, err := page.GetStorage(ctx, kind, in.Key)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Get storage failed: %v", err), IsError: true}, nil
	}

	if in.Clear {
		if clearErr := page.ClearStorage(ctx, kind); clearErr != nil {
			return &ToolResult{Content: fmt.Sprintf("Get storage succeeded but clear failed: %v", clearErr), IsError: true}, nil
		}
	}

	if len(values) == 0 {
		msg := fmt.Sprintf("No %sStorage entries", storageType)
		if in.Key != "" {
			msg = fmt.Sprintf("Key %q not found in %sStorage", in.Key, storageType)
		}
		return &ToolResult{Content: msg}, nil
	}

	data, _ := json.MarshalIndent(map[string]any{
		"storage_type": storageType,
		"entries":      values,
		"count":        len(values),
	}, "", "  ")

	content := string(data)
	chunk, totalChunks := ChunkText(content, defaultChunkSize, in.Offset)
	if totalChunks > 1 {
		chunk = fmt.Sprintf("Chunk: %d/%d (use offset parameter to read more)\n\n%s", in.Offset+1, totalChunks, chunk)
	}
	return &ToolResult{Content: chunk}, nil
}

func (t *WebDomainTool) handleDevtoolsDom(ctx context.Context, in WebDomainInput, page *browser.Page) (*ToolResult, error) {
	if in.Selector == "" && in.Text == "" {
		return &ToolResult{Content: "Error: selector (CSS selector to inspect) or text (JavaScript to evaluate) is required for dom action", IsError: true}, nil
	}

	// If text is provided, use it as raw JavaScript evaluation
	if in.Text != "" {
		result, err := page.Evaluate(ctx, in.Text)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("DOM evaluate failed: %v", err), IsError: true}, nil
		}
		switch v := result.(type) {
		case string:
			chunk, totalChunks := ChunkText(v, defaultChunkSize, in.Offset)
			if totalChunks > 1 {
				chunk = fmt.Sprintf("Chunk: %d/%d (use offset parameter to read more)\n\n%s", in.Offset+1, totalChunks, chunk)
			}
			return &ToolResult{Content: chunk}, nil
		case nil:
			return &ToolResult{Content: "undefined"}, nil
		default:
			jsonResult, err := json.MarshalIndent(result, "", "  ")
			if err != nil {
				return &ToolResult{Content: fmt.Sprintf("(non-serializable %T)", result)}, nil
			}
			content := string(jsonResult)
			chunk, totalChunks := ChunkText(content, defaultChunkSize, in.Offset)
			if totalChunks > 1 {
				chunk = fmt.Sprintf("Chunk: %d/%d (use offset parameter to read more)\n\n%s", in.Offset+1, totalChunks, chunk)
			}
			return &ToolResult{Content: chunk}, nil
		}
	}

	// Use selector to extract structured element info
	script := fmt.Sprintf(`(() => {
		const els = document.querySelectorAll(%q);
		return Array.from(els).slice(0, 50).map((el, i) => ({
			tag: el.tagName.toLowerCase(),
			id: el.id || undefined,
			classes: el.className ? el.className.split(' ').filter(Boolean) : undefined,
			text: el.textContent ? el.textContent.trim().substring(0, 200) : undefined,
			attributes: Object.fromEntries(
				Array.from(el.attributes)
					.filter(a => !['class','id','style'].includes(a.name))
					.map(a => [a.name, a.value])
			),
			childCount: el.children.length,
			visible: el.offsetParent !== null || el.tagName === 'BODY',
		}));
	})()`, in.Selector)

	result, err := page.Evaluate(ctx, script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("DOM query failed: %v", err), IsError: true}, nil
	}

	data, _ := json.MarshalIndent(map[string]any{
		"selector": in.Selector,
		"elements": result,
	}, "", "  ")

	content := string(data)
	chunk, totalChunks := ChunkText(content, defaultChunkSize, in.Offset)
	if totalChunks > 1 {
		chunk = fmt.Sprintf("Chunk: %d/%d (use offset parameter to read more)\n\n%s", in.Offset+1, totalChunks, chunk)
	}
	return &ToolResult{Content: chunk}, nil
}

func (t *WebDomainTool) handleDevtoolsCookies(ctx context.Context, in WebDomainInput, page *browser.Page) (*ToolResult, error) {
	cookies, err := page.GetCookies(ctx)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Get cookies failed: %v", err), IsError: true}, nil
	}

	if in.Clear {
		if clearErr := page.ClearCookies(ctx); clearErr != nil {
			return &ToolResult{Content: fmt.Sprintf("Get cookies succeeded but clear failed: %v", clearErr), IsError: true}, nil
		}
	}

	if len(cookies) == 0 {
		return &ToolResult{Content: "No cookies found"}, nil
	}

	data, _ := json.MarshalIndent(map[string]any{
		"cookies": cookies,
		"count":   len(cookies),
	}, "", "  ")

	content := string(data)
	chunk, totalChunks := ChunkText(content, defaultChunkSize, in.Offset)
	if totalChunks > 1 {
		chunk = fmt.Sprintf("Chunk: %d/%d (use offset parameter to read more)\n\n%s", in.Offset+1, totalChunks, chunk)
	}
	return &ToolResult{Content: chunk}, nil
}

func (t *WebDomainTool) handleDevtoolsPerformance(ctx context.Context, in WebDomainInput, page *browser.Page) (*ToolResult, error) {
	script := `(() => {
		const perf = {};

		// Navigation timing
		if (window.performance && window.performance.timing) {
			const t = window.performance.timing;
			perf.navigation = {
				dns_ms: t.domainLookupEnd - t.domainLookupStart,
				connect_ms: t.connectEnd - t.connectStart,
				ttfb_ms: t.responseStart - t.requestStart,
				response_ms: t.responseEnd - t.responseStart,
				dom_interactive_ms: t.domInteractive - t.navigationStart,
				dom_complete_ms: t.domComplete - t.navigationStart,
				load_ms: t.loadEventEnd - t.navigationStart,
			};
		}

		// Paint timing
		if (window.performance && window.performance.getEntriesByType) {
			const paints = window.performance.getEntriesByType('paint');
			if (paints.length > 0) {
				perf.paint = {};
				paints.forEach(p => {
					perf.paint[p.name] = Math.round(p.startTime) + 'ms';
				});
			}
		}

		// Resource summary
		if (window.performance && window.performance.getEntriesByType) {
			const resources = window.performance.getEntriesByType('resource');
			const summary = {};
			resources.forEach(r => {
				const type = r.initiatorType || 'other';
				if (!summary[type]) summary[type] = { count: 0, total_bytes: 0, total_ms: 0 };
				summary[type].count++;
				summary[type].total_bytes += r.transferSize || 0;
				summary[type].total_ms += Math.round(r.duration);
			});
			perf.resources = { total: resources.length, by_type: summary };
		}

		// Memory (Chrome only)
		if (window.performance && window.performance.memory) {
			const m = window.performance.memory;
			perf.memory = {
				used_mb: Math.round(m.usedJSHeapSize / 1048576 * 100) / 100,
				total_mb: Math.round(m.totalJSHeapSize / 1048576 * 100) / 100,
				limit_mb: Math.round(m.jsHeapSizeLimit / 1048576 * 100) / 100,
			};
		}

		return perf;
	})()`

	result, err := page.Evaluate(ctx, script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Performance metrics failed: %v", err), IsError: true}, nil
	}

	data, _ := json.MarshalIndent(result, "", "  ")
	return &ToolResult{Content: string(data)}, nil
}

// formatJSONResult formats a JSON result with a prefix message.
func formatJSONResult(data json.RawMessage, prefix string) string {
	var m map[string]any
	if err := json.Unmarshal(data, &m); err != nil {
		return prefix + ": " + string(data)
	}
	if errMsg, ok := m["error"]; ok {
		return fmt.Sprintf("Error: %v", errMsg)
	}
	formatted, _ := json.MarshalIndent(m, "", "  ")
	return prefix + "\n" + string(formatted)
}

// Close cleans up browser resources.
func (t *WebDomainTool) Close() {
	// Browser cleanup is handled by the manager
}

// HandleVision analyzes images (placeholder - requires API key).
func (t *WebDomainTool) HandleVision(ctx context.Context, imagePath, imageBase64, prompt string) (*ToolResult, error) {
	var b64 string
	if imagePath != "" {
		data, err := io.ReadAll(strings.NewReader(imagePath))
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Error reading image: %v", err), IsError: true}, nil
		}
		b64 = base64.StdEncoding.EncodeToString(data)
	} else {
		b64 = imageBase64
	}

	if b64 == "" {
		return &ToolResult{Content: "Error: image_path or image_base64 is required", IsError: true}, nil
	}

	return &ToolResult{Content: "Vision analysis requires ANTHROPIC_API_KEY configuration", IsError: true}, nil
}

// --- Search implementations ---

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
	req.Header.Set("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
	req.Header.Set("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
	req.Header.Set("Accept-Language", "en-US,en;q=0.5")

	resp, err := t.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, err
	}

	html := string(body)

	// Detect bot protection (CAPTCHA, challenge pages)
	if strings.Contains(html, "please click") && strings.Contains(html, "bot") ||
		strings.Contains(html, "challenge") && strings.Contains(html, "captcha") ||
		strings.Contains(html, "Select all squares") {
		return nil, fmt.Errorf("search blocked by bot protection (CAPTCHA)")
	}

	return parseWebDuckDuckGoHTML(html, limit), nil
}

func (t *WebDomainTool) searchViaNativeBrowser(ctx context.Context, query string, limit int) ([]webSearchResult, error) {
	mgr := webview.GetManager()
	if !mgr.IsAvailable() {
		return nil, fmt.Errorf("native browser not available")
	}

	searchURL := fmt.Sprintf("https://duckduckgo.com/?q=%s", url.QueryEscape(query))

	win, err := mgr.CreateWindow(searchURL, "Nebo Search", GetSessionKey(ctx))
	if err != nil {
		return nil, fmt.Errorf("failed to create search window: %w", err)
	}
	defer mgr.CloseWindow(win.ID)

	// Wait for search results to render
	waitTimeout := 10 * time.Second
	_, _ = webview.Wait(ctx, mgr, win.ID, "article[data-testid='result']", waitTimeout)

	// Extract results via JavaScript
	js := `(function() {
		var results = [];
		var items = document.querySelectorAll('article[data-testid="result"]');
		if (items.length === 0) items = document.querySelectorAll('.result.results_links');
		if (items.length === 0) items = document.querySelectorAll('.nrn-react-div .result');
		items.forEach(function(item) {
			var titleEl = item.querySelector('a[data-testid="result-title-a"]') || item.querySelector('h2 a') || item.querySelector('a');
			var snippetEl = item.querySelector('[data-testid="result-snippet"]') || item.querySelector('.snippet, .result__snippet');
			if (titleEl && titleEl.href) {
				results.push({
					title: titleEl.textContent.trim(),
					url: titleEl.href,
					snippet: snippetEl ? snippetEl.textContent.trim() : ''
				});
			}
		});
		return JSON.stringify(results);
	})()`

	evalTimeout := 15 * time.Second
	resultJSON, err := webview.Evaluate(ctx, mgr, win.ID, js, evalTimeout)
	if err != nil {
		return nil, fmt.Errorf("failed to extract results: %w", err)
	}

	// Evaluate returns JSON — the JS returns a JSON.stringify'd string,
	// so resultJSON is a quoted JSON string that needs double-unmarshal.
	var rawStr string
	if err := json.Unmarshal(resultJSON, &rawStr); err != nil {
		rawStr = string(resultJSON)
	}

	var jsResults []struct {
		Title   string `json:"title"`
		URL     string `json:"url"`
		Snippet string `json:"snippet"`
	}
	if err := json.Unmarshal([]byte(rawStr), &jsResults); err != nil {
		return nil, fmt.Errorf("failed to parse search results: %w", err)
	}

	results := make([]webSearchResult, 0, len(jsResults))
	for _, r := range jsResults {
		if len(results) >= limit {
			break
		}
		if r.Title != "" && r.URL != "" {
			results = append(results, webSearchResult{
				Title:   r.Title,
				URL:     r.URL,
				Snippet: r.Snippet,
			})
		}
	}

	return results, nil
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

// --- Helpers ---

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

func writeScreenshotFile(path string, data []byte) error {
	if strings.HasPrefix(path, "~/") {
		homeDir, err := os.UserHomeDir()
		if err != nil {
			return fmt.Errorf("failed to get home directory: %w", err)
		}
		path = homeDir + path[1:]
	}

	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return fmt.Errorf("failed to create directory: %w", err)
	}

	return os.WriteFile(path, data, 0644)
}

// --- SSRF Protection ---

var ssrfBlockedNets = func() []*net.IPNet {
	cidrs := []string{
		"127.0.0.0/8", "10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16",
		"169.254.0.0/16", "0.0.0.0/8", "100.64.0.0/10", "192.0.0.0/24",
		"198.18.0.0/15", "::1/128", "fc00::/7", "fe80::/10",
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

func isBlockedIP(ip net.IP) bool {
	if ip == nil {
		return true
	}
	for _, n := range ssrfBlockedNets {
		if n.Contains(ip) {
			return true
		}
	}
	return false
}

func validateFetchURL(rawURL string) error {
	u, err := url.Parse(rawURL)
	if err != nil {
		return fmt.Errorf("invalid URL: %w", err)
	}

	if u.Scheme != "http" && u.Scheme != "https" {
		return fmt.Errorf("blocked: scheme %q not allowed (only http/https)", u.Scheme)
	}

	hostname := u.Hostname()
	if hostname == "" {
		return fmt.Errorf("blocked: empty hostname")
	}

	metadataHosts := []string{"metadata.google.internal", "metadata.google.com"}
	lowerHost := strings.ToLower(hostname)
	for _, mh := range metadataHosts {
		if lowerHost == mh {
			return fmt.Errorf("blocked: cloud metadata endpoint %q", hostname)
		}
	}

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
