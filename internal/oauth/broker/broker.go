package broker

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"

	"github.com/neboloop/nebo/internal/db"
	mcpclient "github.com/neboloop/nebo/internal/mcp/client"
)

// AppTokenReceiver pushes OAuth tokens to running apps via the settings pipeline.
type AppTokenReceiver interface {
	PushOAuthTokens(appID, provider string, tokens map[string]string) error
}

// Grant is the public view of an OAuth grant (no secrets).
type Grant struct {
	Provider         string     `json:"provider"`
	Scopes           string     `json:"scopes"`
	ConnectionStatus string     `json:"connection_status"`
	ExpiresAt        *time.Time `json:"expires_at,omitempty"`
}

// tokenResponse is the standard OAuth 2.0 token response.
type tokenResponse struct {
	AccessToken  string `json:"access_token"`
	TokenType    string `json:"token_type"`
	ExpiresIn    int    `json:"expires_in"`
	RefreshToken string `json:"refresh_token"`
	Scope        string `json:"scope"`
}

// Broker manages OAuth flows for Nebo apps.
type Broker struct {
	db            *db.Store
	encryptionKey []byte
	providers     map[string]OAuthProvider
	appReceiver   AppTokenReceiver
	baseURL       string
	httpClient    *http.Client
	mu            sync.RWMutex
}

// Config holds the initialization parameters for a Broker.
type Config struct {
	DB            *db.Store
	EncryptionKey []byte
	BaseURL       string // e.g. "http://localhost:27895"
	Providers     map[string]OAuthProvider
}

// New creates a new OAuth Broker.
func New(cfg Config) *Broker {
	return &Broker{
		db:            cfg.DB,
		encryptionKey: cfg.EncryptionKey,
		providers:     cfg.Providers,
		baseURL:       strings.TrimRight(cfg.BaseURL, "/"),
		httpClient:    &http.Client{Timeout: 30 * time.Second},
	}
}

// SetAppReceiver sets the callback for pushing tokens to apps.
// Called after AppRegistry is initialized (avoids circular init).
func (b *Broker) SetAppReceiver(receiver AppTokenReceiver) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.appReceiver = receiver
}

// StartFlow initiates the OAuth flow for an app+provider and returns the authorization URL.
func (b *Broker) StartFlow(ctx context.Context, appID, providerName, scopes string) (string, error) {
	b.mu.RLock()
	provider, ok := b.providers[providerName]
	b.mu.RUnlock()
	if !ok {
		return "", fmt.Errorf("unsupported OAuth provider: %s", providerName)
	}

	if provider.ClientID == "" {
		return "", fmt.Errorf("OAuth provider %s has no client_id configured — set it in config.yaml under AppOAuth", providerName)
	}

	// Generate PKCE
	verifier, challenge, err := mcpclient.GeneratePKCE()
	if err != nil {
		return "", fmt.Errorf("generate PKCE: %w", err)
	}

	// Generate state
	state, err := mcpclient.GenerateState()
	if err != nil {
		return "", fmt.Errorf("generate state: %w", err)
	}

	// Encrypt verifier before storing
	encryptedVerifier, err := mcpclient.EncryptString(verifier, b.encryptionKey)
	if err != nil {
		return "", fmt.Errorf("encrypt verifier: %w", err)
	}

	// Upsert grant with pending state
	if err := b.db.UpsertAppOAuthGrant(ctx, db.UpsertAppOAuthGrantParams{
		ID:               uuid.New().String(),
		AppID:            appID,
		Provider:         providerName,
		Scopes:           scopes,
		OauthState:       sql.NullString{String: state, Valid: true},
		PkceVerifier:     sql.NullString{String: encryptedVerifier, Valid: true},
		ConnectionStatus: "pending",
	}); err != nil {
		return "", fmt.Errorf("store OAuth grant: %w", err)
	}

	// Build authorization URL
	authEndpoint := b.resolveEndpoint(provider.AuthorizationEndpoint, provider.TenantID)
	authURL, err := url.Parse(authEndpoint)
	if err != nil {
		return "", fmt.Errorf("invalid authorization endpoint: %w", err)
	}

	q := authURL.Query()
	q.Set("response_type", "code")
	q.Set("client_id", provider.ClientID)
	q.Set("redirect_uri", b.redirectURI())
	q.Set("state", state)
	q.Set("scope", scopes)
	q.Set("access_type", "offline") // request refresh token (Google)
	q.Set("prompt", "consent")      // force consent to get refresh token

	if provider.SupportsPKCE {
		q.Set("code_challenge", challenge)
		q.Set("code_challenge_method", "S256")
	}

	authURL.RawQuery = q.Encode()
	return authURL.String(), nil
}

