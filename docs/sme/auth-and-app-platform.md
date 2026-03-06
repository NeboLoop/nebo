# Auth & App Platform: Comprehensive Logic Deep-Dive

This document provides a complete reference for the authentication system and app platform
in the Go codebase. Every function signature, struct, constant, and flow is documented
from the actual source code.

---

## Table of Contents

1. [Authentication System](#authentication-system)
   - [Architecture Overview](#auth-architecture-overview)
   - [Auth Service Core (`internal/local/auth.go`)](#auth-service-core)
   - [JWT Module (`internal/auth/jwt.go`)](#jwt-module)
   - [Password Module (`internal/auth/password.go`)](#password-module)
   - [Auth Handlers (`internal/handler/auth/`)](#auth-handlers)
   - [Auth Types (`internal/types/types.go`)](#auth-types)
   - [Auth Config (`internal/config/config.go`)](#auth-config)
   - [Flow Diagrams](#auth-flow-diagrams)
2. [App Platform](#app-platform)
   - [Architecture Overview](#app-architecture-overview)
   - [Manifest (`internal/apps/manifest.go`)](#manifest)
   - [NApp Extraction (`internal/apps/napp.go`)](#napp-extraction)
   - [Runtime (`internal/apps/runtime.go`)](#runtime)
   - [Sandbox (`internal/apps/sandbox.go`)](#sandbox)
   - [Signing (`internal/apps/signing.go`)](#signing)
   - [Supervisor (`internal/apps/supervisor.go`)](#supervisor)
   - [File Watcher (`internal/apps/watcher.go`)](#file-watcher)
   - [gRPC Adapters (`internal/apps/adapter.go`)](#grpc-adapters)
   - [Schedule Adapter (`internal/apps/schedule_adapter.go`)](#schedule-adapter)
   - [Hooks (`internal/apps/hooks.go`)](#hooks)
   - [Registry (`internal/apps/registry.go`)](#registry)
   - [Install (`internal/apps/install.go`)](#install)
   - [Inspector (`internal/apps/inspector/`)](#inspector)
   - [Settings Store (`internal/apps/settings/`)](#settings-store)
   - [Channel Envelope (`internal/apps/envelope.go`)](#channel-envelope)
   - [Process Management](#process-management)
   - [Protobuf Definitions (`proto/apps/v0/`)](#protobuf-definitions)

---

## Authentication System

### Auth Architecture Overview

The auth system has three layers:

1. **Handlers** (`internal/handler/auth/`) -- HTTP handlers that parse requests and call the service
2. **Auth Service** (`internal/local/auth.go`) -- Business logic: user CRUD, token generation, password management
3. **Auth Utilities** (`internal/auth/`) -- Standalone JWT and password utilities (used by both local auth and JWT middleware)

The handler layer is intentionally thin. Each handler:
- Parses the request using `httputil.Parse`
- Checks `svcCtx.Auth != nil`
- Delegates to the `AuthService`
- Returns a typed JSON response

### Auth Service Core

**File:** `internal/local/auth.go`
**Package:** `local`

#### Sentinel Errors

```go
var (
    ErrUserNotFound       = errors.New("user not found")
    ErrInvalidCredentials = errors.New("invalid credentials")
    ErrEmailExists        = errors.New("email already exists")
    ErrInvalidToken       = errors.New("invalid or expired token")
)
```

#### Structs

```go
type AuthService struct {
    store  *db.Store
    config config.Config
}

type AuthResponse struct {
    Token        string
    RefreshToken string
    ExpiresAt    time.Time
    CheckoutURL  string // Only set during registration with paid plan
}
```

#### Constructor

```go
func NewAuthService(store *db.Store, cfg config.Config) *AuthService
```

#### Register

```go
func (s *AuthService) Register(ctx context.Context, email, password, name string) (*AuthResponse, error)
```

Flow:
1. Check if email already exists via `s.store.CheckEmailExists(ctx, email)` -- returns `ErrEmailExists` if count == 1
2. Hash password with `bcrypt.GenerateFromPassword([]byte(password), bcrypt.DefaultCost)` -- **Note:** uses `bcrypt.DefaultCost` (10), NOT the bcrypt cost 12 defined in `internal/auth/password.go`
3. Create user via `s.store.CreateUser()` with params: `{ID: generateID(), Email, PasswordHash, Name}`
4. Create default preferences via `s.store.CreateUserPreferences(ctx, user.ID)`
5. Generate and return tokens via `s.generateTokens(ctx, user.ID, user.Email)`

**Important discrepancy:** The auth service in `internal/local/auth.go` uses `bcrypt.DefaultCost` (10) while `internal/auth/password.go` defines `bcryptCost = 12`. The handler does NOT call `ValidatePassword()` from the password module -- there is no password strength validation at the service level. The password module exists as a standalone utility but is not wired into the auth service.

#### Login

```go
func (s *AuthService) Login(ctx context.Context, email, password string) (*AuthResponse, error)
```

Flow:
1. Look up user by email via `s.store.GetUserByEmail(ctx, email)` -- returns `ErrInvalidCredentials` if `sql.ErrNoRows`
2. Verify password via `bcrypt.CompareHashAndPassword([]byte(user.PasswordHash), []byte(password))` -- returns `ErrInvalidCredentials` on mismatch
3. Generate and return tokens via `s.generateTokens(ctx, user.ID, user.Email)`

**Security note:** Login does NOT check `EmailVerified` -- unverified users can log in.

#### RefreshToken

```go
func (s *AuthService) RefreshToken(ctx context.Context, refreshToken string) (*AuthResponse, error)
```

Flow:
1. Hash the incoming refresh token via `hashToken(refreshToken)`
2. Look up the token hash in DB via `s.store.GetRefreshTokenByHash(ctx, tokenHash)` -- returns `ErrInvalidToken` if not found
3. Look up user by `token.UserID` -- returns `ErrUserNotFound` if gone
4. **Delete the old token** via `s.store.DeleteRefreshToken(ctx, tokenHash)` -- this implements **token rotation**: each refresh token is single-use
5. Generate new token pair via `s.generateTokens(ctx, user.ID, user.Email)`

**Key behavior:** Refresh tokens are opaque hex strings stored as hashes in `refresh_tokens` table. They are NOT JWTs (unlike the access token). Each use deletes the old token and creates a new one.

#### VerifyEmail

```go
func (s *AuthService) VerifyEmail(ctx context.Context, token string) error
```

**Currently a no-op** -- returns `nil` always. Email verification is not implemented in the local auth service.

#### CreatePasswordResetToken

```go
func (s *AuthService) CreatePasswordResetToken(ctx context.Context, email string) (string, error)
```

Flow:
1. Look up user by email -- returns `""` (empty string) if not found (does NOT reveal whether email exists)
2. Generate a random 32-byte hex token via `generateToken()`
3. Set expiry to `time.Now().Add(1 * time.Hour).Unix()`
4. Store token and expiry in DB via `s.store.SetPasswordResetToken()` with params: `{ID: user.ID, Token: NullString{token}, Expires: NullInt64{expires}}`
5. Return the token string

**Security:** Non-existent emails silently return empty string. The handler always returns the same success message regardless.

#### ResetPassword

```go
func (s *AuthService) ResetPassword(ctx context.Context, token, newPassword string) error
```

Flow:
1. Look up user by reset token via `s.store.GetUserByPasswordResetToken(ctx, NullString{token})`
2. Returns `ErrInvalidToken` if not found
3. Hash new password with `bcrypt.GenerateFromPassword([]byte(newPassword), bcrypt.DefaultCost)`
4. Update password AND clear reset token via `s.store.UpdateUserPassword()`

**Note:** Token expiry is NOT checked in Go code -- it relies on the SQL query to filter expired tokens. No password strength validation is applied.

#### ChangePassword

```go
func (s *AuthService) ChangePassword(ctx context.Context, userID, currentPassword, newPassword string) error
```

Flow:
1. Get user by ID
2. Verify current password with bcrypt
3. Hash new password with `bcrypt.DefaultCost`
4. Update via `s.store.UpdateUserPassword()`

#### GenerateTokensForUser

```go
func (s *AuthService) GenerateTokensForUser(ctx context.Context, userID, email string) (*AuthResponse, error)
```

Public wrapper around `generateTokens()`. Used for admin login bypass scenarios.

#### Internal Token Generation

```go
func (s *AuthService) generateTokens(ctx context.Context, userID, email string) (*AuthResponse, error)
```

Flow:
1. Calculate `accessExpiry = now + config.Auth.AccessExpire seconds`
2. Calculate `refreshExpiry = now + config.Auth.RefreshTokenExpire seconds`
3. Create JWT access token with claims:
   ```json
   {
     "userId": "<user-id>",
     "email": "<email>",
     "iat": <unix-timestamp>,
     "exp": <unix-timestamp>
   }
   ```
   Signed with `jwt.SigningMethodHS256` using `config.Auth.AccessSecret`
4. Generate opaque refresh token: 32 random bytes -> hex string (64 chars)
5. Hash the refresh token and store in DB via `s.store.CreateRefreshToken()`:
   ```go
   {ID: generateID(), UserID: userID, TokenHash: tokenHash, ExpiresAt: refreshExpiry.Unix()}
   ```
6. Return `AuthResponse{Token: accessToken, RefreshToken: refreshToken, ExpiresAt: accessExpiry}`

**Critical difference from `internal/auth/jwt.go`:** The local auth service creates:
- Access token: JWT with HS256
- Refresh token: Opaque hex string stored hashed in DB

The `internal/auth/jwt.go` module creates BOTH as JWTs (including a "type":"refresh" claim). The local auth service does NOT use `internal/auth/jwt.go` for token generation -- it has its own implementation.

#### Utility Functions

```go
func generateID() string     // 16 random bytes -> 32-char hex string
func generateToken() string  // 32 random bytes -> 64-char hex string
func hashToken(token string) string  // copies first 32 bytes of token into buffer -> hex
```

**Warning:** `hashToken` is NOT a cryptographic hash. It copies the first 32 bytes of the token string into a fixed 32-byte buffer and hex-encodes it. This means the "hash" is effectively the first 32 bytes of the token, which is reversible. This is a potential security concern for token storage.

### JWT Module

**File:** `internal/auth/jwt.go`
**Package:** `auth`

This module is used by the JWT middleware for token validation and by context extraction helpers. It is NOT used by the local auth service for token generation.

#### Sentinel Errors

```go
var (
    ErrInvalidToken      = errors.New("invalid token")
    ErrExpiredToken      = errors.New("token has expired")
    ErrInvalidClaims     = errors.New("invalid token claims")
    ErrMissingUserID     = errors.New("missing user ID in token")
    ErrInvalidSignMethod = errors.New("invalid signing method")
)
```

#### TokenPair Struct

```go
type TokenPair struct {
    AccessToken  string
    RefreshToken string
    ExpiresAt    int64  // Unix timestamp in milliseconds
}
```

#### GenerateTokens

```go
func GenerateTokens(userID, email, name, accessSecret string, accessExpireSecs, refreshExpireSecs int64) (*TokenPair, error)
```

Creates both access and refresh tokens as JWTs:

**Access token claims:**
```json
{
  "userId": "<user-id>",
  "email": "<email>",
  "name": "<name>",
  "iat": <unix-seconds>,
  "exp": <unix-seconds>
}
```

**Refresh token claims:**
```json
{
  "userId": "<user-id>",
  "email": "<email>",
  "type": "refresh",
  "iat": <unix-seconds>,
  "exp": <unix-seconds>
}
```

Both signed with `jwt.SigningMethodHS256` using the same `accessSecret`.

**Note:** This function includes `name` in access token claims, which the local auth service does NOT. The refresh token here is a JWT with a `"type":"refresh"` marker, unlike the opaque hex token in the local auth service.

#### Context Extraction Helpers

```go
func GetUserIDFromContext(ctx interface{ Value(any) any }) (uuid.UUID, error)
```
- Tries `ctx.Value("userId")` first, then `ctx.Value("sub")` as fallback
- Parses as `uuid.UUID`

```go
func GetEmailFromContext(ctx interface{ Value(any) any }) (string, error)
```
- Reads `ctx.Value("email")`

```go
func GetCustomerIDFromContext(ctx interface{ Value(any) any }) (string, error)
```
- Tries `ctx.Value("customer_id")`, then `"sub"`, then `"userId"` as fallbacks
- Returns as plain string (not UUID)

### Password Module

**File:** `internal/auth/password.go`
**Package:** `auth`

#### Constants

```go
const (
    bcryptCost        = 12   // Higher than bcrypt.DefaultCost (10)
    minPasswordLength = 8
    maxPasswordLength = 72   // bcrypt's internal limit
)
```

#### Sentinel Errors

```go
var (
    ErrPasswordTooShort  = errors.New("password must be at least 8 characters")
    ErrPasswordTooLong   = errors.New("password must be at most 72 characters")
    ErrPasswordNoUpper   = errors.New("password must contain at least one uppercase letter")
    ErrPasswordNoLower   = errors.New("password must contain at least one lowercase letter")
    ErrPasswordNoDigit   = errors.New("password must contain at least one digit")
    ErrPasswordNoSpecial = errors.New("password must contain at least one special character")
    ErrInvalidEmail      = errors.New("invalid email format")
    ErrEmailTooLong      = errors.New("email must be at most 255 characters")
    ErrNameTooShort      = errors.New("name must be at least 1 character")
    ErrNameTooLong       = errors.New("name must be at most 100 characters")
)
```

#### Email Regex

```go
var emailRegex = regexp.MustCompile(`^[a-zA-Z0-9._%+\-]+@[a-zA-Z0-9.\-]+\.[a-zA-Z]{2,}$`)
```

#### Functions

```go
func HashPassword(password string) (string, error)
```
- Uses `bcrypt.GenerateFromPassword` with cost factor **12**

```go
func VerifyPassword(password, hash string) bool
```
- Uses `bcrypt.CompareHashAndPassword`

```go
func ValidatePassword(password string) error
```
- Checks length (8-72 chars)
- Requires: uppercase, lowercase, digit, special character (punct or symbol per `unicode`)

```go
func ValidateEmail(email string) error
```
- Trims whitespace, checks length <= 255, validates against regex

```go
func ValidateName(name string) error
```
- Trims whitespace, checks length 1-100

```go
func NormalizeEmail(email string) string
```
- `strings.ToLower(strings.TrimSpace(email))`

**Important:** This module is a standalone utility. The auth service (`internal/local/auth.go`) does NOT call `ValidatePassword`, `ValidateEmail`, `ValidateName`, or `NormalizeEmail`. These would need to be explicitly wired in.

### Auth Handlers

**File:** `internal/handler/auth/` (8 files)
**Package:** `auth`

All handlers follow the same pattern:
1. Parse request with `httputil.Parse(r, &req)`
2. Check `svcCtx.Auth != nil`
3. Call the appropriate auth service method
4. Return JSON response

A shared logger is defined in `verifyemailhandler.go`:
```go
var authLog = logging.L("Auth")
```

#### LoginHandler

```go
func LoginHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```
- Request: `types.LoginRequest{Email, Password}`
- Calls: `svcCtx.Auth.Login(r.Context(), req.Email, req.Password)`
- Response: `types.LoginResponse{Token, RefreshToken, ExpiresAt}` where `ExpiresAt` is `authResp.ExpiresAt.UnixMilli()`

#### RegisterHandler

```go
func RegisterHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```
- Request: `types.RegisterRequest{Email, Password, Name}`
- Calls: `svcCtx.Auth.Register(r.Context(), req.Email, req.Password, req.Name)`
- Response: `types.LoginResponse{Token, RefreshToken, ExpiresAt}` -- reuses `LoginResponse` type

#### VerifyEmailHandler

```go
func VerifyEmailHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```
- Request: `types.EmailVerificationRequest{Token}`
- Calls: `svcCtx.Auth.VerifyEmail(r.Context(), req.Token)`
- Response: `types.MessageResponse{Message: "Email verified successfully."}`
- **Currently a no-op** since `VerifyEmail` always returns nil

#### ResendVerificationHandler

```go
func ResendVerificationHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```
- Request: `types.ResendVerificationRequest{Email}`
- **Does NOT call any auth service method** -- always returns success
- Response: `types.MessageResponse{Message: "If the email address is registered and unverified, a new verification email has been sent."}`
- Stub implementation -- no email is actually sent

#### ForgotPasswordHandler

```go
func ForgotPasswordHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```
- Request: `types.ForgotPasswordRequest{Email}`
- Calls: `svcCtx.Auth.CreatePasswordResetToken(r.Context(), req.Email)`
- If token is non-empty AND `svcCtx.Email != nil`:
  - Constructs reset URL: `{baseURL}/auth/reset-password?token={token}`
  - Sends HTML email via `svcCtx.Email.SendEmail()` with:
    - Subject: `"Reset your password"`
    - HTML body with styled button linking to reset URL
    - Plain text fallback
    - Expiry notice: "This link will expire in 1 hour"
- Always returns: `types.MessageResponse{Message: "If an account with that email exists, a password reset link has been sent."}`
- **Security:** Same response regardless of whether email exists or email was sent

#### ResetPasswordHandler

```go
func ResetPasswordHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```
- Request: `types.ResetPasswordRequest{Token, NewPassword}`
- Calls: `svcCtx.Auth.ResetPassword(r.Context(), req.Token, req.NewPassword)`
- Response: `types.MessageResponse{Message: "Password has been reset successfully."}`

#### RefreshTokenHandler

```go
func RefreshTokenHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```
- Request: `types.RefreshTokenRequest{RefreshToken}`
- Calls: `svcCtx.Auth.RefreshToken(r.Context(), req.RefreshToken)`
- Response: `types.RefreshTokenResponse{Token, RefreshToken, ExpiresAt}` where `ExpiresAt` is `authResp.ExpiresAt.UnixMilli()`

#### GetAuthConfigHandler

```go
func GetAuthConfigHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```
- No request body
- Logic:
  - `googleEnabled = false`, `githubEnabled = false`
  - If `svcCtx.UseLocal()` AND `svcCtx.Config.IsOAuthEnabled()`:
    - Check `svcCtx.Config.IsGoogleOAuthEnabled()`
    - Check `svcCtx.Config.IsGitHubOAuthEnabled()`
- Response: `types.AuthConfigResponse{GoogleEnabled, GitHubEnabled}`
- **Purpose:** Tells the frontend which OAuth providers are available

### Auth Types

**File:** `internal/types/types.go`

```go
type LoginRequest struct {
    Email    string `json:"email"`
    Password string `json:"password"`
}

type RegisterRequest struct {
    Email    string `json:"email"`
    Password string `json:"password"`
    Name     string `json:"name"`
}

type LoginResponse struct {
    Token        string `json:"token"`
    RefreshToken string `json:"refreshToken"`
    ExpiresAt    int64  `json:"expiresAt"`
}

type RefreshTokenRequest struct {
    RefreshToken string `json:"refreshToken"`
}

type RefreshTokenResponse struct {
    Token        string `json:"token"`
    RefreshToken string `json:"refreshToken"`
    ExpiresAt    int64  `json:"expiresAt"`
}

type ForgotPasswordRequest struct {
    Email string `json:"email"`
}

type ResetPasswordRequest struct {
    Token       string `json:"token"`
    NewPassword string `json:"newPassword"`
}

type EmailVerificationRequest struct {
    Token string `json:"token"`
}

type ResendVerificationRequest struct {
    Email string `json:"email"`
}

type AuthConfigResponse struct {
    GoogleEnabled bool `json:"googleEnabled"`
    GitHubEnabled bool `json:"githubEnabled"`
}

type MessageResponse struct {
    Message string `json:"message"`
}

type ChangePasswordRequest struct {
    CurrentPassword string `json:"currentPassword"`
    NewPassword     string `json:"newPassword"`
}
```

### Auth Config

**File:** `internal/config/config.go`

```go
Auth struct {
    AccessSecret       string `yaml:"AccessSecret"`
    AccessExpire       int64  `yaml:"AccessExpire"`       // seconds, no default set in code
    RefreshTokenExpire int64  `yaml:"RefreshTokenExpire"`  // seconds, default: 604800 (7 days)
}
```

Defaults applied in `applyDefaults()`:
- `RefreshTokenExpire`: 604800 seconds (7 days) if not set
- `AccessExpire`: **no default** -- must be set in config or will be 0 (tokens expire immediately at `iat`)
- `AccessSecret`: **no default** -- must be set via `JWT_SECRET` env var or config

Rate limiting config for auth routes:
```go
Security struct {
    AuthRateLimitRequests int `yaml:"AuthRateLimitRequests"` // no default shown
    AuthRateLimitInterval int `yaml:"AuthRateLimitInterval"` // no default shown
}
```

### Auth Flow Diagrams

#### Login Flow

```
Client                  Handler              AuthService              DB
  |                       |                      |                    |
  |-- POST /auth/login -->|                      |                    |
  |  {email, password}    |                      |                    |
  |                       |-- Login(email, pw) ->|                    |
  |                       |                      |-- GetUserByEmail ->|
  |                       |                      |<-- User row -------|
  |                       |                      |                    |
  |                       |                      | bcrypt.Compare()   |
  |                       |                      |                    |
  |                       |                      |-- generateTokens ->|
  |                       |                      |   JWT(HS256)       |
  |                       |                      |   + opaque refresh |
  |                       |                      |-- CreateRefreshToken|
  |                       |                      |<-- stored ---------|
  |                       |                      |                    |
  |                       |<-- AuthResponse -----|                    |
  |<-- LoginResponse -----|                      |                    |
  |  {token,refreshToken, |                      |                    |
  |   expiresAt}          |                      |                    |
```

#### Refresh Token Flow (Token Rotation)

```
Client                  Handler              AuthService              DB
  |                       |                      |                    |
  |-- POST /auth/refresh->|                      |                    |
  |  {refreshToken}       |                      |                    |
  |                       |-- RefreshToken() --->|                    |
  |                       |                      |-- hashToken()      |
  |                       |                      |-- GetByHash ------>|
  |                       |                      |<-- token row ------|
  |                       |                      |-- GetUserByID ---->|
  |                       |                      |<-- user row -------|
  |                       |                      |-- DELETE old token>|  <-- rotation
  |                       |                      |-- generateTokens ->|
  |                       |                      |-- CREATE new token>|
  |                       |<-- AuthResponse -----|                    |
  |<-- RefreshResponse ---|                      |                    |
  |  {token,refreshToken, |                      |                    |
  |   expiresAt}          |                      |                    |
```

#### Forgot/Reset Password Flow

```
Client                  Handler              AuthService         EmailService
  |                       |                      |                    |
  |-- POST /auth/forgot ->|                      |                    |
  |  {email}              |                      |                    |
  |                       |-- CreateResetToken ->|                    |
  |                       |                      | generateToken()    |
  |                       |                      | expires = now+1h   |
  |                       |                      | store in DB        |
  |                       |<-- token string -----|                    |
  |                       |                      |                    |
  |                       | if token != "" && email != nil:           |
  |                       |-- SendEmail(resetURL) ------------------>|
  |                       |                      |                    |
  |<-- MessageResponse ---|  (always same msg)   |                    |
  |                       |                      |                    |
  |   ... user clicks link ...                   |                    |
  |                       |                      |                    |
  |-- POST /auth/reset -->|                      |                    |
  |  {token, newPassword} |                      |                    |
  |                       |-- ResetPassword() -->|                    |
  |                       |                      | GetByResetToken    |
  |                       |                      | bcrypt(newPassword)|
  |                       |                      | UpdatePassword     |
  |                       |                      | (clears token)     |
  |<-- MessageResponse ---|                      |                    |
```

---

## App Platform

### App Architecture Overview

The app platform is a sandboxed plugin system for Nebo. Key concepts:

- **`.napp` packages**: tar.gz archives containing `manifest.json`, a native binary, `signatures.json`, and optional `ui/` directory
- **gRPC over Unix sockets**: Apps communicate with Nebo via gRPC on `{appDir}/app.sock`
- **Deny-by-default permissions**: Apps declare what they need; Nebo enforces
- **ED25519 code signing**: NeboLoop signs apps; Nebo verifies before launch
- **Capability-based architecture**: Apps declare what they provide (gateway, tool, channel, comm, ui, schedule, hooks)

### Manifest

**File:** `internal/apps/manifest.go`
**Package:** `apps`

#### Capability Constants

```go
const (
    CapGateway  = "gateway"   // LLM gateway (e.g., Janus)
    CapVision   = "vision"    // Vision processing
    CapBrowser  = "browser"   // Browser automation
    CapComm     = "comm"      // Inter-agent communication
    CapUI       = "ui"        // Settings/config UI
    CapSchedule = "schedule"  // Scheduling (replaces built-in cron)
    CapHooks    = "hooks"     // Lifecycle hooks
)

const (
    CapPrefixTool    = "tool:"     // e.g., "tool:calculator"
    CapPrefixChannel = "channel:"  // e.g., "channel:telegram"
)
```

#### Permission Prefixes (Complete Taxonomy)

```go
// Storage & Config
PermPrefixNetwork    = "network:"      // suffixes: flexible (hostnames, "*")
PermPrefixFilesystem = "filesystem:"   // suffixes: "read", "write"
PermPrefixSettings   = "settings:"     // suffixes: "read", "write"
PermPrefixCapability = "capability:"   // suffixes: "register"

// Agent Core
PermPrefixMemory  = "memory:"   // suffixes: "read", "write"
PermPrefixSession = "session:"  // suffixes: "read", "write", "create"
PermPrefixContext = "context:"  // suffixes: "read"

// Execution
PermPrefixTool     = "tool:"     // suffixes: "file", "shell", "web", "agent", "skill"
PermPrefixShell    = "shell:"    // suffixes: "exec"
PermPrefixSubagent = "subagent:" // suffixes: "spawn"
PermPrefixLane     = "lane:"     // suffixes: "enqueue"

// Communication
PermPrefixChannel      = "channel:"      // suffixes: "send", "receive"
PermPrefixComm         = "comm:"         // suffixes: "send", "receive"
PermPrefixNotification = "notification:" // suffixes: "send"

// Knowledge
PermPrefixEmbedding = "embedding:" // suffixes: "search", "store"
PermPrefixSkill     = "skill:"     // suffixes: "invoke"
PermPrefixAdvisor   = "advisor:"   // suffixes: "consult"

// AI
PermPrefixModel = "model:" // suffixes: "chat", "embed"
PermPrefixMCP   = "mcp:"   // suffixes: "connect"

// Storage
PermPrefixDatabase = "database:" // suffixes: "query", "read", "write"
PermPrefixStorage  = "storage:"  // suffixes: "read", "write"

// System
PermPrefixSchedule = "schedule:" // suffixes: "create", "delete", "list"
PermPrefixVoice    = "voice:"    // suffixes: "record"
PermPrefixBrowser  = "browser:"  // suffixes: "navigate"
PermPrefixOAuth    = "oauth:"    // suffixes: flexible (provider names)
PermPrefixUser     = "user:"     // suffixes: "token", "id"

// Hooks
PermPrefixHook = "hook:" // suffixes: flexible (hook names)
```

Wildcard support: `"network:*"` matches any `"network:..."` permission check.

#### AppManifest Struct

```go
type AppManifest struct {
    ID             string             `json:"id"`
    Name           string             `json:"name"`
    Version        string             `json:"version"`
    Description    string             `json:"description,omitempty"`
    Runtime        string             `json:"runtime"`           // "local" or "remote"
    Protocol       string             `json:"protocol"`          // "grpc"
    Signature      ManifestSignature  `json:"signature,omitempty"`
    StartupTimeout int                `json:"startup_timeout,omitempty"` // seconds, 0=default(10s), max 120s
    Provides       []string           `json:"provides"`          // capabilities
    Permissions    []string           `json:"permissions"`       // required permissions
    Overrides      []string           `json:"overrides,omitempty"` // hook names this app fully replaces
    OAuth          []OAuthRequirement `json:"oauth,omitempty"`
}

type OAuthRequirement struct {
    Provider string   `json:"provider"` // "google", "microsoft", "github"
    Scopes   []string `json:"scopes"`   // e.g., ["https://www.googleapis.com/auth/calendar"]
}

type ManifestSignature struct {
    Algorithm string `json:"algorithm,omitempty"`  // "ed25519"
    PublicKey string `json:"public_key,omitempty"`
    Signature string `json:"signature,omitempty"`  // Base64 signature of manifest
    BinarySig string `json:"binary_sig,omitempty"` // Base64 signature of binary
}
```

#### Functions

```go
func LoadManifest(dir string) (*AppManifest, error)
```
- Reads `manifest.json` from `dir`
- Calls `ValidateManifest()`

```go
func ValidateManifest(m *AppManifest) error
```
Validates:
- Required fields: `id`, `name`, `version`, `provides` (must have at least one)
- `protocol` must be `""` or `"grpc"`
- `runtime` must be `""`, `"local"`, or `"remote"`
- `startup_timeout` must be 0-120
- Each capability validated via `isValidCapability()`
- Each permission validated via `isValidPermission()` -- unknown prefixes rejected
- Each override must be in `ValidHookNames` and have corresponding `hook:<name>` permission

```go
func HasCapability(m *AppManifest, cap string) bool
func HasCapabilityPrefix(m *AppManifest, prefix string) bool
func HasPermissionPrefix(m *AppManifest, prefix string) bool
func CheckPermission(m *AppManifest, perm string) bool  // supports wildcards
func VerifySignature(m *AppManifest, binaryPath string) error  // stub, always nil
```

### NApp Extraction

**File:** `internal/apps/napp.go`
**Package:** `apps`

```go
func ExtractNapp(nappPath, destDir string) error
```

Delegates entirely to `neboloopsdk.ExtractNapp()`. The SDK handles all security validation including path traversal prevention.

### Runtime

**File:** `internal/apps/runtime.go`
**Package:** `apps`

#### AppProcess Struct

```go
type AppProcess struct {
    ID       string
    Dir      string
    Manifest *AppManifest
    SockPath string

    // Capability-specific gRPC clients (set based on manifest.provides)
    GatewayClient  pb.GatewayServiceClient
    ToolClient     pb.ToolServiceClient
    ChannelClient  pb.ChannelServiceClient
    CommClient     pb.CommServiceClient
    UIClient       pb.UIServiceClient
    ScheduleClient pb.ScheduleServiceClient
    HookClient     pb.HookServiceClient

    cmd        *exec.Cmd
    conn       *grpc.ClientConn
    startedAt  time.Time
    logCleanup func()
    waitDone   chan struct{}  // closed when cmd.Wait() returns
    mu         sync.RWMutex
}
```

#### Runtime Struct

```go
type Runtime struct {
    dataDir     string
    sandbox     SandboxConfig
    keyProvider *SigningKeyProvider
    revChecker  *RevocationChecker
    inspector   *inspector.Inspector
    processes   map[string]*AppProcess
    mu          sync.RWMutex
    launchMu    sync.Map  // map[appID]*sync.Mutex -- per-app launch serialization
    restarting  sync.Map  // map[appID]time.Time -- suppresses watcher during restarts
}
```

#### Constructor

```go
func NewRuntime(dataDir string, sandbox SandboxConfig) *Runtime
```

#### FindBinary

```go
func FindBinary(appDir string) (string, error)
```

Search order:
1. `{appDir}/binary`
2. `{appDir}/app`
3. First executable file in `{appDir}/tmp/`

#### Launch

```go
func (rt *Runtime) Launch(appDir string) (*AppProcess, error)
```

Complete launch sequence:
1. `LoadManifest(appDir)` -- parse and validate manifest
2. Acquire per-app mutex (`appLaunchMutex`) -- serializes concurrent launches for same app
3. `FindBinary(appDir)` -- locate the executable
4. **Revocation check**: if `revChecker != nil`, check if app is revoked -- refuse to launch if so
5. **Signature verification**:
   - Skip for symlinks (sideloaded dev apps)
   - If `keyProvider != nil`: fetch signing key, call `VerifyAppSignatures()`
   - If key unavailable: log warning, proceed
   - If no `keyProvider`: log warning, proceed (dev mode)
6. **Binary validation**: `validateBinary(binaryPath, sandbox)` -- rejects symlinks, oversized, non-executable, non-native
7. Clean up stale socket at `{appDir}/app.sock`
8. Create `{appDir}/data/` directory (0700)
9. Set up per-app log files (`appLogWriter`)
10. Start binary with `exec.Command`:
    - `cmd.Dir = appDir`
    - `cmd.Env = sanitizeEnv(manifest, appDir, sockPath)` -- stripped environment
    - `setProcGroup(cmd)` -- process group isolation
11. Write PID file to `{appDir}/.pid`
12. Start reaper goroutine (calls `cmd.Wait()` to prevent zombies)
13. Wait for socket with exponential backoff (50ms -> 500ms, timeout from manifest or 10s default)
14. Set socket permissions to 0600
15. Connect via gRPC with `insecure.NewCredentials()` over Unix socket
    - If inspector is set, add unary + stream interceptors
16. Create `AppProcess` with capability-specific gRPC clients based on `manifest.Provides`
17. Health check via gRPC (tries gateway, tool, channel, comm, ui in order)
18. Store in `rt.processes` map (stops old process if collision)
19. Log: "launched app" with name, version, pid, provides

#### Other Runtime Methods

```go
func (rt *Runtime) Stop(appID string) error
func (rt *Runtime) StopAll() error
func (rt *Runtime) Get(appID string) (*AppProcess, bool)
func (rt *Runtime) IsRunning(appID string) bool
func (rt *Runtime) List() []string
func (rt *Runtime) SuppressWatcher(appID string, d time.Duration)
func (rt *Runtime) ClearWatcherSuppression(appID string)
func (rt *Runtime) IsWatcherSuppressed(appID string) bool
```

#### AppProcess.HealthCheck

```go
func (p *AppProcess) HealthCheck(ctx context.Context) error
```

Tries health check via gRPC clients in priority order: Gateway > Tool > Channel > Comm > UI.

#### AppProcess.stop

```go
func (p *AppProcess) stop() error
```

Two-phase shutdown:
1. **Phase 1 (under lock, microseconds):** Snapshot and nil-out all fields (conn, cmd, gRPC clients)
2. **Phase 2 (no lock, 2+ seconds):** Close gRPC conn, SIGTERM process group, wait for reaper (2s timeout), SIGKILL if needed, close log files, remove socket and PID file

#### Orphan Cleanup

```go
func cleanupStaleProcess(appDir string)
```
1. Read `.pid` file, kill that PID if alive
2. Scan process table (`pgrep -f`) for any process matching the app's binary path

### Sandbox

**File:** `internal/apps/sandbox.go`
**Package:** `apps`

#### SandboxConfig

```go
type SandboxConfig struct {
    MaxBinarySizeMB int  // 0 = no limit
    LogToFile       bool // redirect app stdout/stderr to per-app log files
}

func DefaultSandboxConfig() SandboxConfig {
    return SandboxConfig{
        MaxBinarySizeMB: 500,
        LogToFile:       true,
    }
}
```

#### Environment Sanitization

```go
var allowedEnvKeys = map[string]bool{
    "PATH": true, "HOME": true, "TMPDIR": true,
    "LANG": true, "LC_ALL": true, "TZ": true,
}
```

```go
func sanitizeEnv(manifest *AppManifest, appDir, sockPath string) []string
```

Produces a minimal environment:
```
NEBO_APP_DIR={appDir}
NEBO_APP_SOCK={sockPath}
NEBO_APP_ID={manifest.ID}
NEBO_APP_NAME={manifest.Name}
NEBO_APP_VERSION={manifest.Version}
NEBO_APP_DATA={appDir}/data
PATH=...
HOME=...
TMPDIR=...
LANG=...
LC_ALL=...
TZ=...
```

All other env vars (API keys, JWT secrets, tokens) are stripped.

#### Binary Validation

```go
func validateBinary(path string, cfg SandboxConfig) error
```

Rejects:
- Symlinks (uses `Lstat` to detect)
- Non-regular files (devices, pipes)
- Non-executable (Unix only, skipped on Windows)
- Oversized binaries (if `MaxBinarySizeMB > 0`)
- Non-native binary format (delegates to `neboloopsdk.ValidateBinaryFormat`)

#### App Log Writer

```go
func appLogWriter(appDir string, cfg SandboxConfig) (stdout, stderr io.Writer, cleanup func(), err error)
```

- If `LogToFile=false`: returns `prefixWriter` that writes to `os.Stderr` with `[app:{id}]` prefix
- If `LogToFile=true`:
  - Creates `{appDir}/logs/` directory
  - Rotates existing logs if > 2MB (`maxAppLogSize = 2 * 1024 * 1024`)
  - Opens `stdout.log` and `stderr.log`
  - Tees output to both log file AND Nebo's stderr with `[app:{id}]` prefix

### Signing

**File:** `internal/apps/signing.go`
**Package:** `apps`

#### SignaturesFile

```go
type SignaturesFile struct {
    KeyID             string `json:"key_id"`
    Algorithm         string `json:"algorithm"`
    BinarySHA256      string `json:"binary_sha256"`
    BinarySignature   string `json:"binary_signature"`
    ManifestSignature string `json:"manifest_signature"`
}
```

#### SigningKey

```go
type SigningKey struct {
    Algorithm string `json:"algorithm"`
    KeyID     string `json:"keyId"`
    PublicKey string `json:"publicKey"` // base64-encoded ED25519 public key
}
```

#### SigningKeyProvider

```go
type SigningKeyProvider struct {
    neboloopURL string
    key         *SigningKey
    fetchedAt   time.Time
    ttl         time.Duration  // 24 hours
    mu          sync.RWMutex
}

func NewSigningKeyProvider(neboloopURL string) *SigningKeyProvider
func (p *SigningKeyProvider) GetKey() (*SigningKey, error)   // cached, auto-refresh
func (p *SigningKeyProvider) Refresh() (*SigningKey, error)  // force fetch
```

Key is fetched from `{neboloopURL}/api/v1/apps/signing-key`. Cached for 24 hours.

#### LoadSignatures

```go
func LoadSignatures(appDir string) (*SignaturesFile, error)
```

Reads `{appDir}/signatures.json`. Validates algorithm is `"ed25519"` and all required fields are present.

#### VerifyAppSignatures

```go
func VerifyAppSignatures(appDir, binaryPath string, key *SigningKey) error
```

Verification steps:
1. Load `signatures.json` from app directory
2. Verify key ID matches the server's current key
3. Decode base64 public key, verify it's 32 bytes (ED25519 public key size)
4. **Verify manifest signature**: `ed25519.Verify(pubKey, manifestBytes, manifestSig)` -- signs raw manifest.json bytes, NOT a hash
5. **Verify binary SHA256**: compute `sha256.Sum256(binaryBytes)`, compare hex to `sigs.BinarySHA256`
6. **Verify binary signature**: `ed25519.Verify(pubKey, binaryBytes, binarySig)` -- signs raw binary bytes, NOT a hash

#### RevocationChecker

```go
type RevocationChecker struct {
    neboloopURL string
    revoked     map[string]bool
    fetchedAt   time.Time
    ttl         time.Duration  // 1 hour
    mu          sync.RWMutex
}

type RevocationList struct {
    Revocations []RevocationEntry `json:"revocations"`
}

type RevocationEntry struct {
    ID        string `json:"id"`
    Name      string `json:"name"`
    Slug      string `json:"slug"`
    Version   string `json:"version"`
    RevokedAt string `json:"revoked_at"`
}

func NewRevocationChecker(neboloopURL string) *RevocationChecker
func (rc *RevocationChecker) IsRevoked(appID string) (bool, error)
```

Fetches from `{neboloopURL}/api/v1/apps/revocations`. Cached for 1 hour. Double-checked locking pattern.

### Supervisor

**File:** `internal/apps/supervisor.go`
**Package:** `apps`

#### Constants

```go
const (
    maxRestartsPerHour = 5
    restartWindow      = 1 * time.Hour
    minBackoff         = 10 * time.Second
    maxBackoff         = 5 * time.Minute
)
```

#### Supervisor Struct

```go
type Supervisor struct {
    registry *AppRegistry
    runtime  *Runtime
    interval time.Duration  // 15 seconds
    cancel   context.CancelFunc
    done     chan struct{}
    mu       sync.Mutex
    appState map[string]*appRestartState
}

type appRestartState struct {
    lastRestart  time.Time
    restartCount int
    windowStart  time.Time
    backoffUntil time.Time
}
```

#### Methods

```go
func NewSupervisor(registry *AppRegistry, runtime *Runtime) *Supervisor
func (s *Supervisor) Start(ctx context.Context)
func (s *Supervisor) Stop()
```

#### Check Logic (every 15s)

For each running app:
1. Skip if in backoff period (`time.Now().Before(state.backoffUntil)`)
2. Reset counter if window expired (`time.Since(windowStart) > 1h`)
3. Skip if restart budget exhausted (`count >= 5`)
4. **OS-level check**: `isProcessAlive(pid)` -- restart if dead
5. **gRPC health check**: `proc.HealthCheck(ctx)` with 5s timeout -- restart if failed

#### Restart Policy

- Exponential backoff: 10s, 20s, 40s, 80s, 160s (capped at 5 minutes)
- Max 5 restarts per hour per app
- After exceeding limit: deregisters capabilities and stops monitoring until Nebo restart
- Suppresses watcher for 30s during restart to prevent double-launch

### File Watcher

**File:** `internal/apps/watcher.go`
**Package:** `apps`

```go
func (ar *AppRegistry) Watch(ctx context.Context) error
```

Uses `fsnotify` to monitor the apps directory. Watches both the top-level `apps/` directory and all subdirectories.

#### Events Handled

| Event | File | Action |
|-------|------|--------|
| Create | New directory in `apps/` | Start watching it, launch if `manifest.json` present |
| Create | `manifest.json` in subdirectory | Launch new app |
| Create | `binary` or `app` in subdirectory | Debounced restart (500ms) |
| Write | `manifest.json` | Debounced restart (500ms) |
| Write | `binary` or `app` | Debounced restart (500ms) |
| Remove | Top-level directory | Stop app, remove watcher |

#### Debouncing

- Uses `time.AfterFunc(500ms)` to coalesce rapid Create+Write events
- Checks `runtime.IsWatcherSuppressed(appID)` to avoid double-restart during managed restarts
- All debounce timers cancelled on context cancellation

### gRPC Adapters

**File:** `internal/apps/adapter.go`
**Package:** `apps`

Four adapter types bridge gRPC clients to Nebo's internal interfaces:

#### GatewayProviderAdapter

```go
type GatewayProviderAdapter struct {
    client    pb.GatewayServiceClient
    manifest  *AppManifest
    appID     string
    profileID string
}

func NewGatewayProviderAdapter(client pb.GatewayServiceClient, manifest *AppManifest, profileID string) *GatewayProviderAdapter
func (g *GatewayProviderAdapter) ID() string
func (g *GatewayProviderAdapter) ProfileID() string
func (g *GatewayProviderAdapter) HandlesTools() bool  // always false
func (g *GatewayProviderAdapter) Stream(ctx context.Context, req *ai.ChatRequest) (<-chan ai.StreamEvent, error)
```

Implements `ai.Provider`. Converts `ai.ChatRequest` to `pb.GatewayRequest`, streams events back.

**UserContext filtering:** All apps get `user_id` and `plan`. Only apps with `"user:token"` permission get the full JWT.

**Stream event mapping:**
- `"text"` -> `EventTypeText`
- `"tool_call"` -> `EventTypeToolCall` (parses JSON: `{id, name, arguments}`)
- `"thinking"` -> `EventTypeThinking`
- `"error"` -> `EventTypeError`
- `"done"` -> `EventTypeDone`

#### AppToolAdapter

```go
type AppToolAdapter struct {
    client   pb.ToolServiceClient
    name     string
    desc     string
    schema   json.RawMessage
    approval bool
}

func NewAppToolAdapter(ctx context.Context, client pb.ToolServiceClient) (*AppToolAdapter, error)
func (a *AppToolAdapter) Name() string
func (a *AppToolAdapter) Description() string
func (a *AppToolAdapter) Schema() json.RawMessage
func (a *AppToolAdapter) RequiresApproval() bool
func (a *AppToolAdapter) Execute(ctx context.Context, input json.RawMessage) (*tools.ToolResult, error)
```

Implements `tools.Tool`. Queries the app on creation for name, description, schema, and approval flag.

#### AppCommAdapter

```go
type AppCommAdapter struct {
    client  pb.CommServiceClient
    name    string
    version string
    handler atomic.Value  // func(comm.CommMessage) -- lock-free
    cancel  context.CancelFunc
}

func NewAppCommAdapter(ctx context.Context, client pb.CommServiceClient) (*AppCommAdapter, error)
func (a *AppCommAdapter) Name() string
func (a *AppCommAdapter) Version() string
func (a *AppCommAdapter) Connect(ctx context.Context, config map[string]string) error
func (a *AppCommAdapter) Disconnect(ctx context.Context) error
func (a *AppCommAdapter) IsConnected() bool
func (a *AppCommAdapter) Send(ctx context.Context, msg comm.CommMessage) error
func (a *AppCommAdapter) Subscribe(ctx context.Context, topic string) error
func (a *AppCommAdapter) Unsubscribe(ctx context.Context, topic string) error
func (a *AppCommAdapter) Register(ctx context.Context, agentID string, card *comm.AgentCard) error
func (a *AppCommAdapter) Deregister(ctx context.Context) error
func (a *AppCommAdapter) SetMessageHandler(handler func(comm.CommMessage))
```

Implements `comm.CommPlugin`. On `Connect()`, starts a background goroutine reading from `Receive()` stream.

#### AppChannelAdapter

```go
type AppChannelAdapter struct {
    client  pb.ChannelServiceClient
    id      string
    handler atomic.Value  // func(channelID, userID, text, metadata string) -- lock-free
    cancel  context.CancelFunc
}

func NewAppChannelAdapter(ctx context.Context, client pb.ChannelServiceClient) (*AppChannelAdapter, error)
func (a *AppChannelAdapter) ID() string
func (a *AppChannelAdapter) Connect(ctx context.Context, config map[string]string) error
func (a *AppChannelAdapter) Disconnect(ctx context.Context) error
func (a *AppChannelAdapter) Send(ctx context.Context, channelID, text string) error
func (a *AppChannelAdapter) SetMessageHandler(handler func(channelID, userID, text, metadata string))
```

On `Connect()`, starts a background goroutine reading from `Receive()` stream.

### Schedule Adapter

**File:** `internal/apps/schedule_adapter.go`
**Package:** `apps`

```go
type AppScheduleAdapter struct {
    client  pb.ScheduleServiceClient
    handler atomic.Value  // func(tools.ScheduleTriggerEvent)
    cancel  context.CancelFunc
}

func NewAppScheduleAdapter(ctx context.Context, client pb.ScheduleServiceClient) (*AppScheduleAdapter, error)
func (a *AppScheduleAdapter) Create(ctx context.Context, item tools.ScheduleItem) (*tools.ScheduleItem, error)
func (a *AppScheduleAdapter) Get(ctx context.Context, name string) (*tools.ScheduleItem, error)
func (a *AppScheduleAdapter) List(ctx context.Context, limit, offset int, enabledOnly bool) ([]tools.ScheduleItem, int64, error)
func (a *AppScheduleAdapter) Update(ctx context.Context, item tools.ScheduleItem) (*tools.ScheduleItem, error)
func (a *AppScheduleAdapter) Delete(ctx context.Context, name string) error
func (a *AppScheduleAdapter) Enable(ctx context.Context, name string) (*tools.ScheduleItem, error)
func (a *AppScheduleAdapter) Disable(ctx context.Context, name string) (*tools.ScheduleItem, error)
func (a *AppScheduleAdapter) Trigger(ctx context.Context, name string) (string, error)
func (a *AppScheduleAdapter) History(ctx context.Context, name string, limit, offset int) ([]tools.ScheduleHistoryEntry, int64, error)
func (a *AppScheduleAdapter) SetTriggerHandler(fn func(tools.ScheduleTriggerEvent))
func (a *AppScheduleAdapter) Close() error
```

Implements `tools.Scheduler`. `SetTriggerHandler` starts a background goroutine reading from the `Triggers()` server-streaming RPC.

### Hooks

**File:** `internal/apps/hooks.go`
**Package:** `apps`

#### Constants

```go
const hookTimeout = 500 * time.Millisecond
const circuitBreakerThreshold = 3
```

#### Valid Hook Names

```go
var ValidHookNames = map[string]bool{
    "tool.pre_execute":       true,
    "tool.post_execute":      true,
    "message.pre_send":       true,
    "message.post_receive":   true,
    "memory.pre_store":       true,
    "memory.pre_recall":      true,
    "session.message_append": true,
    "prompt.system_sections": true,
    "steering.generate":      true,
    "response.stream":        true,
}
```

#### HookDispatcher

```go
type HookDispatcher struct {
    mu       sync.RWMutex
    hooks    map[string][]*hookEntry  // hook name -> sorted entries
    failures map[string]int           // appID -> consecutive failures
    disabled map[string]bool          // appID -> circuit breaker tripped
}

type hookEntry struct {
    appID    string
    hookType string  // "action" or "filter"
    priority int     // lower = runs first, default 10
    client   pb.HookServiceClient
}
```

#### Methods

```go
func NewHookDispatcher() *HookDispatcher
func (d *HookDispatcher) Register(appID string, reg *pb.HookRegistration, client pb.HookServiceClient)
func (d *HookDispatcher) UnregisterApp(appID string)
func (d *HookDispatcher) HasSubscribers(hook string) bool
```

**Filters (data transformation chain):**
```go
func (d *HookDispatcher) ApplyFilter(ctx context.Context, hook string, payload []byte) ([]byte, bool)
```
- Calls all filter subscribers in priority order (lower first)
- Each filter receives the previous filter's output (chain pattern)
- If any filter sets `handled=true`, returns immediately (short-circuit)
- Failed filters are skipped (logged as warning)
- Returns `(modifiedPayload, wasHandled)`

**Actions (fire-and-forget):**
```go
func (d *HookDispatcher) DoAction(ctx context.Context, hook string, payload []byte)
```
- Calls all action subscribers sequentially in priority order
- Results are discarded
- Failed actions are skipped

#### Circuit Breaker

- Tracks consecutive failures per app
- After 3 consecutive failures: disables ALL hooks for that app until Nebo restart
- A single success resets the failure counter to 0

### Registry

**File:** `internal/apps/registry.go`
**Package:** `apps`

The `AppRegistry` is the central coordinator that ties together runtime, supervisor, watcher, and all capability adapters.

#### AppRegistryConfig

```go
type AppRegistryConfig struct {
    DataDir     string
    NeboLoopURL string              // enables signing + revocation if set
    Queries     db.Querier
    PluginStore *settings.Store
    ToolReg     *tools.Registry
    SkillTool   *tools.SkillDomainTool
    CommMgr     *comm.CommPluginManager
}
```

#### AppRegistry Struct

```go
type AppRegistry struct {
    runtime       *Runtime
    appsDir       string
    queries       db.Querier
    pluginStore   *settings.Store
    toolReg       *tools.Registry
    skillTool     *tools.SkillDomainTool
    commMgr       *comm.CommPluginManager
    grpcInspector *inspector.Inspector

    supervisor          *Supervisor
    onQuarantine        func(QuarantineEvent)
    onGatewayRegistered func()
    onChannelMsg        func(channelType, channelID, userID, text, metadata string)
    providers           []ai.Provider
    uiApps              map[string]*AppProcess
    channelAdapters     map[string]*AppChannelAdapter
    scheduleAdapter     *AppScheduleAdapter
    commNames           map[string]string  // appID -> comm plugin name
    hookDispatcher      *HookDispatcher
    nappDownloader      func(ctx context.Context, url, destDir string) error
    mu                  sync.RWMutex
}
```

#### Constructor

```go
func NewAppRegistry(cfg AppRegistryConfig) *AppRegistry
```

- Creates `{dataDir}/apps/` directory
- Initializes `Runtime` with `DefaultSandboxConfig()`
- If `NeboLoopURL` is set: enables `SigningKeyProvider` (24h cache) and `RevocationChecker` (1h cache)
- Creates `inspector.Inspector` with 1024-entry ring buffer
- Creates `HookDispatcher`

#### DiscoverAndLaunch

```go
func (ar *AppRegistry) DiscoverAndLaunch(ctx context.Context) error
```

Scans `apps/` directory. For each subdirectory:
1. Uses `os.Stat` (not `entry.IsDir()`) to follow symlinks (sideloaded apps)
2. Checks for `manifest.json` -- skips if missing
3. Checks for `.quarantined` marker -- skips if present
4. Calls `launchAndRegister(ctx, appDir)`

#### launchAndRegister (private, core function)

```go
func (ar *AppRegistry) launchAndRegister(ctx context.Context, appDir string) error
```

This is the central function that launches an app and wires up all its capabilities:

1. Refuse if `.quarantined` marker exists
2. Clean up stale processes from previous runs
3. `runtime.Launch(appDir)` -- full launch sequence (see Runtime section)
4. Register in `plugin_registry` DB table (upsert, preserves store IDs)
5. For each capability in `manifest.Provides`:

| Capability | Permission Check | Registration |
|------------|-----------------|--------------|
| `gateway` | `network:*` required | `GatewayProviderAdapter` -> `ar.providers`, fires `onGatewayRegistered` callback, auto-configures user token if `user:token` permission |
| `tool:*`, `vision`, `browser` | none | `AppToolAdapter` -> `skillTool.Register()` or `toolReg.Register()` |
| `comm` | `comm:*` required | `AppCommAdapter` -> `commMgr.Register()` |
| `channel:*` | `channel:*` required | `AppChannelAdapter` -> `registerChannel()` |
| `ui` | none | Stored in `uiApps` map |
| `schedule` | `schedule:*` required | `AppScheduleAdapter` -> `ar.scheduleAdapter` |
| `hooks` | `hook:*` required | Queries `ListHooks()`, registers each in `HookDispatcher` (with per-hook permission check) |

6. Register as `Configurable` in `pluginStore` for hot-reload
7. Update `connection_status` to `"connected"` in DB

#### Lifecycle Operations

```go
func (ar *AppRegistry) InstallFromURL(ctx context.Context, downloadURL string) error
func (ar *AppRegistry) Uninstall(appID string) error
func (ar *AppRegistry) Quarantine(appID string) error
func (ar *AppRegistry) Sideload(ctx context.Context, projectPath string) (*AppManifest, error)
func (ar *AppRegistry) Unsideload(appID string) error
func (ar *AppRegistry) IsSideloaded(appID string) bool
```

**InstallFromURL:** Downloads to temp dir, validates manifest, moves to `apps/{id}/`, launches.

**Uninstall:** Deregisters capabilities, stops process, removes DB row, removes directory entirely.

**Quarantine:** Deregisters capabilities, stops process, removes binary (but preserves `data/` and `logs/`), writes `.quarantined` marker file, fires `onQuarantine` callback for UI notification.

**Sideload:** Validates manifest, runs `make build` if Makefile exists, creates symlink `apps/{id}` -> project path, launches immediately.

**Unsideload:** Verifies target is a symlink, deregisters, stops, removes symlink (NOT the project directory).

#### Revocation Sweep

```go
func (ar *AppRegistry) StartRevocationSweep(ctx context.Context)
```

Background goroutine that checks all running apps against the revocation list every hour. Quarantines any revoked apps found.

#### OAuth Token Push

```go
func (ar *AppRegistry) PushOAuthTokens(appID, provider string, tokens map[string]string) error
```

Pushes OAuth tokens to a running app via gRPC `Configure` RPC. Implements `broker.AppTokenReceiver`.

#### Auto-Configure User Token

```go
func (ar *AppRegistry) autoConfigureUserToken(ctx context.Context, appName string) error
```

For apps with `user:token` permission: reads NeboLoop JWT from `auth_profiles` table and stores it as the app's `"token"` setting.

#### App Catalog (System Prompt Injection)

```go
func (ar *AppRegistry) AppCatalog() string
```

Returns markdown listing all running apps with names, IDs, descriptions, and capabilities. Injected into agent's system prompt.

### Install

**File:** `internal/apps/install.go`
**Package:** `apps`

#### HandleInstallEvent

```go
func (ar *AppRegistry) HandleInstallEvent(ctx context.Context, evt neboloopsdk.InstallEvent)
```

Routes NeboLoop install events:
- `"installed"` / `"app_installed"` -> `handleInstall()`
- `"updated"` / `"app_updated"` -> `handleUpdate()`
- `"uninstalled"` / `"app_uninstalled"` -> `handleUninstall()`
- `"revoked"` / `"app_revoked"` -> `handleRevoke()`

#### handleInstall

Downloads .napp, extracts to `apps/{appID}/`, launches.

#### handleUpdate

```go
func (ar *AppRegistry) handleUpdate(ctx context.Context, appID, version, downloadURL string)
```

Update flow:
1. Load old manifest (for permission diff)
2. Stop running app
3. Download new version to `{appDir}.updating/`
4. **Permission diff**: compare old vs new permissions
   - If new permissions added: stage to `{appDir}.pending/`, do NOT launch -- requires user approval
   - If no new permissions: auto-update
5. Preserve `data/` and `logs/` directories across updates (move, not copy)
6. Atomic swap: `os.Rename(tmpDir, appDir)`
7. Relaunch

#### DownloadAndExtractNapp

```go
func DownloadAndExtractNapp(downloadURL, destDir string) error
```

- HTTP GET the download URL
- Rejects `application/octet-stream` content type (must be packaged as .napp)
- Max download size: 600 MB
- Downloads to temp file, then calls `ExtractNapp()`

### Inspector

**File:** `internal/apps/inspector/inspector.go` and `interceptor.go`
**Package:** `inspector`

#### Event Struct

```go
type Event struct {
    ID         uint64          `json:"id"`
    Timestamp  time.Time       `json:"timestamp"`
    AppID      string          `json:"appId"`
    Method     string          `json:"method"`
    Type       string          `json:"type"`       // "unary", "stream_send", "stream_recv", "stream_open"
    Direction  string          `json:"direction"`   // "request" or "response"
    Payload    json.RawMessage `json:"payload"`
    DurationMs int64           `json:"durationMs,omitempty"`
    Error      string          `json:"error,omitempty"`
    StreamSeq  int             `json:"streamSeq,omitempty"`
}
```

#### Inspector

```go
type Inspector struct {
    ring           []*Event       // bounded ring buffer
    ringSize       int            // default 1024
    counter        atomic.Uint64  // monotonic event ID
    subscribers    map[uint64]chan *Event
    hasSubscribers atomic.Int32   // fast-path check
}

func New(ringSize int) *Inspector
func (ins *Inspector) Record(e *Event)
func (ins *Inspector) Subscribe() (<-chan *Event, func())
func (ins *Inspector) HasSubscribers() bool
func (ins *Inspector) Recent(appID string, n int) []*Event
```

**Zero-cost fast path:** When no subscribers exist, `HasSubscribers()` returns false via atomic load. Interceptors skip all serialization work.

**Subscribe:** Returns a buffered channel (128) and an unsubscribe function. Non-blocking send to subscribers (slow subscribers drop events).

**Recent:** Returns up to N most recent events for an app (or all apps if appID empty), in chronological order.

#### gRPC Interceptors

```go
func UnaryInterceptor(ins *Inspector, appID string) grpc.UnaryClientInterceptor
func StreamInterceptor(ins *Inspector, appID string) grpc.StreamClientInterceptor
```

**Unary interceptor:** Records request event before invocation, response event after (with duration).

**Stream interceptor:** Records `stream_open` event, wraps stream in `wrappedStream` that records `stream_send` (with sequence number) on `SendMsg()` and `stream_recv` on `RecvMsg()`.

Payload serialization: tries `protojson.Marshal` first for proper field naming, falls back to `json.Marshal`.

### Settings Store

**File:** `internal/apps/settings/store.go` and `manifest.go`
**Package:** `settings`

#### Configurable Interface

```go
type Configurable interface {
    OnSettingsChanged(settings map[string]string) error
}
```

#### Store

```go
type Store struct {
    queries       *db.Queries
    handlers      []ChangeHandler
    configurables map[string]Configurable
}

type ChangeHandler func(appName string, settings map[string]string)

func NewStore(sqlDB *sql.DB) *Store
```

#### Methods

```go
func (s *Store) OnChange(fn ChangeHandler)                                    // register change listener
func (s *Store) RegisterConfigurable(appName string, c Configurable)          // register hot-reload target
func (s *Store) DeregisterConfigurable(appName string)
func (s *Store) GetPlugin(ctx context.Context, name string) (*db.PluginRegistry, error)
func (s *Store) GetPluginByID(ctx context.Context, id string) (*db.PluginRegistry, error)
func (s *Store) ListPlugins(ctx context.Context, pluginType string) ([]db.PluginRegistry, error)
func (s *Store) GetSettings(ctx context.Context, pluginID string) (map[string]string, error)
func (s *Store) GetSettingsByName(ctx context.Context, appName string) (map[string]string, error)
func (s *Store) UpdateSettings(ctx context.Context, pluginID string, values map[string]string, secrets map[string]bool) error
func (s *Store) TogglePlugin(ctx context.Context, pluginID string, enabled bool) error
func (s *Store) UpdateStatus(ctx context.Context, pluginID, status string, lastError string) error
func (s *Store) DeleteSetting(ctx context.Context, pluginID, key string) error
```

**Secret handling in UpdateSettings:**
- If `secrets[key] == true`: encrypts value via `credential.Encrypt()` before storing, sets `is_secret=1`
- On read (`GetSettings`): if `is_secret != 0`, decrypts via `credential.Decrypt()`

**Hot-reload flow in UpdateSettings:**
1. Upsert each setting in DB
2. Fetch full current settings map
3. Call `Configurable.OnSettingsChanged(allSettings)` if registered
4. Call all `ChangeHandler` functions (WebSocket broadcast, etc.)

### Channel Envelope

**File:** `internal/apps/envelope.go`
**Package:** `apps`

MQTT channel bridge message format (v1):

```go
type ChannelEnvelope struct {
    MessageID    string          `json:"message_id"`              // UUID v7 (time-ordered)
    ChannelID    string          `json:"channel_id"`              // format: {type}:{platform_id}
    Sender       EnvelopeSender  `json:"sender"`
    Text         string          `json:"text"`
    Attachments  []Attachment    `json:"attachments,omitempty"`
    ReplyTo      string          `json:"reply_to,omitempty"`
    Actions      []Action        `json:"actions,omitempty"`
    PlatformData json.RawMessage `json:"platform_data,omitempty"`
    Timestamp    time.Time       `json:"timestamp"`
}

type EnvelopeSender struct {
    Name  string `json:"name"`
    Role  string `json:"role,omitempty"`
    BotID string `json:"bot_id,omitempty"`
}

type Attachment struct {
    Type     string `json:"type"`      // "image", "file", "audio", "video"
    URL      string `json:"url"`
    Filename string `json:"filename,omitempty"`
    Size     int    `json:"size,omitempty"`
}

type Action struct {
    Label      string `json:"label"`
    CallbackID string `json:"callback_id"`
}

func NewMessageID() string  // UUID v7 (time-ordered)
```

### Process Management

#### Unix (`internal/apps/process_unix.go`)

```go
func isProcessAlive(pid int) bool          // syscall.Kill(pid, 0)
func setProcGroup(cmd *exec.Cmd)           // Setpgid: true
func killProcGroup(cmd *exec.Cmd)          // SIGKILL to -pid (entire group)
func killProcGroupTerm(cmd *exec.Cmd)      // SIGTERM to -pid
func killOrphanGroup(pid int)              // SIGTERM, wait 500ms, SIGKILL if alive
```

#### Windows (`internal/apps/process_windows.go`)

```go
func isProcessAlive(pid int) bool          // FindProcess + Signal(0)
func setProcGroup(cmd *exec.Cmd)           // CREATE_NEW_PROCESS_GROUP
func killProcGroup(cmd *exec.Cmd)          // taskkill.exe /t /f /pid
func killProcGroupTerm(cmd *exec.Cmd)      // taskkill.exe /t /pid (no /f = graceful)
func killOrphanGroup(pid int)              // taskkill.exe /t /f /pid
```

#### Orphan Scanner Unix (`internal/apps/orphan_unix.go`)

```go
func killOrphansByBinary(binaryPath, appID string)
```
Uses `pgrep -f` to find processes matching binary path, verifies with `ps -p`, kills with `killOrphan()`.

#### Orphan Scanner Windows (`internal/apps/orphan_windows.go`)

```go
func killOrphansByBinary(binaryPath, appID string)  // no-op
```

### Protobuf Definitions

**Location:** `proto/apps/v0/`

#### common.proto

```protobuf
message HealthCheckRequest {}
message HealthCheckResponse { bool healthy; string version; string name; }
message SettingsMap { map<string, string> values; }
message UserContext { string token; string user_id; string plan; }
message Empty {}
message ErrorResponse { string message; string code; }
```

#### tool.proto -- ToolService

```protobuf
service ToolService {
    rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
    rpc Name(Empty) returns (NameResponse);
    rpc Description(Empty) returns (DescriptionResponse);
    rpc Schema(Empty) returns (SchemaResponse);
    rpc Execute(ExecuteRequest) returns (ExecuteResponse);
    rpc RequiresApproval(Empty) returns (ApprovalResponse);
    rpc Configure(SettingsMap) returns (Empty);
}

message NameResponse { string name; }
message DescriptionResponse { string description; }
message SchemaResponse { bytes schema; }           // JSON Schema
message ExecuteRequest { bytes input; }            // JSON-encoded tool input
message ExecuteResponse { string content; bool is_error; }
message ApprovalResponse { bool requires_approval; }
```

#### gateway.proto -- GatewayService

```protobuf
service GatewayService {
    rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
    rpc Stream(GatewayRequest) returns (stream GatewayEvent);
    rpc Poll(PollRequest) returns (PollResponse);
    rpc Cancel(CancelRequest) returns (CancelResponse);
    rpc Configure(SettingsMap) returns (Empty);
}

message GatewayRequest {
    string request_id; repeated GatewayMessage messages;
    repeated GatewayToolDef tools; int32 max_tokens;
    double temperature; string system; UserContext user;
}
message GatewayMessage { string role; string content; string tool_call_id; string tool_calls; }
message GatewayToolDef { string name; string description; bytes input_schema; }
message GatewayEvent { string type; string content; string model; string request_id; }
message PollRequest { string request_id; }
message PollResponse { repeated GatewayEvent events; bool complete; }
message CancelRequest { string request_id; }
message CancelResponse { bool cancelled; }
```

#### channel.proto -- ChannelService

```protobuf
service ChannelService {
    rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
    rpc ID(Empty) returns (IDResponse);
    rpc Connect(ChannelConnectRequest) returns (ChannelConnectResponse);
    rpc Disconnect(Empty) returns (ChannelDisconnectResponse);
    rpc Send(ChannelSendRequest) returns (ChannelSendResponse);
    rpc Receive(Empty) returns (stream InboundMessage);
    rpc Configure(SettingsMap) returns (Empty);
}

message IDResponse { string id; }
message ChannelConnectRequest { map<string, string> config; }
message ChannelSendRequest {
    string channel_id; string text; string message_id;
    MessageSender sender; repeated Attachment attachments;
    string reply_to; repeated MessageAction actions; bytes platform_data;
}
message InboundMessage {
    string channel_id; string user_id; string text; string metadata;
    string message_id; MessageSender sender; repeated Attachment attachments;
    string reply_to; repeated MessageAction actions; bytes platform_data; string timestamp;
}
message MessageSender { string name; string role; string bot_id; }
message Attachment { string type; string url; string filename; int64 size; }
message MessageAction { string label; string callback_id; }
```

#### comm.proto -- CommService

```protobuf
service CommService {
    rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
    rpc Name(Empty) returns (CommNameResponse);
    rpc Version(Empty) returns (CommVersionResponse);
    rpc Connect(CommConnectRequest) returns (CommConnectResponse);
    rpc Disconnect(Empty) returns (CommDisconnectResponse);
    rpc IsConnected(Empty) returns (CommIsConnectedResponse);
    rpc Send(CommSendRequest) returns (CommSendResponse);
    rpc Subscribe(CommSubscribeRequest) returns (CommSubscribeResponse);
    rpc Unsubscribe(CommUnsubscribeRequest) returns (CommUnsubscribeResponse);
    rpc Register(CommRegisterRequest) returns (CommRegisterResponse);
    rpc Deregister(Empty) returns (CommDeregisterResponse);
    rpc Receive(Empty) returns (stream CommMessage);
    rpc Configure(SettingsMap) returns (Empty);
}

message CommMessage {
    string id; string from; string to; string topic;
    string conversation_id; string type; string content;
    map<string, string> metadata; int64 timestamp;
    bool human_injected; string human_id;
}
```

#### ui.proto -- UIService

```protobuf
service UIService {
    rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
    rpc Configure(SettingsMap) returns (Empty);
    rpc HandleRequest(HttpRequest) returns (HttpResponse);
}

message HttpRequest {
    string method; string path; string query;
    map<string, string> headers; bytes body;
}
message HttpResponse {
    int32 status_code; map<string, string> headers; bytes body;
}
```

#### schedule.proto -- ScheduleService

```protobuf
service ScheduleService {
    rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
    rpc Create(CreateScheduleRequest) returns (ScheduleResponse);
    rpc Get(GetScheduleRequest) returns (ScheduleResponse);
    rpc List(ListSchedulesRequest) returns (ListSchedulesResponse);
    rpc Update(UpdateScheduleRequest) returns (ScheduleResponse);
    rpc Delete(DeleteScheduleRequest) returns (DeleteScheduleResponse);
    rpc Enable(ScheduleNameRequest) returns (ScheduleResponse);
    rpc Disable(ScheduleNameRequest) returns (ScheduleResponse);
    rpc Trigger(ScheduleNameRequest) returns (TriggerResponse);
    rpc History(ScheduleHistoryRequest) returns (ScheduleHistoryResponse);
    rpc Triggers(Empty) returns (stream ScheduleTrigger);
    rpc Configure(SettingsMap) returns (Empty);
}

message Schedule {
    string id; string name; string expression; string task_type;
    string command; string message; string deliver; bool enabled;
    string last_run; string next_run; int64 run_count;
    string last_error; string created_at; map<string, string> metadata;
}
message ScheduleTrigger {
    string schedule_id; string name; string task_type;
    string command; string message; string deliver;
    string fired_at; map<string, string> metadata;
}
```

#### hooks.proto -- HookService

```protobuf
service HookService {
    rpc ApplyFilter(HookRequest) returns (HookResponse);
    rpc DoAction(HookRequest) returns (Empty);
    rpc ListHooks(Empty) returns (HookList);
    rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
}

message HookRequest { string hook; bytes payload; int64 timestamp_ms; }
message HookResponse { bytes payload; bool handled; string error; }
message HookList { repeated HookRegistration hooks; }
message HookRegistration { string hook; string type; int32 priority; }
```

---

## Summary of Key Design Decisions

### Auth

1. **Two JWT implementations exist**: `internal/auth/jwt.go` (both tokens are JWTs) and `internal/local/auth.go` (access=JWT, refresh=opaque). The local auth service is what is actually used.
2. **No password validation** at the service layer -- the `ValidatePassword()` function in `internal/auth/password.go` is available but not wired in.
3. **Email verification is a no-op** -- the endpoint exists but does nothing.
4. **Resend verification is a stub** -- always returns success without sending email.
5. **Token rotation** is implemented -- each refresh token is single-use.
6. **hashToken is NOT cryptographic** -- it copies bytes, not hashes them.
7. **Login does not check email verification status**.

### App Platform

1. **Deny-by-default**: Apps must declare permissions; capabilities require matching permissions.
2. **ED25519 signing over raw bytes** (not hashes) for both manifest and binary.
3. **Symlinks = dev mode**: Sideloaded apps (symlinks) skip signature verification.
4. **Process group isolation**: Apps run in their own process group for clean kill.
5. **Environment sanitization**: Only 6 system env vars pass through; all secrets stripped.
6. **Circuit breaker on hooks**: 3 consecutive failures disables an app's hooks until restart.
7. **Permission diff on update**: New permissions require user approval (staged as `.pending`).
8. **Quarantine preserves data**: Revoked apps have binaries removed but `data/` and `logs/` preserved.
9. **Per-app launch mutex**: Prevents duplicate processes when watcher/supervisor/registry race.
10. **8 gRPC service types**: Gateway, Tool, Channel, Comm, UI, Schedule, Hooks, plus Common messages.
