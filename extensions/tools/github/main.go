// GitHub Plugin - Interact with GitHub API
// Build: go build -o ~/.nebo/plugins/tools/github
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

type GitHubTool struct {
	token string
}

type githubInput struct {
	Action  string `json:"action"`  // repos, issues, prs, create_issue, create_pr, search
	Owner   string `json:"owner"`   // Repository owner
	Repo    string `json:"repo"`    // Repository name
	Number  int    `json:"number"`  // Issue/PR number
	Title   string `json:"title"`   // Title for create operations
	Body    string `json:"body"`    // Body for create operations
	Labels  string `json:"labels"`  // Comma-separated labels
	State   string `json:"state"`   // Filter by state (open, closed, all)
	Query   string `json:"query"`   // Search query
	Base    string `json:"base"`    // Base branch for PR
	Head    string `json:"head"`    // Head branch for PR
}

type ToolResult struct {
	Content string `json:"content"`
	IsError bool   `json:"is_error"`
}

func (t *GitHubTool) Name() string {
	return "github"
}

func (t *GitHubTool) Description() string {
	return "Interact with GitHub API: list repos, issues, PRs, create issues/PRs, and search."
}

func (t *GitHubTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["repos", "issues", "prs", "issue", "pr", "create_issue", "create_pr", "search", "comment"],
				"description": "Action: repos (list), issues (list), prs (list), issue (get one), pr (get one), create_issue, create_pr, search, comment"
			},
			"owner": {
				"type": "string",
				"description": "Repository owner (user or org)"
			},
			"repo": {
				"type": "string",
				"description": "Repository name"
			},
			"number": {
				"type": "integer",
				"description": "Issue or PR number"
			},
			"title": {
				"type": "string",
				"description": "Title for new issue or PR"
			},
			"body": {
				"type": "string",
				"description": "Body content for issue, PR, or comment"
			},
			"labels": {
				"type": "string",
				"description": "Comma-separated labels"
			},
			"state": {
				"type": "string",
				"enum": ["open", "closed", "all"],
				"description": "Filter by state"
			},
			"query": {
				"type": "string",
				"description": "Search query"
			},
			"base": {
				"type": "string",
				"description": "Base branch for PR"
			},
			"head": {
				"type": "string",
				"description": "Head branch for PR"
			}
		},
		"required": ["action"]
	}`)
}

func (t *GitHubTool) RequiresApproval() bool {
	return false
}

func (t *GitHubTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params githubInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	// Get token
	token := t.token
	if token == "" {
		token = os.Getenv("GITHUB_TOKEN")
	}
	if token == "" {
		return &ToolResult{Content: "GITHUB_TOKEN not set", IsError: true}, nil
	}

	var result string
	var err error

	switch params.Action {
	case "repos":
		result, err = t.listRepos(ctx, token, params)
	case "issues":
		result, err = t.listIssues(ctx, token, params)
	case "prs":
		result, err = t.listPRs(ctx, token, params)
	case "issue":
		result, err = t.getIssue(ctx, token, params)
	case "pr":
		result, err = t.getPR(ctx, token, params)
	case "create_issue":
		result, err = t.createIssue(ctx, token, params)
	case "create_pr":
		result, err = t.createPR(ctx, token, params)
	case "search":
		result, err = t.search(ctx, token, params)
	case "comment":
		result, err = t.addComment(ctx, token, params)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", params.Action), IsError: true}, nil
	}

	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("GitHub API error: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: result, IsError: false}, nil
}

func (t *GitHubTool) apiRequest(ctx context.Context, token, method, endpoint string, body io.Reader) ([]byte, error) {
	url := "https://api.github.com" + endpoint
	req, err := http.NewRequestWithContext(ctx, method, url, body)
	if err != nil {
		return nil, err
	}

	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Accept", "application/vnd.github+json")
	req.Header.Set("X-GitHub-Api-Version", "2022-11-28")
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}

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

func (t *GitHubTool) listRepos(ctx context.Context, token string, params githubInput) (string, error) {
	endpoint := "/user/repos?sort=updated&per_page=20"
	if params.Owner != "" {
		endpoint = fmt.Sprintf("/users/%s/repos?sort=updated&per_page=20", params.Owner)
	}

	data, err := t.apiRequest(ctx, token, "GET", endpoint, nil)
	if err != nil {
		return "", err
	}

	var repos []struct {
		FullName    string `json:"full_name"`
		Description string `json:"description"`
		Private     bool   `json:"private"`
		StarCount   int    `json:"stargazers_count"`
		UpdatedAt   string `json:"updated_at"`
	}
	if err := json.Unmarshal(data, &repos); err != nil {
		return "", err
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Repositories (%d):\n\n", len(repos)))
	for _, r := range repos {
		visibility := "public"
		if r.Private {
			visibility = "private"
		}
		sb.WriteString(fmt.Sprintf("- %s [%s] (%d stars)\n", r.FullName, visibility, r.StarCount))
		if r.Description != "" {
			sb.WriteString(fmt.Sprintf("  %s\n", r.Description))
		}
	}

	return sb.String(), nil
}

func (t *GitHubTool) listIssues(ctx context.Context, token string, params githubInput) (string, error) {
	if params.Owner == "" || params.Repo == "" {
		return "", fmt.Errorf("owner and repo are required")
	}

	state := params.State
	if state == "" {
		state = "open"
	}

	endpoint := fmt.Sprintf("/repos/%s/%s/issues?state=%s&per_page=20", params.Owner, params.Repo, state)
	data, err := t.apiRequest(ctx, token, "GET", endpoint, nil)
	if err != nil {
		return "", err
	}

	var issues []struct {
		Number    int    `json:"number"`
		Title     string `json:"title"`
		State     string `json:"state"`
		User      struct{ Login string } `json:"user"`
		Labels    []struct{ Name string } `json:"labels"`
		CreatedAt string `json:"created_at"`
	}
	if err := json.Unmarshal(data, &issues); err != nil {
		return "", err
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Issues for %s/%s (%d):\n\n", params.Owner, params.Repo, len(issues)))
	for _, i := range issues {
		labels := ""
		if len(i.Labels) > 0 {
			var labelNames []string
			for _, l := range i.Labels {
				labelNames = append(labelNames, l.Name)
			}
			labels = " [" + strings.Join(labelNames, ", ") + "]"
		}
		sb.WriteString(fmt.Sprintf("#%d %s%s\n  by @%s\n", i.Number, i.Title, labels, i.User.Login))
	}

	return sb.String(), nil
}

func (t *GitHubTool) listPRs(ctx context.Context, token string, params githubInput) (string, error) {
	if params.Owner == "" || params.Repo == "" {
		return "", fmt.Errorf("owner and repo are required")
	}

	state := params.State
	if state == "" {
		state = "open"
	}

	endpoint := fmt.Sprintf("/repos/%s/%s/pulls?state=%s&per_page=20", params.Owner, params.Repo, state)
	data, err := t.apiRequest(ctx, token, "GET", endpoint, nil)
	if err != nil {
		return "", err
	}

	var prs []struct {
		Number    int    `json:"number"`
		Title     string `json:"title"`
		State     string `json:"state"`
		User      struct{ Login string } `json:"user"`
		Head      struct{ Ref string } `json:"head"`
		Base      struct{ Ref string } `json:"base"`
		CreatedAt string `json:"created_at"`
	}
	if err := json.Unmarshal(data, &prs); err != nil {
		return "", err
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Pull Requests for %s/%s (%d):\n\n", params.Owner, params.Repo, len(prs)))
	for _, p := range prs {
		sb.WriteString(fmt.Sprintf("#%d %s\n  %s -> %s by @%s\n", p.Number, p.Title, p.Head.Ref, p.Base.Ref, p.User.Login))
	}

	return sb.String(), nil
}

func (t *GitHubTool) getIssue(ctx context.Context, token string, params githubInput) (string, error) {
	if params.Owner == "" || params.Repo == "" || params.Number == 0 {
		return "", fmt.Errorf("owner, repo, and number are required")
	}

	endpoint := fmt.Sprintf("/repos/%s/%s/issues/%d", params.Owner, params.Repo, params.Number)
	data, err := t.apiRequest(ctx, token, "GET", endpoint, nil)
	if err != nil {
		return "", err
	}

	var issue struct {
		Number    int    `json:"number"`
		Title     string `json:"title"`
		Body      string `json:"body"`
		State     string `json:"state"`
		User      struct{ Login string } `json:"user"`
		Labels    []struct{ Name string } `json:"labels"`
		Comments  int    `json:"comments"`
		CreatedAt string `json:"created_at"`
	}
	if err := json.Unmarshal(data, &issue); err != nil {
		return "", err
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Issue #%d: %s\n", issue.Number, issue.Title))
	sb.WriteString(fmt.Sprintf("State: %s | Author: @%s | Comments: %d\n", issue.State, issue.User.Login, issue.Comments))
	if len(issue.Labels) > 0 {
		var labels []string
		for _, l := range issue.Labels {
			labels = append(labels, l.Name)
		}
		sb.WriteString(fmt.Sprintf("Labels: %s\n", strings.Join(labels, ", ")))
	}
	sb.WriteString(fmt.Sprintf("\n%s", issue.Body))

	return sb.String(), nil
}

func (t *GitHubTool) getPR(ctx context.Context, token string, params githubInput) (string, error) {
	if params.Owner == "" || params.Repo == "" || params.Number == 0 {
		return "", fmt.Errorf("owner, repo, and number are required")
	}

	endpoint := fmt.Sprintf("/repos/%s/%s/pulls/%d", params.Owner, params.Repo, params.Number)
	data, err := t.apiRequest(ctx, token, "GET", endpoint, nil)
	if err != nil {
		return "", err
	}

	var pr struct {
		Number    int    `json:"number"`
		Title     string `json:"title"`
		Body      string `json:"body"`
		State     string `json:"state"`
		User      struct{ Login string } `json:"user"`
		Head      struct{ Ref string } `json:"head"`
		Base      struct{ Ref string } `json:"base"`
		Mergeable *bool  `json:"mergeable"`
		Additions int    `json:"additions"`
		Deletions int    `json:"deletions"`
		Comments  int    `json:"comments"`
	}
	if err := json.Unmarshal(data, &pr); err != nil {
		return "", err
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("PR #%d: %s\n", pr.Number, pr.Title))
	sb.WriteString(fmt.Sprintf("%s -> %s by @%s\n", pr.Head.Ref, pr.Base.Ref, pr.User.Login))
	sb.WriteString(fmt.Sprintf("State: %s | +%d/-%d | Comments: %d\n", pr.State, pr.Additions, pr.Deletions, pr.Comments))
	if pr.Mergeable != nil {
		if *pr.Mergeable {
			sb.WriteString("Mergeable: Yes\n")
		} else {
			sb.WriteString("Mergeable: No (conflicts)\n")
		}
	}
	sb.WriteString(fmt.Sprintf("\n%s", pr.Body))

	return sb.String(), nil
}

func (t *GitHubTool) createIssue(ctx context.Context, token string, params githubInput) (string, error) {
	if params.Owner == "" || params.Repo == "" || params.Title == "" {
		return "", fmt.Errorf("owner, repo, and title are required")
	}

	body := map[string]interface{}{
		"title": params.Title,
		"body":  params.Body,
	}
	if params.Labels != "" {
		body["labels"] = strings.Split(params.Labels, ",")
	}

	jsonBody, _ := json.Marshal(body)
	endpoint := fmt.Sprintf("/repos/%s/%s/issues", params.Owner, params.Repo)
	data, err := t.apiRequest(ctx, token, "POST", endpoint, strings.NewReader(string(jsonBody)))
	if err != nil {
		return "", err
	}

	var issue struct {
		Number  int    `json:"number"`
		HtmlURL string `json:"html_url"`
	}
	if err := json.Unmarshal(data, &issue); err != nil {
		return "", err
	}

	return fmt.Sprintf("Created issue #%d: %s", issue.Number, issue.HtmlURL), nil
}

func (t *GitHubTool) createPR(ctx context.Context, token string, params githubInput) (string, error) {
	if params.Owner == "" || params.Repo == "" || params.Title == "" || params.Head == "" || params.Base == "" {
		return "", fmt.Errorf("owner, repo, title, head, and base are required")
	}

	body := map[string]interface{}{
		"title": params.Title,
		"body":  params.Body,
		"head":  params.Head,
		"base":  params.Base,
	}

	jsonBody, _ := json.Marshal(body)
	endpoint := fmt.Sprintf("/repos/%s/%s/pulls", params.Owner, params.Repo)
	data, err := t.apiRequest(ctx, token, "POST", endpoint, strings.NewReader(string(jsonBody)))
	if err != nil {
		return "", err
	}

	var pr struct {
		Number  int    `json:"number"`
		HtmlURL string `json:"html_url"`
	}
	if err := json.Unmarshal(data, &pr); err != nil {
		return "", err
	}

	return fmt.Sprintf("Created PR #%d: %s", pr.Number, pr.HtmlURL), nil
}

func (t *GitHubTool) search(ctx context.Context, token string, params githubInput) (string, error) {
	if params.Query == "" {
		return "", fmt.Errorf("query is required")
	}

	endpoint := fmt.Sprintf("/search/repositories?q=%s&per_page=10", params.Query)
	data, err := t.apiRequest(ctx, token, "GET", endpoint, nil)
	if err != nil {
		return "", err
	}

	var result struct {
		TotalCount int `json:"total_count"`
		Items      []struct {
			FullName    string `json:"full_name"`
			Description string `json:"description"`
			StarCount   int    `json:"stargazers_count"`
			Language    string `json:"language"`
		} `json:"items"`
	}
	if err := json.Unmarshal(data, &result); err != nil {
		return "", err
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Search results for '%s' (%d total):\n\n", params.Query, result.TotalCount))
	for _, r := range result.Items {
		lang := r.Language
		if lang == "" {
			lang = "unknown"
		}
		sb.WriteString(fmt.Sprintf("- %s [%s] (%d stars)\n", r.FullName, lang, r.StarCount))
		if r.Description != "" {
			sb.WriteString(fmt.Sprintf("  %s\n", r.Description))
		}
	}

	return sb.String(), nil
}

func (t *GitHubTool) addComment(ctx context.Context, token string, params githubInput) (string, error) {
	if params.Owner == "" || params.Repo == "" || params.Number == 0 || params.Body == "" {
		return "", fmt.Errorf("owner, repo, number, and body are required")
	}

	body := map[string]string{"body": params.Body}
	jsonBody, _ := json.Marshal(body)

	endpoint := fmt.Sprintf("/repos/%s/%s/issues/%d/comments", params.Owner, params.Repo, params.Number)
	data, err := t.apiRequest(ctx, token, "POST", endpoint, strings.NewReader(string(jsonBody)))
	if err != nil {
		return "", err
	}

	var comment struct {
		ID      int    `json:"id"`
		HtmlURL string `json:"html_url"`
	}
	if err := json.Unmarshal(data, &comment); err != nil {
		return "", err
	}

	return fmt.Sprintf("Added comment: %s", comment.HtmlURL), nil
}

// RPC wrapper
type GitHubToolRPC struct {
	tool *GitHubTool
}

func (t *GitHubToolRPC) Name(args interface{}, reply *string) error {
	*reply = t.tool.Name()
	return nil
}

func (t *GitHubToolRPC) Description(args interface{}, reply *string) error {
	*reply = t.tool.Description()
	return nil
}

func (t *GitHubToolRPC) Schema(args interface{}, reply *json.RawMessage) error {
	*reply = t.tool.Schema()
	return nil
}

func (t *GitHubToolRPC) RequiresApproval(args interface{}, reply *bool) error {
	*reply = t.tool.RequiresApproval()
	return nil
}

type ExecuteArgs struct {
	Input json.RawMessage
}

func (t *GitHubToolRPC) Execute(args *ExecuteArgs, reply *ToolResult) error {
	result, err := t.tool.Execute(context.Background(), args.Input)
	if err != nil {
		return err
	}
	*reply = *result
	return nil
}

type GitHubPlugin struct {
	tool *GitHubTool
}

func (p *GitHubPlugin) Server(*plugin.MuxBroker) (interface{}, error) {
	return &GitHubToolRPC{tool: p.tool}, nil
}

func (p *GitHubPlugin) Client(b *plugin.MuxBroker, c *rpc.Client) (interface{}, error) {
	return &GitHubToolRPCClient{client: c}, nil
}

type GitHubToolRPCClient struct {
	client *rpc.Client
}

func (c *GitHubToolRPCClient) Name() string {
	var reply string
	c.client.Call("Plugin.Name", new(interface{}), &reply)
	return reply
}

func (c *GitHubToolRPCClient) Description() string {
	var reply string
	c.client.Call("Plugin.Description", new(interface{}), &reply)
	return reply
}

func (c *GitHubToolRPCClient) Schema() json.RawMessage {
	var reply json.RawMessage
	c.client.Call("Plugin.Schema", new(interface{}), &reply)
	return reply
}

func (c *GitHubToolRPCClient) RequiresApproval() bool {
	var reply bool
	c.client.Call("Plugin.RequiresApproval", new(interface{}), &reply)
	return reply
}

func (c *GitHubToolRPCClient) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var reply ToolResult
	err := c.client.Call("Plugin.Execute", &ExecuteArgs{Input: input}, &reply)
	return &reply, err
}

func main() {
	plugin.Serve(&plugin.ServeConfig{
		HandshakeConfig: Handshake,
		Plugins: map[string]plugin.Plugin{
			"tool": &GitHubPlugin{tool: &GitHubTool{}},
		},
	})
}