// HandleCallback processes the OAuth callback: exchanges the code for tokens,
// encrypts and stores them, then pushes to the app.
func (b *Broker) HandleCallback(ctx context.Context, state, code string) error {
	// Look up grant by state
	grant, err := b.db.GetAppOAuthGrantByState(ctx, sql.NullString{String: state, Valid: true})
	if err != nil {
		return fmt.Errorf("invalid OAuth state: %w", err)
	}

	b.mu.RLock()
	provider, ok := b.providers[grant.Provider]
	b.mu.RUnlock()
	if !ok {
		return fmt.Errorf("unknown provider: %s", grant.Provider)
	}

	// Decrypt PKCE verifier
	verifier := ""
	if grant.PkceVerifier.Valid && grant.PkceVerifier.String != "" {
		verifier, err = mcpclient.DecryptString(grant.PkceVerifier.String, b.encryptionKey)
		if err != nil {
			return fmt.Errorf("decrypt PKCE verifier: %w", err)
		}
	}

	// Exchange code for tokens
	tokenEndpoint := b.resolveEndpoint(provider.TokenEndpoint, provider.TenantID)

	data := url.Values{}
	data.Set("grant_type", "authorization_code")
	data.Set("code", code)
	data.Set("redirect_uri", b.redirectURI())
	data.Set("client_id", provider.ClientID)
	if provider.ClientSecret != "" {
		data.Set("client_secret", provider.ClientSecret)
	}
	if verifier != "" {
		data.Set("code_verifier", verifier)
	}

	req, err := http.NewRequestWithContext(ctx, "POST", tokenEndpoint, strings.NewReader(data.Encode()))
	if err != nil {
		return fmt.Errorf("create token request: %w", err)
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	req.Header.Set("Accept", "application/json")

	resp, err := b.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("token exchange: %w", err)
	}
	defer resp.Body.Close()

	body, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("token exchange failed with status %d: %s", resp.StatusCode, string(body))
	}

	var tokenResp tokenResponse
	if err := json.Unmarshal(body, &tokenResp); err != nil {
		return fmt.Errorf("decode token response: %w", err)
	}

	return b.storeAndPushTokens(ctx, grant.AppID, grant.Provider, &tokenResp, "")
}

// GetGrants returns the connection status for all OAuth providers for an app.
func (b *Broker) GetGrants(ctx context.Context, appID string) ([]Grant, error) {
	grants, err := b.db.ListAppOAuthGrants(ctx, appID)
	if err != nil {
		return nil, err
	}

	result := make([]Grant, len(grants))
	for i, g := range grants {
		result[i] = Grant{
			Provider:         g.Provider,
			Scopes:           g.Scopes,
			ConnectionStatus: g.ConnectionStatus,
		}
		if g.ExpiresAt.Valid {
			t := g.ExpiresAt.Time
			result[i].ExpiresAt = &t
		}
	}
	return result, nil
}

// Disconnect removes an OAuth grant for an app+provider.
func (b *Broker) Disconnect(ctx context.Context, appID, providerName string) error {
	if err := b.db.DeleteAppOAuthGrant(ctx, db.DeleteAppOAuthGrantParams{
		AppID:    appID,
		Provider: providerName,
	}); err != nil {
		return err
	}

	// Push empty tokens to the app so it knows the connection is gone
	b.mu.RLock()
	receiver := b.appReceiver
	b.mu.RUnlock()
	if receiver != nil {
		_ = receiver.PushOAuthTokens(appID, providerName, map[string]string{
			"oauth:" + providerName + ":access_token": "",
			"oauth:" + providerName + ":token_type":   "",
			"oauth:" + providerName + ":expires_at":   "",
		})
	}
	return nil
}

