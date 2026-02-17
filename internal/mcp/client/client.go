package client

import (
	"bytes"
	"context"
	"crypto/rand"
	"crypto/sha256"
	"database/sql"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"

	"github.com/google/uuid"

	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/logging"
)

// Client handles OAuth flows for connecting to external MCP servers
type Client struct {
	db            *db.Store
	encryptionKey []byte
	httpClient    *http.Client
	baseURL       string // Nebo's base URL for redirect URIs
}

// ServerMetadata contains OAuth configuration discovered from an MCP server
type ServerMetadata struct {
	Issuer                           string   `json:"issuer"`
	AuthorizationEndpoint            string   `json:"authorization_endpoint"`
	TokenEndpoint                    string   `json:"token_endpoint"`
	RegistrationEndpoint             string   `json:"registration_endpoint,omitempty"`
	RevocationEndpoint               string   `json:"revocation_endpoint,omitempty"`
	ScopesSupported                  []string `json:"scopes_supported,omitempty"`
	ResponseTypesSupported           []string `json:"response_types_supported,omitempty"`
	CodeChallengeMethodsSupported    []string `json:"code_challenge_methods_supported,omitempty"`
	TokenEndpointAuthMethodsSupported []string `json:"token_endpoint_auth_methods_supported,omitempty"`
}

// TokenResponse is the response from the token endpoint
type TokenResponse struct {
	AccessToken  string `json:"access_token"`
	TokenType    string `json:"token_type"`
	ExpiresIn    int    `json:"expires_in,omitempty"`
	RefreshToken string `json:"refresh_token,omitempty"`
	Scope        string `json:"scope,omitempty"`
}

// OAuthError represents an OAuth error response
type OAuthError struct {
	Error            string `json:"error"`
	ErrorDescription string `json:"error_description,omitempty"`
}

// NewClient creates a new MCP OAuth client
func NewClient(database *db.Store, encryptionKey []byte, baseURL string) *Client {
	return &Client{
		db:            database,
		encryptionKey: encryptionKey,
		baseURL:       baseURL,
		httpClient: &http.Client{
			Timeout: 30 * time.Second,
		},
	}
}

// Discover fetches OAuth metadata from an MCP server using the well-known endpoint
func (c *Client) Discover(ctx context.Context, serverURL string) (*ServerMetadata, error) {
	// Parse the server URL
	u, err := url.Parse(serverURL)
	if err != nil {
		return nil, fmt.Errorf("invalid server URL: %w", err)
	}

	// Build the well-known URL
	wellKnownURL := fmt.Sprintf("%s://%s/.well-known/oauth-authorization-server", u.Scheme, u.Host)

	req, err := http.NewRequestWithContext(ctx, "GET", wellKnownURL, nil)
	if err != nil {
		return nil, fmt.Errorf("failed to create request: %w", err)
	}
	req.Header.Set("Accept", "application/json")

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("failed to fetch metadata: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		return nil, fmt.Errorf("metadata request failed with status %d: %s", resp.StatusCode, string(body))
	}

	var metadata ServerMetadata
	if err := json.NewDecoder(resp.Body).Decode(&metadata); err != nil {
		return nil, fmt.Errorf("failed to decode metadata: %w", err)
	}

	return &metadata, nil
}

// GeneratePKCE generates a PKCE code verifier and S256 challenge
func GeneratePKCE() (verifier, challenge string, err error) {
	// Generate 32 random bytes for the verifier
	verifierBytes := make([]byte, 32)
	if _, err := rand.Read(verifierBytes); err != nil {
		return "", "", fmt.Errorf("failed to generate verifier: %w", err)
	}

	// Base64 URL encode the verifier (no padding)
	verifier = base64.RawURLEncoding.EncodeToString(verifierBytes)

	// Create S256 challenge: BASE64URL(SHA256(verifier))
	hash := sha256.Sum256([]byte(verifier))
	challenge = base64.RawURLEncoding.EncodeToString(hash[:])

	return verifier, challenge, nil
}

// GenerateState generates a random state parameter for CSRF protection
func GenerateState() (string, error) {
	stateBytes := make([]byte, 16)
	if _, err := rand.Read(stateBytes); err != nil {
		return "", fmt.Errorf("failed to generate state: %w", err)
	}
	return base64.RawURLEncoding.EncodeToString(stateBytes), nil
}

