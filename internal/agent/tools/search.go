package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"
)

// SearchTool performs web searches using various search engines
type SearchTool struct {
	client  *http.Client
	apiKey  string // Optional: for Google Custom Search API
	cx      string // Optional: Google Custom Search Engine ID
}

// SearchInput defines the input for the search tool
type SearchInput struct {
	Query   string `json:"query"`             // Search query
	Engine  string `json:"engine,omitempty"`  // "google", "duckduckgo" (default: duckduckgo)
	Limit   int    `json:"limit,omitempty"`   // Max results (default: 10)
}

// SearchResult represents a single search result
type SearchResult struct {
	Title   string `json:"title"`
	URL     string `json:"url"`
	Snippet string `json:"snippet"`
}

// NewSearchTool creates a new search tool
func NewSearchTool() *SearchTool {
	return &SearchTool{
		client: &http.Client{
			Timeout: 30 * time.Second,
		},
	}
}

// NewSearchToolWithGoogle creates a search tool with Google Custom Search API
func NewSearchToolWithGoogle(apiKey, cx string) *SearchTool {
	t := NewSearchTool()
	t.apiKey = apiKey
	t.cx = cx
	return t
}

// Name returns the tool name
func (t *SearchTool) Name() string {
	return "web_search"
}

// Description returns the tool description
func (t *SearchTool) Description() string {
	return "Search the web for information. Returns titles, URLs, and snippets from search results."
}

// Schema returns the JSON schema
func (t *SearchTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"query": {
				"type": "string",
				"description": "The search query"
			},
			"engine": {
				"type": "string",
				"description": "Search engine to use: 'duckduckgo' (default, no API key needed) or 'google' (requires API key)",
				"enum": ["duckduckgo", "google"]
			},
			"limit": {
				"type": "integer",
				"description": "Maximum number of results to return (default: 10)",
				"default": 10
			}
		},
		"required": ["query"]
	}`)
}

// RequiresApproval returns false - search is safe
func (t *SearchTool) RequiresApproval() bool {
	return false
}

// Execute performs the search
func (t *SearchTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params SearchInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Invalid input: %v", err),
			IsError: true,
		}, nil
	}

	if params.Query == "" {
		return &ToolResult{
			Content: "Error: 'query' is required",
			IsError: true,
		}, nil
	}

	if params.Limit <= 0 {
		params.Limit = 10
	}

	if params.Engine == "" {
		params.Engine = "duckduckgo"
	}

	var results []SearchResult
	var err error

	switch params.Engine {
	case "google":
		if t.apiKey != "" && t.cx != "" {
			results, err = t.searchGoogle(ctx, params.Query, params.Limit)
		} else {
			return &ToolResult{
				Content: "Google search requires API key configuration. Using DuckDuckGo instead.",
				IsError: false,
			}, nil
		}
	default:
		results, err = t.searchDuckDuckGo(ctx, params.Query, params.Limit)
	}

	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Search error: %v", err),
			IsError: true,
		}, nil
	}

	if len(results) == 0 {
		return &ToolResult{
			Content: "No results found for: " + params.Query,
			IsError: false,
		}, nil
	}

	// Format results
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Search results for: %s\n\n", params.Query))

	for i, r := range results {
		sb.WriteString(fmt.Sprintf("%d. %s\n", i+1, r.Title))
		sb.WriteString(fmt.Sprintf("   URL: %s\n", r.URL))
		if r.Snippet != "" {
			sb.WriteString(fmt.Sprintf("   %s\n", r.Snippet))
		}
		sb.WriteString("\n")
	}

	return &ToolResult{
		Content: sb.String(),
		IsError: false,
	}, nil
}

// searchDuckDuckGo uses DuckDuckGo's HTML search (no API key needed)
func (t *SearchTool) searchDuckDuckGo(ctx context.Context, query string, limit int) ([]SearchResult, error) {
	// Use DuckDuckGo HTML interface
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

	return t.parseDuckDuckGoHTML(string(body), limit), nil
}

// parseDuckDuckGoHTML extracts search results from DuckDuckGo HTML
func (t *SearchTool) parseDuckDuckGoHTML(html string, limit int) []SearchResult {
	var results []SearchResult

	// Simple HTML parsing for DuckDuckGo results
	// Looking for <a class="result__a" href="...">title</a>
	// and <a class="result__snippet">snippet</a>

	// Split by result divs
	parts := strings.Split(html, `class="result__body"`)

	for i, part := range parts[1:] { // Skip first part (before results)
		if i >= limit {
			break
		}

		result := SearchResult{}

		// Extract URL and title from result__a
		if idx := strings.Index(part, `class="result__a"`); idx != -1 {
			// Find href
			hrefStart := strings.Index(part[idx:], `href="`)
			if hrefStart != -1 {
				hrefStart += idx + 6
				hrefEnd := strings.Index(part[hrefStart:], `"`)
				if hrefEnd != -1 {
					rawURL := part[hrefStart : hrefStart+hrefEnd]
					// DuckDuckGo uses redirect URLs, extract actual URL
					if u, err := url.Parse(rawURL); err == nil {
						if uddg := u.Query().Get("uddg"); uddg != "" {
							result.URL = uddg
						} else {
							result.URL = rawURL
						}
					}
				}
			}

			// Find title (text between > and </a>)
			titleStart := strings.Index(part[idx:], ">")
			if titleStart != -1 {
				titleStart += idx + 1
				titleEnd := strings.Index(part[titleStart:], "</a>")
				if titleEnd != -1 {
					result.Title = strings.TrimSpace(stripHTML(part[titleStart : titleStart+titleEnd]))
				}
			}
		}

		// Extract snippet
		if idx := strings.Index(part, `class="result__snippet"`); idx != -1 {
			snippetStart := strings.Index(part[idx:], ">")
			if snippetStart != -1 {
				snippetStart += idx + 1
				snippetEnd := strings.Index(part[snippetStart:], "</a>")
				if snippetEnd == -1 {
					snippetEnd = strings.Index(part[snippetStart:], "</span>")
				}
				if snippetEnd != -1 {
					result.Snippet = strings.TrimSpace(stripHTML(part[snippetStart : snippetStart+snippetEnd]))
				}
			}
		}

		if result.Title != "" && result.URL != "" {
			results = append(results, result)
		}
	}

	return results
}

// searchGoogle uses Google Custom Search API
func (t *SearchTool) searchGoogle(ctx context.Context, query string, limit int) ([]SearchResult, error) {
	if limit > 10 {
		limit = 10 // Google CSE limit per request
	}

	searchURL := fmt.Sprintf(
		"https://www.googleapis.com/customsearch/v1?key=%s&cx=%s&q=%s&num=%d",
		t.apiKey, t.cx, url.QueryEscape(query), limit,
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

	results := make([]SearchResult, 0, len(data.Items))
	for _, item := range data.Items {
		results = append(results, SearchResult{
			Title:   item.Title,
			URL:     item.Link,
			Snippet: item.Snippet,
		})
	}

	return results, nil
}

// stripHTML removes HTML tags from a string
func stripHTML(s string) string {
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

	// Clean up entities
	text := result.String()
	text = strings.ReplaceAll(text, "&amp;", "&")
	text = strings.ReplaceAll(text, "&lt;", "<")
	text = strings.ReplaceAll(text, "&gt;", ">")
	text = strings.ReplaceAll(text, "&quot;", "\"")
	text = strings.ReplaceAll(text, "&#x27;", "'")
	text = strings.ReplaceAll(text, "&nbsp;", " ")

	return text
}