// storeAndPushTokens encrypts tokens, saves to DB, and pushes to the app.
func (b *Broker) storeAndPushTokens(ctx context.Context, appID, providerName string, tokenResp *tokenResponse, existingRefreshToken string) error {
	accessToken := tokenResp.AccessToken
	refreshToken := tokenResp.RefreshToken
	if refreshToken == "" {
		refreshToken = existingRefreshToken // keep existing on refresh
	}

	// Encrypt tokens
	encAccessToken, err := mcpclient.EncryptString(accessToken, b.encryptionKey)
	if err != nil {
		return fmt.Errorf("encrypt access token: %w", err)
	}

	encRefreshToken := ""
	if refreshToken != "" {
		encRefreshToken, err = mcpclient.EncryptString(refreshToken, b.encryptionKey)
		if err != nil {
			return fmt.Errorf("encrypt refresh token: %w", err)
		}
	}

	// Calculate expiry
	var expiresAt sql.NullTime
	if tokenResp.ExpiresIn > 0 {
		expiresAt = sql.NullTime{
			Time:  time.Now().Add(time.Duration(tokenResp.ExpiresIn) * time.Second),
			Valid: true,
		}
	}

	tokenType := tokenResp.TokenType
	if tokenType == "" {
		tokenType = "Bearer"
	}

	// Store encrypted tokens in DB
	if err := b.db.UpdateAppOAuthTokens(ctx, db.UpdateAppOAuthTokensParams{
		AccessToken:  encAccessToken,
		RefreshToken: encRefreshToken,
		TokenType:    tokenType,
		ExpiresAt:    expiresAt,
		AppID:        appID,
		Provider:     providerName,
	}); err != nil {
		return fmt.Errorf("store tokens: %w", err)
	}

	// Push plaintext tokens to the app via Configure RPC
	b.mu.RLock()
	receiver := b.appReceiver
	b.mu.RUnlock()
	if receiver != nil {
		expiresAtStr := ""
		if expiresAt.Valid {
			expiresAtStr = expiresAt.Time.Format(time.RFC3339)
		}
		if err := receiver.PushOAuthTokens(appID, providerName, map[string]string{
			"oauth:" + providerName + ":access_token": accessToken,
			"oauth:" + providerName + ":token_type":   tokenType,
			"oauth:" + providerName + ":expires_at":   expiresAtStr,
		}); err != nil {
			fmt.Printf("[oauth-broker] failed to push tokens to %s: %v\n", appID, err)
		}
	}

	return nil
}

// redirectURI returns the OAuth callback URL for the broker.
func (b *Broker) redirectURI() string {
	return b.baseURL + "/api/v1/apps/oauth/callback"
}

// resolveEndpoint replaces {tenant} placeholder in Microsoft endpoints.
func (b *Broker) resolveEndpoint(endpoint, tenantID string) string {
	if tenantID == "" {
		tenantID = "common"
	}
	return strings.ReplaceAll(endpoint, "{tenant}", tenantID)
}

// PushExistingTokens loads and pushes stored tokens for an app on launch.
// Called when an app starts so it immediately has its OAuth tokens.
func (b *Broker) PushExistingTokens(ctx context.Context, appID string) error {
	grants, err := b.db.ListAppOAuthGrants(ctx, appID)
	if err != nil {
		return err
	}

	b.mu.RLock()
	receiver := b.appReceiver
	b.mu.RUnlock()
	if receiver == nil {
		return nil
	}

	for _, grant := range grants {
		if grant.ConnectionStatus != "connected" || grant.AccessToken == "" {
			continue
		}

		accessToken, err := mcpclient.DecryptString(grant.AccessToken, b.encryptionKey)
		if err != nil {
			fmt.Printf("[oauth-broker] failed to decrypt token for %s/%s: %v\n", appID, grant.Provider, err)
			continue
		}

		expiresAtStr := ""
		if grant.ExpiresAt.Valid {
			expiresAtStr = grant.ExpiresAt.Time.Format(time.RFC3339)
		}

		if err := receiver.PushOAuthTokens(appID, grant.Provider, map[string]string{
			"oauth:" + grant.Provider + ":access_token": accessToken,
			"oauth:" + grant.Provider + ":token_type":   grant.TokenType,
			"oauth:" + grant.Provider + ":expires_at":   expiresAtStr,
		}); err != nil {
			fmt.Printf("[oauth-broker] failed to push existing tokens to %s: %v\n", appID, err)
		}
	}

	return nil
}