// StartOAuthFlow initiates the OAuth flow for an integration and returns the authorization URL
func (c *Client) StartOAuthFlow(ctx context.Context, integrationID string) (string, error) {
	// Get the integration
	integration, err := c.db.GetMCPIntegration(ctx, integrationID)
	if err != nil {
		return "", fmt.Errorf("failed to get integration: %w", err)
	}

	if integration.AuthType != "oauth" {
		return "", fmt.Errorf("integration %s does not use OAuth authentication", integrationID)
	}

	// Get the server URL
	serverURL := integration.ServerUrl.String
	if serverURL == "" {
		// Try to get from registry
		registry, err := c.db.GetMCPServerRegistry(ctx, integration.ServerType)
		if err == nil && registry.DefaultServerUrl.Valid {
			serverURL = registry.DefaultServerUrl.String
		}
	}

	if serverURL == "" {
		return "", fmt.Errorf("no server URL configured for integration %s", integrationID)
	}

	// Discover OAuth metadata
	metadata, err := c.Discover(ctx, serverURL)
	if err != nil {
		return "", fmt.Errorf("failed to discover OAuth metadata: %w", err)
	}

	// Generate PKCE
	verifier, challenge, err := GeneratePKCE()
	if err != nil {
		return "", fmt.Errorf("failed to generate PKCE: %w", err)
	}

	// Generate state
	state, err := GenerateState()
	if err != nil {
		return "", fmt.Errorf("failed to generate state: %w", err)
	}

	// Encrypt the verifier before storing
	encryptedVerifier, err := EncryptString(verifier, c.encryptionKey)
	if err != nil {
		return "", fmt.Errorf("failed to encrypt verifier: %w", err)
	}

	// Get or create client credentials
	clientID, clientSecret, err := c.getOrCreateClientCredentials(ctx, integrationID, metadata)
	if err != nil {
		return "", fmt.Errorf("failed to get client credentials: %w", err)
	}

	// Encrypt client secret if present
	encryptedClientSecret := ""
	if clientSecret != "" {
		encryptedClientSecret, err = EncryptString(clientSecret, c.encryptionKey)
		if err != nil {
			return "", fmt.Errorf("failed to encrypt client secret: %w", err)
		}
	}

	// Store OAuth flow state in database
	err = c.db.UpdateMCPIntegrationOAuthFlow(ctx, db.UpdateMCPIntegrationOAuthFlowParams{
		OauthState:                 sql.NullString{String: state, Valid: true},
		OauthPkceVerifier:          sql.NullString{String: encryptedVerifier, Valid: true},
		OauthClientID:              sql.NullString{String: clientID, Valid: true},
		OauthClientSecret:          sql.NullString{String: encryptedClientSecret, Valid: encryptedClientSecret != ""},
		OauthAuthorizationEndpoint: sql.NullString{String: metadata.AuthorizationEndpoint, Valid: true},
		OauthTokenEndpoint:         sql.NullString{String: metadata.TokenEndpoint, Valid: true},
		ID:                         integrationID,
	})
	if err != nil {
		return "", fmt.Errorf("failed to store OAuth flow state: %w", err)
	}

	// Build authorization URL
	redirectURI := c.getRedirectURI()
	authURL, err := url.Parse(metadata.AuthorizationEndpoint)
	if err != nil {
		return "", fmt.Errorf("invalid authorization endpoint: %w", err)
	}

	// Get scopes from registry
	scopes := "mcp:full"
	registry, err := c.db.GetMCPServerRegistry(ctx, integration.ServerType)
	if err == nil && registry.OauthScopes.Valid && registry.OauthScopes.String != "" {
		scopes = registry.OauthScopes.String
	}

	// Add required OAuth parameters
	q := authURL.Query()
	q.Set("response_type", "code")
	q.Set("client_id", clientID)
	q.Set("redirect_uri", redirectURI)
	q.Set("state", state)
	q.Set("code_challenge", challenge)
	q.Set("code_challenge_method", "S256")
	q.Set("scope", scopes)
	authURL.RawQuery = q.Encode()

	logging.Infof("Starting OAuth flow for integration %s, auth URL: %s", integrationID, authURL.String())

	return authURL.String(), nil
}

