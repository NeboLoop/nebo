package oauth

import (
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"database/sql"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"strings"
	"time"

	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/mcp/mcpauth"
	"github.com/nebolabs/nebo/internal/svc"

	"github.com/go-chi/chi/v5"
	"golang.org/x/crypto/bcrypt"
)

// Handler provides MCP OAuth endpoints
type Handler struct {
	svc     *svc.ServiceContext
	baseURL string
}

// NewHandler creates a new MCP OAuth handler
func NewHandler(svc *svc.ServiceContext, baseURL string) *Handler {
	return &Handler{
		svc:     svc,
		baseURL: strings.TrimSuffix(baseURL, "/"),
	}
}

// RegisterRoutes registers all OAuth routes on the chi router
func (h *Handler) RegisterRoutes(r chi.Router) {
	// Well-known metadata endpoints
	r.Get("/.well-known/oauth-protected-resource", h.HandleProtectedResourceMetadata)
	r.Get("/.well-known/oauth-authorization-server", h.HandleAuthServerMetadata)

	// Dynamic Client Registration
	r.Post("/mcp/oauth/register", h.HandleClientRegistration)

	// OAuth endpoints
	r.Get("/mcp/oauth/authorize", h.HandleAuthorize)
	r.Post("/mcp/oauth/authorize", h.HandleAuthorizeSubmit)
	r.Post("/mcp/oauth/token", h.HandleToken)
}

// setCORSHeaders sets CORS headers for OAuth endpoints (browser-based clients like ChatGPT)
func (h *Handler) setCORSHeaders(w http.ResponseWriter) {
	w.Header().Set("Access-Control-Allow-Origin", "*")
	w.Header().Set("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
	w.Header().Set("Access-Control-Allow-Headers", "Content-Type, Authorization")
	w.Header().Set("Access-Control-Max-Age", "86400")
}

// handleProtectedResourceMetadata returns the protected resource metadata
// Per RFC 9728, authorization_servers points to the OAuth server (at root, not /mcp)
func (h *Handler) HandleProtectedResourceMetadata(w http.ResponseWriter, r *http.Request) {
	h.setCORSHeaders(w)
	if r.Method == http.MethodOptions {
		w.WriteHeader(http.StatusOK)
		return
	}

	metadata := ProtectedResourceMetadata{
		Resource:               h.baseURL + "/mcp",
		AuthorizationServers:   []string{h.baseURL}, // Root URL, not /mcp
		ScopesSupported:        []string{"mcp:full"},
		BearerMethodsSupported: []string{"header"},
	}

	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store, no-cache, must-revalidate")
	json.NewEncoder(w).Encode(metadata)
}

// handleAuthServerMetadata returns the authorization server metadata
// Uses root-level URLs for issuer and endpoints (matching working MCP OAuth servers)
func (h *Handler) HandleAuthServerMetadata(w http.ResponseWriter, r *http.Request) {
	h.setCORSHeaders(w)
	if r.Method == http.MethodOptions {
		w.WriteHeader(http.StatusOK)
		return
	}

	metadata := AuthorizationServerMetadata{
		Issuer:                            h.baseURL, // Root URL, not /mcp
		AuthorizationEndpoint:             h.baseURL + "/authorize",
		TokenEndpoint:                     h.baseURL + "/token",
		RegistrationEndpoint:              h.baseURL + "/register",
		ScopesSupported:                   []string{"mcp:full", "offline_access"},
		ResponseTypesSupported:            []string{"code"},
		ResponseModesSupported:            []string{"query"},
		GrantTypesSupported:               []string{"authorization_code", "refresh_token"},
		TokenEndpointAuthMethodsSupported: []string{"client_secret_basic", "client_secret_post", "none"},
		CodeChallengeMethodsSupported:     []string{"S256"},
	}

	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store, no-cache, must-revalidate")
	json.NewEncoder(w).Encode(metadata)
}

// HandleJWKS returns an empty JWKS (we use opaque tokens, not JWTs)
func (h *Handler) HandleJWKS(w http.ResponseWriter, r *http.Request) {
	jwks := map[string]interface{}{
		"keys": []interface{}{},
	}
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "public, max-age=3600")
	json.NewEncoder(w).Encode(jwks)
}

