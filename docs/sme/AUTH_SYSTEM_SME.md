# AUTH_SYSTEM_SME.md

Subject Matter Expert document for the Nebo Authentication and Credential System.

Last updated: 2026-05-15

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Crate Structure and Dependencies](#2-crate-structure-and-dependencies)
3. [JWT Token System](#3-jwt-token-system)
4. [AuthService — User Account Management](#4-authservice--user-account-management)
5. [AES-256-GCM Encryption System](#5-aes-256-gcm-encryption-system)
6. [Keyring Integration](#6-keyring-integration)
7. [Credential Module — Encrypt-at-Rest for Secrets](#7-credential-module--encrypt-at-rest-for-secrets)
8. [Master Key Resolution and Lifecycle](#8-master-key-resolution-and-lifecycle)
9. [Auth Middleware and HTTP Security](#9-auth-middleware-and-http-security)
10. [Rate Limiting](#10-rate-limiting)
11. [Security Headers](#11-security-headers)
12. [Auth API Routes](#12-auth-api-routes)
13. [Protected vs Public Routes](#13-protected-vs-public-routes)
14. [Agent WebSocket Authentication](#14-agent-websocket-authentication)
15. [MCP Endpoint Authentication](#15-mcp-endpoint-authentication)
16. [OAuth Configuration](#16-oauth-configuration)
17. [Auth Profiles — AI Provider Credentials](#17-auth-profiles--ai-provider-credentials)
18. [Database Schema](#18-database-schema)
19. [Configuration Reference](#19-configuration-reference)
20. [Cross-System Interactions](#20-cross-system-interactions)
21. [Error Handling](#21-error-handling)
22. [Threat Model and Security Considerations](#22-threat-model-and-security-considerations)
23. [Flow Diagrams](#23-flow-diagrams)
24. [Testing](#24-testing)
25. [Key Implementation Files](#25-key-implementation-files)

---

## 1. Architecture Overview

The Nebo auth system is a multi-layered security architecture designed for a
locally-running desktop AI companion. It handles user authentication, credential
encryption, API key management, and HTTP request authorization.

```
+-----------------------------------------------------------------------+
|                        NEBO AUTH ARCHITECTURE                          |
+-----------------------------------------------------------------------+
|                                                                       |
|  +------------------+    +------------------+    +-----------------+  |
|  |   Frontend        |    |   HTTP/WS Layer  |    |  Auth Crate     |  |
|  |   (SvelteKit)     |--->|   (Axum Server)  |--->|  nebo-auth      |  |
|  |                   |    |                  |    |                 |  |
|  | - Login form      |    | - JWT middleware |    | - jwt.rs        |  |
|  | - Token storage   |    | - Rate limiter   |    | - service.rs    |  |
|  | - Bearer header   |    | - Security hdrs  |    | - credential.rs |  |
|  +------------------+    | - CORS layer     |    | - keyring.rs    |  |
|                          +------------------+    +-----------------+  |
|                                  |                       |            |
|                                  v                       v            |
|  +------------------+    +------------------+    +-----------------+  |
|  |   Config Crate    |    |   DB Crate       |    |  MCP Crate      |  |
|  |                   |    |   (SQLite)       |    |  (crypto.rs)    |  |
|  | - AuthConfig      |    |                  |    |                 |  |
|  | - SecurityConfig  |    | - users          |    | - Encryptor     |  |
|  | - OAuthConfig     |    | - refresh_tokens |    | - AES-256-GCM   |  |
|  | - nebo.yaml       |    | - auth_profiles  |    | - Key resolve   |  |
|  +------------------+    | - plugin_settings|    +-----------------+  |
|                          +------------------+            |            |
|                                                          v            |
|                                                  +-----------------+  |
|                                                  |   OS Keyring    |  |
|                                                  |   (macOS/Win/   |  |
|                                                  |    Linux)       |  |
|                                                  +-----------------+  |
+-----------------------------------------------------------------------+
```

### Design Principles

1. **Local-first security**: Nebo runs on `127.0.0.1` by default. Auth exists
   for multi-user scenarios and to protect the local API from unauthorized
   access by other programs on the machine.

2. **Encryption at rest**: All sensitive credentials (API keys, OAuth tokens,
   skill secrets) are encrypted with AES-256-GCM before storage in SQLite.
   The master encryption key is stored in the OS keyring when available.

3. **Dual-purpose JWT**: JWTs authenticate both user sessions (access tokens)
   and agent WebSocket connections (short-lived `agent_ws` tokens).

4. **Graceful degradation**: If the OS keyring is unavailable (headless server,
   CI), the system falls back to file-based key storage or environment variables.

---

## 2. Crate Structure and Dependencies

The `nebo-auth` crate (`crates/auth/`) contains four modules:

```
crates/auth/
  Cargo.toml
  src/
    lib.rs           # Public re-exports
    jwt.rs           # JWT generation, validation, claims
    service.rs       # AuthService (register, login, refresh, password mgmt)
    credential.rs    # Encrypt/decrypt credentials via global Encryptor
    keyring.rs       # OS keyring integration (master key storage)
```

### Cargo.toml Dependencies

```toml
[dependencies]
types      = { workspace = true }    # NeboError, constants
config     = { workspace = true }    # Config struct (AuthConfig)
db         = { workspace = true }    # Store (SQLite queries)
mcp        = { workspace = true }    # mcp::crypto::Encryptor (AES-256-GCM)
jsonwebtoken = { workspace = true }  # JWT encode/decode (HS256)
bcrypt     = { workspace = true }    # Password hashing (bcrypt)
serde      = { workspace = true }    # Serialization
serde_json = { workspace = true }    # JSON claims
chrono     = { workspace = true }    # Timestamps
rand       = { workspace = true }    # Token/ID generation (CSPRNG)
hex        = { workspace = true }    # Hex encoding for tokens/IDs
sha2       = { workspace = true }    # SHA-256 for refresh token hashing
thiserror  = { workspace = true }    # Error derivation
tracing    = { workspace = true }    # Structured logging
keyring    = "3"                     # OS keychain access
```

### Public API (lib.rs)

```rust
pub mod credential;
mod jwt;
pub mod keyring;
mod service;

pub use jwt::{
    Claims, JWTClaims, generate_agent_ws_token, validate_agent_ws_token,
    validate_jwt, validate_jwt_claims,
};
pub use service::AuthService;
```

Note: `jwt` and `service` are private modules with selected re-exports.
`credential` and `keyring` are fully public modules, accessed directly by
the server and tools crates as `auth::credential::encrypt()` and
`auth::keyring::get()`.

---

## 3. JWT Token System

### Overview

Nebo uses HS256 (HMAC-SHA256) JWTs for all token-based authentication. There
are two distinct JWT types:

1. **User access tokens** — authenticate HTTP API requests
2. **Agent WebSocket tokens** — short-lived tokens for agent-to-server WS

### Claims Structures

```rust
/// Raw JWT claims as a HashMap (matches Go's jwt.MapClaims).
pub type Claims = std::collections::HashMap<String, serde_json::Value>;

/// Structured JWT claims for user access tokens.
pub struct JWTClaims {
    pub sub: String,     // Subject (user ID) — checks both "sub" and "userId"
    pub email: String,   // User email
    pub name: String,    // User display name
    pub iss: String,     // Issuer
    pub exp: i64,        // Expiration (Unix timestamp)
    pub iat: i64,        // Issued-at (Unix timestamp)
}

/// Agent WebSocket token claims (private struct).
struct AgentWSClaims {
    token_type: String,  // Always "agent_ws"
    iat: i64,            // Issued-at
    exp: i64,            // Expiration
}
```

### User Access Token Payload

When `AuthService::generate_tokens()` creates an access token, the JWT payload
contains:

```json
{
  "userId": "<32-char-hex-id>",
  "email": "user@example.com",
  "iat": 1715000000,
  "exp": 1746536000
}
```

Note the `userId` key (not `sub`). The validator checks both `sub` and `userId`
for backward compatibility with the original Go implementation.

### Validation Functions

```rust
/// Validate a JWT and return raw claims (HashMap).
/// - Algorithm: HS256
/// - Validates expiration (exp)
/// - No required spec claims (extracted manually)
pub fn validate_jwt(token: &str, secret: &str) -> Result<Claims, NeboError>;

/// Validate and extract structured claims.
/// - Calls validate_jwt() first
/// - Extracts "sub" or "userId" as subject
/// - Returns NeboError::InvalidToken if no subject found
pub fn validate_jwt_claims(token: &str, secret: &str) -> Result<JWTClaims, NeboError>;

/// Mint a short-lived agent WS token.
/// - type: "agent_ws"
/// - TTL: caller-specified (typically 60-300 seconds)
pub fn generate_agent_ws_token(secret: &str, ttl_seconds: i64) -> Result<String, NeboError>;

/// Validate an agent WS token.
/// - Verifies signature + expiration
/// - Checks type == "agent_ws"
pub fn validate_agent_ws_token(token: &str, secret: &str) -> Result<(), NeboError>;
```

### Token Lifetimes

| Token Type      | Default TTL          | Configurable?           |
|-----------------|----------------------|-------------------------|
| Access token    | 31,536,000s (1 year) | Yes (`Auth.AccessExpire`) |
| Refresh token   | 31,536,000s (1 year) | Yes (`Auth.RefreshTokenExpire`) |
| Agent WS token  | Caller-specified     | Via `ttl_seconds` param |
| Password reset  | 3,600s (1 hour)      | Hardcoded in service.rs |

The 1-year default for access/refresh tokens reflects Nebo's desktop-app
nature: local users should not need to re-authenticate frequently.

---

## 4. AuthService -- User Account Management

`AuthService` is the central auth orchestrator. It is constructed once at server
startup and stored in `AppState.auth` as `Arc<AuthService>`.

### Construction

```rust
pub struct AuthService {
    store: Arc<Store>,   // SQLite DB handle
    config: Config,      // Full Nebo config (contains auth.access_secret, etc.)
}

impl AuthService {
    pub fn new(store: Arc<Store>, config: Config) -> Self;
}
```

### Public Methods

#### Registration

```rust
pub fn register(&self, email: &str, password: &str, name: &str)
    -> Result<AuthResponse, NeboError>;
```

Flow:
1. Check if email already exists (`store.check_email_exists` -- case-insensitive)
2. Hash password with bcrypt (DEFAULT_COST = 12)
3. Generate 16-byte random hex ID
4. Insert user row in `users` table
5. Create default preferences row in `user_preferences`
6. Generate and return access + refresh tokens

#### Login

```rust
pub fn login(&self, email: &str, password: &str) -> Result<AuthResponse, NeboError>;
```

Flow:
1. Look up user by email (case-insensitive via `LOWER()`)
2. Verify password against stored bcrypt hash
3. Generate and return tokens

#### Token Refresh

```rust
pub fn refresh_token(&self, refresh_token: &str) -> Result<AuthResponse, NeboError>;
```

Flow:
1. SHA-256 hash the incoming refresh token
2. Look up hash in `refresh_tokens` table (must not be expired)
3. Look up associated user
4. Delete the used refresh token (single-use rotation)
5. Generate and return new token pair

This implements **refresh token rotation**: each refresh token is single-use.
After exchange, the old token is deleted and a new one is issued.

#### Password Management

```rust
pub fn change_password(&self, user_id: &str, current: &str, new: &str) -> Result<(), NeboError>;
pub fn create_password_reset_token(&self, email: &str) -> Result<Option<String>, NeboError>;
pub fn reset_password(&self, token: &str, new_password: &str) -> Result<(), NeboError>;
```

- `change_password`: Requires current password verification before accepting new.
- `create_password_reset_token`: Returns `None` (not error) if email doesn't
  exist -- prevents user enumeration.
- `reset_password`: Validates token + expiry, then updates hash and clears token.

#### User Lookup

```rust
pub fn get_user_by_id(&self, user_id: &str) -> Result<Option<User>, NeboError>;
pub fn get_user_by_email(&self, email: &str) -> Result<Option<User>, NeboError>;
pub fn delete_user(&self, user_id: &str) -> Result<(), NeboError>;
```

#### Admin Token Generation

```rust
pub fn generate_tokens_for_user(&self, user_id: &str, email: &str)
    -> Result<AuthResponse, NeboError>;
```

Used by the setup handler to generate tokens for the initial admin user without
requiring a login flow.

### AuthResponse

```rust
pub struct AuthResponse {
    pub token: String,          // JWT access token
    pub refresh_token: String,  // Opaque 64-char hex token
    pub expires_at: i64,        // Unix timestamp of access token expiry
}
```

### Internal Helpers

```rust
/// 16-byte random hex ID (32 characters).
fn generate_id() -> String;

/// 32-byte random hex token (64 characters).
fn generate_token() -> String;

/// SHA-256 hash of a token string for storage.
/// Refresh tokens are NEVER stored in plaintext.
fn hash_token(token: &str) -> String;
```

---

## 5. AES-256-GCM Encryption System

The encryption primitives live in `crates/mcp/src/crypto.rs` and are used
throughout the system for encrypting sensitive data at rest.

### Encryptor Struct

```rust
pub struct Encryptor {
    key: [u8; 32],  // 256-bit AES key
}
```

### Construction Methods

```rust
/// From a raw 32-byte key (used when loading from keyring/file).
pub fn new(key: [u8; 32]) -> Self;

/// Derive key from passphrase via SHA-256.
/// Used for env vars MCP_ENCRYPTION_KEY or JWT_SECRET.
pub fn from_passphrase(passphrase: &str) -> Self;

/// Generate a cryptographically random key using OsRng.
pub fn generate() -> Self;
```

### Encryption/Decryption

```
+-------------------------------------------+
|         ENCRYPTED OUTPUT FORMAT            |
+-------------------------------------------+
| Nonce (12 bytes) | Ciphertext + Auth Tag  |
+-------------------------------------------+
```

```rust
/// Encrypt: generates random 12-byte nonce, returns nonce || ciphertext.
pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, McpError>;

/// Decrypt: splits nonce from ciphertext, verifies auth tag.
pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>, McpError>;

/// Encrypt to base64 string (for storage in text columns).
pub fn encrypt_b64(&self, plaintext: &[u8]) -> Result<String, McpError>;

/// Decrypt from base64 string.
pub fn decrypt_b64(&self, b64: &str) -> Result<Vec<u8>, McpError>;
```

### Nonce Management

- **Size**: 12 bytes (96 bits), the standard for AES-256-GCM
- **Generation**: Random via `OsRng.fill_bytes()` per encryption operation
- **Storage**: Prepended to ciphertext (first 12 bytes of the encrypted blob)
- **Uniqueness**: With a random 96-bit nonce and a 256-bit key, collision
  probability is negligible for practical volumes of encrypted values

### Key Resolution

The `resolve_encryption_key()` function determines which encryption key to use:

```rust
pub fn resolve_encryption_key(data_dir: &Path) -> Encryptor;
```

Priority order:
1. `MCP_ENCRYPTION_KEY` environment variable (passphrase-derived)
2. `JWT_SECRET` environment variable (passphrase-derived)
3. Persistent key file at `<data_dir>/.mcp-key` (raw 32 bytes)
4. Generate new random key and persist to `.mcp-key` file

This function is the **fallback** path. The primary path in the server
startup tries the OS keyring first (see Section 8).

---

## 6. Keyring Integration

The `keyring` module (`crates/auth/src/keyring.rs`) stores and retrieves the
master encryption key from the operating system's credential store.

### Platform Backends

| Platform | Backend                    |
|----------|----------------------------|
| macOS    | Keychain (Security.framework) |
| Windows  | Windows Credential Manager |
| Linux    | Secret Service (libsecret/GNOME Keyring) |

### Keyring Entry

```
Service Name: "nebo"
Account Name: "master-encryption-key"
Value:        Hex-encoded 32-byte key (64 hex characters)
```

### Functions

```rust
/// Check if the system keyring is accessible.
/// Returns true even if no entry exists (NoEntry is OK).
/// Returns false on PlatformFailure or NoStorageAccess.
pub fn available() -> bool;

/// Retrieve the master key. Returns None if no entry or access error.
pub fn get() -> Option<String>;

/// Store the master key. Returns descriptive error string on failure.
pub fn set(key: &str) -> Result<(), String>;

/// Delete the master key. Silently succeeds if already absent.
pub fn delete() -> Result<(), String>;
```

### Availability Detection

```rust
match entry.get_password() {
    Ok(_) => true,                          // Key exists, keyring works
    Err(keyring::Error::NoEntry) => true,   // No key yet, but keyring works
    Err(keyring::Error::PlatformFailure(_)) => false,  // Keyring broken
    Err(keyring::Error::NoStorageAccess(_)) => false,  // No access (headless)
    Err(_) => false,                        // Other errors
}
```

This graceful detection allows Nebo to run headless (CLI server, Docker, CI)
without requiring an interactive keyring unlock prompt.

---

## 7. Credential Module -- Encrypt-at-Rest for Secrets

The `credential` module (`crates/auth/src/credential.rs`) provides a global
encrypt/decrypt API used by tools, skills, and server handlers to protect
sensitive values before SQLite storage.

### Global State

```rust
/// Write-once encryptor set at startup.
/// OnceLock (not mutable state) because tools and agent code need
/// encryption without access to AppState.
static ENCRYPTOR: OnceLock<mcp::crypto::Encryptor> = OnceLock::new();

/// Prefix for encrypted values.
const ENCRYPTED_PREFIX: &str = "enc:";
```

### Functions

```rust
/// Initialize with a resolved Encryptor. Called once at server startup.
pub fn init(encryptor: mcp::crypto::Encryptor);

/// Check if the credential system is ready.
pub fn is_initialized() -> bool;

/// Encrypt a plaintext string. Returns "enc:<base64>" on success.
/// Returns error if encryptor not initialized.
pub fn encrypt(plaintext: &str) -> Result<String, String>;

/// Decrypt a value. If it lacks the "enc:" prefix, returns as-is.
/// This provides transparent migration: old plaintext values pass through.
pub fn decrypt(value: &str) -> Result<String, String>;

/// Check if a value has the "enc:" prefix.
pub fn is_encrypted(value: &str) -> bool;
```

### Storage Format

Encrypted values in the database look like:

```
enc:Base64EncodedString==
```

Where the base64 payload contains: `nonce (12 bytes) || ciphertext || auth_tag (16 bytes)`

### Usage Sites

The credential module is used across the codebase for:

| Usage | Location | Direction |
|-------|----------|-----------|
| Skill secrets | `crates/tools/src/skill_tool.rs` | Encrypt on store |
| Skill secrets | `crates/tools/src/skills/expand.rs` | Decrypt on use |
| Skill secrets | `crates/tools/src/execute_tool.rs` | Decrypt for env vars |
| Skill secrets | `crates/server/src/handlers/skills.rs` | Encrypt via API |
| License keys | `crates/server/src/codes.rs` | Encrypt on store |
| License keys | `crates/server/src/lib.rs` (startup) | Decrypt cached keys |
| MCP OAuth tokens | `crates/server/src/handlers/integrations.rs` | Encrypt/decrypt |

### Plaintext Passthrough

The `decrypt()` function transparently handles unencrypted values:

```rust
pub fn decrypt(value: &str) -> Result<String, String> {
    if !value.starts_with(ENCRYPTED_PREFIX) {
        return Ok(value.to_string());  // Pass through plaintext
    }
    // ... decrypt base64 payload
}
```

This is critical for backward compatibility during migration from the Go
codebase, where some values were stored in plaintext.

---

## 8. Master Key Resolution and Lifecycle

At server startup, the master encryption key is resolved through a cascading
priority system. This happens in `crates/server/src/lib.rs` in the `run()`
function.

### Resolution Flow

```
                    +-------------------+
                    |   Server Startup  |
                    +-------------------+
                            |
                            v
                    +-------------------+
                    | keyring::get()    |
                    | Try OS keyring    |
                    +-------------------+
                      |             |
                  Has key       No key
                      |             |
                      v             v
              +-------------+  +------------------------+
              | Parse hex   |  | resolve_encryption_key |
              | or use as   |  | (env/file/generate)    |
              | passphrase  |  +------------------------+
              +-------------+           |
                      |                 v
                      |         +-------------------+
                      |         | keyring::set()    |
                      |         | Store in keyring  |
                      |         | for next time     |
                      |         +-------------------+
                      |                 |
                      v                 v
              +-------------------------------+
              | credential::init(encryptor)   |
              | Set global OnceLock           |
              +-------------------------------+
```

### Detailed Steps

```rust
// Step 1: Try OS keyring
let encryptor = if let Some(key_hex) = auth::keyring::get() {
    // Keyring has the master key
    if key_hex.len() == 64 {
        // Hex-encoded 32-byte key — decode to raw bytes
        let mut key = [0u8; 32];
        if hex::decode_to_slice(&key_hex, &mut key).is_ok() {
            mcp::crypto::Encryptor::new(key)
        } else {
            mcp::crypto::Encryptor::from_passphrase(&key_hex)
        }
    } else {
        // Not hex — derive via SHA-256
        mcp::crypto::Encryptor::from_passphrase(&key_hex)
    }
} else {
    // Step 2: Resolve from env/file or generate new
    // Priority: MCP_ENCRYPTION_KEY env -> JWT_SECRET env -> .mcp-key file -> generate
    let enc = mcp::crypto::resolve_encryption_key(&data_dir);

    // Step 3: Store in keyring for next startup
    if auth::keyring::available() {
        let key_hex = hex::encode(enc.key_bytes());
        if let Err(e) = auth::keyring::set(&key_hex) {
            warn!("failed to store master key in keyring: {}", e);
        }
    }
    enc
};

// Step 4: Initialize the global credential system
auth::credential::init(mcp::crypto::Encryptor::new(*encryptor.key_bytes()));
```

### Key Persistence Hierarchy

```
+-----+  Priority 1       +-----+  Priority 2       +-----+  Priority 3       +-----+
| OS  |  =============>   | ENV |  =============>   |FILE |  =============>   | GEN |
| KEY |  (Keyring)        | VAR |  (MCP_ENCRYPTION  |.mcp |  (~/.nebo/data/   | NEW |
| RING|                   |     |   _KEY or          |-key |   .mcp-key)       |     |
+-----+                   +-----+   JWT_SECRET)     +-----+                   +-----+
   ^                                                                              |
   |                                                                              |
   +--- New key stored back to keyring on first startup -------------------------+
```

---

## 9. Auth Middleware and HTTP Security

### JWT Auth Middleware

The `jwt_auth` middleware (`crates/server/src/middleware.rs`) runs on protected
routes and validates the `Authorization: Bearer <token>` header.

```rust
pub struct AuthClaims {
    pub user_id: String,
    pub email: String,
}

pub async fn jwt_auth(mut request: Request, next: Next) -> Response {
    // 1. Extract JWT secret from request extensions (set via layer)
    let secret = request.extensions().get::<JwtSecret>().map(|s| s.0.clone());

    // 2. Parse Authorization: Bearer <token>
    let token = extract_bearer_token(request.headers());

    // 3. Validate JWT and extract claims
    match auth::validate_jwt_claims(token, &secret) {
        Ok(claims) => {
            // 4. Insert AuthClaims into request extensions
            request.extensions_mut().insert(AuthClaims {
                user_id: claims.sub,
                email: claims.email,
            });
            next.run(request).await
        }
        Err(_) => auth_error("invalid token")  // 401 Unauthorized
    }
}
```

### JwtSecret Extension

The JWT secret is injected into request extensions via an Axum layer:

```rust
// In routes/mod.rs
let protected = user::protected_routes()
    .layer(axum::Extension(jwt_secret))       // Injects JwtSecret
    .layer(axum::middleware::from_fn(middleware::jwt_auth));  // Validates
```

The secret comes from `config.auth.access_secret` which is set in
`etc/nebo.yaml` as `"placeholder-replaced-at-runtime"` and can be overridden.

### Extracting Claims in Handlers

Protected route handlers extract `AuthClaims` from the request extensions:

```rust
pub async fn get_current_user(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<AuthClaims>,
) -> HandlerResult<serde_json::Value> {
    let user = state.auth.get_user_by_id(&claims.user_id)?;
    // ...
}
```

---

## 10. Rate Limiting

Auth routes have a dedicated rate limiter to prevent brute-force attacks.

### Implementation

```rust
pub struct RateLimiter {
    buckets: Arc<Mutex<HashMap<IpAddr, (u32, Instant)>>>,
    max_requests: u32,
    window: Duration,
}
```

### Configuration

| Parameter | Auth Routes | Default |
|-----------|-------------|---------|
| Requests per window | 10 | `DEFAULT_AUTH_RATE_LIMIT_REQUESTS = 5` |
| Window duration | 60 seconds | `DEFAULT_AUTH_RATE_LIMIT_INTERVAL = 60` |

The auth routes are configured with 10 req/min in `routes/mod.rs`:

```rust
let auth_limiter = middleware::RateLimiter::new(10, Duration::from_secs(60));
let auth_routes = auth::auth_routes()
    .layer(axum::Extension(auth_limiter))
    .layer(axum::middleware::from_fn(middleware::rate_limit));
```

### IP Extraction

```
IMPORTANT: Uses ConnectInfo (peer address) ONLY.
X-Forwarded-For is intentionally IGNORED — it is trivially spoofable.
```

This is a deliberate security decision documented in the code. For deployments
behind a trusted reverse proxy, a `TrustedProxy` variant would need to be
added.

### Rate Limit Response

When exceeded, returns HTTP 429 with:
```json
{ "error": "rate limit exceeded, try again later" }
```

---

## 11. Security Headers

Two layers of security headers are applied:

### Global Headers (`security_headers` middleware)

Applied to all routes. Sets:

| Header | Value |
|--------|-------|
| `Strict-Transport-Security` | `max-age=31536000; includeSubDomains; preload` |
| `Permissions-Policy` | Restricts camera, microphone to self; blocks accelerometer, etc. |
| `X-Content-Type-Options` | `nosniff` |
| `X-Frame-Options` | `DENY` (except `/chat-embed/` routes which need framing) |
| `X-XSS-Protection` | `1; mode=block` |
| `Referrer-Policy` | `strict-origin-when-cross-origin` |

### API-Only Headers (`api_security_headers` middleware)

Applied to all `/api/v1/` routes:

| Header | Value |
|--------|-------|
| `Content-Security-Policy` | `default-src 'none'; frame-ancestors 'none'` |
| `Cache-Control` | `no-store, no-cache, must-revalidate, private` |
| `Pragma` | `no-cache` |

### CORS Layer

The server configures CORS to allow:

- `http://localhost:27895` / `http://127.0.0.1:27895` (production)
- `http://localhost:5173` / `http://127.0.0.1:5173` (dev)
- `http://localhost:4173` / `http://127.0.0.1:4173` (preview)
- `neboapp://` origins (Tauri custom protocol for app windows)

---

## 12. Auth API Routes

All auth routes are under `/api/v1/auth/` and are rate-limited.

### Rate-Limited Routes

| Method | Path | Handler | Description |
|--------|------|---------|-------------|
| POST | `/auth/login` | `handlers::auth::login` | Email/password login |
| POST | `/auth/register` | `handlers::auth::register` | Create account |
| POST | `/auth/refresh` | `handlers::auth::refresh` | Exchange refresh token |
| POST | `/auth/forgot` | `handlers::auth::forgot_password` | Request reset |
| POST | `/auth/reset` | `handlers::auth::reset_password` | Reset with token |
| POST | `/auth/verify` | `handlers::auth::verify_email` | Verify email (stub) |
| POST | `/auth/resend` | `handlers::auth::resend_verification` | Resend (stub) |

### Public Routes (No Rate Limit)

| Method | Path | Handler | Description |
|--------|------|---------|-------------|
| GET | `/auth/config` | `handlers::auth::config` | Auth provider config |

### Auth Config Response

```json
{
  "requiresSetup": true,
  "googleEnabled": false,
  "githubEnabled": false
}
```

Used by the frontend to determine which auth options to show and whether
first-run setup is needed.

### Login/Register Response

```json
{
  "token": "<jwt-access-token>",
  "refreshToken": "<64-char-hex-token>",
  "expiresAt": 1746536000
}
```

### Forgot Password Response

Always returns the same message regardless of whether the email exists:

```json
{ "message": "If an account exists, reset instructions have been sent" }
```

This prevents user enumeration attacks.

---

## 13. Protected vs Public Routes

The route composition in `routes/mod.rs` creates three tiers:

```
+------------------------------------------------------------------+
|                    /api/v1  Route Tree                             |
+------------------------------------------------------------------+
|                                                                    |
|  TIER 1: Auth Routes (rate-limited, no JWT required)               |
|  +---------------------------------------------------------+      |
|  | /auth/login, /auth/register, /auth/refresh, etc.        |      |
|  | Rate limit: 10 req/min per IP                            |      |
|  +---------------------------------------------------------+      |
|                                                                    |
|  TIER 2: Public Routes (no JWT, no rate limit)                     |
|  +---------------------------------------------------------+      |
|  | /auth/config, /setup/*, /chat/*, /agent/*, /memory/*,   |      |
|  | /provider/*, /skills/*, /tasks/*, /integrations/*,      |      |
|  | /files/*, /neboai/*, /workflows/*, /roles/*,          |      |
|  | /commander/*, /plugins/*, /store/*, /entity-config/*,   |      |
|  | /notifications/*, /voice/*, /apps/*, /user/me/profile,  |      |
|  | /user/me/preferences, /user/me/permissions, /codes,     |      |
|  | /deps/approve, /runs/active                              |      |
|  +---------------------------------------------------------+      |
|                                                                    |
|  TIER 3: Protected Routes (JWT required)                           |
|  +---------------------------------------------------------+      |
|  | /user/me (GET, PUT, DELETE)                              |      |
|  | /user/me/change-password (POST)                          |      |
|  +---------------------------------------------------------+      |
|                                                                    |
+------------------------------------------------------------------+
```

**Design note**: Most routes in Nebo are public (Tier 2) because the server
binds to `127.0.0.1` only. The JWT-protected routes (Tier 3) are specifically
the user account management endpoints where identity verification matters. The
bulk of the API (chat, agents, skills, etc.) operates without JWT because
localhost access is considered trusted for a single-user desktop app.

---

## 14. Agent WebSocket Authentication

Agent-to-server WebSocket connections use a separate authentication mechanism:
short-lived JWTs with a `type: "agent_ws"` claim.

### Token Generation

```rust
pub fn generate_agent_ws_token(secret: &str, ttl_seconds: i64)
    -> Result<String, NeboError>
```

Creates an HS256 JWT with claims:
```json
{
  "type": "agent_ws",
  "iat": 1715000000,
  "exp": 1715000060
}
```

### Token Validation

```rust
pub fn validate_agent_ws_token(token: &str, secret: &str) -> Result<(), NeboError>
```

Verifies:
1. Valid HS256 signature against the shared secret
2. Token not expired
3. `type` claim equals `"agent_ws"`

### WebSocket Endpoints

The server exposes multiple WebSocket endpoints with different auth models:

| Endpoint | Auth | Purpose |
|----------|------|---------|
| `/ws` | None (localhost trust) | Main client WebSocket |
| `/ws/app/{agent_id}` | Agent existence check | App frontend WebSocket |
| `/ws/extension` | None (localhost trust) | Chrome extension bridge |
| `/ws/voice/dictation` | None (localhost trust) | Voice dictation |
| `/ws/voice/conversation` | None (localhost trust) | Voice conversation |
| `/api/v1/agent/ws` | None (localhost trust) | Agent-to-server WS |

Note: The agent WS token generation and validation functions exist in the auth
crate but the current agent WS handler does not enforce token validation.
The `generate_agent_ws_token` and `validate_agent_ws_token` functions are
available for future use when agent authentication is tightened.

---

## 15. MCP Endpoint Authentication

The MCP (Model Context Protocol) server endpoint has opt-in API key
authentication via the `mcp_api_key_auth` middleware.

```rust
pub async fn mcp_api_key_auth(request: Request, next: Next) -> Response {
    let expected = std::env::var("NEBO_MCP_API_KEY").ok();

    // No key configured -> skip auth (zero-config localhost mode)
    let expected = match expected {
        Some(k) if !k.is_empty() => k,
        _ => return next.run(request).await,
    };

    // Validate: Authorization: Bearer <key>
    // Returns JSON-RPC error on failure (not standard HTTP error)
}
```

### Behavior

| Scenario | Result |
|----------|--------|
| `NEBO_MCP_API_KEY` not set | All requests pass through (no auth) |
| `NEBO_MCP_API_KEY` set, valid key | Request proceeds |
| `NEBO_MCP_API_KEY` set, invalid key | JSON-RPC error -32000 |

Error response format (JSON-RPC, not REST):
```json
{
  "jsonrpc": "2.0",
  "id": null,
  "error": {
    "code": -32000,
    "message": "invalid MCP API key"
  }
}
```

---

## 16. OAuth Configuration

The config system supports OAuth provider configuration for social login:

### OAuthConfig

```rust
pub struct OAuthConfig {
    pub google_enabled: String,        // "true"/"false"
    pub google_client_id: String,
    pub google_client_secret: String,
    pub github_enabled: String,        // "true"/"false"
    pub github_client_id: String,
    pub github_client_secret: String,
    pub callback_base_url: String,
}
```

### Auth Config Endpoint

The `/api/v1/auth/config` endpoint reports which OAuth providers are enabled:

```rust
pub async fn config(State(state): State<AppState>) -> Json<serde_json::Value> {
    let google_enabled = !state.config.oauth.google_client_id.is_empty();
    let github_enabled = !state.config.oauth.github_client_id.is_empty();
    Json(serde_json::json!({
        "requiresSetup": state.store.count_users().unwrap_or(0) == 0,
        "googleEnabled": google_enabled,
        "githubEnabled": github_enabled,
    }))
}
```

### AppOAuth (Per-App OAuth)

The config also supports per-app OAuth providers via the `AppOAuth` config
section, used by MCP integrations and external service connections:

```rust
pub app_oauth: HashMap<String, AppOAuthProviderConfig>,
```

MCP integrations store encrypted OAuth tokens in the database via the
`store_mcp_credentials()` function. These tokens are encrypted with
`auth::credential::encrypt()` and refreshed automatically on startup if
expired.

---

## 17. Auth Profiles -- AI Provider Credentials

Auth profiles store API keys and connection details for AI providers
(Anthropic, OpenAI, Google, Ollama, DeepSeek, NeboAI/Janus).

### AuthProfile Model

```rust
pub struct AuthProfile {
    pub id: String,
    pub name: String,
    pub provider: String,           // "anthropic", "openai", "google", etc.
    #[serde(skip_serializing)]      // Never sent to frontend
    pub api_key: String,
    pub model: Option<String>,      // Default model for this provider
    pub base_url: Option<String>,   // Custom API URL (Ollama, DeepSeek)
    pub priority: Option<i64>,      // Provider selection priority
    pub is_active: Option<i64>,     // 0/1 toggle
    pub cooldown_until: Option<i64>,// Rate limit backoff
    pub last_used_at: Option<i64>,
    pub usage_count: Option<i64>,
    pub error_count: Option<i64>,
    pub metadata: Option<String>,   // JSON blob (e.g., janus_provider flag)
    pub created_at: i64,
    pub updated_at: i64,
    pub auth_type: Option<String>,  // "api_key", "oauth", etc.
}
```

### Security Note

The `api_key` field has `#[serde(skip_serializing)]` which means it is never
included in API responses to the frontend. However, the API key is stored in
the database in the `auth_profiles` table. These keys are loaded at server
startup by `build_providers()` to construct AI provider instances.

### Provider Construction

At startup and whenever providers are reloaded, `build_providers()` reads all
active auth profiles and constructs the appropriate provider:

```
auth_profiles table
       |
       v
+------------------+     +------------------+
| list_auth_profiles() | --> | build_providers() |
+------------------+     +------------------+
       |                          |
       v                          v
  Anthropic Profile    -->  AnthropicProvider::new(api_key, model)
  OpenAI Profile       -->  OpenAIProvider::new(api_key, model)
  Google Profile       -->  GeminiProvider::new(api_key, model)
  Ollama Profile       -->  OllamaProvider::new(base_url, model)
  DeepSeek Profile     -->  OpenAIProvider::with_base_url(...)
  NeboAI Profile     -->  OpenAIProvider (Janus gateway)
```

---

## 18. Database Schema

### Users Table

```sql
CREATE TABLE users (
    id                    TEXT PRIMARY KEY,
    email                 TEXT UNIQUE NOT NULL,
    password_hash         TEXT NOT NULL,       -- bcrypt hash
    name                  TEXT NOT NULL,
    avatar_url            TEXT,
    email_verified        INTEGER DEFAULT 0,
    email_verify_token    TEXT,
    email_verify_expires  INTEGER,
    password_reset_token  TEXT,                -- Plaintext token (compared directly)
    password_reset_expires INTEGER,            -- Unix timestamp
    created_at            INTEGER NOT NULL,
    updated_at            INTEGER NOT NULL,
    role                  TEXT DEFAULT 'user'  -- 'user' or 'admin'
);
```

### Refresh Tokens Table

```sql
CREATE TABLE refresh_tokens (
    id          TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL,
    token_hash  TEXT NOT NULL,      -- SHA-256 of the actual token
    expires_at  INTEGER NOT NULL,
    created_at  INTEGER DEFAULT (strftime('%s','now'))
);
```

**Security**: Refresh tokens are stored as SHA-256 hashes, not plaintext. Even
if the database is compromised, the attacker cannot use the stored hashes to
generate valid refresh tokens.

### Auth Profiles Table

```sql
CREATE TABLE auth_profiles (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    provider     TEXT NOT NULL,
    api_key      TEXT NOT NULL,     -- May be plaintext or "enc:" prefixed
    model        TEXT,
    base_url     TEXT,
    priority     INTEGER DEFAULT 0,
    is_active    INTEGER DEFAULT 1,
    cooldown_until INTEGER,
    last_used_at   INTEGER,
    usage_count    INTEGER DEFAULT 0,
    error_count    INTEGER DEFAULT 0,
    metadata       TEXT,            -- JSON blob
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    auth_type      TEXT DEFAULT 'api_key'
);
```

### User Preferences Table

```sql
CREATE TABLE user_preferences (
    user_id              TEXT PRIMARY KEY,
    email_notifications  INTEGER DEFAULT 1,
    marketing_emails     INTEGER DEFAULT 0,
    timezone             TEXT DEFAULT '',
    language             TEXT DEFAULT '',
    theme                TEXT DEFAULT '',
    updated_at           INTEGER,
    inapp_notifications  INTEGER DEFAULT 1
);
```

### User Profile Table

```sql
CREATE TABLE user_profile (
    user_id              TEXT PRIMARY KEY,
    display_name         TEXT,
    bio                  TEXT,
    location             TEXT,
    timezone             TEXT,
    occupation           TEXT,
    interests            TEXT,
    communication_style  TEXT,
    goals                TEXT,
    context              TEXT,
    onboarding_completed INTEGER DEFAULT 0,
    onboarding_step      TEXT,
    created_at           INTEGER,
    updated_at           INTEGER,
    tool_permissions     TEXT,
    terms_accepted_at    INTEGER
);
```

---

## 19. Configuration Reference

### nebo.yaml Auth Section

```yaml
Auth:
  AccessSecret: placeholder-replaced-at-runtime
  AccessExpire: 31536000      # 1 year in seconds
  RefreshTokenExpire: 31536000 # 1 year in seconds
```

### Rust Config Structs

```rust
pub struct AuthConfig {
    pub access_secret: String,        // JWT signing secret
    pub access_expire: i64,           // Access token TTL (seconds)
    pub refresh_token_expire: i64,    // Refresh token TTL (seconds)
}

pub struct SecurityConfig {
    pub csrf_enabled: String,
    pub csrf_secret: String,
    pub csrf_token_expiry: i64,       // 43,200 (12 hours)
    pub csrf_secure_cookie: String,
    pub rate_limit_enabled: String,
    pub rate_limit_requests: u32,     // 100
    pub rate_limit_interval: u32,     // 60 seconds
    pub rate_limit_burst: u32,        // 20
    pub auth_rate_limit_requests: u32, // 5
    pub auth_rate_limit_interval: u32, // 60 seconds
    pub enable_security_headers: String,
    pub content_security_policy: String,
    pub allowed_origins: String,
    pub force_https: String,
    pub max_request_body_size: i64,
    pub max_url_length: u32,
}
```

### Environment Variable Overrides

| Variable | Purpose | Used By |
|----------|---------|---------|
| `MCP_ENCRYPTION_KEY` | Override encryption key | `mcp::crypto::resolve_encryption_key` |
| `JWT_SECRET` | Fallback encryption key | `mcp::crypto::resolve_encryption_key` |
| `NEBO_MCP_API_KEY` | MCP endpoint auth | `middleware::mcp_api_key_auth` |

---

## 20. Cross-System Interactions

### Auth <-> Server

```
server::run()
    |-- AuthService::new(store, config)           --> AppState.auth
    |-- auth::keyring::get()                      --> Master key retrieval
    |-- mcp::crypto::resolve_encryption_key()     --> Fallback key resolution
    |-- auth::credential::init(encryptor)         --> Global encryptor
    |-- auth::keyring::set(key_hex)               --> Persist key to keyring
    |
    |-- middleware::jwt_auth                       --> Uses auth::validate_jwt_claims
    |-- handlers::auth::*                         --> Uses AppState.auth (AuthService)
    |-- handlers::setup::create_admin             --> Uses auth.register()
    |-- handlers::user::*                         --> Uses auth.get_user_by_id(), etc.
```

### Auth <-> Tools

```
tools::skill_tool
    |-- auth::credential::encrypt(value)          --> Store skill secrets
    |
tools::skills::expand / execute_tool
    |-- auth::credential::decrypt(value)          --> Decrypt for env injection
```

### Auth <-> DB

```
AuthService
    |-- store.check_email_exists(email)
    |-- store.create_user(id, email, hash, name)
    |-- store.create_user_preferences(user_id)
    |-- store.get_user_by_email(email)
    |-- store.get_user_by_id(user_id)
    |-- store.update_user_password(user_id, hash)
    |-- store.delete_user(user_id)
    |-- store.set_password_reset_token(user_id, token, expires)
    |-- store.get_user_by_password_reset_token(token)
    |-- store.create_refresh_token(id, user_id, hash, expires)
    |-- store.get_refresh_token_by_hash(hash)
    |-- store.delete_refresh_token(hash)
    |-- store.count_users()
```

### Auth <-> Config

```
AuthService
    |-- config.auth.access_secret                 --> JWT signing key
    |-- config.auth.access_expire                 --> Access token TTL
    |-- config.auth.refresh_token_expire          --> Refresh token TTL
    |
Middleware
    |-- JwtSecret(config.auth.access_secret)      --> Injected via extension
    |
Auth Handlers
    |-- config.oauth.google_client_id             --> OAuth availability
    |-- config.oauth.github_client_id             --> OAuth availability
```

---

## 21. Error Handling

### NeboError Variants (Auth-Related)

```rust
pub enum NeboError {
    UserNotFound,          // 404 — user lookup failed
    InvalidCredentials,    // 401 — wrong email/password
    EmailExists,           // 409 — duplicate email on register
    InvalidToken,          // 401 — JWT validation failed, expired, or wrong type
    Unauthorized,          // 401 — generic unauthorized
    // ...
}
```

### HTTP Status Code Mapping

```rust
match self {
    Self::UserNotFound | Self::NotFound => 404,
    Self::InvalidCredentials | Self::Unauthorized | Self::InvalidToken => 401,
    Self::EmailExists => 409,
    Self::RateLimit => 429,
    // ...
}
```

### Error Response Format

All auth errors return a consistent JSON structure:

```json
{
  "error": "human-readable error message"
}
```

### Error Handling Patterns

1. **Password verification**: Uses `bcrypt::verify()` which returns an error
   on hash mismatch. The error is mapped to `NeboError::InvalidCredentials`
   without revealing whether the email or password was wrong.

2. **Token validation**: All JWT validation errors are mapped to
   `NeboError::InvalidToken`. The middleware returns a generic "invalid token"
   message without leaking validation details.

3. **User enumeration prevention**: The forgot-password handler always returns
   success even if the email doesn't exist. The reset-password handler returns
   `InvalidToken` (not "user not found") if the token is invalid.

4. **Credential encryption errors**: Return descriptive `String` errors (not
   `NeboError`) since they indicate system misconfiguration, not user errors.

---

## 22. Threat Model and Security Considerations

### Threat: Database Compromise

**Mitigations:**
- Passwords are bcrypt-hashed (cost=12), not reversible
- Refresh tokens stored as SHA-256 hashes, not plaintext
- API keys and secrets are AES-256-GCM encrypted with `enc:` prefix
- Master encryption key is stored in OS keyring, not in the database

**Residual risk:** If both the database AND the master key (keyring/file) are
compromised, encrypted credentials can be decrypted. The master key is the
single point of trust.

### Threat: JWT Secret Compromise

**Mitigations:**
- The default JWT secret is `"placeholder-replaced-at-runtime"` -- a
  deliberately weak value that should be replaced in production
- Access tokens contain the user ID and email; forged tokens could impersonate
  any user

**Residual risk:** The 1-year token lifetime means a compromised token remains
valid for a very long time. This is acceptable for the local-first desktop
context but would be problematic in a multi-tenant deployment.

### Threat: Local Network Access

**Mitigations:**
- Server binds to `127.0.0.1` by default (not `0.0.0.0`)
- CORS restricts origins to localhost ports
- HSTS headers enforce HTTPS for any non-localhost deployment
- X-Frame-Options: DENY prevents clickjacking (except embed routes)

**Residual risk:** Any process on the local machine can access the API. This is
by design for a desktop application.

### Threat: Brute Force

**Mitigations:**
- Auth routes rate-limited to 10 requests/minute per IP
- bcrypt with cost=12 makes each password verification slow (~250ms)
- Rate limiter uses actual peer IP (ignores X-Forwarded-For to prevent spoofing)

### Threat: Token Replay (Refresh)

**Mitigations:**
- Refresh token rotation: each token is single-use and deleted after exchange
- Tokens have expiration times checked on every validation
- Expired tokens are filtered out at the database query level

### Threat: Keyring Unavailability

**Mitigations:**
- Graceful fallback: keyring -> env var -> file -> generate new
- Key is persisted to keyring on first startup for subsequent boots
- File-based key (`~/.nebo/data/.mcp-key`) serves as a fallback

**Residual risk:** On headless systems without a keyring, the master key is
stored as a file on disk, protected only by filesystem permissions.

### Threat: Nonce Reuse in AES-GCM

**Mitigations:**
- Each encryption call generates a fresh random 12-byte nonce via `OsRng`
- With a 96-bit random nonce and practical encryption volumes (thousands,
  not billions), the birthday-bound collision probability is negligible

### Threat: Password Reset Token Leakage

**Mitigations:**
- Tokens are 64-character hex (256 bits of entropy)
- Tokens expire after 1 hour
- Successfully used tokens are cleared from the database

**Note:** Password reset tokens are stored in the `users` table as plaintext
(the `password_reset_token` column is compared directly). This is acceptable
because the token itself has 256 bits of entropy and a 1-hour lifetime,
making brute-force infeasible.

---

## 23. Flow Diagrams

### User Registration Flow

```
Frontend                    Server                       DB
   |                          |                           |
   |  POST /auth/register     |                           |
   |  {email,password,name}   |                           |
   |------------------------->|                           |
   |                          | check_email_exists(email) |
   |                          |-------------------------->|
   |                          |<------ false -------------|
   |                          |                           |
   |                          | bcrypt::hash(password,12) |
   |                          |-----+                     |
   |                          |<----+                     |
   |                          |                           |
   |                          | generate_id() -> user_id  |
   |                          |                           |
   |                          | create_user(id,email,     |
   |                          |   hash,name)              |
   |                          |-------------------------->|
   |                          |<------ OK ----------------|
   |                          |                           |
   |                          | create_user_preferences   |
   |                          |   (user_id)               |
   |                          |-------------------------->|
   |                          |<------ OK ----------------|
   |                          |                           |
   |                          | generate_tokens()         |
   |                          |  JWT(userId,email,exp)    |
   |                          |  refresh = random hex     |
   |                          |  SHA-256(refresh) -> hash |
   |                          |                           |
   |                          | create_refresh_token      |
   |                          |   (id,user_id,hash,exp)   |
   |                          |-------------------------->|
   |                          |<------ OK ----------------|
   |                          |                           |
   |  {token, refreshToken,   |                           |
   |   expiresAt}             |                           |
   |<-------------------------|                           |
```

### User Login Flow

```
Frontend                    Server                       DB
   |                          |                           |
   |  POST /auth/login        |                           |
   |  {email, password}       |                           |
   |------------------------->|                           |
   |                          | get_user_by_email(email)  |
   |                          |    (case-insensitive)     |
   |                          |-------------------------->|
   |                          |<------ User --------------|
   |                          |                           |
   |                          | bcrypt::verify(password,  |
   |                          |   user.password_hash)     |
   |                          |-----+                     |
   |                          |<----+ (true)              |
   |                          |                           |
   |                          | generate_tokens()         |
   |                          |   (same as registration)  |
   |                          |                           |
   |  {token, refreshToken,   |                           |
   |   expiresAt}             |                           |
   |<-------------------------|                           |
```

### Token Refresh Flow (Rotation)

```
Frontend                    Server                       DB
   |                          |                           |
   |  POST /auth/refresh      |                           |
   |  {refreshToken}          |                           |
   |------------------------->|                           |
   |                          | SHA-256(refreshToken)     |
   |                          |   -> token_hash           |
   |                          |                           |
   |                          | get_refresh_token_by_hash |
   |                          |   (token_hash)            |
   |                          |   WHERE expires_at > now  |
   |                          |-------------------------->|
   |                          |<------ RefreshToken ------|
   |                          |                           |
   |                          | get_user_by_id(user_id)   |
   |                          |-------------------------->|
   |                          |<------ User --------------|
   |                          |                           |
   |                          | delete_refresh_token      |
   |                          |   (old_token_hash)        |
   |                          |-------------------------->|  <-- Single-use
   |                          |<------ OK ----------------|      rotation
   |                          |                           |
   |                          | generate_tokens()         |
   |                          |   (new access + refresh)  |
   |                          |                           |
   |  {token, refreshToken,   |                           |
   |   expiresAt}             |                           |
   |<-------------------------|                           |
```

### Credential Encryption Flow

```
Caller (skill_tool, codes, etc.)      credential module       Encryptor (AES-256-GCM)
        |                                    |                         |
        | encrypt("sk-my-api-key")           |                         |
        |----------------------------------->|                         |
        |                                    | ENCRYPTOR.get()         |
        |                                    |---+                     |
        |                                    |<--+ (OnceLock)          |
        |                                    |                         |
        |                                    | encrypt_b64(bytes)      |
        |                                    |------------------------>|
        |                                    |                         |
        |                                    |   OsRng -> nonce (12B)  |
        |                                    |   AES-256-GCM encrypt   |
        |                                    |   nonce || ciphertext   |
        |                                    |   base64 encode         |
        |                                    |                         |
        |                                    |<---- base64 string -----|
        |                                    |                         |
        |  "enc:nonce+ciphertext_base64"     |                         |
        |<-----------------------------------|                         |
        |                                                              |
        | --> stored in SQLite                                         |
```

### Master Key Resolution at Startup

```
                    +-- Server Start --+
                    |                  |
                    v                  |
            +--------------+           |
            | Keyring has  | --Yes-->  |  Parse hex or passphrase
            | master key?  |          |
            +--------------+           |
                    | No               |
                    v                  |
            +--------------+           |
            | MCP_ENCRYPT  | --Yes-->  |  Encryptor::from_passphrase
            | ION_KEY env? |          |
            +--------------+           |
                    | No               |
                    v                  |
            +--------------+           |
            | JWT_SECRET   | --Yes-->  |  Encryptor::from_passphrase
            | env?         |          |
            +--------------+           |
                    | No               |
                    v                  |
            +--------------+           |
            | .mcp-key     | --Yes-->  |  Encryptor::new(raw bytes)
            | file exists? |          |
            +--------------+           |
                    | No               |
                    v                  |
            +--------------+           |
            | Generate new | -------->  |  Encryptor::generate()
            | random key   |          |  Write to .mcp-key file
            +--------------+           |
                    |                  |
                    v                  |
            +--------------+           |
            | Store in     |           |
            | keyring (if  | <---------+
            | available)   |
            +--------------+
                    |
                    v
            +--------------+
            | credential:: |
            | init()       |
            +--------------+
```

### Authenticated API Request Flow

```
Frontend                Axum Middleware Stack            Handler
   |                          |                           |
   |  GET /api/v1/user/me     |                           |
   |  Authorization: Bearer   |                           |
   |    <jwt-token>           |                           |
   |------------------------->|                           |
   |                          |                           |
   |              +-- security_headers() --+              |
   |              | Add HSTS, X-Frame,     |              |
   |              | Referrer-Policy, etc.  |              |
   |              +------------------------+              |
   |                          |                           |
   |              +-- api_security_headers() --+          |
   |              | Add CSP: default-src none  |          |
   |              | Add Cache-Control: no-store|          |
   |              +----------------------------+          |
   |                          |                           |
   |              +-- jwt_auth() --+                      |
   |              | Extract Bearer token    |             |
   |              | validate_jwt_claims()   |             |
   |              | Insert AuthClaims       |             |
   |              +------------------------+              |
   |                          |                           |
   |                          |  AuthClaims { user_id,    |
   |                          |    email } in extensions  |
   |                          |-------------------------->|
   |                          |                           |
   |                          |          get_current_user |
   |                          |          reads user_id    |
   |                          |          from AuthClaims  |
   |                          |                           |
   |  200 OK                  |                           |
   |  {id, email, name, ...}  |                           |
   |<-------------------------|<--------------------------|
```

---

## 24. Testing

### Unit Tests (jwt.rs)

```rust
#[test]
fn test_generate_and_validate_agent_ws_token() {
    let secret = "test-secret-key-for-testing";
    let token = generate_agent_ws_token(secret, 3600).unwrap();
    assert!(validate_agent_ws_token(&token, secret).is_ok());
    assert!(validate_agent_ws_token(&token, "wrong-secret").is_err());
}

#[test]
fn test_validate_jwt_claims() {
    // Tests both "userId" key (Go compat) and structured extraction
    // Verifies sub, email, expiration handling
}
```

### Unit Tests (credential.rs)

```rust
#[test]
fn test_encrypt_decrypt_roundtrip() {
    // Verifies encrypt -> decrypt returns original value
    // Checks "enc:" prefix on encrypted output
}

#[test]
fn test_decrypt_plaintext_passthrough() {
    // Verifies unencrypted values pass through decrypt() unchanged
}

#[test]
fn test_is_encrypted() {
    // Verifies prefix detection
}
```

### Unit Tests (keyring.rs)

```rust
#[test]
fn test_keyring_available() {
    // Non-panicking check (may return false on CI)
}

#[test]
fn test_keyring_get_nonexistent() {
    // Verifies graceful handling on headless systems
}
```

### Unit Tests (crypto.rs)

```rust
#[test]
fn test_encrypt_decrypt() {
    // Raw byte encryption roundtrip
}

#[test]
fn test_encrypt_decrypt_b64() {
    // Base64 encryption roundtrip (passphrase-derived key)
}

#[test]
fn test_different_keys_fail() {
    // Verifies that decryption with wrong key fails
}
```

### Running Auth Tests

```bash
cargo test -p nebo-auth              # All auth crate tests
cargo test -p nebo-auth -- jwt       # JWT tests only
cargo test -p nebo-auth -- credential # Credential tests only
cargo test -p nebo-auth -- keyring   # Keyring tests only
```

---

## 25. Key Implementation Files

| File | Path | Purpose |
|------|------|---------|
| Auth lib.rs | `crates/auth/src/lib.rs` | Public re-exports |
| JWT module | `crates/auth/src/jwt.rs` | Token generation/validation |
| Auth service | `crates/auth/src/service.rs` | User account CRUD |
| Credential module | `crates/auth/src/credential.rs` | Global encrypt/decrypt |
| Keyring module | `crates/auth/src/keyring.rs` | OS keychain integration |
| Auth Cargo.toml | `crates/auth/Cargo.toml` | Dependencies |
| Encryptor | `crates/mcp/src/crypto.rs` | AES-256-GCM primitives + key resolution |
| JWT middleware | `crates/server/src/middleware.rs` | `jwt_auth`, `rate_limit`, `mcp_api_key_auth`, security headers |
| Auth handlers | `crates/server/src/handlers/auth.rs` | Login, register, refresh, reset endpoints |
| Auth routes | `crates/server/src/routes/auth.rs` | Route definitions + public config |
| User handlers | `crates/server/src/handlers/user.rs` | Protected user endpoints |
| User routes | `crates/server/src/routes/user.rs` | Public + protected user routes |
| Setup handler | `crates/server/src/handlers/setup.rs` | Admin creation + setup flow |
| Route composition | `crates/server/src/routes/mod.rs` | Protected/public route tree |
| App state | `crates/server/src/state.rs` | `AppState.auth: Arc<AuthService>` |
| Server startup | `crates/server/src/lib.rs` | Master key resolution, credential init |
| DB models | `crates/db/src/models.rs` | `User`, `RefreshToken`, `AuthProfile` |
| DB users | `crates/db/src/queries/users.rs` | User CRUD queries |
| DB refresh tokens | `crates/db/src/queries/refresh_tokens.rs` | Refresh token queries |
| API types | `crates/types/src/api.rs` | Request/response structs |
| Error types | `crates/types/src/error.rs` | `NeboError` auth variants |
| Constants | `crates/types/src/constants.rs` | Default TTLs, rate limits |
| Config | `crates/config/src/config.rs` | `AuthConfig`, `SecurityConfig`, `OAuthConfig` |
| nebo.yaml | `etc/nebo.yaml` | Auth config defaults |