// getOrCreateClientCredentials gets existing client credentials or dynamically registers
func (c *Client) getOrCreateClientCredentials(ctx context.Context, integrationID string, metadata *ServerMetadata) (clientID, clientSecret string, err error) {
	// Check if we have stored client credentials
	integration, err := c.db.GetMCPIntegration(ctx, integrationID)
	if err != nil {
		return "", "", err
	}

	// If we have stored credentials, decrypt and return them
	if integration.OauthClientID.Valid && integration.OauthClientID.String != "" {
		clientID = integration.OauthClientID.String
		if integration.OauthClientSecret.Valid && integration.OauthClientSecret.String != "" {
			clientSecret, err = DecryptString(integration.OauthClientSecret.String, c.encryptionKey)
			if err != nil {
				logging.Warnf("Failed to decrypt client secret, will re-register: %v", err)
			} else {
				return clientID, clientSecret, nil
			}
		} else {
			// Public client, no secret needed
			return clientID, "", nil
		}
	}

	// Try to get pre-configured client credentials from registry
	registry, err := c.db.GetMCPServerRegistry(ctx, integration.ServerType)
	if err == nil {
		// Some servers have pre-registered clients (configured in registry)
		// For now, we'll use dynamic client registration
	}

	// Dynamic Client Registration if endpoint available
	if metadata.RegistrationEndpoint != "" {
		clientID, clientSecret, err = c.dynamicClientRegistration(ctx, metadata.RegistrationEndpoint, integrationID)
		if err != nil {
			logging.Warnf("Dynamic client registration failed: %v", err)
			// Fall back to a default public client ID if registration fails
			clientID = "nebo-agent-" + integrationID
			clientSecret = ""
		}
		return clientID, clientSecret, nil
	}

	// No registration endpoint, use a default public client ID
	clientID = "nebo-agent-" + integrationID
	_ = registry // suppress unused warning
	return clientID, "", nil
}