// handleClientRegistration handles Dynamic Client Registration (DCR)
func (h *Handler) HandleClientRegistration(w http.ResponseWriter, r *http.Request) {
	h.setCORSHeaders(w)
	if r.Method == http.MethodOptions {
		w.WriteHeader(http.StatusOK)
		return
	}

	var req ClientRegistrationRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		h.sendError(w, http.StatusBadRequest, ErrInvalidRequest, "Invalid request body")
		return
	}

	// Validate required fields
	if req.ClientName == "" {
		h.sendError(w, http.StatusBadRequest, ErrInvalidRequest, "client_name is required")
		return
	}
	if len(req.RedirectURIs) == 0 {
		h.sendError(w, http.StatusBadRequest, ErrInvalidRequest, "redirect_uris is required")
		return
	}

	// Generate client credentials
	clientID := generateSecureToken(16)
	clientSecret := generateSecureToken(32)
	clientSecretHash := mcpauth.HashToken(clientSecret)

	// Set defaults
	if len(req.GrantTypes) == 0 {
		req.GrantTypes = []string{"authorization_code", "refresh_token"}
	}
	if len(req.ResponseTypes) == 0 {
		req.ResponseTypes = []string{"code"}
	}
	if req.TokenEndpointAuthMethod == "" {
		req.TokenEndpointAuthMethod = "client_secret_post"
	}
	if req.Scope == "" {
		req.Scope = "mcp:full"
	}

	// Store client in database
	isConfidential := int64(0)
	if req.TokenEndpointAuthMethod != "none" {
		isConfidential = 1
	}
	_, err := h.svc.DB.CreateMCPOAuthClient(r.Context(), db.CreateMCPOAuthClientParams{
		ID:               generateSecureToken(16),
		ClientID:         clientID,
		ClientSecretHash: clientSecretHash,
		Name:             req.ClientName,
		Description:      sql.NullString{String: "", Valid: false},
		RedirectUris:     strings.Join(req.RedirectURIs, ","),
		Scopes:           sql.NullString{String: req.Scope, Valid: req.Scope != ""},
		IsConfidential:   sql.NullInt64{Int64: isConfidential, Valid: true},
	})
	if err != nil {
		h.sendError(w, http.StatusInternalServerError, ErrServerError, "Failed to register client")
		return
	}

	resp := ClientRegistrationResponse{
		ClientID:                clientID,
		ClientSecret:            clientSecret,
		ClientName:              req.ClientName,
		RedirectURIs:            req.RedirectURIs,
		TokenEndpointAuthMethod: req.TokenEndpointAuthMethod,
		GrantTypes:              req.GrantTypes,
		ResponseTypes:           req.ResponseTypes,
		Scope:                   req.Scope,
		ClientIDIssuedAt:        time.Now().Unix(),
		ClientSecretExpiresAt:   0, // Never expires
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusCreated)
	json.NewEncoder(w).Encode(resp)
}

// handleAuthorize shows the login/authorization page
func (h *Handler) HandleAuthorize(w http.ResponseWriter, r *http.Request) {
	query := r.URL.Query()

	req := AuthorizationRequest{
		ResponseType:        query.Get("response_type"),
		ClientID:            query.Get("client_id"),
		RedirectURI:         query.Get("redirect_uri"),
		Scope:               query.Get("scope"),
		State:               query.Get("state"),
		CodeChallenge:       query.Get("code_challenge"),
		CodeChallengeMethod: query.Get("code_challenge_method"),
	}

	// Validate response_type
	if req.ResponseType != "code" {
		h.sendError(w, http.StatusBadRequest, ErrUnsupportedResponseType, "Only 'code' response type is supported")
		return
	}

	// Validate client
	client, err := h.svc.DB.GetMCPOAuthClientByClientID(r.Context(), req.ClientID)
	if err != nil {
		h.sendError(w, http.StatusBadRequest, ErrInvalidClient, "Unknown client_id")
		return
	}

	// Validate redirect_uri
	validRedirect := false
	for _, uri := range strings.Split(client.RedirectUris, ",") {
		if uri == req.RedirectURI {
			validRedirect = true
			break
		}
	}
	if !validRedirect {
		h.sendError(w, http.StatusBadRequest, ErrInvalidRequest, "Invalid redirect_uri")
		return
	}

	// Validate PKCE (required for MCP)
	if req.CodeChallenge == "" {
		h.redirectError(w, r, req.RedirectURI, req.State, ErrInvalidRequest, "code_challenge is required")
		return
	}
	if req.CodeChallengeMethod != "S256" {
		h.redirectError(w, r, req.RedirectURI, req.State, ErrInvalidRequest, "Only S256 code_challenge_method is supported")
		return
	}

	// Render login page
	h.renderLoginPage(w, req, client.Name, "")
}

