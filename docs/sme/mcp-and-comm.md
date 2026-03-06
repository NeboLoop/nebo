# MCP and Communication Plugins: Comprehensive Logic Deep-Dive

This document provides an exhaustive, function-by-function analysis of the Go implementation for:
1. MCP (Model Context Protocol) server, client, bridge, tools, auth, and OAuth
2. Communication plugin system (CommPlugin interface, PluginManager, CommHandler)
3. NeboLoop communication plugin (WebSocket gateway, DM handling, reconnect)

Source locations:
- `/Users/almatuck/workspaces/nebo/nebo/internal/mcp/`
- `/Users/almatuck/workspaces/nebo/nebo/internal/agent/comm/`
- `/Users/almatuck/workspaces/nebo/nebo/internal/agent/comm/neboloop/`

---

## Table of Contents

1. [MCP Server (`mcp/server.go`)](#1-mcp-server)
2. [MCP Handler (`mcp/handler.go`)](#2-mcp-handler)
3. [MCP Auth (`mcp/mcpauth/auth.go`)](#3-mcp-auth)
4. [MCP Context (`mcp/mcpctx/context.go`)](#4-mcp-context)
5. [MCP OAuth (`mcp/oauth/`)](#5-mcp-oauth)
6. [MCP Client (`mcp/client/`)](#6-mcp-client)
7. [MCP Client Transport (`mcp/client/transport.go`)](#7-mcp-client-transport)
8. [MCP Client Crypto (`mcp/client/crypto.go`)](#8-mcp-client-crypto)
9. [MCP Client Callback (`mcp/client/callback.go`)](#9-mcp-client-callback)
10. [MCP Bridge (`mcp/bridge/bridge.go`)](#10-mcp-bridge)
11. [MCP Tools Registry (`mcp/tools/registry.go`)](#11-mcp-tools-registry)
12. [MCP Tools: Memory (`mcp/tools/memory.go`)](#12-mcp-tools-memory)
13. [MCP Tools: Notification (`mcp/tools/notification.go`)](#13-mcp-tools-notification)
14. [MCP Tools: User (`mcp/tools/user.go`)](#14-mcp-tools-user)
15. [MCP Protocol Routes](#15-mcp-protocol-routes)
16. [CommPlugin Interface (`comm/plugin.go`)](#16-commplugin-interface)
17. [CommMessage Types (`comm/types.go`)](#17-commmessage-types)
18. [CommPluginManager (`comm/manager.go`)](#18-commpluginmanager)
19. [CommHandler (`comm/handler.go`)](#19-commhandler)
20. [Loopback Plugin (`comm/loopback.go`)](#20-loopback-plugin)
21. [NeboLoop Plugin (`comm/neboloop/plugin.go`)](#21-neboloop-plugin)

---

## 1. MCP Server

**File:** `internal/mcp/server.go`
**Package:** `mcp`
**Dependencies:** `modelcontextprotocol/go-sdk/mcp`, `google/uuid`

### Purpose
Creates user-scoped MCP servers with tools registered based on the authenticated user. Each user gets their own MCP server instance with tool access scoped to their data.

### Logger
```go
var mcpServerLog = logging.L("MCP")
```

### Functions

#### `NewServer(svc *svc.ServiceContext, r *http.Request) *mcp.Server`
Convenience wrapper around `NewServerWithContext` that discards the `ToolContext`. Use when the caller does not need session caching.

#### `NewServerWithContext(svc *svc.ServiceContext, r *http.Request) (*mcp.Server, *mcpctx.ToolContext)`
Creates an MCP server scoped to the authenticated user.

**Algorithm:**
1. Create a bare `mcp.Server` with implementation name="nebo", version="1.0.0"
2. Extract `tokenInfo` from request context (set by auth middleware)
3. If no token info, return server without tools (warns)
4. Extract `*mcpauth.UserInfo` from `tokenInfo.Extra["user_info"]`
5. Extract `*db.User` from `tokenInfo.Extra["user"]`
6. Read `Mcp-Session-Id` header from request
7. Generate a `requestID` (UUID) for tracing
8. Read `User-Agent` header
9. Create `mcpctx.ToolContext` with all the above
10. Register tools: `user`, `notification`, `memory`
11. Return server and toolCtx

**Key insight:** The server is per-request or per-session. Tools are registered at creation time, not dynamically. The `ToolContext` carries the user identity so all tool operations are user-scoped.

---

## 2. MCP Handler

**File:** `internal/mcp/handler.go`
**Package:** `mcp`

### Purpose
HTTP handler that serves MCP protocol requests over Streamable HTTP with Bearer token authentication, session management, and OAuth challenge responses.

### Structs

#### `Handler`
```go
type Handler struct {
    svc             *svc.ServiceContext
    authenticator   *mcpauth.Authenticator
    httpHandler     http.Handler
    resourceMetaURL string

    sessionCache sync.Map // map[sessionID]*sessionData
}
```

#### `sessionData`
```go
type sessionData struct {
    server  *mcp.Server
    toolCtx *mcpctx.ToolContext
}
```

### Functions

#### `NewHandler(svc *svc.ServiceContext, baseURL string) *Handler`
Creates the handler, initializes the authenticator, and sets up the streamable HTTP handler.

**Algorithm:**
1. Trim trailing slash from baseURL
2. Create `mcpauth.Authenticator`
3. Set `resourceMetaURL` to `{baseURL}/.well-known/oauth-protected-resource`
4. Create `mcp.NewStreamableHTTPHandler` in **STATELESS** mode
   - Stateless mode means the SDK does not validate session IDs -- the handler manages sessions itself
   - Passes `h.getServerForRequest` as the server factory function
5. Wrap with `h.authMiddleware` for Bearer token validation

#### `authMiddleware(next http.Handler) http.Handler`
Validates Bearer tokens and manages session IDs. RFC 9728 compliant.

**Algorithm:**
1. Check for `Mcp-Session-Id` header
2. If missing, generate a new UUID session ID and set it on the request
3. Extract `Authorization: Bearer <token>` from headers
4. If missing or empty, call `writeUnauthorized` (returns 401 with OAuth challenge)
5. Call `h.authenticator.TokenVerifier()` to validate the token
6. If invalid, call `writeUnauthorized`
7. Always set `Mcp-Session-Id` in the response header
8. Add `tokenInfo` to request context
9. Delegate to the next handler

#### `writeUnauthorized(w http.ResponseWriter, msg string)`
Sends 401 with RFC 9728 compliant `WWW-Authenticate` header:
```
Bearer resource_metadata="{resourceMetaURL}", scope="mcp:full"
```

#### `getServerForRequest(r *http.Request) *mcp.Server`
Server factory function called by the SDK's streamable HTTP handler.

**Algorithm:**
1. Read session ID from `Mcp-Session-Id` header (always set by authMiddleware)
2. Read tokenInfo from context
3. Check `sessionCache` by session ID -- if found, return cached server
4. If not cached, create new server via `NewServerWithContext`
5. Store in `sessionCache` and return

**Key insight:** Sessions are keyed by session ID (not user ID), so a single user can have multiple concurrent MCP sessions from different clients.

#### `ServeHTTP(w http.ResponseWriter, r *http.Request)`
Delegates to `h.httpHandler` (the auth-wrapped streamable handler).

#### `Authenticator() *mcpauth.Authenticator`
Returns the authenticator for external cache invalidation.

---

## 3. MCP Auth

**File:** `internal/mcp/mcpauth/auth.go`
**Package:** `mcpauth`
**Dependencies:** `golang-jwt/jwt/v4`, `modelcontextprotocol/go-sdk/auth`

### Purpose
JWT-based authentication for MCP sessions. Validates Bearer tokens using the same JWT secret as the main Nebo API.

### Functions

#### `HashToken(token string) string`
SHA-256 hashes a token string and returns hex-encoded result. Used for secure storage of OAuth tokens and authorization codes.
```go
func HashToken(token string) string {
    h := sha256.Sum256([]byte(token))
    return hex.EncodeToString(h[:])
}
```

### Structs

#### `UserInfo`
```go
type UserInfo struct {
    UserID string
    Email  string
    Name   string
}
```

#### `Authenticator`
```go
type Authenticator struct {
    svc *svc.ServiceContext
}
```

### Context Keys and Helpers

Two private context key types:
- `userInfoKey{}` -- stores `*UserInfo`
- `tokenInfoKey{}` -- stores `*auth.TokenInfo`

#### `WithUserInfo(ctx context.Context, info *UserInfo) context.Context`
Adds UserInfo to context.

#### `UserInfoFromContext(ctx context.Context) *UserInfo`
Retrieves UserInfo from context. Returns nil if not set or wrong type.

#### `ContextWithTokenInfo(ctx context.Context, info *auth.TokenInfo) context.Context`
Adds TokenInfo to context.

#### `TokenInfoFromContext(ctx context.Context) *auth.TokenInfo`
Retrieves TokenInfo from context. Returns nil if not set.

#### `NewAuthenticator(svc *svc.ServiceContext) *Authenticator`
Creates a new authenticator with access to the service context.

#### `TokenVerifier() func(ctx context.Context, token string, req *http.Request) (*auth.TokenInfo, error)`
Returns a function suitable for use with `auth.RequireBearerToken`. The returned function calls `verifyJWT`.

#### `verifyJWT(ctx context.Context, tokenString string) (*auth.TokenInfo, error)`
Core JWT validation logic.

**Algorithm:**
1. Strip "Bearer " prefix if present
2. Parse JWT using HMAC signing method validation
3. Key function returns `[]byte(a.svc.Config.Auth.AccessSecret)`
4. Extract claims as `jwt.MapClaims`
5. Get user ID from `userId` claim (fallback to `sub` claim)
6. Look up user in DB via `GetUserByID`
7. Build `UserInfo` from DB user
8. Return `auth.TokenInfo` with `Extra` map containing:
   - `"user_id"` -- string
   - `"user_info"` -- `*UserInfo`
   - `"user"` -- `*db.User`

**Error handling:** Returns `auth.ErrInvalidToken` for any validation failure (unexpected signing method, invalid claims, user not found).

#### `Middleware(next http.Handler) http.Handler`
HTTP middleware that authenticates requests and adds both TokenInfo and UserInfo to context.

**Algorithm:**
1. Check `Authorization` header
2. Call `verifyJWT`
3. Add tokenInfo to context via `ContextWithTokenInfo`
4. Extract and add UserInfo via `WithUserInfo`
5. Delegate to next handler

---

## 4. MCP Context

**File:** `internal/mcp/mcpctx/context.go`
**Package:** `mcpctx`

### Purpose
Carries authenticated user context through all MCP tool invocations. Provides structured error types for consistent tool error responses.

### Constants

```go
type AuthMode int

const (
    AuthModeJWT AuthMode = iota
)
```

### Structs

#### `ToolContext`
```go
type ToolContext struct {
    svc       *svc.ServiceContext
    requestID string
    userAgent string
    sessionID string
    authMode  AuthMode
    user      *db.User
}
```

### Constructor

#### `NewToolContext(svc *svc.ServiceContext, user db.User, requestID, userAgent, sessionID string) *ToolContext`
Creates a user-scoped tool context. Note: takes `user` by value (makes a copy), stores a pointer.

### Accessor Methods

| Method | Returns |
|--------|---------|
| `SessionID() string` | MCP session ID |
| `AuthMode() AuthMode` | Authentication mode (always JWT currently) |
| `User() *db.User` | Full user record |
| `UserID() string` | User ID (returns "" if user is nil) |
| `DB() *db.Store` | Database store for queries |
| `Svc() *svc.ServiceContext` | Full service context |
| `RequestID() string` | UUID for request tracing |
| `UserAgent() string` | Client user agent |

### Structured Errors

#### `ToolError`
```go
type ToolError struct {
    Code    string `json:"code"`    // "not_found", "validation", "conflict", "unauthorized"
    Message string `json:"message"` // Human-readable description
    Field   string `json:"field"`   // For validation errors
}
```

`Error() string` formats as `"{code}: {message}"` or `"{code}: {message} (field: {field})"`.

#### Error Constructors

| Constructor | Code |
|------------|------|
| `NewValidationError(message, field string)` | `"validation"` |
| `NewNotFoundError(message string)` | `"not_found"` |
| `NewConflictError(message string)` | `"conflict"` |
| `NewUnauthorizedError(message string)` | `"unauthorized"` |

### Context Propagation

- `toolContextKey{}` -- private context key
- `WithToolContext(ctx, tc)` -- adds ToolContext to context.Context
- `ToolContextFromContext(ctx)` -- retrieves ToolContext from context.Context

---

## 5. MCP OAuth

**Files:** `internal/mcp/oauth/handler.go`, `internal/mcp/oauth/types.go`
**Package:** `oauth`
**Dependencies:** `go-chi/chi/v5`, `x/crypto/bcrypt`

### Purpose
Full OAuth 2.1 authorization server for MCP. Supports Dynamic Client Registration (DCR), PKCE (S256 only), authorization code flow, and token refresh. Renders an HTML login page for authorization.

### Constants

```go
// OAuth 2.1 error codes
const (
    ErrInvalidRequest          = "invalid_request"
    ErrUnauthorizedClient      = "unauthorized_client"
    ErrAccessDenied            = "access_denied"
    ErrUnsupportedResponseType = "unsupported_response_type"
    ErrInvalidScope            = "invalid_scope"
    ErrServerError             = "server_error"
    ErrInvalidClient           = "invalid_client"
    ErrInvalidGrant            = "invalid_grant"
    ErrUnsupportedGrantType    = "unsupported_grant_type"
)

// Token expiration times
const (
    AccessTokenTTL  = 1 * time.Hour
    RefreshTokenTTL = 30 * 24 * time.Hour   // 30 days
    AuthCodeTTL     = 10 * time.Minute
)
```

### Types

#### Request/Response Types

| Type | Fields |
|------|--------|
| `AuthorizationRequest` | ResponseType, ClientID, RedirectURI, Scope, State, CodeChallenge, CodeChallengeMethod |
| `TokenRequest` | GrantType, Code, RedirectURI, RefreshToken, ClientID, ClientSecret, CodeVerifier |
| `TokenResponse` | AccessToken, TokenType, ExpiresIn, RefreshToken, Scope |
| `ErrorResponse` | Error, ErrorDescription |
| `ClientRegistrationRequest` | ClientName, RedirectURIs, TokenEndpointAuthMethod, GrantTypes, ResponseTypes, Scope |
| `ClientRegistrationResponse` | ClientID, ClientSecret, ClientName, RedirectURIs, TokenEndpointAuthMethod, GrantTypes, ResponseTypes, Scope, ClientIDIssuedAt, ClientSecretExpiresAt |
| `ProtectedResourceMetadata` | Resource, AuthorizationServers, ScopesSupported, BearerMethodsSupported |
| `AuthorizationServerMetadata` | Issuer, AuthorizationEndpoint, TokenEndpoint, RegistrationEndpoint, ScopesSupported, ResponseTypesSupported, ResponseModesSupported, GrantTypesSupported, TokenEndpointAuthMethodsSupported, CodeChallengeMethodsSupported |

### Handler Struct

```go
type Handler struct {
    svc     *svc.ServiceContext
    baseURL string
}
```

### Route Registration

#### `RegisterRoutes(r chi.Router)`
Registers all OAuth routes:

| Method | Path | Handler |
|--------|------|---------|
| GET | `/.well-known/oauth-protected-resource` | `HandleProtectedResourceMetadata` |
| GET | `/.well-known/oauth-authorization-server` | `HandleAuthServerMetadata` |
| POST | `/mcp/oauth/register` | `HandleClientRegistration` |
| GET | `/mcp/oauth/authorize` | `HandleAuthorize` |
| POST | `/mcp/oauth/authorize` | `HandleAuthorizeSubmit` |
| POST | `/mcp/oauth/token` | `HandleToken` |

### OAuth Endpoints

#### `HandleProtectedResourceMetadata` (GET `/.well-known/oauth-protected-resource`)
Returns RFC 9728 protected resource metadata.
```json
{
  "resource": "{baseURL}/mcp",
  "authorization_servers": ["{baseURL}"],
  "scopes_supported": ["mcp:full"],
  "bearer_methods_supported": ["header"]
}
```
Sets CORS headers, Cache-Control: no-store.

#### `HandleAuthServerMetadata` (GET `/.well-known/oauth-authorization-server`)
Returns OAuth authorization server metadata.
```json
{
  "issuer": "{baseURL}",
  "authorization_endpoint": "{baseURL}/authorize",
  "token_endpoint": "{baseURL}/token",
  "registration_endpoint": "{baseURL}/register",
  "scopes_supported": ["mcp:full", "offline_access"],
  "response_types_supported": ["code"],
  "response_modes_supported": ["query"],
  "grant_types_supported": ["authorization_code", "refresh_token"],
  "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post", "none"],
  "code_challenge_methods_supported": ["S256"]
}
```

#### `HandleJWKS` (not registered in routes but available)
Returns empty JWKS (Nebo uses opaque tokens, not JWTs for MCP OAuth).

#### `HandleClientRegistration` (POST `/mcp/oauth/register`)
Dynamic Client Registration (DCR).

**Algorithm:**
1. Parse JSON body as `ClientRegistrationRequest`
2. Validate: `client_name` required, `redirect_uris` required (at least one)
3. Generate `clientID` (16 random bytes, base64url) and `clientSecret` (32 random bytes, base64url)
4. Hash clientSecret with `mcpauth.HashToken`
5. Apply defaults: grant_types=["authorization_code","refresh_token"], response_types=["code"], auth_method="client_secret_post", scope="mcp:full"
6. Determine `isConfidential` based on token_endpoint_auth_method
7. Store in DB via `CreateMCPOAuthClient`
8. Return 201 Created with client credentials (secret in cleartext, never expires)

#### `HandleAuthorize` (GET `/mcp/oauth/authorize`)
Shows the login/authorization page.

**Algorithm:**
1. Parse query params into `AuthorizationRequest`
2. Validate `response_type == "code"`
3. Look up client by `client_id` in DB
4. Validate `redirect_uri` against registered URIs (exact match, comma-separated in DB)
5. Require PKCE: `code_challenge` must be present, method must be `S256`
6. Render HTML login page with HTMX form

#### `HandleAuthorizeSubmit` (POST `/mcp/oauth/authorize`)
Processes the login form.

**Algorithm:**
1. Parse form data (OAuth params from hidden fields + email/password)
2. Re-validate client
3. Authenticate user: `GetUserByEmail` then `bcrypt.CompareHashAndPassword`
4. On auth failure, re-render login page with error message
5. Generate authorization code (32 random bytes, base64url)
6. Store code hash in DB with: clientID, userID, redirectURI, PKCE challenge, 10-minute expiry
7. Redirect to `redirect_uri?code={code}&state={state}`

#### `HandleToken` (POST `/mcp/oauth/token`)
Token exchange and refresh.

**Algorithm:**
1. Parse form data into `TokenRequest`
2. Support `client_secret_basic`: extract credentials from `Authorization: Basic {base64}` header (URL-decoded)
3. Dispatch by `grant_type`:
   - `"authorization_code"` -> `handleAuthorizationCodeGrant`
   - `"refresh_token"` -> `handleRefreshTokenGrant`

#### `handleAuthorizationCodeGrant`
**Algorithm:**
1. Validate: code and client_id required
2. Look up auth code by hash
3. Verify client matches (oauth_client_id comparison)
4. Verify redirect_uri matches stored value
5. Verify PKCE: `verifyPKCE(code_verifier, stored_challenge, "S256")`
6. Mark code as used
7. Generate access token (32 bytes) and refresh token (32 bytes)
8. Hash both tokens
9. Store in DB with TTLs (1h access, 30d refresh)
10. Return token response JSON

#### `handleRefreshTokenGrant`
**Algorithm:**
1. Validate refresh_token present
2. Look up old token by refresh hash
3. Revoke old token
4. Generate new access+refresh tokens
5. Store new tokens in DB
6. Return token response JSON

### Helper Functions

#### `generateSecureToken(length int) string`
Generates `length` random bytes, returns base64url encoded (no padding).

#### `verifyPKCE(verifier, challenge, method string) bool`
Only supports S256. Computes `BASE64URL(SHA256(verifier))` and uses `subtle.ConstantTimeCompare` against the challenge.

#### `renderLoginPage(w, req, clientName, errorMsg)`
Renders a complete HTML page with:
- Inline CSS (modern, responsive design)
- HTMX-powered form (hx-post to `/mcp/oauth/authorize`)
- Hidden OAuth fields (response_type, client_id, redirect_uri, scope, state, code_challenge, code_challenge_method)
- Email + password inputs
- Loading spinner on submit

#### `sendError(w, status, errCode, errDesc)`
Returns JSON error response.

#### `redirectError(w, r, redirectURI, state, errCode, errDesc)`
Redirects to `redirect_uri?error={code}&error_description={desc}&state={state}`.

#### `setCORSHeaders(w)`
Sets permissive CORS: `Access-Control-Allow-Origin: *`, methods GET/POST/OPTIONS, headers Content-Type/Authorization, max-age 86400.

---

## 6. MCP Client

**File:** `internal/mcp/client/client.go`
**Package:** `client`

### Purpose
OAuth client for connecting to **external** MCP servers. Handles OAuth discovery, PKCE, dynamic client registration, authorization code exchange, token storage (encrypted), token refresh, and access token retrieval.

### Structs

#### `Client`
```go
type Client struct {
    db            *db.Store
    encryptionKey []byte
    httpClient    *http.Client  // 30s timeout
    baseURL       string        // Nebo's base URL for redirect URIs
}
```

#### `ServerMetadata`
OAuth server metadata from `/.well-known/oauth-authorization-server`:
```go
type ServerMetadata struct {
    Issuer                            string   `json:"issuer"`
    AuthorizationEndpoint             string   `json:"authorization_endpoint"`
    TokenEndpoint                     string   `json:"token_endpoint"`
    RegistrationEndpoint              string   `json:"registration_endpoint,omitempty"`
    RevocationEndpoint                string   `json:"revocation_endpoint,omitempty"`
    ScopesSupported                   []string `json:"scopes_supported,omitempty"`
    ResponseTypesSupported            []string `json:"response_types_supported,omitempty"`
    CodeChallengeMethodsSupported     []string `json:"code_challenge_methods_supported,omitempty"`
    TokenEndpointAuthMethodsSupported []string `json:"token_endpoint_auth_methods_supported,omitempty"`
}
```

#### `TokenResponse`
```go
type TokenResponse struct {
    AccessToken  string `json:"access_token"`
    TokenType    string `json:"token_type"`
    ExpiresIn    int    `json:"expires_in,omitempty"`
    RefreshToken string `json:"refresh_token,omitempty"`
    Scope        string `json:"scope,omitempty"`
}
```

### Functions

#### `NewClient(database *db.Store, encryptionKey []byte, baseURL string) *Client`
Creates client with 30-second HTTP timeout.

#### `Discover(ctx context.Context, serverURL string) (*ServerMetadata, error)`
Fetches OAuth metadata from `{scheme}://{host}/.well-known/oauth-authorization-server`.

#### `GeneratePKCE() (verifier, challenge string, err error)`
Generates PKCE pair: 32 random bytes -> base64url verifier, SHA256 -> base64url challenge.

#### `GenerateState() (string, error)`
Generates 16 random bytes -> base64url state parameter.

#### `StartOAuthFlow(ctx context.Context, integrationID string) (string, error)`
Initiates the full OAuth authorization code flow.

**Algorithm:**
1. Get integration from DB, validate auth_type == "oauth"
2. Resolve server URL (from integration or registry)
3. Discover OAuth metadata from server
4. Generate PKCE verifier + challenge
5. Generate CSRF state
6. Encrypt the PKCE verifier before storing
7. Get or create client credentials (via DCR or stored)
8. Encrypt client secret if present
9. Store OAuth flow state in DB: state, encrypted verifier, client ID, encrypted secret, endpoints
10. Build authorization URL with params: response_type=code, client_id, redirect_uri, state, code_challenge, code_challenge_method=S256, scope
11. Get scopes from registry (default: "mcp:full")
12. Return the authorization URL

**Redirect URI:** `{baseURL}/api/v1/integrations/oauth/callback`

#### `getOrCreateClientCredentials(ctx, integrationID, metadata) (clientID, clientSecret, error)`
**Priority order:**
1. Stored encrypted credentials in DB -> decrypt and return
2. Dynamic Client Registration if endpoint available
3. Fallback: public client ID `"nebo-agent-{integrationID}"`

#### `dynamicClientRegistration(ctx, registrationEndpoint, integrationID) (clientID, clientSecret, error)`
Registers as public client ("none" auth method) with name "Nebo Agent", scope "mcp:full offline_access".

#### `ExchangeCode(ctx context.Context, integrationID, code string) error`
Exchanges authorization code for tokens.

**Algorithm:**
1. Get OAuth config from DB
2. Decrypt PKCE verifier
3. Get client credentials (decrypt secret if present)
4. POST to token endpoint: grant_type=authorization_code, code, redirect_uri, client_id, code_verifier
5. Add Basic auth if client secret present
6. Parse response, store tokens via `storeTokens`

#### `storeTokens(ctx, integrationID, tokens) error`
Encrypts access token and refresh token, calculates expiration, deletes existing credentials, creates new credential record in DB with type "oauth_token".

#### `RefreshToken(ctx context.Context, integrationID string) error`
Refreshes an expired access token.

**Algorithm:**
1. Get current credentials, decrypt refresh token
2. Get OAuth config, decrypt client secret
3. POST to token endpoint: grant_type=refresh_token, refresh_token, client_id
4. Add Basic auth if secret present
5. Preserve original refresh token if server doesn't return a new one
6. Store new tokens

#### `GetAccessToken(ctx context.Context, integrationID string) (string, error)`
Retrieves and optionally refreshes the access token.

**Algorithm:**
1. Get credentials from DB
2. If not oauth_token type, decrypt as API key (strip "enc:" prefix)
3. If token expired or expiring within 60 seconds, try refresh
4. Decrypt and return access token

#### `Disconnect(ctx context.Context, integrationID string) error`
Deletes credentials, clears OAuth state, updates connection status to "disconnected".

#### `getRedirectURI() string`
Returns `{baseURL}/api/v1/integrations/oauth/callback`.

---

## 7. MCP Client Transport

**File:** `internal/mcp/client/transport.go`
**Package:** `client`

### Purpose
Manages MCP client sessions (connections to external MCP servers) with caching, health checking, and automatic reconnection. Provides `ListTools` and `CallTool` operations.

### Structs

#### `AuthenticatedTransport`
```go
type AuthenticatedTransport struct {
    Base          http.RoundTripper
    MCPClient     *Client
    IntegrationID string
}
```

Implements `http.RoundTripper`. On every request:
1. Calls `MCPClient.GetAccessToken` (handles refresh)
2. Clones request, adds `Authorization: Bearer {token}`
3. Delegates to `Base.RoundTrip`

#### `sessionEntry`
```go
type sessionEntry struct {
    session   *mcp.ClientSession
    createdAt time.Time
}
```

### Package-Level State

```go
var (
    sessions   = make(map[string]*sessionEntry)
    sessionsMu sync.Mutex
)

const maxSessionAge = 30 * time.Minute
```

Sessions are cached per integration ID. Uses `sync.Mutex` (not `sync.Map`) because read paths also write (close stale + delete). The map is small (typically <10 entries).

### Functions

#### `isSessionHealthy(entry *sessionEntry) bool`
Returns false if nil or if `time.Since(entry.createdAt) > maxSessionAge`.

#### `getOrCreateSession(ctx, integrationID, serverURL, authType) (*mcp.ClientSession, error)`
**Algorithm:**
1. Lock, check cache -- if healthy, return cached session
2. If stale, delete from cache, unlock, close old session
3. Create authenticated HTTP transport if auth_type is not "" or "none"
4. Create `mcp.StreamableClientTransport` with the serverURL
5. Create MCP client (name="nebo", version="1.0.0", keepalive=30s)
6. Connect to server
7. Lock again, check if another goroutine raced us -- if so, close ours, use theirs
8. Store in cache
9. Start a watcher goroutine that waits on `session.Wait()`:
   - On close: delete from cache, update DB status to "disconnected"

**Race condition handling:** Double-checked locking pattern. After creating a new session (which involves network I/O done without holding the lock), re-check the cache before storing.

#### `CloseSession(integrationID string)`
Closes and removes a single cached session.

#### `CloseAllSessions()`
Snapshots all sessions under lock, clears map, closes all sessions outside lock.

#### `StartHealthChecker(ctx context.Context)`
Starts a background goroutine that runs every `maxSessionAge/3` (10 minutes).

#### `performHealthCheck()`
Under lock: collects stale sessions. Outside lock: closes them. The watcher goroutine handles cache/DB cleanup when Close() triggers.

#### `ListTools(ctx, integrationID) ([]*mcp.Tool, error)`
Lists tools from an external MCP server with retry-once on failure.

**Algorithm:**
1. Get integration from DB, validate server URL
2. Get or create session
3. Call `session.ListTools`
4. On error: close session, get new session, retry once
5. Return `result.Tools`

#### `CallTool(ctx, integrationID, toolName string, input json.RawMessage) (*mcp.CallToolResult, error)`
Calls a tool on an external MCP server with **infinite retry** using exponential backoff.

**Algorithm:**
1. Get integration, validate server URL
2. Unmarshal input to `map[string]any` once
3. Enter infinite retry loop:
   - Get or create session
   - If session error: close session, continue
   - Call `session.CallTool` with params
   - If success: return result
   - If error: close session, continue
   - Calculate exponential backoff: base=100ms, max=10min, 2^min(attempt,9) with +/-25% jitter
   - Wait with context cancellation support
4. Only stops on context cancellation

**Backoff rationale:** 10-minute max delay (not 60s) to avoid thundering herd at scale (1M+ users).

---

## 8. MCP Client Crypto

**File:** `internal/mcp/client/crypto.go`
**Package:** `client`

### Purpose
AES-256-GCM encryption for storing OAuth tokens and secrets. Key management with OS keychain priority.

### Key Management

#### `GetEncryptionKey(dataDir string) ([]byte, error)`
Retrieves or generates the 32-byte encryption key.

**Priority order:**
1. **OS keychain** (macOS Keychain, Windows DPAPI, Linux Secret Service) -- most secure
2. `MCP_ENCRYPTION_KEY` env var (hex-encoded 32 bytes)
3. `JWT_SECRET` env var (first 32 bytes, padded)
4. Persistent file at `{dataDir}/.mcp-key` (hex-encoded)
5. Generate new random key

**Key promotion:** When a key is found in env/file, it is automatically promoted to keychain and the file is deleted. Uses `internal/keyring` package for OS keychain access.

### Encryption Functions

#### `EncryptString(plaintext string, key []byte) (string, error)`
AES-256-GCM encryption.

**Algorithm:**
1. Empty plaintext returns empty string
2. Create AES cipher with key
3. Create GCM wrapper
4. Generate random nonce (GCM nonce size, typically 12 bytes)
5. `gcm.Seal(nonce, nonce, plaintext, nil)` -- prepends nonce to ciphertext
6. Return hex-encoded result

**Wire format:** `hex(nonce || ciphertext || tag)`

#### `DecryptString(ciphertext string, key []byte) (string, error)`
AES-256-GCM decryption.

**Algorithm:**
1. Empty ciphertext returns empty string
2. Hex-decode the ciphertext
3. Create AES cipher and GCM
4. Split: first `NonceSize()` bytes = nonce, rest = cipherdata
5. `gcm.Open(nil, nonce, cipherdata, nil)`
6. Return plaintext string

---

## 9. MCP Client Callback

**File:** `internal/mcp/client/callback.go`
**Package:** `client`

### Purpose
HTTP handlers for OAuth redirect callbacks from external MCP servers. Two variants: redirect-based (for browser flows) and JSON-based (for API flows).

### Functions

#### `OAuthCallbackHandler(database *db.Store, mcpClient *Client, frontendURL string, onConnect func()) http.HandlerFunc`
Handles OAuth redirects from external MCP servers. Redirect-based for browser flows.

**Parameters:**
- `onConnect` -- called after successful token exchange so callers can trigger a bridge re-sync

**Algorithm:**
1. Extract query params: `code`, `state`, `error`, `error_description`
2. If error: redirect to frontend with error
3. Validate code and state present
4. Look up integration by OAuth state in DB
5. Exchange code for tokens via `mcpClient.ExchangeCode`
6. On exchange failure: update integration status to "error", redirect with error
7. Clear OAuth state in DB
8. Update connection status to "connected"
9. Call `onConnect()` if set (triggers bridge re-sync)
10. Redirect to `{frontendURL}/settings/mcp?connected={integrationID}`

#### `OAuthCallbackJSONHandler(database *db.Store, mcpClient *Client) http.HandlerFunc`
Same flow as above but returns JSON responses instead of redirects.

Success response:
```json
{
  "success": true,
  "integrationId": "{id}",
  "message": "Successfully connected"
}
```

#### `redirectWithError(w, r, frontendURL, errCode, errDesc)`
Redirects to `{frontendURL}/settings/mcp?error={code}&error_description={desc}`.

---

## 10. MCP Bridge

**File:** `internal/mcp/bridge/bridge.go`
**Package:** `bridge`

### Purpose
Connects to external MCP servers, discovers their tools, and registers them as proxy tools in the agent's local tool registry. This is how external MCP integrations become available to the Nebo agent.

### Structs

#### `Bridge`
```go
type Bridge struct {
    mu          sync.Mutex
    connections map[string]*connection // integrationID -> live connection
    registry    *tools.Registry        // agent's tool registry
    queries     *db.Queries
    mcpClient   *mcpclient.Client
}
```

#### `connection`
```go
type connection struct {
    IntegrationID string
    ServerType    string
    ToolNames     []string // namespaced names registered in Registry
}
```

#### `proxyTool`
```go
type proxyTool struct {
    name          string           // mcp__{serverType}__{originalName}
    originalName  string           // as reported by external server
    description   string
    inputSchema   json.RawMessage
    integrationID string
    mcpClient     *mcpclient.Client
}
```

Implements `tools.Tool` interface:
- `Name() string` -- returns namespaced name
- `Description() string` -- returns original description
- `RequiresApproval() bool` -- always returns `true`
- `Schema() json.RawMessage` -- returns schema or `{"type":"object"}`
- `Execute(ctx, input) (*tools.ToolResult, error)` -- calls `mcpClient.CallTool`, extracts text content from MCP response

### Tool Naming Convention

#### `toolName(serverType, original string) string`
Generates `mcp__{serverType}__{originalName}` where serverType is lowercased with spaces replaced by underscores.

**Examples:**
- Server type "GitHub", tool "create_issue" -> `mcp__github__create_issue`
- Server type "Brave Search", tool "web_search" -> `mcp__brave_search__web_search`

### Functions

#### `New(registry *tools.Registry, queries *db.Queries, mcpClient *mcpclient.Client) *Bridge`
Creates a new bridge.

#### `SyncAll(ctx context.Context) error`
Loads all enabled MCP integrations and syncs connections. Safe to call multiple times.

**Algorithm:**
1. List all enabled integrations from DB
2. Build set of enabled integration IDs
3. Under lock: disconnect any connections not in the enabled set
4. For each enabled integration:
   - Skip if no server URL
   - Skip OAuth integrations that haven't completed auth yet (status is NULL)
   - Allow "disconnected" and "error" states through for reconnection
   - Call `Connect(ctx, id, serverType)`

#### `Connect(ctx context.Context, integrationID, serverType string) error`
Connects to a single MCP integration.

**Algorithm:**
1. Call `Disconnect(integrationID)` first to clean up any existing connection
2. Call `mcpClient.ListTools` to discover available tools
3. On error: update DB status to "error" with error message
4. For each discovered tool:
   - Generate namespaced proxy name
   - Marshal InputSchema to JSON
   - Create `proxyTool` wrapping the external tool
   - Register in the agent's tool registry
5. Store connection in `connections` map
6. Update DB: tool count and status "connected"

#### `Disconnect(integrationID string)`
Removes all proxy tools for an integration. Under lock: unregisters each tool name from registry, closes MCP session, deletes from connections map.

#### `Close()`
Disconnects all integrations.

### proxyTool.Execute

When a proxy tool is executed:
1. Calls `mcpClient.CallTool(ctx, integrationID, originalName, input)`
2. Iterates over `result.Content` items
3. Extracts text from `*mcp.TextContent` items
4. Concatenates with newlines
5. Returns `tools.ToolResult{Content: text, IsError: result.IsError}`

---

## 11. MCP Tools Registry

**File:** `internal/mcp/tools/registry.go`
**Package:** `tools`

### Purpose
Provides a thread-safe tool function registry for direct invocation (bypassing MCP protocol). Used by the agent executor to call MCP tools programmatically.

### Types

```go
type ToolFunc func(ctx context.Context, args json.RawMessage) (interface{}, error)

type ToolRegistry struct {
    mu    sync.RWMutex
    tools map[string]ToolFunc
}
```

### Functions

#### `NewToolRegistry() *ToolRegistry`
Creates empty registry.

#### `Register(name string, fn ToolFunc)`
Adds a tool under write lock.

#### `Call(ctx context.Context, name string, args map[string]interface{}) (interface{}, error)`
Invokes a tool by name. Marshals `args` to JSON, calls the registered function.

#### `Has(name string) bool`
Checks if a tool is registered (read lock).

#### `List() []string`
Returns all registered tool names (read lock).

#### `NewRegistryWithTools(toolCtx *mcpctx.ToolContext) *ToolRegistry`
Creates a registry with all tools registered:
- `registerUserToolToRegistry(registry, toolCtx)`
- `registerNotificationToolToRegistry(registry, toolCtx)`
- `registerMemoryToolToRegistry(registry, toolCtx)`

---

## 12. MCP Tools: Memory

**File:** `internal/mcp/tools/memory.go`
**Package:** `tools`

### Purpose
Persistent fact storage across MCP sessions using a three-layer memory system.

### Actions Map

```go
var memoryActions = map[string][]string{
    "memory": {"store", "recall", "search", "list", "delete", "clear"},
}
```

### Input Struct

```go
type MemoryInput struct {
    Resource  string            `json:"resource"`   // "memory"
    Action    string            `json:"action"`     // store, recall, search, list, delete, clear

    Key       string            `json:"key,omitempty"`
    Value     string            `json:"value,omitempty"`

    Layer     string            `json:"layer,omitempty"`     // tacit, daily, entity
    Namespace string            `json:"namespace,omitempty"` // default: "default"

    Tags      []string          `json:"tags,omitempty"`
    Metadata  map[string]string `json:"metadata,omitempty"`

    Query     string            `json:"query,omitempty"`
}
```

### Memory Layers

| Layer | Purpose |
|-------|---------|
| `tacit` | Long-term preferences, learned behaviors |
| `daily` | Day-specific facts (keyed by date) |
| `entity` | Information about people, places, things |

### Namespace Resolution

Layer is prepended to namespace: if layer="tacit" and namespace="user", the resolved namespace is "tacit/user". Default namespace is "default".

### Registration

#### `RegisterMemoryTool(server *mcp.Server, toolCtx *mcpctx.ToolContext)`
Registers the "memory" tool on the MCP server with full description and examples.

### Handler

`memoryHandler(toolCtx)` returns a closure that:
1. Defaults resource to "memory" if empty
2. Validates resource against `memoryActions` map
3. Validates action against allowed actions for resource
4. Resolves namespace with layer prefix
5. Dispatches to action handler

### Action Handlers

#### `handleMemoryStore` -> `MemoryStoreOutput{Key, Namespace, Stored}`
Requires key + value. Marshals tags and metadata to JSON. Calls `DB.UpsertMemory`.

#### `handleMemoryRecall` -> `MemoryRecallOutput{Key, Value, Namespace, Tags, Metadata, CreatedAt, AccessCount}`
Requires key. Calls `DB.GetMemoryByKeyAndUser`. Increments access count via `DB.IncrementMemoryAccessByKey`. Filters out "null" and "{}" metadata.

#### `handleMemorySearch` -> `MemorySearchOutput{Query, Count, Results}`
Requires query. If namespace is specific, calls `DB.SearchMemoriesByUserAndNamespace`; otherwise `DB.SearchMemoriesByUser`. Limit 20, truncates values to 200 chars.

#### `handleMemoryList` -> `MemoryListOutput{Namespace, Count, Items}`
Calls `DB.ListMemoriesByUserAndNamespace`. Limit 50, truncates values to 100 chars.

#### `handleMemoryDelete` -> `MemoryDeleteOutput{Key, Namespace, Deleted}`
Requires key. Calls `DB.DeleteMemoryByKeyAndUser`. Returns not_found if 0 rows affected.

#### `handleMemoryClear` -> `MemoryClearOutput{Namespace, Cleared}`
Calls `DB.DeleteMemoriesByNamespaceAndUser`. Returns count of deleted rows.

---

## 13. MCP Tools: Notification

**File:** `internal/mcp/tools/notification.go`
**Package:** `tools`

### Purpose
Manages user notifications through MCP.

### Actions Map

```go
var notificationActions = map[string][]string{
    "notification": {"list", "get", "mark_read", "mark_all_read", "count_unread"},
}
```

### Input Struct

```go
type NotificationInput struct {
    Resource string `json:"resource"` // "notification"
    Action   string `json:"action"`

    ID     string `json:"id,omitempty"`
    Limit  int    `json:"limit,omitempty"`  // default 20
    Offset int    `json:"offset,omitempty"`
    Unread bool   `json:"unread,omitempty"`
}
```

### Output Structs

| Struct | Fields |
|--------|--------|
| `NotificationItem` | ID, Type, Title, Body, ActionURL, Read, CreatedAt |
| `NotificationListOutput` | Notifications []NotificationItem, Total |
| `NotificationGetOutput` | ID, Type, Title, Body, ActionURL, Read, CreatedAt |
| `NotificationMarkReadOutput` | ID, Read, Success |
| `NotificationMarkAllReadOutput` | Success, Message |
| `NotificationCountOutput` | Count |

### Action Handlers

- **list**: If `unread=true`, uses `ListUnreadNotifications`; otherwise `ListUserNotifications` with pagination
- **get**: Requires ID. Calls `GetNotification` with user scoping
- **mark_read**: Requires ID. Verifies ownership first, then calls `MarkNotificationRead`
- **mark_all_read**: Calls `MarkAllNotificationsRead` for the user
- **count_unread**: Calls `CountUnreadNotifications`

---

## 14. MCP Tools: User

**File:** `internal/mcp/tools/user.go`
**Package:** `tools`

### Purpose
Manages user profile and preferences through MCP.

### Actions Map

```go
var userActions = map[string][]string{
    "user":        {"get", "update"},
    "preferences": {"get", "update"},
}
```

### Input Struct

```go
type UserInput struct {
    Resource string `json:"resource"` // "user" or "preferences"
    Action   string `json:"action"`

    // User update
    Name string `json:"name,omitempty"`

    // Preferences
    Theme              string `json:"theme,omitempty"`
    Language           string `json:"language,omitempty"`
    Timezone           string `json:"timezone,omitempty"`
    EmailNotifications *bool  `json:"email_notifications,omitempty"`
    MarketingEmails    *bool  `json:"marketing_emails,omitempty"`
}
```

### User Resource Handlers

- **user.get** -> `UserGetOutput{ID, Email, Name, EmailVerified, CreatedAt}`: Returns profile from `toolCtx.User()`
- **user.update** -> `UserUpdateOutput{ID, Email, Name, Updated}`: Requires name. Calls `DB.UpdateUser`

### Preferences Resource Handlers

- **preferences.get** -> `PreferencesGetOutput{Theme, Language, Timezone, EmailNotifications, MarketingEmails}`: Returns from DB or defaults (theme="system", language="en", timezone="UTC", email_notifications=true, marketing_emails=false)
- **preferences.update** -> `PreferencesUpdateOutput{..., Updated}`: Creates preferences if they don't exist. Only updates fields that are set (non-empty strings, non-nil bools). Uses SQL `NullString`/`NullInt64` for conditional updates.

---

## 15. MCP Protocol Routes

Summary of all MCP-related HTTP routes:

### Well-Known Discovery
| Method | Path | Handler |
|--------|------|---------|
| GET | `/.well-known/oauth-protected-resource` | Protected resource metadata (RFC 9728) |
| GET | `/.well-known/oauth-authorization-server` | Authorization server metadata |

### MCP OAuth
| Method | Path | Handler |
|--------|------|---------|
| POST | `/mcp/oauth/register` | Dynamic Client Registration |
| GET | `/mcp/oauth/authorize` | Show login page |
| POST | `/mcp/oauth/authorize` | Process login form |
| POST | `/mcp/oauth/token` | Token exchange/refresh |

### MCP Protocol Endpoint
| Method | Path | Handler |
|--------|------|---------|
| POST/GET/DELETE | `/mcp` | Streamable HTTP (JSON-RPC over HTTP) |

### Agent MCP Integration
| Method | Path | Handler |
|--------|------|---------|
| GET | `/api/v1/integrations/oauth/callback` | OAuth callback from external servers |

---

## 16. CommPlugin Interface

**File:** `internal/agent/comm/plugin.go`
**Package:** `comm`

### Purpose
Defines the transport abstraction for inter-agent communication. Plugins run in-process (not via hashicorp/go-plugin RPC).

### Interface

```go
type CommPlugin interface {
    // Identity
    Name() string
    Version() string

    // Lifecycle
    Connect(ctx context.Context, config map[string]string) error
    Disconnect(ctx context.Context) error
    IsConnected() bool

    // Messaging
    Send(ctx context.Context, msg CommMessage) error
    Subscribe(ctx context.Context, topic string) error
    Unsubscribe(ctx context.Context, topic string) error

    // Registration with the comm network
    Register(ctx context.Context, agentID string, card *AgentCard) error
    Deregister(ctx context.Context) error

    // Message handler (set by CommPluginManager)
    SetMessageHandler(handler func(msg CommMessage))
}
```

### Method Contracts

| Method | Contract |
|--------|----------|
| `Name()` | Stable identifier (e.g., "neboloop", "loopback") |
| `Version()` | Semantic version string |
| `Connect(ctx, config)` | Establishes connection using string key-value config |
| `Disconnect(ctx)` | Graceful shutdown, idempotent |
| `IsConnected()` | Thread-safe connection state check |
| `Send(ctx, msg)` | Delivers a message, returns error on failure |
| `Subscribe(ctx, topic)` | Joins a topic/channel for receiving messages |
| `Unsubscribe(ctx, topic)` | Leaves a topic/channel |
| `Register(ctx, agentID, card)` | Publishes agent capabilities to the network |
| `Deregister(ctx)` | Removes agent from the network |
| `SetMessageHandler(handler)` | Wires in the callback for incoming messages |

---

## 17. CommMessage Types

**File:** `internal/agent/comm/types.go`
**Package:** `comm`

### Message Types

```go
type CommMessageType string

const (
    CommTypeMessage     CommMessageType = "message"      // General message
    CommTypeMention     CommMessageType = "mention"      // Direct mention, needs response
    CommTypeProposal    CommMessageType = "proposal"     // Vote request
    CommTypeCommand     CommMessageType = "command"      // Direct command (still goes through LLM)
    CommTypeInfo        CommMessageType = "info"         // Informational, may not need response
    CommTypeTask        CommMessageType = "task"         // Incoming A2A task request
    CommTypeTaskResult  CommMessageType = "task_result"  // Completed A2A task result
    CommTypeTaskStatus  CommMessageType = "task_status"  // Intermediate status update
    CommTypeLoopChannel CommMessageType = "loop_channel" // Loop channel message (bot-to-bot)
)
```

### Task Status Lifecycle

```go
type TaskStatus string

const (
    TaskStatusSubmitted     TaskStatus = "submitted"
    TaskStatusWorking       TaskStatus = "working"
    TaskStatusCompleted     TaskStatus = "completed"
    TaskStatusFailed        TaskStatus = "failed"
    TaskStatusCanceled      TaskStatus = "canceled"       // One 'l' per A2A spec
    TaskStatusInputRequired TaskStatus = "input-required"
)
```

Lifecycle: `submitted -> working -> completed | failed | canceled | input-required`

### Core Message Struct

```go
type CommMessage struct {
    ID             string            `json:"id"`
    From           string            `json:"from"`             // Agent ID or bot ID
    To             string            `json:"to"`               // Target agent (or "*" for broadcast)
    Topic          string            `json:"topic"`            // Discussion/channel name
    ConversationID string            `json:"conversation_id"`  // Thread/conversation grouping
    Type           CommMessageType   `json:"type"`
    Content        string            `json:"content"`
    Metadata       map[string]string `json:"metadata,omitempty"`
    Timestamp      int64             `json:"timestamp"`
    HumanInjected  bool              `json:"human_injected,omitempty"`
    HumanID        string            `json:"human_id,omitempty"`

    // A2A task lifecycle fields
    TaskID        string         `json:"task_id,omitempty"`
    CorrelationID string         `json:"correlation_id,omitempty"`
    TaskStatus    TaskStatus     `json:"task_status,omitempty"`
    Artifacts     []TaskArtifact `json:"artifacts,omitempty"`
    Error         string         `json:"error,omitempty"`
}
```

### A2A Types

```go
type ArtifactPart struct {
    Type string `json:"type"`           // "text", "data"
    Text string `json:"text,omitempty"`
    Data []byte `json:"data,omitempty"`
}

type TaskArtifact struct {
    Parts []ArtifactPart `json:"parts"`
}
```

### Agent Card (A2A Discovery)

```go
type AgentCard struct {
    Name               string            `json:"name"`
    Description        string            `json:"description,omitempty"`
    URL                string            `json:"url,omitempty"`
    PreferredTransport string            `json:"preferredTransport,omitempty"`
    ProtocolVersion    string            `json:"protocolVersion,omitempty"`
    DefaultInputModes  []string          `json:"defaultInputModes,omitempty"`
    DefaultOutputModes []string          `json:"defaultOutputModes,omitempty"`
    Capabilities       map[string]any    `json:"capabilities,omitempty"`
    Skills             []AgentCardSkill  `json:"skills,omitempty"`
    Provider           *AgentCardProvider `json:"provider,omitempty"`
}

type AgentCardSkill struct {
    ID          string   `json:"id"`
    Name        string   `json:"name"`
    Description string   `json:"description"`
    Tags        []string `json:"tags,omitempty"`
}

type AgentCardProvider struct {
    Organization string `json:"organization"`
}
```

### Optional Plugin Interfaces

```go
type LoopChannelLister interface {
    ListLoopChannels(ctx context.Context) ([]LoopChannelInfo, error)
}

type LoopLister interface {
    ListLoops(ctx context.Context) ([]LoopInfo, error)
}

type LoopGetter interface {
    GetLoopInfo(ctx context.Context, loopID string) (*LoopInfo, error)
}

type ChannelMessageLister interface {
    ListChannelMessages(ctx context.Context, channelID string, limit int) ([]ChannelMessageItem, error)
}

type ChannelMemberLister interface {
    ListChannelMembers(ctx context.Context, channelID string) ([]ChannelMemberItem, error)
}
```

### Supporting Types

```go
type LoopChannelInfo struct {
    ChannelID, ChannelName, LoopID, LoopName string
}

type LoopInfo struct {
    ID, Name, Description string
}

type ChannelMessageItem struct {
    ID, From, Content, CreatedAt, Role string
}

type ChannelMemberItem struct {
    BotID, BotName, Role string
    IsOnline             bool
}

type ManagerStatus struct {
    PluginName string
    Connected  bool
    Topics     []string
    AgentID    string
}
```

---

## 18. CommPluginManager

**File:** `internal/agent/comm/manager.go`
**Package:** `comm`

### Purpose
Manages loaded comm plugins and routes messages. Only one plugin is active at a time.

### Struct

```go
type CommPluginManager struct {
    plugins map[string]CommPlugin
    active  CommPlugin
    handler func(CommMessage)
    topics  []string
    mu      sync.RWMutex
}
```

### Functions

#### `NewCommPluginManager() *CommPluginManager`
Creates empty manager.

#### `Register(plugin CommPlugin)`
Adds a plugin to the registry (does not activate it).

#### `Unregister(name string)`
Removes a plugin. If the removed plugin was active, disconnects it and clears the active reference.

#### `SetActive(name string) error`
Activates a specific plugin.

**Algorithm:**
1. Look up plugin by name (returns error with available list if not found)
2. If another plugin is active and different, disconnect it
3. Set as active
4. Wire message handler into the plugin if handler is set

#### `GetActive() CommPlugin`
Returns currently active plugin (may be nil). Read lock.

#### `Send(ctx, msg) error`
Sends through active plugin. Returns error if no active plugin or not connected.

#### `Subscribe(ctx, topic) error`
Subscribes on active plugin. Tracks topic in internal list (deduplicates).

#### `Unsubscribe(ctx, topic) error`
Unsubscribes on active plugin. Removes from tracked topics.

#### `SetMessageHandler(handler func(CommMessage))`
Sets the callback for incoming messages. Wires into active plugin if one exists.

#### `ListTopics() []string`
Returns copy of subscribed topics.

#### `Status(agentID string) ManagerStatus`
Returns current status: plugin name, connected state, topics, agent ID.

#### `ListPlugins() []string`
Returns names of all registered plugins.

#### `Shutdown(ctx context.Context) error`
Disconnects all connected plugins, clears active reference and topics.

---

## 19. CommHandler

**File:** `internal/agent/comm/handler.go`
**Package:** `comm`

### Purpose
Processes incoming comm messages through the full agentic loop. Enqueues messages to the comm lane and uses `Runner.Run()` for processing -- the same agentic loop used by the main lane (same memories, tools, personality).

### Struct

```go
type CommHandler struct {
    manager *CommPluginManager
    runner  *runner.Runner
    lanes   *agenthub.LaneManager
    agentID string

    activeTasks   map[string]*activeTask
    activeTasksMu sync.Mutex
}

type activeTask struct {
    Cancel  context.CancelFunc
    Message CommMessage
}
```

### Constructor and Setup

#### `NewCommHandler(manager *CommPluginManager, agentID string) *CommHandler`
Creates handler with empty active tasks map.

#### `SetRunner(r *runner.Runner)`
Called after runner creation during agent startup.

#### `SetLanes(lanes *agenthub.LaneManager)`
Sets the lane manager for enqueueing work.

#### `GetManager() *CommPluginManager`
Returns the underlying plugin manager.

### Message Dispatch

#### `Handle(msg CommMessage)`
Main entry point, called by the plugin's message handler. Returns immediately (async).

**Algorithm:**
1. If runner or lanes not set, drop message with warning
2. Switch on `msg.Type`:
   - `CommTypeTask`:
     - If `TaskStatusCanceled`: call `cancelTask(msg.TaskID)`, return
     - Otherwise: enqueue to comm lane with cancellable context, track task
   - `CommTypeTaskResult`: enqueue to comm lane via `processTaskResult`
   - Default: enqueue to comm lane via `processMessage`

All enqueueing uses `lanes.EnqueueAsync` on `LaneComm`.

### Message Processing

#### `processMessage(ctx, msg) error`
Runs a general comm message through the agentic loop.

**Algorithm:**
1. Build session key: `comm-{topic}-{conversationID}` (falls back to msg.ID)
2. Build prompt: `[Comm Channel: {topic} | From: {from} | Type: {type}]\n\n{content}`
3. Call `runner.Run` with `Origin: tools.OriginComm`
4. Collect text events from stream
5. Send response back via `sendResponse`

#### `processTask(ctx, msg) error`
Handles an incoming A2A task request.

**Algorithm:**
1. Send "working" status
2. Session key: `task-{taskID}`
3. Prompt: `[A2A Task {taskID} from {from}]\n\n{content}`
4. Run through agentic loop with OriginComm
5. On context cancellation: return (status already sent by cancelTask)
6. On error: send task failure
7. On success: send task result with text artifact
8. If no output: send failure "no output produced"

#### `processTaskResult(ctx, msg) error`
Handles incoming A2A task results (from tasks we submitted).

**Algorithm:**
1. Session key: `task-result-{taskID}`
2. Prompt: `[A2A Task Result {taskID} | Status: {status}]\n\n{content}`
3. Run through agentic loop
4. Drain events (agent may take actions but no reply sent)

### Task Lifecycle Messages

#### `sendTaskStatus(ctx, original, status)`
Sends `CommTypeTaskStatus` with the task's lifecycle status (e.g., "working").

#### `sendTaskFailure(ctx, original, errMsg)`
Sends `CommTypeTaskResult` with `TaskStatusFailed` and error message.

#### `sendTaskResult(ctx, original, response)`
Sends `CommTypeTaskResult` with `TaskStatusCompleted` and a single text artifact.

### Task Tracking

#### `trackTask(taskID, cancel, msg)` / `untrackTask(taskID)`
Track/untrack active tasks for cancellation support.

#### `cancelTask(taskID)`
Cancels a running task's context. Sends canceled status back.

### Shutdown

#### `Shutdown(ctx context.Context)`
Cancels all in-progress tasks, sends failure status for each ("bot shutting down").

### CommService Interface Methods

These methods are used by the agent tool (via interface) to avoid import cycles:

| Method | Purpose |
|--------|---------|
| `Send(ctx, to, topic, content, msgType)` | Creates and sends a CommMessage |
| `Subscribe(ctx, topic)` | Delegates to manager |
| `Unsubscribe(ctx, topic)` | Delegates to manager |
| `ListTopics() []string` | Delegates to manager |
| `PluginName() string` | Returns active plugin name |
| `IsConnected() bool` | Returns active plugin connection state |
| `CommAgentID() string` | Returns this agent's ID |

---

## 20. Loopback Plugin

**File:** `internal/agent/comm/loopback.go`
**Package:** `comm`

### Purpose
In-memory comm plugin for testing and development. Delivers sent messages to logging only (does not loop back to handler by default). Provides `InjectMessage` for simulating incoming messages.

### Struct

```go
type LoopbackPlugin struct {
    handler   func(CommMessage)
    connected bool
    topics    map[string]bool
    agentID   string
    mu        sync.RWMutex
}
```

### Key Behaviors

- `Name()` returns `"loopback"`, `Version()` returns `"1.0.0"`
- `Connect`: sets connected=true, no external I/O
- `Disconnect`: sets connected=false, clears topics
- `Send`: logs the message but does NOT deliver to handler
- `Subscribe`/`Unsubscribe`: manage local topic set
- `Register`: stores agentID
- `SetMessageHandler`: stores handler function

### Testing Method

#### `InjectMessage(msg CommMessage)`
Simulates receiving a message from the network. Checks handler is set and topic is subscribed before delivering.

---

## 21. NeboLoop Plugin

**File:** `internal/agent/comm/neboloop/plugin.go`
**Package:** `neboloop`

### Purpose
Production CommPlugin that connects to the NeboLoop WebSocket gateway via the published NeboLoop Comms SDK. Handles DMs, loop channel messages, A2A tasks, history requests, voice streams, account events, and app installs. Features auto-reconnect with exponential backoff and token refresh.

### Dedicated Logger

```go
var commLog *slog.Logger
```

Writes all comms traffic to `{dataDir}/logs/comms.log` (file-based, always Debug level, independent of global log level). Initialized in `init()`.

### Message Types

#### `LoopChannelMessage`
```go
type LoopChannelMessage struct {
    ChannelID      string `json:"channelId"`
    ChannelName    string `json:"channelName,omitempty"`
    LoopID         string `json:"loopId,omitempty"`
    SenderID       string `json:"senderId,omitempty"`
    SenderName     string `json:"senderName,omitempty"`
    Text           string `json:"text"`
    Role           string `json:"role,omitempty"`   // "user" for owner relay
    ConversationID string `json:"-"`                // from Message envelope
    MessageID      string `json:"-"`                // from Message envelope
}
```

#### `DMMessage`
```go
type DMMessage struct {
    SenderID       string
    Text           string
    ConversationID string
    MessageID      string
    IsOwner        bool   // true when sender is bot's owner
    PeerType       string // "bot" or "person"
}
```

#### `HistoryRequest` / `HistoryMessage`
```go
type HistoryRequest struct {
    ConversationID string
    Limit          int   // default 20
}

type HistoryMessage struct {
    Role      string `json:"role"`      // "user" or "assistant"
    Content   string `json:"content"`
    Timestamp int64  `json:"timestamp"` // unix seconds
}
```

#### `AccountEvent`
```go
type AccountEvent struct {
    Type    string          `json:"type"`
    Payload json.RawMessage `json:"payload"`
}
```

#### `VoiceMessage`
```go
type VoiceMessage struct {
    ConversationID string          `json:"conversationId"`
    SenderID       string          `json:"senderId"`
    Content        json.RawMessage `json:"content"` // raw JSON
}
```

### Plugin Struct

```go
type Plugin struct {
    client  *neboloopsdk.Client
    handler func(comm.CommMessage)

    agentID   string
    botID     string
    apiServer string
    gateway   string
    token     string
    ownerID   string   // JWT sub claim

    card *comm.AgentCard  // For re-publish on reconnect

    connected    bool
    authDead     bool     // credentials rejected, stop reconnecting
    reconnecting bool     // prevents concurrent reconnect
    done         chan struct{}
    healthDone   chan struct{}  // per-connection signal
    mu           sync.RWMutex

    // Typed handlers
    onInstall            func(neboloopsdk.InstallEvent)
    onLoopChannelMessage func(LoopChannelMessage)
    onDMMessage          func(DMMessage)
    onHistoryRequest     func(HistoryRequest) []HistoryMessage
    onAccountEvent       func(AccountEvent)
    onVoiceMessage       func(VoiceMessage)
    onConnected          func()

    // Loop channel tracking: channelID -> conversationID
    channelConvs map[string]string

    // Token refresh callback
    tokenRefresher func(ctx context.Context) (string, error)

    // Owner DM conversation ID cache
    ownerConvID string
}
```

### Identity

- `Name()` returns `"neboloop"`
- `Version()` returns `"4.0.0"`

### Handler Registration

| Method | Callback Type |
|--------|--------------|
| `OnInstall(fn)` | Install events from NeboLoop |
| `OnLoopChannelMessage(fn)` | Loop channel messages |
| `OnDMMessage(fn)` | Direct messages |
| `OnHistoryRequest(fn)` | History requests (returns messages) |
| `OnAccountEvent(fn)` | Account events (plan changes) |
| `OnVoiceMessage(fn)` | Voice stream frames |
| `OnConnected(fn)` | Fires after connect/reconnect |
| `SetTokenRefresher(fn)` | JWT refresh callback |
| `SetMessageHandler(handler)` | CommPlugin interface handler |

### Connect

#### `Connect(ctx context.Context, config map[string]string) error`

**Config keys:**
- `gateway` -- WebSocket URL (required, e.g., "wss://comms.neboloop.com")
- `api_server` -- NeboLoop REST API URL (derived from gateway if missing)
- `bot_id` -- Bot UUID
- `token` -- Owner OAuth JWT (required)

**Algorithm:**
1. Validate: must not already be connected
2. Store config values (gateway, botID, apiServer, token)
3. Derive apiServer from gateway if not set (wss://X/ws -> https://X)
4. Extract ownerID from JWT sub claim (unsafe decode, just for routing)
5. Connect via `neboloopsdk.Connect` with config and `handleMessage` callback
6. On auth failure: try token refresh once, retry connect
7. Set connected=true, authDead=false
8. Reset done channel, create healthDone channel
9. Start `watchConnection` goroutine
10. Wire SDK install handler
11. Wire SDK loop message handler:
    - Parse JSON content for text, senderName, channelName, loopID, metadata.role
    - Look up channel metadata from SDK
    - Track channel->conversation mapping
    - Dispatch to `onLoopChannelMessage`
12. Wire SDK DM handler:
    - Parse content for text
    - Set `IsOwner` based on ownerID match
    - Set PeerType from SDK enrichment
    - Dispatch to `onDMMessage`
13. Subscribe to bot streams: "dm", "installs", "chat", "account", "voice"
14. Log channel subscriptions after 2s delay
15. Fire `onConnected` callback in goroutine

### Message Dispatch

#### `handleMessage(msg neboloopsdk.Message)`
Single handler for all incoming SDK messages. Dispatches by `msg.Stream`:

| Stream | Handler |
|--------|---------|
| `"dm"` | Skipped (handled by `client.OnDM()` to avoid double-processing) |
| `"history"` | `handleHistoryRequest` |
| `"account"` | Parse AccountEvent, refresh token on plan_changed, fire handler |
| `"voice"` | Fire `onVoiceMessage` |
| `"a2a"` or `"a2a/*"` | `handleA2AMessage` |

#### `handleHistoryRequest(msg)`
1. Parse limit from content (default 20)
2. Call `onHistoryRequest` callback
3. Marshal response as `{"messages": [...]}`
4. Send back on `history_response` stream

#### `handleA2AMessage(msg)`
Probes content JSON for message subtype:

1. **Task result** (has `status` field): maps to `CommTypeTaskResult`
2. **Task submission** (has `input` field): maps to `CommTypeTask` with `TaskStatusSubmitted`
3. **Direct message** (fallback): maps to `CommTypeMessage`

### Sending Messages

#### `SendDM(ctx, conversationID, text) error`
Sends a DM via SDK. Logs to comms.log.

#### `RelayOwnerDM(ctx, conversationID, text) error`
Sends a DM attributed to the owner (bidirectional sync). Adds metadata `{"relay": true, "role": "user"}`.

#### `SendVoice(ctx, conversationID string, content []byte) error`
Sends voice stream frame on "voice" stream.

#### `SendTyping(ctx, conversationID string, typing bool)`
Sends typing indicator on "typing" stream.

#### `Send(ctx context.Context, msg comm.CommMessage) error`
CommPlugin interface implementation. Dispatches by message type:

| Type | Wire Protocol |
|------|--------------|
| `CommTypeTask` | Marshals as `neboloopsdk.TaskSubmission`, sends on "a2a" stream |
| `CommTypeTaskResult` / `CommTypeTaskStatus` | Marshals as `neboloopsdk.TaskResult`, sends on "a2a" stream |
| `CommTypeLoopChannel` | Resolves channel->conversation mapping, sends on "channel" stream with relay metadata if human-injected |
| Default | Marshals as `neboloopsdk.DirectMessage`, sends on "a2a" stream |

#### `SendLoopChannelMessage(ctx, channelID, conversationID, text) error`
Sends a message to a specific loop channel.

### Owner DM Management

#### `SetOwnerConversationID(id string)`
Caches the owner's DM conversation ID.

#### `OwnerConversationID() string`
Returns cached owner conversation ID. If not cached, looks up from SDK's DM tracking table.

### Subscribe/Unsubscribe

- `Subscribe`: joins a conversation via SDK
- `Unsubscribe`: no-op (no explicit leave support)

### Agent Registration

#### `Register(ctx, agentID, card) error`
Stores card for re-publish on reconnect. Card intended to be published via REST API (TODO: POST to `{apiServer}/api/v1/bots/{botID}/card`).

#### `Deregister(ctx) error`
Clears stored card.

### Disconnect

#### `Disconnect(_ context.Context) error`
Graceful shutdown.

**Algorithm:**
1. Close healthDone channel (stops watchdog + health checker)
2. Close done channel (stops reconnect goroutine)
3. Close SDK client
4. Set connected=false, client=nil

### Auto-Reconnect

#### `watchConnection(client *neboloopsdk.Client)`
Waits for either:
- `p.done` closes (shutdown)
- `p.healthDone` closes (connection replaced by reconnect)
- `client.Done()` closes (SDK detected disconnect)

On SDK disconnect: sets connected=false, client=nil, calls `reconnect()`.

#### `reconnect()`
Exponential backoff reconnection. **Never stops retrying** unless credentials are permanently rejected.

**Algorithm:**
1. Guard against concurrent reconnect via `reconnecting` flag
2. If authDead, return immediately
3. Close old connection's healthDone channel
4. Backoff parameters: base=100ms, max=10min
5. Loop forever:
   - Check p.done (shutdown signal)
   - Calculate delay: `100ms * 2^min(attempt, 9)` with +/-25% jitter
   - Sleep for delay
   - Attempt connect with 10s timeout
   - On auth failure:
     - Try token refresh
     - If refresh succeeds: update token, continue loop
     - If refresh fails: set authDead=true, return
   - On transient failure: continue (never give up)
   - On success:
     - Store new client, set connected=true
     - Create new healthDone channel
     - Refresh token for latest JWT claims
     - Re-wire all typed handlers (OnInstall, OnLoopMessage, OnDM)
     - Re-subscribe to bot streams
     - Re-register agent card if stored
     - Start new watchConnection goroutine
     - Fire onConnected callback
     - Return

### Settings Change

#### `OnSettingsChanged(newSettings map[string]string) error`
Implements `settings.Configurable`. If connected, disconnects and reconnects with new settings.

### REST API Methods

These use `neboloopsdk.APIClient` for REST calls:

| Method | Returns |
|--------|---------|
| `ListLoopChannels(ctx)` | `[]comm.LoopChannelInfo` -- prefers in-memory cache, falls back to REST |
| `ListBotLoops(ctx)` | `[]neboloopsdk.Loop` |
| `GetLoop(ctx, loopID)` | `*neboloopsdk.Loop` |
| `UpdateBotIdentity(ctx, name, role)` | error |
| `ListLoopMembers(ctx, loopID)` | `[]neboloopsdk.LoopMember` |
| `ListChannelMembers(ctx, channelID)` | `[]comm.ChannelMemberItem` |
| `ListChannelMessages(ctx, channelID, limit)` | `[]comm.ChannelMessageItem` |
| `ListLoops(ctx)` | `[]comm.LoopInfo` |
| `GetLoopInfo(ctx, loopID)` | `*comm.LoopInfo` |

### Helper Functions

#### `jwtSubClaim(token string) string`
Extracts "sub" claim from JWT by base64-decoding the payload segment without verification. Safe because the token is our own -- used only for routing decisions, not security.

#### `truncateForLog(s string, maxLen int) string`
Truncates a string, appending "..." if truncated.

#### `deriveAPIServerURL(gateway string) string`
Converts gateway WebSocket URL to REST API base: `wss://X/ws` -> `https://X`.

### Compile-Time Interface Checks

```go
var (
    _ comm.CommPlugin        = (*Plugin)(nil)
    _ settings.Configurable  = (*Plugin)(nil)
    _ comm.LoopChannelLister = (*Plugin)(nil)
    _ comm.LoopLister        = (*Plugin)(nil)
    _ comm.LoopGetter        = (*Plugin)(nil)
)
```

---

## Architecture Summary

### MCP Data Flow (Nebo as MCP Server)

```
External Client (Claude Desktop, etc.)
  |
  | HTTP POST /mcp (Bearer token)
  |
  v
Handler.authMiddleware
  |-- Validate JWT (same secret as main API)
  |-- Generate/validate session ID
  |-- Add tokenInfo to context
  |
  v
mcp.StreamableHTTPHandler (SDK, STATELESS mode)
  |
  v
Handler.getServerForRequest
  |-- Check session cache (sync.Map)
  |-- Create new server if needed (NewServerWithContext)
  |-- Register tools: user, notification, memory
  |
  v
Tool execution (user-scoped via ToolContext)
```

### MCP Data Flow (Nebo as MCP Client)

```
Nebo Agent
  |
  | CallTool("mcp__github__create_issue", input)
  |
  v
Bridge.proxyTool.Execute
  |
  v
mcpclient.Client.CallTool
  |-- getOrCreateSession (cached, 30min max age)
  |   |-- AuthenticatedTransport (auto-refresh Bearer token)
  |   |-- mcp.StreamableClientTransport (JSON-RPC over HTTP)
  |
  v
External MCP Server (e.g., GitHub)
```

### Communication Data Flow

```
NeboLoop Gateway (wss://comms.neboloop.com/ws)
  |
  | WebSocket (binary framing + JSON)
  |
  v
neboloopsdk.Client
  |-- handleMessage (dispatch by stream)
  |
  +-- stream="dm"     --> client.OnDM()  --> plugin.onDMMessage
  +-- stream="a2a"    --> handleA2AMessage --> comm.CommHandler
  +-- stream="history" --> handleHistoryRequest --> respond
  +-- stream="account" --> onAccountEvent
  +-- stream="voice"   --> onVoiceMessage
  +-- channel msgs     --> client.OnLoopMessage --> onLoopChannelMessage
  |
  v
CommHandler.Handle (async, enqueued to comm lane)
  |
  v
runner.Run (full agentic loop, Origin: OriginComm)
  |
  v
Response sent back via plugin.Send / plugin.SendDM
```

### Reconnect Strategy

Both the MCP client transport and the NeboLoop plugin use the same reconnect strategy:
- **Base delay:** 100ms
- **Backoff:** Exponential `2^min(attempt, 9)` (caps at ~50s before max delay)
- **Max delay:** 10 minutes
- **Jitter:** +/-25% of computed delay
- **Behavior:** Never stop retrying on transient errors. Only give up on permanent auth rejection.
- **Rationale:** At scale (1M+ users), a 60s max would create thundering herd. 10 minutes allows graceful stagger.