// dynamicClientRegistration performs OAuth 2.0 Dynamic Client Registration
func (c *Client) dynamicClientRegistration(ctx context.Context, registrationEndpoint, integrationID string) (clientID, clientSecret string, err error) {
	redirectURI := c.getRedirectURI()

	regRequest := map[string]interface{}{
		"client_name":                "Nebo Agent",
		"redirect_uris":              []string{redirectURI},
		"token_endpoint_auth_method": "none", // Public client
		"grant_types":                []string{"authorization_code", "refresh_token"},
		"response_types":             []string{"code"},
		"scope":                      "mcp:full offline_access",
	}

	body, err := json.Marshal(regRequest)
	if err != nil {
		return "", "", fmt.Errorf("failed to marshal registration request: %w", err)
	}

	req, err := http.NewRequestWithContext(ctx, "POST", registrationEndpoint, bytes.NewReader(body))
	if err != nil {
		return "", "", fmt.Errorf("failed to create registration request: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json")

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return "", "", fmt.Errorf("registration request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK && resp.StatusCode != http.StatusCreated {
		body, _ := io.ReadAll(resp.Body)
		return "", "", fmt.Errorf("registration failed with status %d: %s", resp.StatusCode, string(body))
	}

	var regResponse struct {
		ClientID     string `json:"client_id"`
		ClientSecret string `json:"client_secret,omitempty"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&regResponse); err != nil {
		return "", "", fmt.Errorf("failed to decode registration response: %w", err)
	}

	logging.Infof("Dynamic client registration successful, client_id: %s", regResponse.ClientID)

	return regResponse.ClientID, regResponse.ClientSecret, nil
}

// ExchangeCode exchanges an authorization code for tokens
func (c *Client) ExchangeCode(ctx context.Context, integrationID, code string) error {
	// Get OAuth config from integration
	config, err := c.db.GetMCPIntegrationOAuthConfig(ctx, integrationID)
	if err != nil {
		return fmt.Errorf("failed to get OAuth config: %w", err)
	}

	if !config.OauthTokenEndpoint.Valid || config.OauthTokenEndpoint.String == "" {
		return fmt.Errorf("no token endpoint configured")
	}

	// Decrypt the PKCE verifier
	verifier := ""
	if config.OauthPkceVerifier.Valid && config.OauthPkceVerifier.String != "" {
		verifier, err = DecryptString(config.OauthPkceVerifier.String, c.encryptionKey)
		if err != nil {
			return fmt.Errorf("failed to decrypt PKCE verifier: %w", err)
		}
	}

	// Get client credentials
	clientID := config.OauthClientID.String
	clientSecret := ""
	if config.OauthClientSecret.Valid && config.OauthClientSecret.String != "" {
		clientSecret, err = DecryptString(config.OauthClientSecret.String, c.encryptionKey)
		if err != nil {
			logging.Warnf("Failed to decrypt client secret: %v", err)
		}
	}

	// Build token request
	data := url.Values{}
	data.Set("grant_type", "authorization_code")
	data.Set("code", code)
	data.Set("redirect_uri", c.getRedirectURI())
	data.Set("client_id", clientID)
	if verifier != "" {
		data.Set("code_verifier", verifier)
	}

	req, err := http.NewRequestWithContext(ctx, "POST", config.OauthTokenEndpoint.String, strings.NewReader(data.Encode()))
	if err != nil {
		return fmt.Errorf("failed to create token request: %w", err)
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	req.Header.Set("Accept", "application/json")

	// Add Basic auth if we have a client secret
	if clientSecret != "" {
		req.SetBasicAuth(clientID, clientSecret)
	}

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("token request failed: %w", err)
	}
	defer resp.Body.Close()

	body, _ := io.ReadAll(resp.Body)

	if resp.StatusCode != http.StatusOK {
		var oauthErr OAuthError
		if json.Unmarshal(body, &oauthErr) == nil && oauthErr.Error != "" {
			return fmt.Errorf("token exchange failed: %s - %s", oauthErr.Error, oauthErr.ErrorDescription)
		}
		return fmt.Errorf("token exchange failed with status %d: %s", resp.StatusCode, string(body))
	}

	var tokenResp TokenResponse
	if err := json.Unmarshal(body, &tokenResp); err != nil {
		return fmt.Errorf("failed to decode token response: %w", err)
	}

	// Store the tokens
	return c.storeTokens(ctx, integrationID, &tokenResp)
}

// storeTokens stores OAuth tokens in the database
func (c *Client) storeTokens(ctx context.Context, integrationID string, tokens *TokenResponse) error {
	// Encrypt the access token
	encryptedAccessToken, err := EncryptString(tokens.AccessToken, c.encryptionKey)
	if err != nil {
		return fmt.Errorf("failed to encrypt access token: %w", err)
	}

	// Encrypt the refresh token if present
	var encryptedRefreshToken sql.NullString
	if tokens.RefreshToken != "" {
		encrypted, err := EncryptString(tokens.RefreshToken, c.encryptionKey)
		if err != nil {
			return fmt.Errorf("failed to encrypt refresh token: %w", err)
		}
		encryptedRefreshToken = sql.NullString{String: encrypted, Valid: true}
	}

	// Calculate expiration time
	var expiresAt sql.NullInt64
	if tokens.ExpiresIn > 0 {
		expiresAt = sql.NullInt64{Int64: time.Now().Add(time.Duration(tokens.ExpiresIn) * time.Second).Unix(), Valid: true}
	}

	// Delete existing credentials
	if err := c.db.DeleteMCPIntegrationCredentials(ctx, integrationID); err != nil {
		logging.Warnf("Failed to delete existing credentials: %v", err)
	}

	// Create new credential
	_, err = c.db.CreateMCPIntegrationCredential(ctx, db.CreateMCPIntegrationCredentialParams{
		ID:              uuid.New().String(),
		IntegrationID:   integrationID,
		CredentialType:  "oauth_token",
		CredentialValue: encryptedAccessToken,
		RefreshToken:    encryptedRefreshToken,
		ExpiresAt:       expiresAt,
		Scopes:          sql.NullString{String: tokens.Scope, Valid: tokens.Scope != ""},
	})
	if err != nil {
		return fmt.Errorf("failed to store credentials: %w", err)
	}

	logging.Infof("Stored OAuth tokens for integration %s, expires in %d seconds", integrationID, tokens.ExpiresIn)

	return nil
}

// RefreshToken refreshes an expired access token
func (c *Client) RefreshToken(ctx context.Context, integrationID string) error {
	// Get current credentials
	creds, err := c.db.GetMCPIntegrationCredential(ctx, integrationID)
	if err != nil {
		return fmt.Errorf("failed to get credentials: %w", err)
	}

	if !creds.RefreshToken.Valid || creds.RefreshToken.String == "" {
		return fmt.Errorf("no refresh token available")
	}

	// Decrypt refresh token
	refreshToken, err := DecryptString(creds.RefreshToken.String, c.encryptionKey)
	if err != nil {
		return fmt.Errorf("failed to decrypt refresh token: %w", err)
	}

	// Get OAuth config
	config, err := c.db.GetMCPIntegrationOAuthConfig(ctx, integrationID)
	if err != nil {
		return fmt.Errorf("failed to get OAuth config: %w", err)
	}

	if !config.OauthTokenEndpoint.Valid || config.OauthTokenEndpoint.String == "" {
		return fmt.Errorf("no token endpoint configured")
	}

	// Get client credentials
	clientID := config.OauthClientID.String
	clientSecret := ""
	if config.OauthClientSecret.Valid && config.OauthClientSecret.String != "" {
		clientSecret, err = DecryptString(config.OauthClientSecret.String, c.encryptionKey)
		if err != nil {
			logging.Warnf("Failed to decrypt client secret: %v", err)
		}
	}

	// Build refresh request
	data := url.Values{}
	data.Set("grant_type", "refresh_token")
	data.Set("refresh_token", refreshToken)
	data.Set("client_id", clientID)

	req, err := http.NewRequestWithContext(ctx, "POST", config.OauthTokenEndpoint.String, strings.NewReader(data.Encode()))
	if err != nil {
		return fmt.Errorf("failed to create refresh request: %w", err)
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	req.Header.Set("Accept", "application/json")

	// Add Basic auth if we have a client secret
	if clientSecret != "" {
		req.SetBasicAuth(clientID, clientSecret)
	}

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("refresh request failed: %w", err)
	}
	defer resp.Body.Close()

	body, _ := io.ReadAll(resp.Body)

	if resp.StatusCode != http.StatusOK {
		var oauthErr OAuthError
		if json.Unmarshal(body, &oauthErr) == nil && oauthErr.Error != "" {
			return fmt.Errorf("token refresh failed: %s - %s", oauthErr.Error, oauthErr.ErrorDescription)
		}
		return fmt.Errorf("token refresh failed with status %d: %s", resp.StatusCode, string(body))
	}

	var tokenResp TokenResponse
	if err := json.Unmarshal(body, &tokenResp); err != nil {
		return fmt.Errorf("failed to decode token response: %w", err)
	}

	// Preserve the refresh token if not returned in response
	if tokenResp.RefreshToken == "" {
		tokenResp.RefreshToken = refreshToken
	}

	return c.storeTokens(ctx, integrationID, &tokenResp)
}

// GetAccessToken retrieves and optionally refreshes the access token for an integration
func (c *Client) GetAccessToken(ctx context.Context, integrationID string) (string, error) {
	creds, err := c.db.GetMCPIntegrationCredential(ctx, integrationID)
	if err != nil {
		return "", fmt.Errorf("failed to get credentials: %w", err)
	}

	if creds.CredentialType != "oauth_token" {
		// Not an OAuth credential — decrypt API key (strip enc: prefix if present)
		raw := strings.TrimPrefix(creds.CredentialValue, "enc:")
		return DecryptString(raw, c.encryptionKey)
	}

	// Check if token is expired
	if creds.ExpiresAt.Valid && time.Now().Unix() > creds.ExpiresAt.Int64-60 {
		// Token expired or expiring soon, try to refresh
		if creds.RefreshToken.Valid && creds.RefreshToken.String != "" {
			if err := c.RefreshToken(ctx, integrationID); err != nil {
				logging.Warnf("Failed to refresh token: %v", err)
			} else {
				// Get the new credentials
				creds, err = c.db.GetMCPIntegrationCredential(ctx, integrationID)
				if err != nil {
					return "", fmt.Errorf("failed to get refreshed credentials: %w", err)
				}
			}
		}
	}

	// Decrypt and return the access token
	return DecryptString(creds.CredentialValue, c.encryptionKey)
}

// Disconnect revokes tokens and clears credentials for an integration
func (c *Client) Disconnect(ctx context.Context, integrationID string) error {
	// TODO: Implement token revocation if the server supports it

	// Delete credentials
	if err := c.db.DeleteMCPIntegrationCredentials(ctx, integrationID); err != nil {
		return fmt.Errorf("failed to delete credentials: %w", err)
	}

	// Clear OAuth state
	if err := c.db.ClearMCPIntegrationOAuthState(ctx, integrationID); err != nil {
		return fmt.Errorf("failed to clear OAuth state: %w", err)
	}

	// Update connection status
	if err := c.db.UpdateMCPIntegrationConnectionStatus(ctx, db.UpdateMCPIntegrationConnectionStatusParams{
		ConnectionStatus: sql.NullString{String: "disconnected", Valid: true},
		Column2:          "disconnected", // This is used in the CASE statement
		LastError:        sql.NullString{},
		ID:               integrationID,
	}); err != nil {
		return fmt.Errorf("failed to update status: %w", err)
	}

	return nil
}

// getRedirectURI returns the OAuth redirect URI for this Nebo instance
func (c *Client) getRedirectURI() string {
	return c.baseURL + "/api/v1/integrations/oauth/callback"
}