// handleAuthorizeSubmit processes the login form submission
func (h *Handler) HandleAuthorizeSubmit(w http.ResponseWriter, r *http.Request) {
	if err := r.ParseForm(); err != nil {
		h.sendError(w, http.StatusBadRequest, ErrInvalidRequest, "Invalid form data")
		return
	}

	// Get OAuth params from form (hidden fields)
	req := AuthorizationRequest{
		ResponseType:        r.FormValue("response_type"),
		ClientID:            r.FormValue("client_id"),
		RedirectURI:         r.FormValue("redirect_uri"),
		Scope:               r.FormValue("scope"),
		State:               r.FormValue("state"),
		CodeChallenge:       r.FormValue("code_challenge"),
		CodeChallengeMethod: r.FormValue("code_challenge_method"),
	}

	email := r.FormValue("email")
	password := r.FormValue("password")

	// Validate client again
	client, err := h.svc.DB.GetMCPOAuthClientByClientID(r.Context(), req.ClientID)
	if err != nil {
		h.sendError(w, http.StatusBadRequest, ErrInvalidClient, "Unknown client_id")
		return
	}

	// Authenticate user
	user, err := h.svc.DB.GetUserByEmail(r.Context(), email)
	if err != nil {
		h.renderLoginPage(w, req, client.Name, "Invalid email or password")
		return
	}

	// Verify password
	if err := bcrypt.CompareHashAndPassword([]byte(user.PasswordHash), []byte(password)); err != nil {
		h.renderLoginPage(w, req, client.Name, "Invalid email or password")
		return
	}

	// Generate authorization code
	code := generateSecureToken(32)
	codeHash := mcpauth.HashToken(code)

	// Store authorization code
	_, err = h.svc.DB.CreateMCPOAuthCode(r.Context(), db.CreateMCPOAuthCodeParams{
		ID:                  generateSecureToken(16),
		ClientID:            client.ID,
		UserID:              user.ID,
		CodeHash:            codeHash,
		RedirectUri:         req.RedirectURI,
		Scopes:              req.Scope,
		CodeChallenge:       sql.NullString{String: req.CodeChallenge, Valid: req.CodeChallenge != ""},
		CodeChallengeMethod: sql.NullString{String: req.CodeChallengeMethod, Valid: req.CodeChallengeMethod != ""},
		ExpiresAt:           time.Now().Add(AuthCodeTTL).Format(time.RFC3339),
	})
	if err != nil {
		h.redirectError(w, r, req.RedirectURI, req.State, ErrServerError, "Failed to create authorization code")
		return
	}

	// Redirect back with code
	redirectURL, _ := url.Parse(req.RedirectURI)
	q := redirectURL.Query()
	q.Set("code", code)
	if req.State != "" {
		q.Set("state", req.State)
	}
	redirectURL.RawQuery = q.Encode()

	http.Redirect(w, r, redirectURL.String(), http.StatusFound)
}

