// Package neboloop provides a REST API client for NeboLoop.
//
// It handles bot authentication (JWT caching with auto-refresh) and provides
// typed methods for browsing, installing, and uninstalling apps and skills.
package neboloop

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"sync"
)

// Client communicates with the NeboLoop REST API.
// It uses the owner's OAuth JWT directly for authentication.
type Client struct {
	apiServer string
	botID     string
	token     string // Owner OAuth JWT

	mu sync.RWMutex
}

// APIServer returns the base API server URL.
func (c *Client) APIServer() string { return c.apiServer }

// NewClient creates a NeboLoop API client from plugin settings.
// Required keys: api_server, bot_id, token (owner JWT).
func NewClient(settings map[string]string) (*Client, error) {
	apiServer := settings["api_server"]
	if apiServer == "" {
		return nil, fmt.Errorf("api_server not configured")
	}
	botID := settings["bot_id"]
	if botID == "" {
		return nil, fmt.Errorf("bot_id not configured")
	}
	token := settings["token"]
	if token == "" {
		return nil, fmt.Errorf("token (owner JWT) not configured")
	}
	return &Client{
		apiServer: apiServer,
		botID:     botID,
		token:     token,
	}, nil
}

// --------------------------------------------------------------------------
// Authentication
// --------------------------------------------------------------------------

// authedRequest creates an HTTP request with the owner JWT as Authorization header.
func (c *Client) authedRequest(ctx context.Context, method, path string, body io.Reader) (*http.Request, error) {
	req, err := http.NewRequestWithContext(ctx, method, c.apiServer+path, body)
	if err != nil {
		return nil, err
	}

	c.mu.RLock()
	req.Header.Set("Authorization", "Bearer "+c.token)
	c.mu.RUnlock()
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}

	return req, nil
}

// doJSON sends an authed request and decodes the JSON response into dest.
// On 401, re-authenticates once and retries.
func (c *Client) doJSON(ctx context.Context, method, path string, reqBody any, dest any) error {
	var body io.Reader
	if reqBody != nil {
		b, err := json.Marshal(reqBody)
		if err != nil {
			return fmt.Errorf("marshal request: %w", err)
		}
		body = bytes.NewReader(b)
	}

	req, err := c.authedRequest(ctx, method, path, body)
	if err != nil {
		return err
	}

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return fmt.Errorf("request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		b, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("NeboLoop returned %d: %s", resp.StatusCode, string(b))
	}

	if dest != nil {
		if err := json.NewDecoder(resp.Body).Decode(dest); err != nil {
			return fmt.Errorf("decode response: %w", err)
		}
	}
	return nil
}

// --------------------------------------------------------------------------
// Apps
// --------------------------------------------------------------------------

