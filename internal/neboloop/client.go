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
	"time"
)

// Client communicates with the NeboLoop REST API.
// It caches the bot JWT and refreshes it automatically on expiry or 401.
type Client struct {
	apiServer    string
	botID        string
	mqttPassword string

	token     string
	expiresAt time.Time
	mu        sync.RWMutex
}

// APIServer returns the base API server URL.
func (c *Client) APIServer() string { return c.apiServer }

// NewClient creates a NeboLoop API client from plugin settings.
// Required keys: api_server, bot_id, mqtt_password.
func NewClient(settings map[string]string) (*Client, error) {
	apiServer := settings["api_server"]
	if apiServer == "" {
		return nil, fmt.Errorf("api_server not configured")
	}
	botID := settings["bot_id"]
	if botID == "" {
		return nil, fmt.Errorf("bot_id not configured")
	}
	mqttPassword := settings["mqtt_password"]
	if mqttPassword == "" {
		return nil, fmt.Errorf("mqtt_password not configured")
	}
	return &Client{
		apiServer:    apiServer,
		botID:        botID,
		mqttPassword: mqttPassword,
	}, nil
}

// --------------------------------------------------------------------------
// Authentication
// --------------------------------------------------------------------------

type authRequest struct {
	BotID        string `json:"bot_id"`
	MQTTPassword string `json:"mqtt_password"`
}

type authResponse struct {
	Token     string `json:"token"`
	ExpiresIn int    `json:"expires_in"`
}

// Authenticate calls POST /api/v1/bots/auth and caches the JWT.
func (c *Client) Authenticate(ctx context.Context) error {
	body, _ := json.Marshal(authRequest{
		BotID:        c.botID,
		MQTTPassword: c.mqttPassword,
	})

	req, err := http.NewRequestWithContext(ctx, http.MethodPost, c.apiServer+"/api/v1/bots/auth", bytes.NewReader(body))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return fmt.Errorf("auth request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		b, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("auth returned %d: %s", resp.StatusCode, string(b))
	}

	var result authResponse
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return fmt.Errorf("decode auth response: %w", err)
	}

	c.mu.Lock()
	c.token = result.Token
	// Refresh 60s before actual expiry
	c.expiresAt = time.Now().Add(time.Duration(result.ExpiresIn)*time.Second - 60*time.Second)
	c.mu.Unlock()

	return nil
}

// authedRequest creates an HTTP request with a valid Authorization header.
// Auto-authenticates if the token is missing or expired.
func (c *Client) authedRequest(ctx context.Context, method, path string, body io.Reader) (*http.Request, error) {
	c.mu.RLock()
	needsAuth := c.token == "" || time.Now().After(c.expiresAt)
	c.mu.RUnlock()

	if needsAuth {
		if err := c.Authenticate(ctx); err != nil {
			return nil, fmt.Errorf("auto-authenticate: %w", err)
		}
	}

	req, err := http.NewRequestWithContext(ctx, method, c.apiServer+path, body)
	if err != nil {
		return nil, err
	}

	c.mu.RLock()
	req.Header.Set("Authorization", "Bearer "+c.token)
	c.mu.RUnlock()
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json")

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

	// Re-auth on 401 and retry once
	if resp.StatusCode == http.StatusUnauthorized {
		if err := c.Authenticate(ctx); err != nil {
			return fmt.Errorf("re-auth failed: %w", err)
		}
		// Rebuild request body if needed
		if reqBody != nil {
			b, _ := json.Marshal(reqBody)
			body = bytes.NewReader(b)
		}
		req, err = c.authedRequest(ctx, method, path, body)
		if err != nil {
			return err
		}
		resp, err = http.DefaultClient.Do(req)
		if err != nil {
			return fmt.Errorf("retry request failed: %w", err)
		}
		defer resp.Body.Close()
	}

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
	if err := c.doJSON(ctx, http.MethodPost, "/api/v1/skills/"+id+"/install", nil, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

// UninstallSkill uninstalls a skill for this bot.
func (c *Client) UninstallSkill(ctx context.Context, id string) error {
	return c.doJSON(ctx, http.MethodDelete, "/api/v1/skills/"+id+"/install", nil, nil)
}

// --------------------------------------------------------------------------
// Connection Code (pre-auth, no client instance needed)
// --------------------------------------------------------------------------

// DefaultAPIServer is the production NeboLoop API server.
const DefaultAPIServer = "https://neboloop.com"

// RedeemCode exchanges a connection code for a bot identity and one-time token.
// This is an unauthenticated call used during initial setup.
func RedeemCode(ctx context.Context, apiServer, code, name, purpose string) (*RedeemCodeResponse, error) {
	var resp RedeemCodeResponse
	if err := postJSON(ctx, apiServer+"/api/v1/bots/connect/redeem", RedeemCodeRequest{
		Code:    code,
		Name:    name,
		Purpose: purpose,
	}, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

// ExchangeToken exchanges a one-time connection token for MQTT credentials.
// This is an unauthenticated call used during initial setup.
func ExchangeToken(ctx context.Context, apiServer, token string) (*ExchangeTokenResponse, error) {
	var resp ExchangeTokenResponse
	if err := postJSON(ctx, apiServer+"/api/v1/bots/exchange-token", ExchangeTokenRequest{
		Token: token,
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