// handleToken handles token exchange and refresh
func (h *Handler) HandleToken(w http.ResponseWriter, r *http.Request) {
	h.setCORSHeaders(w)
	if r.Method == http.MethodOptions {
		w.WriteHeader(http.StatusOK)
		return
	}

	if err := r.ParseForm(); err != nil {
		h.sendError(w, http.StatusBadRequest, ErrInvalidRequest, "Invalid form data")
		return
	}

	req := TokenRequest{
		GrantType:    r.FormValue("grant_type"),
		Code:         r.FormValue("code"),
		RedirectURI:  r.FormValue("redirect_uri"),
		RefreshToken: r.FormValue("refresh_token"),
		ClientID:     r.FormValue("client_id"),
		ClientSecret: r.FormValue("client_secret"),
		CodeVerifier: r.FormValue("code_verifier"),
	}

	// Support client_secret_basic: extract credentials from Authorization header
	if authHeader := r.Header.Get("Authorization"); strings.HasPrefix(authHeader, "Basic ") {
		if decoded, err := base64.StdEncoding.DecodeString(strings.TrimPrefix(authHeader, "Basic ")); err == nil {
			if parts := strings.SplitN(string(decoded), ":", 2); len(parts) == 2 {
				// URL decode the parts (client_id and secret may be URL-encoded)
				if clientID, err := url.QueryUnescape(parts[0]); err == nil {
					req.ClientID = clientID
				}
				if clientSecret, err := url.QueryUnescape(parts[1]); err == nil {
					req.ClientSecret = clientSecret
				}
			}
		}
	}

	switch req.GrantType {
	case "authorization_code":
		h.handleAuthorizationCodeGrant(w, r, req)
	case "refresh_token":
		h.handleRefreshTokenGrant(w, r, req)
	default:
		h.sendError(w, http.StatusBadRequest, ErrUnsupportedGrantType, "Unsupported grant_type")
	}
}

func (h *Handler) handleAuthorizationCodeGrant(w http.ResponseWriter, r *http.Request, req TokenRequest) {
	// Validate required fields
	if req.Code == "" {
		h.sendError(w, http.StatusBadRequest, ErrInvalidRequest, "code is required")
		return
	}
	if req.ClientID == "" {
		h.sendError(w, http.StatusBadRequest, ErrInvalidRequest, "client_id is required")
		return
	}

	// Get authorization code
	codeHash := mcpauth.HashToken(req.Code)
	authCode, err := h.svc.DB.GetMCPOAuthCodeByHash(r.Context(), codeHash)
	if err != nil {
		h.sendError(w, http.StatusBadRequest, ErrInvalidGrant, "Invalid authorization code")
		return
	}

	// Verify client
	if authCode.OauthClientID != req.ClientID {
		h.sendError(w, http.StatusBadRequest, ErrInvalidClient, "Client mismatch")
		return
	}

	// Verify redirect_uri matches
	if authCode.RedirectUri != req.RedirectURI {
		h.sendError(w, http.StatusBadRequest, ErrInvalidRequest, "redirect_uri mismatch")
		return
	}

	// Verify PKCE
	if authCode.CodeChallenge.Valid {
		if req.CodeVerifier == "" {
			h.sendError(w, http.StatusBadRequest, ErrInvalidRequest, "code_verifier is required")
			return
		}
		if !verifyPKCE(req.CodeVerifier, authCode.CodeChallenge.String, authCode.CodeChallengeMethod.String) {
			h.sendError(w, http.StatusBadRequest, ErrInvalidGrant, "Invalid code_verifier")
			return
		}
	}

	// Mark code as used
	if err := h.svc.DB.MarkMCPOAuthCodeUsed(r.Context(), authCode.ID); err != nil {
		h.sendError(w, http.StatusInternalServerError, ErrServerError, "Failed to process authorization code")
		return
	}

	// Generate tokens
	accessToken := generateSecureToken(32)
	refreshToken := generateSecureToken(32)
	accessTokenHash := mcpauth.HashToken(accessToken)
	refreshTokenHash := mcpauth.HashToken(refreshToken)

	expiresAt := time.Now().Add(AccessTokenTTL)
	refreshExpiresAt := time.Now().Add(RefreshTokenTTL)

	// Store tokens
	_, err = h.svc.DB.CreateMCPOAuthToken(r.Context(), db.CreateMCPOAuthTokenParams{
		ID:               generateSecureToken(16),
		ClientID:         authCode.ClientID,
		UserID:           authCode.UserID,
		AccessTokenHash:  accessTokenHash,
		RefreshTokenHash: sql.NullString{String: refreshTokenHash, Valid: true},
		Scopes:           authCode.Scopes,
		ExpiresAt:        expiresAt.Format(time.RFC3339),
		RefreshExpiresAt: sql.NullString{String: refreshExpiresAt.Format(time.RFC3339), Valid: true},
	})
	if err != nil {
		h.sendError(w, http.StatusInternalServerError, ErrServerError, "Failed to create tokens")
		return
	}

	resp := TokenResponse{
		AccessToken:  accessToken,
		TokenType:    "Bearer",
		ExpiresIn:    int(AccessTokenTTL.Seconds()),
		RefreshToken: refreshToken,
		Scope:        authCode.Scopes,
	}

	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	json.NewEncoder(w).Encode(resp)
}