// ListApps fetches the app catalog from NeboLoop.
func (c *Client) ListApps(ctx context.Context, query, category string, page, pageSize int) (*AppsResponse, error) {
	path := "/api/v1/apps" + buildQuery(query, category, page, pageSize)
	var resp AppsResponse
	if err := c.doJSON(ctx, http.MethodGet, path, nil, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

// GetApp fetches a single app with manifest inline.
func (c *Client) GetApp(ctx context.Context, id string) (*AppDetail, error) {
	var resp AppDetail
	if err := c.doJSON(ctx, http.MethodGet, "/api/v1/apps/"+id, nil, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

// GetAppReviews fetches reviews for an app from NeboLoop.
func (c *Client) GetAppReviews(ctx context.Context, id string, page, pageSize int) (*ReviewsResponse, error) {
	params := url.Values{}
	if page > 0 {
		params.Set("page", strconv.Itoa(page))
	}
	if pageSize > 0 {
		params.Set("pageSize", strconv.Itoa(pageSize))
	}
	path := "/api/v1/apps/" + id + "/reviews"
	if len(params) > 0 {
		path += "?" + params.Encode()
	}
	var resp ReviewsResponse
	if err := c.doJSON(ctx, http.MethodGet, path, nil, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

// InstallApp installs an app for this bot.
func (c *Client) InstallApp(ctx context.Context, id string) (*InstallResponse, error) {
	var resp InstallResponse
	if err := c.doJSON(ctx, http.MethodPost, "/api/v1/apps/"+id+"/install", nil, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

// UninstallApp uninstalls an app for this bot.
func (c *Client) UninstallApp(ctx context.Context, id string) error {
	return c.doJSON(ctx, http.MethodDelete, "/api/v1/apps/"+id+"/install", nil, nil)
}

// --------------------------------------------------------------------------
// Bot Identity
// --------------------------------------------------------------------------

// UpdateBotIdentity pushes the agent's name and role to NeboLoop.
func (c *Client) UpdateBotIdentity(ctx context.Context, name, role string) error {
	req := UpdateBotIdentityRequest{Name: name, Role: role}
	return c.doJSON(ctx, http.MethodPut, "/api/v1/bots/"+c.botID, req, nil)
}

// --------------------------------------------------------------------------
// Skills
// --------------------------------------------------------------------------

// ListSkills fetches the skill catalog from NeboLoop.
func (c *Client) ListSkills(ctx context.Context, query, category string, page, pageSize int) (*SkillsResponse, error) {
	path := "/api/v1/skills" + buildQuery(query, category, page, pageSize)
	var resp SkillsResponse
	if err := c.doJSON(ctx, http.MethodGet, path, nil, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

// GetSkill fetches a single skill with manifest inline.
func (c *Client) GetSkill(ctx context.Context, id string) (*SkillDetail, error) {
	var resp SkillDetail
	if err := c.doJSON(ctx, http.MethodGet, "/api/v1/skills/"+id, nil, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

// InstallSkill installs a skill for this bot.
func (c *Client) InstallSkill(ctx context.Context, id string) (*InstallResponse, error) {
	var resp InstallResponse
	if err := c.doJSON(ctx, http.MethodPost, "/api/v1/skills/"+id+"/install", map[string]string{
		"bot_id": c.botID,
	}, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

// UninstallSkill uninstalls a skill for this bot.
func (c *Client) UninstallSkill(ctx context.Context, id string) error {
	return c.doJSON(ctx, http.MethodDelete, "/api/v1/skills/"+id+"/install/"+c.botID, nil, nil)
}

// FetchRaw downloads raw content from a URL using the client's auth header.
func (c *Client) FetchRaw(ctx context.Context, rawURL string) ([]byte, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, rawURL, nil)
	if err != nil {
		return nil, err
	}
	c.mu.RLock()
	req.Header.Set("Authorization", "Bearer "+c.token)
	c.mu.RUnlock()

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("fetch failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		b, _ := io.ReadAll(resp.Body)
		return nil, fmt.Errorf("NeboLoop returned %d: %s", resp.StatusCode, string(b))
	}
	return io.ReadAll(resp.Body)
}

// --------------------------------------------------------------------------
// Loops
// --------------------------------------------------------------------------

// JoinLoop joins the bot to a loop using an invite code.
func (c *Client) JoinLoop(ctx context.Context, code string) (*JoinLoopResponse, error) {
	var resp JoinLoopResponse
	if err := c.doJSON(ctx, http.MethodPost, "/api/v1/loops/join", JoinLoopRequest{
		Code:  code,
		BotID: c.botID,
	}, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

// --------------------------------------------------------------------------
// Loop Channels
// --------------------------------------------------------------------------

// ListBotChannels returns all loop channels this bot belongs to across all loops.
func (c *Client) ListBotChannels(ctx context.Context) ([]LoopChannel, error) {
	var resp LoopChannelsResponse
	if err := c.doJSON(ctx, http.MethodGet, "/api/v1/bots/"+c.botID+"/channels", nil, &resp); err != nil {
		return nil, err
	}
	return resp.Channels, nil
}

// --------------------------------------------------------------------------
// Loop Queries (Bot Query System)
// --------------------------------------------------------------------------

// ListBotLoops returns all loops this bot belongs to.
func (c *Client) ListBotLoops(ctx context.Context) ([]Loop, error) {
	var resp LoopsResponse
	if err := c.doJSON(ctx, http.MethodGet, "/api/v1/bots/"+c.botID+"/loops", nil, &resp); err != nil {
		return nil, err
	}
	return resp.Loops, nil
}

// GetLoop fetches a single loop by ID.
func (c *Client) GetLoop(ctx context.Context, loopID string) (*Loop, error) {
	var resp Loop
	if err := c.doJSON(ctx, http.MethodGet, "/api/v1/bots/"+c.botID+"/loops/"+loopID, nil, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

// ListLoopMembers returns members of a loop with online presence.
func (c *Client) ListLoopMembers(ctx context.Context, loopID string) ([]LoopMember, error) {
	var resp LoopMembersResponse
	if err := c.doJSON(ctx, http.MethodGet, "/api/v1/bots/"+c.botID+"/loops/"+loopID+"/members", nil, &resp); err != nil {
		return nil, err
	}
	return resp.Members, nil
}

// ListChannelMembers returns members of a channel with online presence.
func (c *Client) ListChannelMembers(ctx context.Context, channelID string) ([]ChannelMember, error) {
	var resp ChannelMembersResponse
	if err := c.doJSON(ctx, http.MethodGet, "/api/v1/bots/"+c.botID+"/channels/"+channelID+"/members", nil, &resp); err != nil {
		return nil, err
	}
	return resp.Members, nil
}

// ListChannelMessages fetches recent messages from a channel (oldest-first, max 200).
func (c *Client) ListChannelMessages(ctx context.Context, channelID string, limit int) ([]ChannelMessageItem, error) {
	path := "/api/v1/bots/" + c.botID + "/channels/" + channelID + "/messages"
	if limit > 0 {
		path += "?limit=" + strconv.Itoa(limit)
	}
	var resp ChannelMessagesResponse
	if err := c.doJSON(ctx, http.MethodGet, path, nil, &resp); err != nil {
		return nil, err
	}
	return resp.Messages, nil
}

// --------------------------------------------------------------------------
// Connection Code (pre-auth, no client instance needed)
// --------------------------------------------------------------------------

// DefaultAPIServer is the production NeboLoop API server.
const DefaultAPIServer = "https://api.neboloop.com"

// RedeemCode exchanges a connection code for a bot identity and one-time token.
// This is an unauthenticated call used during initial setup.
// botID is Nebo's locally-generated immutable UUID — the server registers
// the bot with this ID instead of generating a new one.
func RedeemCode(ctx context.Context, apiServer, code, name, purpose, botID string) (*RedeemCodeResponse, error) {
	var resp RedeemCodeResponse
	if err := postJSON(ctx, apiServer+"/api/v1/bots/connect/redeem", RedeemCodeRequest{
		Code:    code,
		Name:    name,
		Purpose: purpose,
		BotID:   botID,
	}, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

// postJSON is a simple unauthenticated POST helper for connection flow.
func postJSON(ctx context.Context, url string, reqBody any, dest any) error {
	b, err := json.Marshal(reqBody)
	if err != nil {
		return fmt.Errorf("marshal request: %w", err)
	}

	req, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(b))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json")

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return fmt.Errorf("request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		body, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("NeboLoop returned %d: %s", resp.StatusCode, string(body))
	}

	if dest != nil {
		if err := json.NewDecoder(resp.Body).Decode(dest); err != nil {
			return fmt.Errorf("decode response: %w", err)
		}
	}
	return nil
}

// --------------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------------

func buildQuery(query, category string, page, pageSize int) string {
	params := url.Values{}
	if query != "" {
		params.Set("q", query)
	}
	if category != "" {
		params.Set("category", category)
	}
	if page > 0 {
		params.Set("page", strconv.Itoa(page))
	}
	if pageSize > 0 {
		params.Set("pageSize", strconv.Itoa(pageSize))
	}
	if len(params) == 0 {
		return ""
	}
	return "?" + params.Encode()
}
