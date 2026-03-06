# Middleware, Security, Configuration & Infrastructure Deep Dive

Comprehensive logic-level documentation of Go packages in `nebo/internal/`.
Every struct, function signature, constant, and algorithmic detail is documented
from the source code.

---

## Table of Contents

1. [Middleware (`internal/middleware/`)](#1-middleware)
   - [JWT Authentication](#11-jwt-authentication)
   - [CORS](#12-cors)
   - [Rate Limiting](#13-rate-limiting)
   - [Security Headers](#14-security-headers)
   - [CSRF Protection](#15-csrf-protection)
   - [Compression & Cache Control](#16-compression--cache-control)
   - [Validation](#17-validation)
   - [Security Orchestrator](#18-security-orchestrator)
2. [Security Utilities (`internal/security/`)](#2-security-utilities)
   - [Output Encoding](#21-output-encoding)
   - [Input Sanitization](#22-input-sanitization)
   - [SQL Injection Prevention](#23-sql-injection-prevention)
3. [Configuration (`internal/config/`)](#3-configuration)
4. [Data Directory & Defaults (`internal/defaults/`)](#4-data-directory--defaults)
5. [Local Settings (`internal/local/`)](#5-local-settings)
6. [Logging (`internal/logging/`)](#6-logging)
7. [Crash Reporting (`internal/crashlog/`)](#7-crash-reporting)
8. [App Lifecycle (`internal/lifecycle/`)](#8-app-lifecycle)
9. [HTTP Utilities (`internal/httputil/`)](#9-http-utilities)
10. [Markdown Processing (`internal/markdown/`)](#10-markdown-processing)
11. [Ripgrep Wrapper (`internal/ripgrep/`)](#11-ripgrep-wrapper)
12. [Self-Updater (`internal/updater/`)](#12-self-updater)
13. [Services (`internal/services/`)](#13-services)
14. [Service Context (`internal/svc/`)](#14-service-context)

---

## 1. Middleware

Package: `github.com/neboloop/nebo/internal/middleware`

All middleware follows the `func(http.Handler) http.Handler` chi-compatible pattern.

---

### 1.1 JWT Authentication

Two files provide JWT functionality: `chi_jwt.go` (chi middleware) and `jwt.go` (claims extraction and validation utilities).

#### chi_jwt.go -- Chi Router Middleware

```go
func JWTMiddleware(secret string) func(http.Handler) http.Handler
```

**Flow:**
1. Extracts `Authorization` header from the request.
2. Rejects with `httputil.Unauthorized("missing authorization header")` if absent.
3. Splits on space; expects exactly 2 parts with case-insensitive `"bearer"` prefix.
4. Rejects with `httputil.Unauthorized("invalid authorization header format")` if malformed.
5. Parses the token using `github.com/golang-jwt/jwt/v4` with an HMAC signing method check.
6. Rejects with `httputil.Unauthorized("invalid token")` if parsing fails or `!token.Valid`.
7. Extracts `jwt.MapClaims` and injects two context values:
   - Key `"userId"` (string) -- from `claims["userId"]`
   - Key `"email"` (string) -- from `claims["email"]`
8. Calls `next.ServeHTTP(w, r.WithContext(ctx))`.

**IMPORTANT**: Context keys are plain strings (`"userId"`, `"email"`), not typed keys. This is for backward compatibility.

#### jwt.go -- Claims Types, Extraction, Validation

**Structs:**

```go
type JWTClaims struct {
    Sub   string `json:"sub"`   // Subject (customer ID)
    Email string `json:"email"` // Customer email
    Name  string `json:"name"`  // Customer name
    Iss   string `json:"iss"`   // Issuer
    Exp   int64  `json:"exp"`   // Expiration time (unix)
    Iat   int64  `json:"iat"`   // Issued at (unix)
}

type ContextKey string

const (
    UserIDKey    ContextKey = "userId"
    UserEmailKey ContextKey = "userEmail"
    UserNameKey  ContextKey = "userName"
)
```

**Functions:**

```go
func GetUserID(ctx context.Context) string
```
- Tries `ctx.Value(UserIDKey)` first (typed key).
- Falls back to `ctx.Value("userId")` (plain string key, for chi_jwt.go compat).
- Returns `""` if neither is found.

```go
func GetUserEmail(ctx context.Context) string
func GetUserName(ctx context.Context) string
```
- Same pattern, typed key only, no fallback.

```go
func ValidateJWT(tokenString, secret string) (jwt.MapClaims, error)
```
- Parses with `jwt.Parse`, enforces `*jwt.SigningMethodHMAC`.
- Returns `ErrInvalidToken` (a `*tokenError`) on failure.

```go
func ValidateJWTClaims(tokenString, secret string) (*JWTClaims, error)
```
- Calls `ValidateJWT`, then maps claims to `JWTClaims`.
- Subject extraction: checks `claims["sub"]` first, then `claims["userId"]` as fallback.
- Returns error `"token missing subject claim"` if neither is present.
- Maps `exp` and `iat` from `float64` (JSON number) to `int64`.

```go
func GenerateAgentWSToken(secret string, ttl time.Duration) (string, error)
```
- Mints a short-lived HS256 JWT with claims: `type: "agent_ws"`, `iat`, `exp`.
- Used for agent WebSocket authentication.

```go
func ValidateAgentWSToken(tokenString, secret string) error
```
- Validates signature and expiration via `ValidateJWT`.
- Checks that `type` claim equals `"agent_ws"`.
- Returns error if type is wrong or missing.

**Error types:**

```go
var ErrInvalidToken = &tokenError{message: "invalid token"}

type tokenError struct{ message string }
func (e *tokenError) Error() string { return e.message }
```

**Private helper:**

```go
func unauthorized(w http.ResponseWriter, message string)
```
- Writes JSON `{"error": message}` with status 401.
- Used internally (the public version is in `httputil`).

---

### 1.2 CORS

File: `cors.go`

**Structs:**

```go
type CORSConfig struct {
    AllowedOrigins   []string  // List of allowed origins, "*" for all (not recommended with credentials)
    AllowedMethods   []string  // Allowed HTTP methods
    AllowedHeaders   []string  // Allowed request headers
    ExposedHeaders   []string  // Headers exposed to the client
    AllowCredentials bool      // Allow cookies/auth headers
    MaxAge           int       // Preflight cache time in seconds
    Debug            bool      // Debug logging
}

type CORS struct {
    config *CORSConfig
}
```

**Preset Configurations:**

```go
func DefaultCORSConfig() *CORSConfig
```
Returns:
- `AllowedOrigins: []string{}` (empty -- effectively blocks all)
- `AllowedMethods: ["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"]`
- `AllowedHeaders: ["Accept", "Authorization", "Content-Type", "X-CSRF-Token", "X-Requested-With"]`
- `ExposedHeaders: ["X-RateLimit-Limit", "X-RateLimit-Remaining", "X-RateLimit-Reset"]`
- `AllowCredentials: true`
- `MaxAge: 86400` (24 hours)

```go
func ProductionCORSConfig(allowedOrigins []string) *CORSConfig
```
Same as default but:
- Uses provided `allowedOrigins`.
- `AllowedMethods`: no `PATCH` (only GET, POST, PUT, DELETE, OPTIONS).
- `AllowedHeaders`: no `X-Requested-With` (only Accept, Authorization, Content-Type, X-CSRF-Token).

**Middleware Flow:**

```go
func (c *CORS) Middleware() func(http.Handler) http.Handler
```

1. Reads `Origin` header from request.
2. If origin is present and allowed:
   - Sets `Access-Control-Allow-Origin: <origin>` (not `*`).
   - Sets `Vary: Origin`.
   - If `AllowCredentials`: sets `Access-Control-Allow-Credentials: true`.
   - If `ExposedHeaders` configured: sets `Access-Control-Expose-Headers`.
3. If method is `OPTIONS` (preflight):
   - Calls `handlePreflight(w, r)`.
   - Returns (does not call next handler).
4. Otherwise calls `next.ServeHTTP(w, r)`.

**Preflight Handler:**

```go
func (c *CORS) handlePreflight(w http.ResponseWriter, r *http.Request)
```
- Checks origin is allowed; returns 204 if not.
- Checks `Access-Control-Request-Method` is in allowed methods; returns 204 if not.
- Checks `Access-Control-Request-Headers` are all in allowed headers; returns 204 if not.
- Always allows simple headers: `accept`, `accept-language`, `content-language`, `content-type`.
- Sets `Access-Control-Allow-Methods`, `Access-Control-Allow-Headers`, `Access-Control-Max-Age`.
- Returns 204 No Content.

**Origin Matching:**

```go
func (c *CORS) isOriginAllowed(origin string) bool
```
- Empty `AllowedOrigins` list returns `false` (blocks all).
- `"*"` matches everything.
- Exact string match.
- Wildcard subdomain: `"*.example.com"` matches any origin ending in `.example.com`.

**Utility Functions:**

```go
func IsLocalhostOrigin(origin string) bool
```
- Returns true if origin starts with `http://localhost`, `https://localhost`, `http://127.0.0.1`, or `https://127.0.0.1` (with or without a port suffix).
- Used by CORS middleware and WebSocket upgraders to restrict to local connections.

```go
func ParseAllowedOrigins(origins string) []string
```
- Splits a comma-separated string of origins.
- Trims whitespace from each.
- Filters out empty strings.

```go
func CORSMiddleware(config *CORSConfig) func(http.Handler) http.Handler
```
- Convenience function: `NewCORS(config).Middleware()`.

---

### 1.3 Rate Limiting

File: `ratelimit.go`

Two implementations: **Token Bucket** and **Sliding Window**.

#### Token Bucket Rate Limiter

**Structs:**

```go
type RateLimiter struct {
    Rate            int                            // Requests allowed per interval
    Interval        time.Duration                  // Time window
    Burst           int                            // Maximum burst size
    clients         sync.Map                       // Per-client state
    KeyFunc         func(*http.Request) string     // Client identifier extractor
    ExceededHandler func(http.ResponseWriter, *http.Request) // Rate limit exceeded handler
    SkipFunc        func(*http.Request) bool       // Skip predicate
}

type clientState struct {
    tokens    float64
    lastCheck time.Time
    mu        sync.Mutex
}

type RateLimitConfig struct {
    Rate            int
    Interval        time.Duration
    Burst           int
    KeyFunc         func(*http.Request) string
    ExceededHandler func(http.ResponseWriter, *http.Request)
    SkipFunc        func(*http.Request) bool
}
```

**Preset Configurations:**

| Config | Rate | Interval | Burst |
|--------|------|----------|-------|
| `DefaultRateLimitConfig()` | 100 | 1 min | 20 |
| `AuthRateLimitConfig()` | 5 | 1 min | 5 |
| `APIRateLimitConfig()` | 1000 | 1 min | 100 |

**Algorithm (Token Bucket):**

```go
func (rl *RateLimiter) Allow(key string) bool
```
1. Gets or creates `clientState` (initial tokens = `Burst`).
2. Calculates elapsed time since last check.
3. Adds tokens: `elapsed_seconds * (Rate / Interval_seconds)`.
4. Caps tokens at `Burst` using `min()`.
5. Updates `lastCheck` to now.
6. If tokens >= 1: decrements by 1, returns `true`.
7. Otherwise returns `false`.

**Middleware:**

```go
func (rl *RateLimiter) Middleware() func(http.Handler) http.Handler
```
1. Checks `SkipFunc` -- if returns true, passes through.
2. Gets client key via `KeyFunc(r)`.
3. If `!Allow(key)`:
   - Sets headers: `X-RateLimit-Limit`, `X-RateLimit-Remaining`, `X-RateLimit-Reset`, `Retry-After`.
   - Calls `ExceededHandler`.
   - Returns.
4. If allowed: sets the same headers (without Retry-After), calls next handler.

**Cleanup Goroutine:**

```go
func (rl *RateLimiter) cleanup()
```
- Runs every 5 minutes.
- Deletes client states with `lastCheck` older than `2 * Interval`.

**Key Functions:**

```go
func DefaultKeyFunc(r *http.Request) string
```
- Returns `r.RemoteAddr` only.
- Intentionally ignores `X-Forwarded-For` and `X-Real-IP` (spoofable).

```go
func TrustedProxyKeyFunc(trustedProxies []string) func(*http.Request) string
```
- Returns a closure that:
  - Strips port from `RemoteAddr`.
  - If `RemoteAddr` IP is in trusted proxy list:
    - Uses first IP from `X-Forwarded-For`.
    - Falls back to `X-Real-IP`.
    - Falls back to `RemoteAddr`.
  - Otherwise uses `RemoteAddr`.

```go
func UserKeyFunc(userIDKey interface{}) func(*http.Request) string
```
- Extracts user ID from context; returns `"user:" + id`.
- Falls back to `DefaultKeyFunc` if no user in context.

```go
func PathKeyFunc(r *http.Request) string
```
- Returns `DefaultKeyFunc(r) + ":" + r.URL.Path`.

**Exceeded Handler:**

```go
func DefaultExceededHandler(w http.ResponseWriter, r *http.Request)
```
- Returns JSON `{"error":"rate limit exceeded","message":"too many requests, please try again later"}` with 429 status.

**Helper utilities:**

```go
func splitHostPort(addr string) (string, string, error)
```
- Custom implementation handling IPv6 bracket notation `[::1]:port` and IPv4 `host:port`.

```go
func splitAndTrim(s, sep string) []string
func split(s, sep string) []string
func trim(s string) string
func min(a, b float64) float64
```

#### Sliding Window Rate Limiter

**Structs:**

```go
type SlidingWindowLimiter struct {
    Window          time.Duration
    Limit           int
    clients         sync.Map
    KeyFunc         func(*http.Request) string
    ExceededHandler func(http.ResponseWriter, *http.Request)
}

type windowState struct {
    requests []time.Time
    mu       sync.Mutex
}
```

**Algorithm:**

```go
func (swl *SlidingWindowLimiter) Allow(key string) bool
```
1. Calculates window start: `now - Window`.
2. Gets or creates `windowState`.
3. Filters out expired requests (before `windowStart`).
4. If `len(requests) >= Limit`: returns `false`.
5. Appends current timestamp, returns `true`.

**Middleware:**

```go
func (swl *SlidingWindowLimiter) Middleware() func(http.Handler) http.Handler
```
- If not allowed: sets `Retry-After` header, calls `ExceededHandler`.

**Cleanup:**
- Runs every `Window` duration.
- Deletes entries with no valid requests within `2 * Window`.

---

### 1.4 Security Headers

File: `security_headers.go`

**Struct:**

```go
type SecurityHeaders struct {
    ContentSecurityPolicy   string
    PermissionsPolicy       string
    ReferrerPolicy          string
    StrictTransportSecurity string
    XContentTypeOptions     string
    XFrameOptions           string
    XXSSProtection          string
    CacheControl            string
    Pragma                  string
}
```

**Preset Configurations:**

| Header | `DefaultSecurityHeaders()` | `APISecurityHeaders()` |
|--------|---------------------------|------------------------|
| Content-Security-Policy | `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; font-src 'self'; object-src 'none'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'` | `default-src 'none'; frame-ancestors 'none'` |
| Permissions-Policy | `accelerometer=(), camera=(self), geolocation=(), gyroscope=(), magnetometer=(), microphone=(self), payment=(), usb=()` | Same (except camera=() instead of camera=(self)) |
| Referrer-Policy | `strict-origin-when-cross-origin` | Same |
| Strict-Transport-Security | `max-age=31536000; includeSubDomains; preload` | Same |
| X-Content-Type-Options | `nosniff` | Same |
| X-Frame-Options | `DENY` | Same |
| X-XSS-Protection | `1; mode=block` | Same |
| Cache-Control | `no-store, no-cache, must-revalidate, private` | Same |
| Pragma | `no-cache` | Same |

**Note:** The API preset uses `camera=()` (blocked) vs the default's `camera=(self)` (allowed for self).

**Middleware:**

```go
func SecurityHeadersMiddleware(config *SecurityHeaders) func(http.Handler) http.Handler
```
- Falls back to `DefaultSecurityHeaders()` if `config` is nil.
- Sets each header only if its config value is non-empty.

**Convenience wrappers:**

```go
func SecureHandler(handler http.Handler, config *SecurityHeaders) http.Handler
func SecureHandlerFunc(handler http.HandlerFunc, config *SecurityHeaders) http.HandlerFunc
```

---

### 1.5 CSRF Protection

File: `csrf.go`

Two implementations: **Server-Side Token Store** and **Double Submit Cookie**.

#### Server-Side CSRF Protection

**Structs:**

```go
type CSRFConfig struct {
    Secret        []byte          // Signing key (>= 32 bytes)
    TokenLength   int             // Random bytes length (default: 32)
    TokenExpiry   time.Duration   // Token TTL (default: 12h)
    CookieName    string          // Cookie name (default: "csrf_token")
    HeaderName    string          // Header name (default: "X-CSRF-Token")
    FormFieldName string          // Form field name (default: "csrf_token")
    SecureCookie  bool            // Secure flag (default: true)
    HTTPOnly      bool            // HttpOnly flag (default: false for JS SPA access)
    SameSite      http.SameSite   // SameSite attribute (default: Strict)
    CookiePath    string          // Cookie path (default: "/")
    CookieDomain  string          // Cookie domain (optional)
    SkipPaths     []string        // Paths that skip CSRF (prefix match)
    SkipMethods   []string        // Methods that skip CSRF (default: GET, HEAD, OPTIONS, TRACE)
    ErrorHandler  func(w http.ResponseWriter, r *http.Request, err error)
}

type CSRFToken struct {
    Token     string
    ExpiresAt time.Time
}

type CSRFProtection struct {
    config *CSRFConfig
    tokens sync.Map   // In-memory token hash -> expiry
}
```

**Errors:**

```go
var ErrInvalidCSRFToken = errors.New("invalid or missing CSRF token")
var ErrCSRFTokenExpired = errors.New("CSRF token expired")
```

**Token Generation Algorithm:**

```go
func (c *CSRFProtection) GenerateToken() (*CSRFToken, error)
```
1. Generate `TokenLength` (32) random bytes via `crypto/rand`.
2. Base64-URL-encode the random bytes -> `token`.
3. Set `expiresAt = now + TokenExpiry`.
4. Sign: `signedToken = signToken(token, expiresAt)`.
5. Hash signed token via SHA-256 -> store in `sync.Map` with expiry.
6. Return `CSRFToken{Token: signedToken, ExpiresAt: expiresAt}`.

**Token Signing:**

```go
func (c *CSRFProtection) signToken(token string, expiresAt time.Time) string
```
- Message: `token + "|" + expiresAt.Format(RFC3339)`
- Signature: `SHA256(Secret + message)` (not HMAC, just hash concatenation)
- Output: `message + "|" + hex(signature)` (3-part pipe-delimited string)

**Token Validation:**

```go
func (c *CSRFProtection) ValidateToken(token string) error
```
1. Empty token -> `ErrInvalidCSRFToken`.
2. Hash the token -> look up in `sync.Map`.
3. If found and not expired -> `nil` (valid).
4. If found and expired -> delete from map, return `ErrCSRFTokenExpired`.
5. If not found -> verify token signature directly via `verifyTokenSignature`.

**Signature Verification:**

```go
func (c *CSRFProtection) verifyTokenSignature(signedToken string) bool
```
1. Split on `|` -> expect exactly 3 parts: `[token, expiryStr, signature]`.
2. Parse expiry; reject if expired.
3. Recreate signature from `token + "|" + expiryStr`.
4. Constant-time compare (`crypto/subtle.ConstantTimeCompare`).

**Middleware Flow:**

```go
func (c *CSRFProtection) Middleware() func(http.Handler) http.Handler
```
1. If method is safe (GET/HEAD/OPTIONS/TRACE):
   - If no CSRF cookie exists, generate and set one.
   - Pass through.
2. If path matches `SkipPaths` (prefix match): pass through.
3. Extract token from request:
   - Header: `X-CSRF-Token` first.
   - Form field: `csrf_token` (POST only, after `ParseForm`).
4. If no token found: call error handler (403 Forbidden).
5. If `ValidateToken` fails: call error handler.
6. Otherwise: pass through.

**Token Cleanup:**
- Background goroutine every 1 hour.
- Iterates `sync.Map`, deletes expired entries.

**Token Handler:**

```go
func (c *CSRFProtection) GetTokenHandler() http.HandlerFunc
```
- Generates a new token.
- Sets it as a cookie.
- Returns JSON `{"token": "<signedToken>"}` with 200 OK.

**Default Error Handler:**

```go
func defaultCSRFErrorHandler(w http.ResponseWriter, r *http.Request, err error)
```
- Returns JSON `{"error":"CSRF validation failed","message":"forbidden"}` with 403.

#### Double Submit Cookie

**Struct:**

```go
type DoubleSubmitCookie struct {
    config *CSRFConfig
}
```

**Algorithm (Stateless):**
1. Safe methods: ensure cookie exists (generate 32 random bytes, base64-URL-encode), pass through.
2. Unsafe methods:
   - Read cookie value.
   - Read header/form token.
   - Constant-time compare cookie vs header token.
   - If mismatch: 403.
3. Cookie attributes: `HttpOnly: false` (must be readable by JS), `MaxAge` from `TokenExpiry`.

---

### 1.6 Compression & Cache Control

File: `compression.go`

**Gzip Middleware:**

```go
func Gzip(next http.Handler) http.Handler
```

**Skip conditions (no compression):**
- Client does not send `Accept-Encoding: gzip`.
- WebSocket requests: path starts with `/ws` OR `Upgrade: websocket` header.
- API routes: path starts with `/api/` (small JSON, often proxied).
- Already-compressed file extensions: `.png`, `.jpg`, `.jpeg`, `.gif`, `.webp`, `.mp4`, `.webm`, `.pdf`.

**Compression behavior:**
- Sets `Content-Encoding: gzip`.
- Removes `Content-Length` (will change after compression).
- Wraps response writer in `gzipResponseWriter` that redirects `Write()` to `gzip.NewWriter`.

**Content-Type Middleware:**

```go
func ContentType(next http.Handler) http.Handler
```
- Sets `Content-Type` based on file extension using `mime.TypeByExtension`.
- Only sets if extension is non-empty and recognized.

**Cache Control Middleware:**

```go
func CacheControl(next http.Handler) http.Handler
```

| Path Pattern | Cache-Control Header |
|---|---|
| `/api/*` | `no-store, no-cache, must-revalidate` |
| `*/_app/immutable/*` | `public, max-age=31536000, immutable` (1 year, SvelteKit hashed assets) |
| `*.html` or `/` | `no-cache, must-revalidate` |
| Everything else | `public, max-age=86400` (24 hours) |

---

### 1.7 Validation

File: `validation.go`

**Structs:**

```go
type ValidationConfig struct {
    MaxBodySize          int64  // Default: 10MB (10 * 1024 * 1024)
    MaxURLLength         int    // Default: 2048
    ValidateSQLInjection bool   // Default: true
    ValidateXSS          bool   // Default: true
    SanitizeInput        bool   // Default: true
}

type RequestValidator struct {
    config *ValidationConfig
}
```

**Middleware Flow:**

```go
func (rv *RequestValidator) Middleware() func(http.Handler) http.Handler
```
1. URL length check: `len(r.URL.String()) > MaxURLLength` -> 414 URI Too Long.
2. Body size check: `r.ContentLength > MaxBodySize` -> 413 Payload Too Large.
3. Wraps body in `http.MaxBytesReader`.
4. Validates URL query parameters:
   - Each key/value pair checked against `security.DetectSQLInjection()` and `security.DetectXSS()`.
   - On detection: 400 Bad Request with JSON `{"error":"validation_error","message":"Invalid input detected in query parameter: <key>"}`.
5. Validates URL path:
   - Same SQL injection and XSS checks.
   - On detection: `"Invalid input detected in URL path"`.
6. Passes through on success.

**JSON Body Validation:**

```go
func ValidateJSONBody(r *http.Request, maxSize int64) ([]byte, error)
```
- Limits body with `MaxBytesReader`.
- Reads all bytes.
- Validates UTF-8; sanitizes invalid sequences.
- Returns sanitized bytes.

**Request Map Sanitization:**

```go
func SanitizeRequestMap(params map[string]interface{}) map[string]interface{}
```
- Recursively sanitizes all string values via `security.SanitizeString(v, DefaultSanitizeOptions())`.
- Sanitizes map keys via `security.SanitizeName(key)`.
- Handles nested maps and slices.

**Content-Type Validator:**

```go
type ContentTypeValidator struct {
    allowedTypes []string
}

func (cv *ContentTypeValidator) Middleware() func(http.Handler) http.Handler
```
- Skips GET, HEAD, OPTIONS.
- Requires `Content-Type` header on other methods.
- Parses media type (strips parameters like `; charset=utf-8`).
- Case-insensitive comparison against allowed types.
- Returns 415 Unsupported Media Type on mismatch.

```go
func JSONContentTypeValidator() func(http.Handler) http.Handler
```
- Convenience: only allows `application/json`.

---

### 1.8 Security Orchestrator

File: `security.go`

Combines all security middleware into a single configurable wrapper.

**Struct:**

```go
type SecurityMiddleware struct {
    config          config.Config
    securityHeaders *SecurityHeaders
    csrf            *CSRFProtection
    rateLimiter     *RateLimiter
    authRateLimiter *RateLimiter
    cors            *CORS
}
```

**Initialization:**

```go
func NewSecurityMiddleware(cfg config.Config) *SecurityMiddleware
```

1. **Security Headers**: If `cfg.IsSecurityHeadersEnabled()`:
   - Uses `APISecurityHeaders()` as base.
   - Overrides CSP if `cfg.Security.ContentSecurityPolicy` is set.

2. **CSRF**: If `cfg.IsCSRFEnabled()`:
   - Secret: `cfg.Security.CSRFSecret`, falls back to `cfg.Auth.AccessSecret`.
   - Token expiry from config (seconds).
   - Secure cookie from config.
   - `SkipPaths: ["/api/v1/"]` (API uses JWT, not CSRF).

3. **Rate Limiting**: If `cfg.IsRateLimitEnabled()`:
   - General limiter: `Rate`, `Interval`, `Burst` from config, `DefaultKeyFunc`.
   - Auth limiter (stricter): `AuthRateLimitRequests`, `AuthRateLimitInterval` from config. Burst = Rate.

4. **CORS**:
   - If `cfg.Security.AllowedOrigins` is set: parse origins, use `ProductionCORSConfig`.
   - Otherwise: use `DefaultCORSConfig()`.

**Handler Wrapping:**

```go
func (sm *SecurityMiddleware) Handler(handler http.Handler) http.Handler
```
Application order (reverse of execution order):
1. Security headers (outermost -- runs last, ensures headers are always set).
2. CORS.
3. Rate limiting (general).

```go
func (sm *SecurityMiddleware) AuthHandler(handler http.Handler) http.Handler
```
Application order:
1. Security headers.
2. CORS.
3. Auth rate limiter (stricter).
4. CSRF protection.

**Additional Middleware:**

```go
func HTTPSRedirectMiddleware(next http.Handler) http.Handler
```
- Redirects HTTP to HTTPS (301 Moved Permanently).
- Detects SSL termination via `X-Forwarded-Proto` header or `r.TLS` presence.

```go
func RequestSizeMiddleware(maxBytes int64) func(http.Handler) http.Handler
```
- Checks `r.ContentLength` against max.
- Wraps body in `http.MaxBytesReader`.

```go
func ChainMiddleware(middlewares ...func(http.Handler) http.Handler) func(http.Handler) http.Handler
```
- Applies middlewares in reverse order so the first middleware in the list runs first.

```go
func RequestIDMiddleware(next http.Handler) http.Handler
```
- Uses `X-Request-ID` from request if present.
- Otherwise generates `"20060102150405-<8-char-random>"`.
- Sets `X-Request-ID` on response.

```go
func GetCSRFTokenHandler() http.HandlerFunc
```
- Returns CSRF token handler if CSRF is enabled.
- Otherwise returns `{"message":"CSRF protection disabled"}`.

---

## 2. Security Utilities

Package: `github.com/neboloop/nebo/internal/security`

---

### 2.1 Output Encoding

File: `encoding.go`

**Struct:**

```go
type OutputEncoder struct{}
```

**Methods:**

| Method | Purpose | Implementation |
|--------|---------|----------------|
| `HTMLEncode(input)` | Safe HTML output | `html.EscapeString` |
| `HTMLDecode(input)` | Decode HTML entities | `html.UnescapeString` |
| `HTMLAttributeEncode(input)` | Safe HTML attributes | Manual: escapes `& < > " ' / = `` ` with named/hex entities |
| `URLEncode(input)` | Query string encoding | `url.QueryEscape` |
| `URLDecode(input)` | Query string decoding | `url.QueryUnescape` |
| `URLPathEncode(input)` | URL path encoding | `url.PathEscape` |
| `URLPathDecode(input)` | URL path decoding | `url.PathUnescape` |
| `JavaScriptEncode(input)` | Safe inline JS | Escapes `\ ' " \n \r \t < > & = /`, non-ASCII as `\uXXXX` |
| `CSSEncode(input)` | Safe CSS contexts | Only alphanumeric pass through; everything else as `\XXXXXX ` |
| `Base64Encode(input)` | Standard base64 | `base64.StdEncoding.EncodeToString` |
| `Base64Decode(input)` | Standard base64 decode | `base64.StdEncoding.DecodeString` |
| `Base64URLEncode(input)` | URL-safe base64 | `base64.URLEncoding.EncodeToString` |
| `Base64URLDecode(input)` | URL-safe base64 decode | `base64.URLEncoding.DecodeString` |

**Context-Based Encoding:**

```go
type OutputContext int

const (
    ContextHTML OutputContext = iota
    ContextHTMLAttribute
    ContextJavaScript
    ContextCSS
    ContextURL
    ContextURLPath
    ContextJSON
)

func EncodeForContext(input string, ctx OutputContext) string
```
- Dispatches to the appropriate encoder method.
- Default (unknown context) falls back to HTML encoding.

**UTF-8 Utilities:**

```go
func ValidateUTF8(input string) bool    // utf8.ValidString wrapper
func SanitizeUTF8(input string) string  // Replaces invalid sequences with U+FFFD
```

**Convenience Functions (package-level):**

```go
func HTMLEncode(input string) string
func HTMLDecode(input string) string
func URLEncode(input string) string
func URLDecode(input string) (string, error)
```

**Safe JSON Response Types:**

```go
type SafeJSONResponse struct { encoder *OutputEncoder }
func (s *SafeJSONResponse) Encode(v interface{}) ([]byte, error)      // json.Marshal
func (s *SafeJSONResponse) EncodeHTML(v interface{}) (string, error)   // json.Marshal (already HTML-safe)

type SafeResponseHeaders struct {
    ContentType string  // "application/json; charset=utf-8"
    NoSniff     bool    // X-Content-Type-Options: nosniff
    NoCache     bool    // Cache prevention
}
```

---

### 2.2 Input Sanitization

File: `sanitize.go`

**Compiled Regex Patterns:**

| Pattern Variable | Matches |
|------------------|---------|
| `sqlInjectionPattern` | `--`, `;`, `'`, `"`, `\`, `/*`, `*/`, `xp_`, `sp_`, `0x`, `union select`, `select...from`, `insert into`, `delete from`, `drop table`, `update...set`, `exec(`, `execute(` |
| `xssPattern` | `<script>`, `</script>`, `javascript:`, `on*=` event handlers, `<iframe`, `<object`, `<embed`, `<form`, `<input`, `<button`, `data:text/html`, `vbscript:` |
| `pathTraversalPattern` | `../`, `..\`, URL-encoded variants (`%2e%2e%2f`, etc.) |
| `nullBytePattern` | `\x00`, `%00` |
| `excessiveWhitespace` | 2+ consecutive whitespace/Unicode space chars |
| `htmlTagPattern` | Any HTML tag `<...>` |
| `multipleNewlines` | 3+ consecutive newlines |
| `controlCharsPattern` | ASCII control chars `\x00-\x08`, `\x0B`, `\x0C`, `\x0E-\x1F`, `\x7F` |

**Safe URL Schemes:**

```go
var safeURLSchemes = map[string]bool{
    "http":   true,
    "https":  true,
    "mailto": true,
}
```

**Sanitization Options:**

```go
type SanitizeOptions struct {
    AllowHTML           bool  // false: strip HTML tags
    TrimWhitespace      bool  // true: trim leading/trailing
    NormalizeWhitespace bool  // true: collapse multiple spaces
    MaxLength           int   // 0: no limit
    AllowNewlines       bool  // true: preserve \n
    StripControlChars   bool  // true: remove control chars
    EscapeHTML          bool  // true: HTML-encode special chars
}
```

| Preset | AllowHTML | Trim | Normalize | MaxLen | Newlines | StripCtrl | EscapeHTML |
|--------|----------|------|-----------|--------|----------|-----------|------------|
| `DefaultSanitizeOptions()` | false | true | true | 0 | true | true | true |
| `StrictSanitizeOptions()` | false | true | true | 10000 | false | true | true |

**Core Sanitization Function:**

```go
func SanitizeString(input string, opts SanitizeOptions) string
```

Processing order:
1. Remove null bytes.
2. Strip control characters (if enabled).
3. Strip HTML tags (if `!AllowHTML`).
4. Escape HTML entities (if `EscapeHTML`).
5. Trim whitespace (if enabled).
6. Normalize whitespace:
   - If newlines allowed: normalize spaces per line, collapse 3+ newlines to 2.
   - If newlines not allowed: collapse all whitespace.
7. Remove newlines (if `!AllowNewlines`): replace `\n` with space, remove `\r`.
8. Truncate to `MaxLength` (if > 0).

**Specialized Sanitizers:**

```go
func SanitizeEmail(email string) string    // Trim, lowercase, remove null/control chars
func SanitizeName(name string) string      // AllowHTML=false, MaxLength=100, no newlines, escape HTML
func SanitizeURL(url string) string        // Trim, null byte removal, rejects path traversal/XSS
func SanitizeFilename(filename string) string  // Strips traversal, replaces /\ with _, max 255 chars
func SanitizeForLog(input string) string   // Escape \n\r, strip control chars, truncate to 1000 + "...[truncated]"
func SanitizeJSON(input string) string     // Remove null bytes, keep only printable + whitespace
```

**Detection Functions:**

```go
func DetectSQLInjection(input string) bool  // Uses sqlInjectionPattern regex
func DetectXSS(input string) bool           // Uses xssPattern regex
func DetectPathTraversal(input string) bool  // Uses pathTraversalPattern regex
func ContainsNullByte(input string) bool    // Uses nullBytePattern regex
func IsValidURLScheme(url string) bool      // Checks against safeURLSchemes map
func IsPrintable(s string) bool             // All chars are unicode.IsPrint or \n\r\t
```

**String Utilities:**

```go
func StripHTMLTags(input string) string              // htmlTagPattern.ReplaceAll
func EscapeHTML(input string) string                  // html.EscapeString
func UnescapeHTML(input string) string                // html.UnescapeString
func ValidateInputLength(input string, minLen, maxLen int) bool
func TruncateString(input string, maxLen int) string
```

---

### 2.3 SQL Injection Prevention

File: `sql.go`

**Errors:**

```go
var ErrSQLInjectionDetected = errors.New("potential SQL injection detected")
var ErrInvalidInput = errors.New("invalid input")
```

**Pattern List (`SQLInjectionPatterns`):**

30 regex patterns compiled at `init()` time into `sqlInjectionRegexes []*regexp.Regexp`:

- SQL comments: `--`, `/*`, `*/`, `#`
- Keywords with context: `union select`, `select...from`, `insert into`, `delete from`, `drop table`, `drop database`, `truncate table`, `update...set`, `exec(`, `execute(`
- Stored procedure prefixes: `xp_`, `sp_`
- String concatenation: `concat(`, `||`
- Boolean injection: `or 1=1`, `or '...'='...'`, `and 1=1`, `and '...'='...'`
- Time-based: `waitfor delay`, `sleep(`, `benchmark(`
- Information gathering: `information_schema`, `sys.`, `sysobjects`, `syscolumns`
- LIKE abuse: `like '%`
- Hex encoding: `0x[0-9a-fA-F]+`
- Null bytes: `\x00`, `%00`
- Character encoding: `char(\d+)`
- Stacked queries: `; select|insert|update|delete|drop|truncate|exec|execute`

**SQL Validator:**

```go
type SQLValidator struct {
    AdditionalPatterns []*regexp.Regexp
    AllowedPatterns    []string
    MaxInputLength     int  // Default: 10000
}

func NewSQLValidator() *SQLValidator
```

```go
func (v *SQLValidator) ValidateInput(input string) error
```
1. Empty input -> nil.
2. Length check against `MaxInputLength` -> `ErrInvalidInput`.
3. Null byte check -> `ErrSQLInjectionDetected`.
4. Default pattern check (all 30 patterns) -> `ErrSQLInjectionDetected`.
5. Additional pattern check -> `ErrSQLInjectionDetected`.

```go
func (v *SQLValidator) ValidateInputs(inputs ...string) error  // Validates all inputs
func (v *SQLValidator) IsSafe(input string) bool                // Returns true if no error
func ValidateSQLInput(input string) error                       // Convenience: NewSQLValidator().ValidateInput()
```

**SQL Sanitizer:**

```go
func SanitizeForSQL(input string) string
```
- Removes null bytes (`\x00`, `%00`).
- Removes SQL comments (`--`, `/*`, `*/`).
- Doubles single quotes (`'` -> `''`).
- **Note:** Supplement to parameterized queries, not a replacement.

**Identifier Validator:**

```go
type IdentifierValidator struct {
    AllowedChars  *regexp.Regexp     // `^[a-zA-Z_][a-zA-Z0-9_]*$`
    MaxLength     int                 // 128
    ReservedWords map[string]bool     // 38 SQL reserved words
}

func (v *IdentifierValidator) ValidateIdentifier(identifier string) error
func (v *IdentifierValidator) IsValidIdentifier(identifier string) bool
```

Reserved words include: `select`, `insert`, `update`, `delete`, `drop`, `create`, `alter`, `truncate`, `from`, `where`, `and`, `or`, `not`, `in`, `like`, `between`, `join`, `inner`, `outer`, `left`, `right`, `cross`, `on`, `group`, `by`, `order`, `having`, `limit`, `offset`, `union`, `except`, `intersect`, `all`, `distinct`, `as`, `null`, `true`, `false`, `table`, `index`, `database`, `schema`, `grant`, `revoke`.

**ORDER BY / Pagination Utilities:**

```go
func ValidateOrderByColumn(column string, allowedColumns []string) bool  // Whitelist check, case-insensitive
func ValidateOrderDirection(direction string) bool                        // "ASC", "DESC", or ""
func ValidatePagination(page, pageSize int) bool                         // page >= 0, 0 < pageSize <= 1000
func SafeLimit(requested, maxAllowed int) int                            // Clamp to [1, max], default 10
func SafeOffset(offset int) int                                          // Clamp to >= 0
```

---

## 3. Configuration

Package: `github.com/neboloop/nebo/internal/config`

File: `config.go`

### Config Struct

```go
type Config struct {
    Name string `yaml:"Name"`
    Host string `yaml:"Host"`   // Default: "127.0.0.1"
    Port int    `yaml:"Port"`   // Default: 27895

    App struct {
        BaseURL        string `yaml:"BaseURL"`        // Default: "http://localhost:27895"
        Domain         string `yaml:"Domain"`         // Default: "localhost"
        ProductionMode string `yaml:"ProductionMode"` // "true"/"false"
        AdminEmail     string `yaml:"AdminEmail"`
    } `yaml:"App"`

    Auth struct {
        AccessSecret       string `yaml:"AccessSecret"`
        AccessExpire       int64  `yaml:"AccessExpire"`        // seconds
        RefreshTokenExpire int64  `yaml:"RefreshTokenExpire"`  // Default: 604800 (7 days)
    } `yaml:"Auth"`

    Database struct {
        SQLitePath string `yaml:"SQLitePath"`  // Default: <DataDir>/data/nebo.db
    } `yaml:"Database"`

    Security struct {
        CSRFEnabled           string `yaml:"CSRFEnabled"`            // Default: "true"
        CSRFSecret            string `yaml:"CSRFSecret"`
        CSRFTokenExpiry       int64  `yaml:"CSRFTokenExpiry"`        // Default: 43200 (12h) seconds
        CSRFSecureCookie      string `yaml:"CSRFSecureCookie"`       // Default: "true"
        RateLimitEnabled      string `yaml:"RateLimitEnabled"`       // Default: "true"
        RateLimitRequests     int    `yaml:"RateLimitRequests"`      // Default: 100
        RateLimitInterval     int    `yaml:"RateLimitInterval"`      // Default: 60 (seconds)
        RateLimitBurst        int    `yaml:"RateLimitBurst"`         // Default: 20
        AuthRateLimitRequests int    `yaml:"AuthRateLimitRequests"`  // Default: 5
        AuthRateLimitInterval int    `yaml:"AuthRateLimitInterval"`  // Default: 60 (seconds)
        EnableSecurityHeaders string `yaml:"EnableSecurityHeaders"`  // Default: "true"
        ContentSecurityPolicy string `yaml:"ContentSecurityPolicy"`  // Custom CSP override
        AllowedOrigins        string `yaml:"AllowedOrigins"`         // Comma-separated origins
        ForceHTTPS            string `yaml:"ForceHTTPS"`             // Default: "false"
        MaxRequestBodySize    int64  `yaml:"MaxRequestBodySize"`     // Default: 10485760 (10MB)
        MaxURLLength          int    `yaml:"MaxURLLength"`           // Default: 2048
    } `yaml:"Security"`

    Email struct {
        SMTPHost    string `yaml:"SMTPHost"`
        SMTPPort    int    `yaml:"SMTPPort"`      // Default: 587
        SMTPUser    string `yaml:"SMTPUser"`
        SMTPPass    string `yaml:"SMTPPass"`
        FromAddress string `yaml:"FromAddress"`
        FromName    string `yaml:"FromName"`      // Default: "nebo"
        ReplyTo     string `yaml:"ReplyTo"`
        BaseURL     string `yaml:"BaseURL"`       // Default: "http://localhost:27458"
    } `yaml:"Email"`

    OAuth struct {
        GoogleEnabled      string `yaml:"GoogleEnabled"`
        GoogleClientID     string `yaml:"GoogleClientID"`
        GoogleClientSecret string `yaml:"GoogleClientSecret"`
        GitHubEnabled      string `yaml:"GitHubEnabled"`
        GitHubClientID     string `yaml:"GitHubClientID"`
        GitHubClientSecret string `yaml:"GitHubClientSecret"`
        CallbackBaseURL    string `yaml:"CallbackBaseURL"`
    } `yaml:"OAuth"`

    Features struct {
        NotificationsEnabled string `yaml:"NotificationsEnabled"` // Default: "true"
        OAuthEnabled         string `yaml:"OAuthEnabled"`
    } `yaml:"Features"`

    NeboLoop struct {
        Enabled  string `yaml:"Enabled"`  // Default: "true"
        ApiURL   string `yaml:"ApiURL"`
        JanusURL string `yaml:"JanusURL"`
        CommsURL string `yaml:"CommsURL"`
    } `yaml:"NeboLoop"`

    AppOAuth map[string]AppOAuthProviderConfig `yaml:"AppOAuth"`
}

type AppOAuthProviderConfig struct {
    ClientID     string `yaml:"ClientID"`
    ClientSecret string `yaml:"ClientSecret"`
    TenantID     string `yaml:"TenantID"` // Microsoft only
}
```

### Loading

```go
func LoadFromBytes(data []byte) (Config, error)
```

1. **Environment variable expansion**: `os.ExpandEnv(string(data))` -- replaces `$VAR` and `${VAR}` in YAML.
2. **YAML unmarshal**: `yaml.Unmarshal`.
3. **Apply defaults**: `applyDefaults(&c)`.

### Default Application

`applyDefaults(c *Config)` sets defaults for all zero-valued fields (see table in struct above).

**NeboLoop URL overrides from environment:**
- `NEBOLOOP_API_URL` -> `c.NeboLoop.ApiURL`
- `NEBOLOOP_JANUS_URL` -> `c.NeboLoop.JanusURL`
- `NEBOLOOP_COMMS_URL` -> `c.NeboLoop.CommsURL`

### Boolean Helpers

All boolean config fields are stored as strings ("true"/"false"/"1"/"yes").

```go
func parseBool(s string, defaultVal bool) bool
```
- Trims, lowercases.
- Empty -> returns `defaultVal`.
- `"true"`, `"1"`, `"yes"` -> true.
- Everything else -> false.

**Config boolean methods:**

| Method | Default |
|--------|---------|
| `IsProductionMode()` | false |
| `IsCSRFEnabled()` | true |
| `IsCSRFSecureCookie()` | true |
| `IsRateLimitEnabled()` | true |
| `IsSecurityHeadersEnabled()` | true |
| `IsForceHTTPS()` | false |
| `IsGoogleOAuthEnabled()` | false |
| `IsGitHubOAuthEnabled()` | false |
| `IsNotificationsEnabled()` | true |
| `IsOAuthEnabled()` | false |
| `IsNeboLoopEnabled()` | true |

---

## 4. Data Directory & Defaults

Package: `github.com/neboloop/nebo/internal/defaults`

File: `defaults.go`

### Embedded Default Files

```go
//go:embed dotnebo/*
var defaultFiles embed.FS
```

Files embedded from `dotnebo/` directory: `config.yaml`, `models.yaml`, `SOUL.md`, `HEARTBEAT.md`.

### Data Directory Resolution

```go
func DataDir() (string, error)
```

| Priority | Source | Path |
|----------|--------|------|
| 1 | `NEBO_DATA_DIR` env var | Exact value |
| 2 | `os.UserConfigDir()` | macOS: `~/Library/Application Support/Nebo/`, Windows: `%AppData%\Nebo\`, Linux: `~/.config/nebo/` |

**Platform naming:** Linux uses lowercase `nebo` (XDG convention), macOS/Windows use title case `Nebo`.

### Directory Setup

```go
func EnsureDataDir() (string, error)
```
1. Gets data dir via `DataDir()`.
2. Creates directory with `os.MkdirAll(dir, 0755)`.
3. Copies default files (only if they don't exist).
4. Returns the directory path.

```go
func Reset(dir string) error
```
- Copies default files with overwrite=true.
- Database and settings.json are preserved (they're not in the embedded defaults).

### File Copy Logic

```go
func copyDefaults(dir string, overwrite bool) error
```
- Walks `defaultFiles` embed.FS.
- Uses `strings.TrimPrefix` (not `filepath.Rel`) because embed.FS uses forward slashes but `filepath.Rel` produces backslashes on Windows.
- Creates subdirectories with `os.MkdirAll(destPath, 0755)`.
- Files written with `os.WriteFile(destPath, data, 0644)`.
- Skips existing files unless `overwrite` is true.

### Bot ID Management

```go
const BotIDFile = "bot_id"

func ReadBotID() string
```
- Reads `<data_dir>/bot_id`.
- Trims whitespace.
- Returns `""` if file doesn't exist or content is not exactly 36 characters (UUID format).

```go
func WriteBotID(id string) error
```
- Removes existing file first (`os.Remove` -- needed because file has 0400 permissions).
- Writes with `os.WriteFile(path, []byte(id), 0400)` (read-only).

### Setup Completion

```go
const SetupCompleteFile = ".setup-complete"

func IsSetupComplete() (bool, error)
```
- Checks existence of `<data_dir>/.setup-complete`.

```go
func MarkSetupComplete() error
```
- Ensures data directory exists.
- Writes current unix timestamp to `.setup-complete` with permissions 0644.

### Utility Functions

```go
func GetDefault(name string) ([]byte, error)   // Read embedded file by name
func ListDefaults() ([]string, error)           // List all embedded file names
```

---

## 5. Local Settings

Package: `github.com/neboloop/nebo/internal/local`

### 5.1 Settings (settings.go)

File location: `<data_dir>/settings.json`

**Struct:**

```go
type Settings struct {
    AccessSecret       string `json:"accessSecret"`
    AccessExpire       int64  `json:"accessExpire"`        // Default: 31536000 (1 year)
    RefreshTokenExpire int64  `json:"refreshTokenExpire"`  // Default: 31536000 (1 year)
}
```

**Loading Logic:**

```go
func LoadSettings() (*Settings, error)
```
1. Ensures directory exists with `os.MkdirAll(dir, 0700)`.
2. Tries to read and parse existing `settings.json`.
3. If loaded and `AccessSecret` is empty: generates secret, saves, returns.
4. If file doesn't exist or is corrupt: creates new settings with generated secret.
5. File permissions: `0600` (owner read/write only).

**Secret Generation:**

```go
func generateSecret() string
```
- 32 bytes from `crypto/rand.Read`.
- Hex-encoded -> 64-character string.
- Fallback: `"nebo-<pid>"` (if crypto/rand fails, which should never happen).

### 5.2 Auth Service (auth.go)

**Struct:**

```go
type AuthService struct {
    store  *db.Store
    config config.Config
}
```

**Errors:**

```go
var ErrUserNotFound       = errors.New("user not found")
var ErrInvalidCredentials = errors.New("invalid credentials")
var ErrEmailExists        = errors.New("email already exists")
var ErrInvalidToken       = errors.New("invalid or expired token")
```

**Registration:**

```go
func (s *AuthService) Register(ctx context.Context, email, password, name string) (*AuthResponse, error)
```
1. Checks email uniqueness via `store.CheckEmailExists`.
2. Hashes password with `bcrypt.GenerateFromPassword([]byte(password), bcrypt.DefaultCost)`.
3. Creates user with random 32-byte hex ID.
4. Creates default user preferences.
5. Generates access + refresh tokens.

**Login:**

```go
func (s *AuthService) Login(ctx context.Context, email, password string) (*AuthResponse, error)
```
1. Looks up user by email.
2. Verifies password with `bcrypt.CompareHashAndPassword`.
3. Returns `ErrInvalidCredentials` for wrong password OR non-existent user.

**Token Generation:**

```go
func (s *AuthService) generateTokens(ctx context.Context, userID, email string) (*AuthResponse, error)
```
1. Access token: HS256 JWT with claims `userId`, `email`, `iat`, `exp`.
   - Expiry: `config.Auth.AccessExpire` seconds from now.
   - Signed with `config.Auth.AccessSecret`.
2. Refresh token: 64-byte random hex string.
   - Stored in DB as a hex-encoded copy (first 32 bytes) -- this is the "hash".
   - Expiry: `config.Auth.RefreshTokenExpire` seconds from now.

**AuthResponse:**

```go
type AuthResponse struct {
    Token        string
    RefreshToken string
    ExpiresAt    time.Time
    CheckoutURL  string // Only set during registration with paid plan
}
```

**Other Methods:**

```go
func (s *AuthService) RefreshToken(ctx context.Context, refreshToken string) (*AuthResponse, error)
func (s *AuthService) GetUserByID(ctx context.Context, userID string) (*db.User, error)
func (s *AuthService) GetUserByEmail(ctx context.Context, email string) (*db.User, error)
func (s *AuthService) UpdateUser(ctx context.Context, user *db.User) error
func (s *AuthService) VerifyEmail(ctx context.Context, token string) error  // No-op (returns nil)
func (s *AuthService) ChangePassword(ctx context.Context, userID, currentPassword, newPassword string) error
func (s *AuthService) DeleteUser(ctx context.Context, userID string) error
func (s *AuthService) CreatePasswordResetToken(ctx context.Context, email string) (string, error)
func (s *AuthService) ResetPassword(ctx context.Context, token, newPassword string) error
func (s *AuthService) GenerateTokensForUser(ctx context.Context, userID, email string) (*AuthResponse, error)
```

**Password Reset Token:**
- 64-byte random hex string.
- Expires in 1 hour.
- `CreatePasswordResetToken` does not reveal whether email exists (returns `""` for non-existent emails).

### 5.3 Agent Settings (agentsettings.go)

**Struct:**

```go
type AgentSettings struct {
    AutonomousMode           bool   `json:"autonomousMode"`
    AutoApproveRead          bool   `json:"autoApproveRead"`
    AutoApproveWrite         bool   `json:"autoApproveWrite"`
    AutoApproveBash          bool   `json:"autoApproveBash"`
    HeartbeatIntervalMinutes int    `json:"heartbeatIntervalMinutes"`
    CommEnabled              bool   `json:"commEnabled"`
    CommPlugin               string `json:"commPlugin,omitempty"`
    DeveloperMode            bool   `json:"developerMode"`
}
```

**Singleton Pattern:**

```go
var instance     *AgentSettingsStore
var instanceOnce sync.Once

func InitSettings(database *sql.DB)
func GetAgentSettings() *AgentSettingsStore
```

- `InitSettings` called once at startup via `sync.Once`.
- Default values: `AutoApproveRead: true`, `HeartbeatIntervalMinutes: 30`, all others zero/false.
- Loads from `settings` table in SQLite.

**Store:**

```go
type AgentSettingsStore struct {
    queries   *db.Queries
    mu        sync.RWMutex
    cached    AgentSettings
    callbacks []SettingsChangeCallback
}

type SettingsChangeCallback func(AgentSettings)
```

**Methods:**

```go
func (s *AgentSettingsStore) Get() AgentSettings                    // Read from cache (RLock)
func (s *AgentSettingsStore) Update(settings AgentSettings) error   // Write to DB + cache + fire callbacks
func (s *AgentSettingsStore) OnChange(cb SettingsChangeCallback)    // Register change listener
```

`Update` fires callbacks outside the lock to prevent deadlocks.

### 5.4 Skill Settings (skillsettings.go)

File location: `<data_dir>/skill-settings.json`

**Struct:**

```go
type SkillSettings struct {
    DisabledSkills []string `json:"disabledSkills"`
}

type SkillSettingsStore struct {
    filePath string
    mu       sync.RWMutex
    settings SkillSettings
    onChange func(name string, enabled bool)
}
```

**Logic:**
- Skills are enabled by default (not in `DisabledSkills` list).
- `Toggle(name)` adds/removes from the disabled list and returns the new state.
- `SetEnabled(name, enabled)` explicitly sets state.
- Persists to `skill-settings.json` with `json.MarshalIndent`.
- `OnChange` callback receives `(name, enabled)` after save.

### 5.5 Email Service (email.go)

**Struct:**

```go
type EmailService struct {
    config config.Config
}

type SendEmailRequest struct {
    To       string
    Subject  string
    Body     string   // HTML body
    TextBody string   // Plain text fallback
}

type SendEmailResponse struct {
    Success   bool
    MessageID string
    Status    string
    Message   string
}
```

**Logic:**
- `IsConfigured()` checks `SMTPHost != ""` AND `FromAddress != ""`.
- If not configured: returns success with `Status: "skipped"`.
- Port 465: uses `tls.Dial` (implicit TLS) via `sendMailTLS`.
- Other ports: uses `smtp.SendMail` (STARTTLS via stdlib).
- Auth via `smtp.PlainAuth` if both `SMTPUser` and `SMTPPass` are set.
- Content-Type: `text/html` if `Body` is set, else `text/plain` with `TextBody`.

---

## 6. Logging

Package: `github.com/neboloop/nebo/internal/logging`

### Init and Configuration

```go
func Init(opts ...Option)
```
- Called once via `sync.Once`.
- Configures `slog.Default()`.
- Console handler: `tint.NewHandler` on stderr with `TimeFormat: "15:04:05"`.
- Optional file handler: opens file with append mode, creates parent dirs.
- If both: uses `fanoutHandler` to write to both.

**Options:**

```go
type Option func(*config)

func WithFile(path string) Option   // Enable file logging
func WithLevel(l slog.Level) Option // Set initial level (default: Info)
func WithNoColor(v bool) Option     // Disable colored console output
```

**Runtime level change:**

```go
func SetLevel(l slog.Level)
```
- Thread-safe via `slog.LevelVar`.

**Component loggers:**

```go
func L(component string) *slog.Logger
```
- Returns `slog.Default().With("component", component)`.
- Output format: `15:04:05 INF [Component] message key=val`.

### Custom Handlers

**`componentHandler`:**
- Wraps any `slog.Handler`.
- Intercepts `"component"` attribute via `WithAttrs` and stores it.
- In `Handle`: prepends `[Component]` to the message instead of showing it as `component=value`.

**`fanoutHandler`:**
- Sends each record to multiple handlers.
- `Enabled` returns true if ANY handler is enabled at that level.
- `Handle` sends to all enabled handlers; returns first error.

### Cleanup

```go
func Close()
```
- Closes file writer if open.
- Should be deferred after `Init`.

---

## 7. Crash Reporting

Package: `github.com/neboloop/nebo/internal/crashlog`

File: `crashlog.go`

**Struct:**

```go
type Logger struct {
    queries *db.Queries
    mu      sync.Mutex
}
```

**Global singleton:**

```go
var global   *Logger
var globalMu sync.Mutex

func Init(sqlDB *sql.DB)
```

**Functions:**

```go
func LogPanic(module string, r any, ctx map[string]string)
```
1. Formats panic value as string.
2. Captures stack trace: `runtime.Stack(stack, false)` with 4096-byte buffer.
3. Always logs via `slog` for immediate visibility.
4. If global logger initialized: inserts into `error_logs` table.
5. Safe to call even if `Init()` was never called.

```go
func LogError(module string, err error, ctx map[string]string)
```
- No-op if `err == nil`.
- Falls back to slog if `Init()` not called.

```go
func LogWarn(module string, msg string, ctx map[string]string)
```
- Same fallback behavior.

**Insert logic:**

```go
func (l *Logger) insert(level, module, message, stacktrace string, ctx map[string]string)
```
- Marshals context map to JSON (if non-empty) -> `sql.NullString`.
- Inserts into `error_logs` table via sqlc-generated `InsertErrorLog`.
- Levels: `"panic"`, `"error"`, `"warn"`.

---

## 8. App Lifecycle

Package: `github.com/neboloop/nebo/internal/lifecycle`

File: `lifecycle.go`

### Event System

**Event Types:**

```go
type Event string

const (
    // Server lifecycle
    EventServerStarted      Event = "server_started"
    EventShutdownStarted    Event = "shutdown_started"
    EventShutdownComplete   Event = "shutdown_complete"

    // Agent connection
    EventAgentConnected     Event = "agent_connected"
    EventAgentDisconnected  Event = "agent_disconnected"

    // Session lifecycle
    EventSessionNew         Event = "session_new"
    EventSessionReset       Event = "session_reset"
    EventSessionBootstrap   Event = "session_bootstrap"
    EventSessionUpdate      Event = "session_update"

    // Agent run
    EventAgentRunStart      Event = "agent_run_start"
    EventAgentRunComplete   Event = "agent_run_complete"
    EventAgentRunError      Event = "agent_run_error"
    EventSubagentSpawn      Event = "subagent_spawn"

    // Command
    EventCommandExecute     Event = "command_execute"
    EventCommandApprove     Event = "command_approve"
    EventCommandDeny        Event = "command_deny"
)
```

**Manager:**

```go
type Manager struct {
    mu       sync.RWMutex
    handlers map[Event][]Handler
}

type Handler func(event Event, data any)
```

**Global Instance:**

```go
var global = &Manager{handlers: make(map[Event][]Handler)}

func On(event Event, handler Handler)   // Register handler
func Emit(event Event, data any)        // Dispatch to all handlers (synchronous)
func EmitAsync(event Event, data any)   // Dispatch asynchronously (go Emit)
```

**Emit behavior:**
- Acquires read lock.
- Copies handler slice.
- Releases read lock.
- Logs event name via slog.
- Runs each handler synchronously (handlers can spawn goroutines if needed).

**Event Data Types:**

```go
type SessionEventData struct {
    SessionID  string
    SessionKey string
    UserID     string
}

type AgentRunEventData struct {
    SessionID     string
    UserID        string
    ModelOverride string
    DurationMS    int64
    Error         error
}
```

**Convenience Registration Functions:**

```go
func OnAgentConnected(handler func(agentID string))
func OnAgentDisconnected(handler func(agentID string))
func OnServerStarted(handler func())
func OnShutdown(handler func())
func OnSessionNew(handler func(data SessionEventData))
func OnAgentRunStart(handler func(data AgentRunEventData))
func OnAgentRunComplete(handler func(data AgentRunEventData))
```

Each wraps the generic `On()` with type assertion on the data parameter.

---

## 9. HTTP Utilities

Package: `github.com/neboloop/nebo/internal/httputil`

File: `httputil.go`

### Request Parsing

```go
func Parse(r *http.Request, v any) error
```

**Supported struct tags:**
- `path:"name"` -- Extracts from chi URL params (`chi.URLParam(r, name)`).
- `form:"name"` -- Extracts from query string (`r.URL.Query().Get(name)`).
- JSON body -- Decoded if `r.Body != nil && r.ContentLength > 0` and Content-Type is `application/json` or empty.

**Processing order:**
1. Path parameters (populated first).
2. Query parameters.
3. JSON body (last, so it can override query params if keys overlap).

**Type conversion (`setFieldValue`):**

| Reflect Kind | Conversion |
|---|---|
| `String` | Direct assignment |
| `Int*` | `strconv.ParseInt(value, 10, 64)` |
| `Uint*` | `strconv.ParseUint(value, 10, 64)` |
| `Bool` | `strconv.ParseBool(value)` |
| `Float*` | `strconv.ParseFloat(value, 64)` |

Silently ignores conversion errors (leaves field at zero value).

### Response Helpers

```go
func PathVar(r *http.Request, name string) string          // chi.URLParam wrapper
func QueryInt(r *http.Request, name string, defaultVal int) int
func QueryString(r *http.Request, name string, defaultVal string) string

func OkJSON(w http.ResponseWriter, v any)                  // 200 + JSON
func WriteJSON(w http.ResponseWriter, status int, v any)    // Custom status + JSON
func Error(w http.ResponseWriter, err error)                // 400 + err.Error()
func ErrorWithCode(w http.ResponseWriter, code int, message string) // Custom code + message
func Unauthorized(w http.ResponseWriter, message string)    // 401 (default: "unauthorized")
func NotFound(w http.ResponseWriter, message string)        // 404 (default: "not found")
func InternalError(w http.ResponseWriter, message string)   // 500 (default: "internal server error")
func BadRequest(w http.ResponseWriter, message string)      // 400 (default: "bad request")
```

**Error Response Format:**

```json
{
    "code": 400,
    "message": "error description"
}
```

All response functions set `Content-Type: application/json; charset=utf-8`.

---

## 10. Markdown Processing

Package: `github.com/neboloop/nebo/internal/markdown`

File: `markdown.go`

### Goldmark Configuration (init)

```go
var md goldmark.Markdown
```

Extensions:
- **GFM** (GitHub Flavored Markdown): tables, strikethrough, autolinks, task lists.
- **Syntax Highlighting**: Monokai style via `yuin/goldmark-highlighting`.

Parser options:
- `WithAutoHeadingID()` -- auto-generates heading IDs.

Renderer options:
- `WithHardWraps()` -- single newlines produce `<br>`.
- `WithUnsafe()` -- allows raw HTML in markdown source.

### Render Function

```go
func Render(content string) string
```
1. Returns `""` for empty input.
2. Converts markdown to HTML via goldmark.
3. On error: returns `""` (frontend falls back to client-side parsing).
4. Processes embeds: `processEmbeds(result)`.
5. Processes external links: `processExternalLinks(result)`.

### Embed Detection

Regex patterns match the same URLs as the frontend `markdown-embeds.ts`:

| Pattern | Captures |
|---------|----------|
| YouTube | `watch?v=`, `youtu.be/`, `/embed/`, `/shorts/` -> 11-char video ID |
| Vimeo | `vimeo.com/<digits>` -> video ID |
| Twitter/X | `twitter.com/<user>/status/<id>` or `x.com/...` -> user + tweet ID |

**Embed replacement only happens when:**
- The URL is a standalone autolinked paragraph: `<p><a href="URL">URL</a></p>`.
- The link text matches the href (i.e., it was autolinked, not a named link).
- URLs inside text paragraphs are NOT embedded.

**Embed HTML output:**
- YouTube: `<div class="embed-container embed-video"><iframe src="https://www.youtube.com/embed/{id}" ...></iframe></div>`
- Vimeo: `<div class="embed-container embed-video"><iframe src="https://player.vimeo.com/video/{id}?dnt=1" ...></iframe></div>`
- Twitter: `<div class="embed-container embed-tweet"><blockquote class="twitter-tweet" data-dnt="true" data-theme="dark">...</blockquote></div>`

### External Link Processing

```go
var linkRe = regexp.MustCompile(`<a href="(https?://[^"]*)"`)

func processExternalLinks(s string) string
```
- Adds `target="_blank" rel="noopener noreferrer"` to all links with `http://` or `https://` hrefs.
- Internal links (relative paths) are not modified.

---

## 11. Ripgrep Wrapper

Package: `github.com/neboloop/nebo/internal/ripgrep`

### Embedded Binary

```go
const Version = "14.1.1"
```

Platform-specific embed files use build tags:
- `embed_ripgrep && darwin && arm64` -> `embed_darwin_arm64.go`
- `embed_ripgrep && darwin && amd64` -> `embed_darwin_amd64.go`
- `embed_ripgrep && linux && arm64` -> `embed_linux_arm64.go`
- `embed_ripgrep && linux && amd64` -> `embed_linux_amd64.go`
- `embed_ripgrep && windows && amd64` -> `embed_windows_amd64.go`
- Fallback (no `embed_ripgrep` tag or unsupported platform) -> `embed_fallback.go`: `binary = []byte{}` (empty)

Each platform file embeds a `rg-<os>-<arch>` binary via `//go:embed`.

### Extraction

```go
func Path() string
```
- Calls `extract()` once via `sync.Once`.
- Returns cached path on subsequent calls.

```go
func extract() string
```
1. If `binary` is empty (no embedded binary): returns `""`.
2. Destination: `<data_dir>/bin/rg` (or `rg.exe` on Windows).
3. Version check: reads `<data_dir>/bin/.rg-version`.
   - If matches `Version` and binary exists: returns existing path (no extraction needed).
4. Creates `<data_dir>/bin/` directory.
5. Writes binary to `rg.tmp` first, then renames atomically.
6. Writes version file atomically (also via tmp + rename).
7. Binary permissions: `0755`.

---

## 12. Self-Updater

Package: `github.com/neboloop/nebo/internal/updater`

### Version Check

```go
const releaseURL = "https://cdn.neboloop.com/releases/version.json"
const timeout = 5 * time.Second

type versionManifest struct {
    Version     string `json:"version"`
    ReleaseURL  string `json:"release_url"`
    PublishedAt string `json:"published_at"`
}

type Result struct {
    Available      bool   `json:"available"`
    CurrentVersion string `json:"currentVersion"`
    LatestVersion  string `json:"latestVersion"`
    ReleaseURL     string `json:"releaseUrl,omitempty"`
    PublishedAt    string `json:"publishedAt,omitempty"`
}
```

```go
func Check(ctx context.Context, currentVersion string) (*Result, error)
```
1. Creates HTTP GET to `releaseURL` with 5-second timeout.
2. Sets `User-Agent: nebo/<currentVersion>`.
3. Decodes JSON manifest.
4. Normalizes versions (strips `v` prefix).
5. `Available = latest != current && current != "dev" && isNewer(latest, current)`.

**Version comparison:**

```go
func isNewer(latest, current string) bool
```
- Splits on `.` into `[major, minor, patch]` via `Sscanf`.
- Compares left to right; first difference wins.
- Returns true if latest > current.

### Background Checker

```go
type BackgroundChecker struct {
    version        string
    interval       time.Duration
    notify         NotifyFunc
    lastNotified   string
    mu             sync.Mutex
}

type NotifyFunc func(result *Result)

func NewBackgroundChecker(currentVersion string, interval time.Duration, notify NotifyFunc) *BackgroundChecker
```

```go
func (b *BackgroundChecker) Run(ctx context.Context)
```
1. Waits 30 seconds (let app finish booting).
2. Performs initial check.
3. Rechecks every `interval` (typically 1 hour).
4. Blocks until context is cancelled.

**Deduplication:**
- Tracks `lastNotified` version string.
- Only calls `notify` once per new version.

### Install Method Detection

```go
func DetectInstallMethod() string
```
Returns:
- `"homebrew"` -- if binary path contains `/opt/homebrew/` or `/usr/local/Cellar/`.
- `"package_manager"` -- if `dpkg -S <binary>` succeeds (Linux only).
- `"direct"` -- otherwise.

### Download

```go
const releaseDownloadURL = "https://cdn.neboloop.com/releases"

type ProgressFunc func(downloaded, total int64)

func Download(ctx context.Context, tagName string, progress ProgressFunc) (string, error)
```

**Asset naming:**

```go
func AssetName() string
```
- Pattern: `nebo-<os>-<arch>` (or `.exe` suffix on Windows).
- Examples: `nebo-darwin-arm64`, `nebo-linux-amd64`, `nebo-windows-amd64.exe`.

**Download flow:**
1. URL: `https://cdn.neboloop.com/releases/<tagName>/<assetName>`.
2. 10-minute timeout.
3. Streams to temp file with 32KB buffer.
4. Reports progress via callback.
5. Sets `chmod 0755` on non-Windows.
6. Returns temp file path.

### Checksum Verification

```go
func VerifyChecksum(ctx context.Context, binaryPath, tagName string) error
```
1. Downloads `checksums.txt` from `https://cdn.neboloop.com/releases/<tagName>/checksums.txt`.
2. If 404: skips verification (older releases without checksums).
3. Parses lines: `{sha256}  {filename}` or `{sha256} {filename}`.
4. Finds matching asset.
5. Computes SHA-256 of downloaded binary.
6. Case-insensitive comparison.

### Binary Replacement

**Pre-Apply Hook:**

```go
var preApplyFunc func()

func SetPreApplyHook(fn func())
```
- Called before process restart to release resources (lock files, connections).

**Health Check:**

```go
func healthCheck(binaryPath string) error
```
- Runs `<binary> --version` with 5-second timeout.
- Kills process if timeout.

#### Unix (apply_unix.go)

```go
func Apply(newBinaryPath string) error
```
1. Resolves current executable path (follows symlinks).
2. Health check on new binary.
3. Backup current binary to `<path>.old`.
4. Copy new binary over current path.
5. On copy failure: rollback from backup.
6. Remove temp file.
7. Run pre-apply hook.
8. `syscall.Exec(realPath, os.Args, os.Environ())` -- replaces process in-place.

#### Windows (apply_windows.go)

```go
func Apply(newBinaryPath string) error
```
1. Resolve current executable path.
2. Health check on new binary.
3. Rename current exe to `.old` (Windows allows renaming a running exe but not overwriting).
4. Copy new binary to original path.
5. On copy failure: rename backup back.
6. Remove temp file.
7. Run pre-apply hook.
8. Spawn new process with `exec.Command(currentExe, os.Args[1:]...)`.
9. `os.Exit(0)`.

**Shared utility:**

```go
func copyFile(src, dst string) error
```
- Copies file preserving permissions from source.

---

## 13. Services

Package: `github.com/neboloop/nebo/internal/services/email`

File: `sender.go`

**Struct:**

```go
type Service struct {
    smtpHost, smtpUser, smtpPass, fromAddress, fromName, replyTo string
    smtpPort int
}

type Config struct {
    SMTPHost, SMTPUser, SMTPPass, FromAddress, FromName, ReplyTo string
    SMTPPort int
}
```

**Methods:**

```go
func (s *Service) IsConfigured() bool  // smtpHost && smtpUser && smtpPass all non-empty
func (s *Service) SendEmail(ctx context.Context, to, subject, htmlBody string) error
func (s *Service) SendEmailFrom(ctx context.Context, fromEmail, fromName, to, subject, htmlBody string) error
func (s *Service) SendPasswordResetEmail(ctx context.Context, to, resetURL string) error
func (s *Service) SendWelcomeEmail(ctx context.Context, to, name, appURL string) error
```

**Note:** This is a separate email service from `local.EmailService`. This one:
- Requires all three SMTP credentials (host + user + pass).
- Uses `smtp.PlainAuth` always.
- Uses `smtp.SendMail` (no implicit TLS support unlike the local version).
- Has pre-built HTML templates for password reset and welcome emails.

---

## 14. Service Context

Package: `github.com/neboloop/nebo/internal/svc`

File: `servicecontext.go`

### ServiceContext Struct

```go
type ServiceContext struct {
    Config             config.Config
    SecurityMiddleware *middleware.SecurityMiddleware
    NeboDir            string  // Root Nebo data directory
    Version            string  // Build version

    DB             *db.Store
    Auth           *local.AuthService
    Email          *local.EmailService
    SkillSettings  *local.SkillSettingsStore
    PluginStore    *settings.Store

    AgentHub    *agenthub.Hub
    MCPClient   *mcpclient.Client
    OAuthBroker *broker.Broker

    // Lazy-initialized via setters (avoid import cycles with `any`)
    appUI        AppUIProvider    // apps.AppRegistry
    appRegistry  any              // apps.AppRegistry
    toolRegistry any              // tools.Registry
    scheduler    any              // tools.Scheduler

    JanusUsage   atomic.Pointer[ai.RateLimitInfo]  // In-memory + persisted to janus_usage.json

    // Desktop-only callbacks
    browseDir     func() (string, error)
    browseFiles   func() ([]string, error)
    openDevWindow func()
    openPopup     func(url, title string, width, height int)

    updateMgr     *UpdateMgr
    clientHub     any  // realtime.Hub
    neboLoopClient func(ctx context.Context) (any, error)
}
```

All `any`-typed fields use `sync.RWMutex` for thread-safe access.

### Interfaces

```go
type AppUIProvider interface {
    HandleRequest(ctx context.Context, appID string, req *AppHTTPRequest) (*AppHTTPResponse, error)
    ListUIApps() []AppUIInfo
    AppsDir() string
}

type AppHTTPRequest struct {
    Method, Path, Query string
    Headers             map[string]string
    Body                []byte
}

type AppHTTPResponse struct {
    StatusCode int
    Headers    map[string]string
    Body       []byte
}

type AppUIInfo struct {
    ID, Name, Version string
}
```

### UpdateMgr

```go
type UpdateMgr struct {
    mu          sync.Mutex
    pendingPath string
    version     string
}

func (u *UpdateMgr) SetPending(path, version string)
func (u *UpdateMgr) PendingPath() string
func (u *UpdateMgr) PendingVersion() string
func (u *UpdateMgr) Clear()
```

Tracks a verified binary ready for installation (in-memory only).

### Initialization

```go
func NewServiceContext(c config.Config, database ...*db.Store) *ServiceContext
```

**Initialization order:**

1. **Security middleware**: `middleware.NewSecurityMiddleware(c)`.
2. **Data directory**: derives from `c.Database.SQLitePath`; ensures default files via `defaults.EnsureDataDir()`.
3. **Models store**: `provider.InitModelsStore(neboDir)` (loads `models.yaml`).
4. **Agent hub**: `agenthub.NewHub()`.
5. **Skill settings**: `local.NewSkillSettingsStore(dataDir)`.
6. **Email service**: `local.NewEmailService(c)` (only stored if configured).
7. **Database**: uses provided `*db.Store` or creates new via `db.NewSQLite(path)`.
8. If DB available:
   a. **Agent settings**: `local.InitSettings(db.GetDB())` (singleton).
   b. **Auth service**: `local.NewAuthService(db, c)`.
   c. **Plugin store**: `settings.NewStore(db.GetDB())` -- with `OnChange` callback that broadcasts `plugin_settings_updated` event via agent hub.
   d. **MCP encryption key**: `mcpclient.GetEncryptionKey(neboDir)` + `credential.Init(encKey)` + `credential.Migrate(...)`.
   e. **MCP client**: `mcpclient.NewClient(db, encKey, baseURL)`.
   f. **App OAuth Broker**: `broker.New(...)` -- merges built-in providers with config overrides from `c.AppOAuth`.
9. **Janus usage**: loads from `janus_usage.json` if it exists.

### Setter/Getter Pattern

All lazy-initialized fields follow the same pattern:

```go
func (svc *ServiceContext) SetXxx(v Type)  // Lock, set, unlock
func (svc *ServiceContext) Xxx() Type      // RLock, get, RUnlock
```

Fields with setters/getters:
- `AppUIProvider` / `AppUI()`
- `AppRegistry` / `AppRegistry()`
- `ToolRegistry` / `ToolRegistry()`
- `Scheduler` / `Scheduler()`
- `BrowseDirectory` / `BrowseDirectory()`
- `BrowseFiles` / `BrowseFiles()`
- `OpenDevWindow` / `OpenDevWindow()`
- `OpenPopup` / `OpenPopup()`
- `UpdateManager` / `UpdateManager()`
- `ClientHub` / `ClientHub()`
- `NeboLoopClient` / `NeboLoopClient()`

### Janus Usage Persistence

```go
func (svc *ServiceContext) SaveJanusUsage()  // Marshal to JSON, write to <neboDir>/janus_usage.json (0600)
func (svc *ServiceContext) LoadJanusUsage()  // Read from disk, unmarshal, store in atomic pointer
```

### Cleanup

```go
func (svc *ServiceContext) Close()
```
- Closes the SQLite database connection.

```go
func (svc *ServiceContext) UseLocal() bool
```
- Returns `true` if `DB != nil` (local SQLite mode).