func (h *Handler) handleRefreshTokenGrant(w http.ResponseWriter, r *http.Request, req TokenRequest) {
	if req.RefreshToken == "" {
		h.sendError(w, http.StatusBadRequest, ErrInvalidRequest, "refresh_token is required")
		return
	}

	// Get existing token by refresh token hash
	refreshTokenHash := mcpauth.HashToken(req.RefreshToken)
	oldToken, err := h.svc.DB.GetMCPOAuthTokenByRefreshHash(r.Context(), sql.NullString{String: refreshTokenHash, Valid: true})
	if err != nil {
		h.sendError(w, http.StatusBadRequest, ErrInvalidGrant, "Invalid refresh token")
		return
	}

	// Revoke old token
	if err := h.svc.DB.RevokeMCPOAuthToken(r.Context(), oldToken.ID); err != nil {
		h.sendError(w, http.StatusInternalServerError, ErrServerError, "Failed to refresh token")
		return
	}

	// Generate new tokens
	accessToken := generateSecureToken(32)
	refreshToken := generateSecureToken(32)
	accessTokenHash := mcpauth.HashToken(accessToken)
	newRefreshTokenHash := mcpauth.HashToken(refreshToken)

	expiresAt := time.Now().Add(AccessTokenTTL)
	refreshExpiresAt := time.Now().Add(RefreshTokenTTL)

	// Store new tokens
	_, err = h.svc.DB.CreateMCPOAuthToken(r.Context(), db.CreateMCPOAuthTokenParams{
		ID:               generateSecureToken(16),
		ClientID:         oldToken.ClientID,
		UserID:           oldToken.UserID,
		AccessTokenHash:  accessTokenHash,
		RefreshTokenHash: sql.NullString{String: newRefreshTokenHash, Valid: true},
		Scopes:           oldToken.Scopes,
		ExpiresAt:        expiresAt.Format(time.RFC3339),
		RefreshExpiresAt: sql.NullString{String: refreshExpiresAt.Format(time.RFC3339), Valid: true},
	})
	if err != nil {
		h.sendError(w, http.StatusInternalServerError, ErrServerError, "Failed to create tokens")
		return
	}

	resp := TokenResponse{
		AccessToken:  accessToken,
		TokenType:    "Bearer",
		ExpiresIn:    int(AccessTokenTTL.Seconds()),
		RefreshToken: refreshToken,
		Scope:        oldToken.Scopes,
	}

	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	json.NewEncoder(w).Encode(resp)
}

// renderLoginPage renders the OAuth login page with HTMX
func (h *Handler) renderLoginPage(w http.ResponseWriter, req AuthorizationRequest, clientName, errorMsg string) {
	w.Header().Set("Content-Type", "text/html; charset=utf-8")

	errorHTML := ""
	if errorMsg != "" {
		errorHTML = fmt.Sprintf(`<div class="error">%s</div>`, errorMsg)
	}

	html := fmt.Sprintf(`<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Sign in</title>
    <script src="/static/js/htmx.min.js"></script>
    <style>
        * { box-sizing: border-box; margin: 0; padding: 0; }
        body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; background: #f3f4f6; min-height: 100vh; display: flex; align-items: center; justify-content: center; }
        .container { background: white; padding: 32px; border-radius: 12px; box-shadow: 0 4px 6px rgba(0,0,0,0.1); width: 100%%; max-width: 400px; }
        h1 { font-size: 24px; margin-bottom: 8px; color: #111827; }
        .subtitle { color: #6b7280; margin-bottom: 24px; }
        .client-name { font-weight: 600; color: #111827; }
        .form-group { margin-bottom: 16px; }
        label { display: block; font-size: 14px; font-weight: 500; color: #374151; margin-bottom: 6px; }
        input[type="email"], input[type="password"] { width: 100%%; padding: 10px 12px; border: 1px solid #d1d5db; border-radius: 6px; font-size: 16px; }
        input:focus { outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37,99,235,0.1); }
        button { width: 100%%; padding: 12px; background: #2563eb; color: white; border: none; border-radius: 6px; font-size: 16px; font-weight: 500; cursor: pointer; transition: background 0.15s; }
        button:hover { background: #1d4ed8; }
        button:disabled { background: #93c5fd; cursor: not-allowed; }
        .scope { background: #f3f4f6; padding: 12px; border-radius: 6px; margin-bottom: 20px; font-size: 14px; color: #4b5563; }
        .error { color: #dc2626; background: #fef2f2; padding: 12px; border-radius: 6px; margin-bottom: 16px; font-size: 14px; }
        .htmx-request button { pointer-events: none; }
        .htmx-request .btn-text { display: none; }
        .htmx-request .btn-loading { display: inline-flex; align-items: center; gap: 8px; }
        .btn-loading { display: none; }
        .spinner { width: 16px; height: 16px; border: 2px solid #fff; border-top-color: transparent; border-radius: 50%%; animation: spin 0.6s linear infinite; }
        @keyframes spin { to { transform: rotate(360deg); } }
    </style>
</head>
<body>
    <div class="container">
        <h1>Sign in</h1>
        <p class="subtitle"><span class="client-name">%s</span> wants to access your account</p>

        <div id="error-container">%s</div>

        <div class="scope">
            <strong>Requested access:</strong> Full MCP access to manage your account
        </div>

        <form hx-post="/mcp/oauth/authorize"
              hx-target="#error-container"
              hx-swap="innerHTML"
              hx-indicator="this">
            <input type="hidden" name="response_type" value="%s">
            <input type="hidden" name="client_id" value="%s">
            <input type="hidden" name="redirect_uri" value="%s">
            <input type="hidden" name="scope" value="%s">
            <input type="hidden" name="state" value="%s">
            <input type="hidden" name="code_challenge" value="%s">
            <input type="hidden" name="code_challenge_method" value="%s">

            <div class="form-group">
                <label for="email">Email</label>
                <input type="email" id="email" name="email" required autofocus>
            </div>

            <div class="form-group">
                <label for="password">Password</label>
                <input type="password" id="password" name="password" required>
            </div>

            <button type="submit">
                <span class="btn-text">Sign in and Authorize</span>
                <span class="btn-loading"><span class="spinner"></span> Signing in...</span>
            </button>
        </form>
    </div>
</body>
</html>`,
		clientName,
		errorHTML,
		req.ResponseType,
		req.ClientID,
		req.RedirectURI,
		req.Scope,
		req.State,
		req.CodeChallenge,
		req.CodeChallengeMethod,
	)

	w.Write([]byte(html))
}

func (h *Handler) sendError(w http.ResponseWriter, status int, errCode, errDesc string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	json.NewEncoder(w).Encode(ErrorResponse{
		Error:            errCode,
		ErrorDescription: errDesc,
	})
}

func (h *Handler) redirectError(w http.ResponseWriter, r *http.Request, redirectURI, state, errCode, errDesc string) {
	u, _ := url.Parse(redirectURI)
	q := u.Query()
	q.Set("error", errCode)
	q.Set("error_description", errDesc)
	if state != "" {
		q.Set("state", state)
	}
	u.RawQuery = q.Encode()
	http.Redirect(w, r, u.String(), http.StatusFound)
}

func generateSecureToken(length int) string {
	b := make([]byte, length)
	rand.Read(b)
	return base64.RawURLEncoding.EncodeToString(b)
}

func verifyPKCE(verifier, challenge, method string) bool {
	if method != "S256" {
		return false
	}

	h := sha256.Sum256([]byte(verifier))
	computed := base64.RawURLEncoding.EncodeToString(h[:])

	return subtle.ConstantTimeCompare([]byte(computed), []byte(challenge)) == 1
}
