# Plugins, Store, OAuth & Developer Routes -- Logic Deep-Dive

Source: Go codebase at `/Users/almatuck/workspaces/nebo/nebo/`

---

## Table of Contents

1. [Plugin System](#1-plugin-system)
2. [NeboLoop Store Integration](#2-neboloop-store-integration)
3. [NeboLoop Connection Code](#3-neboloop-connection-code)
4. [User OAuth (Login/Signup)](#4-user-oauth-loginsignup)
5. [OAuth Broker (App OAuth)](#5-oauth-broker-app-oauth)
6. [App OAuth Handlers](#6-app-oauth-handlers)
7. [Developer Routes](#7-developer-routes)

---

## 1. Plugin System

**File:** `internal/handler/plugins/handler.go` (939 lines)
**Package:** `plugins`
**Route prefix:** `/api/v1/` (protected)

### 1.1 Core Types

```go
// types/types.go
type PluginItem struct {
    Id               string            `json:"id"`
    Name             string            `json:"name"`
    PluginType       string            `json:"pluginType"`
    DisplayName      string            `json:"displayName"`
    Description      string            `json:"description"`
    Icon             string            `json:"icon"`
    Version          string            `json:"version"`
    IsEnabled        bool              `json:"isEnabled"`
    IsInstalled      bool              `json:"isInstalled"`
    ConnectionStatus string            `json:"connectionStatus"`
    LastConnectedAt  string            `json:"lastConnectedAt,omitempty"`
    LastError        string            `json:"lastError,omitempty"`
    Settings         map[string]string `json:"settings,omitempty"`
    Capabilities     []string          `json:"capabilities,omitempty"`
    Permissions      []string          `json:"permissions,omitempty"`
    AppID            string            `json:"appId,omitempty"`
    CreatedAt        string            `json:"createdAt"`
    UpdatedAt        string            `json:"updatedAt"`
}

type UpdatePluginSettingsRequest struct {
    Id       string            `path:"id"`
    Settings map[string]string `json:"settings"`
    Secrets  map[string]bool   `json:"secrets,omitempty"`
}

type TogglePluginRequest struct {
    Id        string `path:"id"`
    IsEnabled bool   `json:"isEnabled"`
}
```

### 1.2 Route Map

| Method | Path | Handler | Description |
|--------|------|---------|-------------|
| GET | `/plugins` | `ListPluginsHandler` | List all plugins, optional `?type=` filter |
| GET | `/plugins/{id}` | `GetPluginHandler` | Get single plugin by ID |
| PUT | `/plugins/{id}/settings` | `UpdatePluginSettingsHandler` | Upsert settings for a plugin |
| PUT | `/plugins/{id}/toggle` | `TogglePluginHandler` | Enable/disable a plugin |

### 1.3 Handler Signatures

```go
func ListPluginsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
func GetPluginHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
func UpdatePluginSettingsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
func TogglePluginHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```

### 1.4 List Plugins Logic

1. Read query param `type` from URL.
2. If `type` is non-empty, call `svcCtx.DB.ListPluginsByType(ctx, pluginType)`.
3. Otherwise call `svcCtx.DB.ListPlugins(ctx)`.
4. For each row, convert to `PluginItem` via `toPluginItem()`.
5. Load settings via `svcCtx.DB.ListPluginSettings(ctx, p.ID)`.
6. **Secret masking:** If `s.IsSecret != 0`, the value is replaced with `"--------"` (8 bullet chars). Non-secret values are returned as-is.
7. Return `ListPluginsResponse{Plugins: result}`.

### 1.5 Get Single Plugin Logic

1. Extract `id` from chi URL param.
2. Call `svcCtx.DB.GetPlugin(ctx, id)`.
3. Convert via `toPluginItem()`, load and mask settings identically to list.
4. Return `GetPluginResponse{Plugin: item}`.

### 1.6 Update Plugin Settings Logic

1. Extract `id` from chi URL param.
2. Parse JSON body into `UpdatePluginSettingsRequest`.
3. Verify plugin exists via `svcCtx.DB.GetPlugin(ctx, id)`.
4. **Primary path (hot-reload):** If `svcCtx.PluginStore != nil`, call `svcCtx.PluginStore.UpdateSettings(ctx, id, req.Settings, req.Secrets)`. This triggers `OnSettingsChanged` for hot-reload.
5. **Fallback path (no hot-reload):** Raw DB upsert loop. For each key/value, check if key is in `req.Secrets` map. If so, set `isSecret = 1`. Call `svcCtx.DB.UpsertPluginSetting()` with a new UUID for each setting row.
6. Return updated plugin with masked settings.

### 1.7 Toggle Plugin Logic

1. Extract `id` from chi URL param.
2. Parse JSON body into `TogglePluginRequest`.
3. Convert `bool` to `int64` (0 or 1).
4. Call `svcCtx.DB.TogglePlugin(ctx, TogglePluginParams{IsEnabled: val, ID: id})`.
5. Re-fetch and return the updated plugin.
6. **No side effects beyond the DB flag.** The toggle does not start/stop processes -- that is handled elsewhere by the agent/app registry checking `IsEnabled`.

### 1.8 `toPluginItem()` Conversion

```go
func toPluginItem(p db.PluginRegistry) types.PluginItem
```

- Extracts `capabilities`, `permissions`, and `app_id` from the `Metadata` JSON column.
- Metadata schema parsed: `{"provides": [...], "permissions": [...], "app_id": "..."}`
- `IsEnabled` and `IsInstalled` converted from `int64` to `bool` (non-zero = true).
- `CreatedAt` and `UpdatedAt` are unix timestamps converted to RFC3339 strings.
- `LastConnectedAt` uses `nullTimeString()` (returns empty string if null/zero).
- `LastError` uses `nullString()` (returns empty string if null).

### 1.9 Hot-Reload Mechanism (PluginStore)

**File:** `internal/apps/settings/store.go`
**File:** `internal/apps/settings/manifest.go`

```go
// Configurable interface -- apps implement this for hot-reload
type Configurable interface {
    OnSettingsChanged(settings map[string]string) error
}

// ChangeHandler -- external notification (e.g., WebSocket broadcast)
type ChangeHandler func(appName string, settings map[string]string)
```

**Store struct:**

```go
type Store struct {
    queries       *db.Queries
    mu            sync.RWMutex
    handlers      []ChangeHandler
    configurables map[string]Configurable  // app name -> Configurable
}
```

**Key methods:**

| Method | Signature | Description |
|--------|-----------|-------------|
| `NewStore` | `func NewStore(sqlDB *sql.DB) *Store` | Creates store backed by DB |
| `OnChange` | `func (s *Store) OnChange(fn ChangeHandler)` | Register external change handler |
| `RegisterConfigurable` | `func (s *Store) RegisterConfigurable(appName string, c Configurable)` | Register hot-reload target |
| `DeregisterConfigurable` | `func (s *Store) DeregisterConfigurable(appName string)` | Remove hot-reload target |
| `GetPlugin` | `func (s *Store) GetPlugin(ctx, name string) (*db.PluginRegistry, error)` | Lookup by name |
| `GetPluginByID` | `func (s *Store) GetPluginByID(ctx, id string) (*db.PluginRegistry, error)` | Lookup by ID |
| `ListPlugins` | `func (s *Store) ListPlugins(ctx, pluginType string) ([]db.PluginRegistry, error)` | List, optional type filter |
| `GetSettings` | `func (s *Store) GetSettings(ctx, pluginID string) (map[string]string, error)` | Get decrypted settings by ID |
| `GetSettingsByName` | `func (s *Store) GetSettingsByName(ctx, appName string) (map[string]string, error)` | Get decrypted settings by name |
| `UpdateSettings` | `func (s *Store) UpdateSettings(ctx, pluginID string, values map[string]string, secrets map[string]bool) error` | Upsert + hot-reload |
| `TogglePlugin` | `func (s *Store) TogglePlugin(ctx, pluginID string, enabled bool) error` | Enable/disable |
| `UpdateStatus` | `func (s *Store) UpdateStatus(ctx, pluginID, status, lastError string) error` | Update connection status |
| `DeleteSetting` | `func (s *Store) DeleteSetting(ctx, pluginID, key string) error` | Remove single setting |

**`UpdateSettings` flow (the hot-reload trigger):**

1. For each key/value pair:
   - If key is in `secrets` map and true, set `isSecret = 1` and encrypt the value via `credential.Encrypt(value)`.
   - Call `queries.UpsertPluginSetting()` with a new UUID.
2. After all upserts, fetch the full current settings map via `GetSettings()` (decrypts secrets).
3. Look up the plugin by ID to get the app name.
4. If a `Configurable` is registered for that app name, call `c.OnSettingsChanged(allSettings)`.
5. Call `notifyChange(p.Name, allSettings)` which iterates over all registered `ChangeHandler` callbacks.

**`GetSettings` decryption:**

- Reads all `plugin_settings` rows for a plugin ID.
- If `row.IsSecret != 0`, attempts `credential.Decrypt(val)`. On success, returns plaintext. On failure, returns the raw (encrypted) value.
- Non-secret values returned as-is.

---

## 2. NeboLoop Store Integration

**File:** `internal/handler/plugins/handler.go` (lines 260-939)
**Route prefix:** `/api/v1/store/` and `/api/v1/neboloop/`

### 2.1 Route Map

| Method | Path | Handler | Description |
|--------|------|---------|-------------|
| GET | `/store/apps` | `ListStoreAppsHandler` | List apps from NeboLoop marketplace |
| GET | `/store/apps/{id}` | `GetStoreAppHandler` | Get single app detail |
| GET | `/store/apps/{id}/reviews` | `GetStoreAppReviewsHandler` | Get app reviews |
| POST | `/store/apps/{id}/install` | `InstallStoreAppHandler` | Install an app |
| DELETE | `/store/apps/{id}/install` | `UninstallStoreAppHandler` | Uninstall an app |
| GET | `/store/skills` | `ListStoreSkillsHandler` | List skills from NeboLoop |
| POST | `/store/skills/{id}/install` | `InstallStoreSkillHandler` | Install a skill |
| DELETE | `/store/skills/{id}/install` | `UninstallStoreSkillHandler` | Uninstall a skill |

### 2.2 Handler Signatures

```go
func ListStoreAppsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
func GetStoreAppHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
func GetStoreAppReviewsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
func InstallStoreAppHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
func UninstallStoreAppHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
func ListStoreSkillsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
func InstallStoreSkillHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
func UninstallStoreSkillHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```

### 2.3 NeboLoop Client Resolution

```go
func neboLoopClient(ctx context.Context, svcCtx *svc.ServiceContext) (*neboloopsdk.APIClient, error)
```

**Two-tier resolution:**

1. **Primary:** `svcCtx.NeboLoopClient()` -- returns the agent-wired provider function that injects the OAuth JWT from `auth_profiles`. The provider is a `func(ctx) (interface{}, error)` that returns `*neboloopsdk.APIClient`. Type-asserted.
2. **Fallback:** If no agent-wired provider, reads settings directly from `PluginStore.GetSettingsByName(ctx, "neboloop")` and creates a client via `neboloopsdk.NewAPIClient(settings)`. This path has no OAuth JWT and may fail on authenticated endpoints.
3. On any failure, returns an error surfaced as `"NeboLoop not configured: ..."`.

### 2.4 List Store Apps

1. Obtain NeboLoop client.
2. Read query params: `q` (search), `category`, `page`, `pageSize`.
3. Call `client.ListApps(ctx, query, category, page, pageSize)`.
4. Get locally installed plugin names via `installedPluginNames()`.
5. For each upstream app, call `enrichApp()` to mark `IsInstalled = true` if the app's slug matches a local plugin name.
6. Return `ListStoreAppsResponse{Apps, TotalCount, Page, PageSize}`.

### 2.5 List Store Skills

Identical pattern to List Store Apps but calls `client.ListSkills()` and uses `enrichSkill()`.

### 2.6 Get Store App Detail

1. Obtain NeboLoop client.
2. Call `client.GetApp(ctx, id)` -- returns `*neboloopsdk.AppDetail`.
3. Get installed plugin names.
4. Call `enrichAppDetail()` which:
   - Wraps into `types.StoreAppDetail` (strips `ManifestURL` and `Manifest` from the response -- security: prevents frontend from seeing download URLs).
   - Copies `AgeRating`, `Platforms`, `Size`, `Language`, `Screenshots`, `Changelog`, `WebsiteURL`, `PrivacyURL`, `SupportURL`.
   - Marks installed status via `enrichApp()`.
5. Return `GetStoreAppResponse{App: detail}`.

### 2.7 Get Store App Reviews

1. Obtain NeboLoop client.
2. Read `page` and `pageSize` from query params (default 0 if missing).
3. Call `client.GetAppReviews(ctx, id, page, pageSize)`.
4. Return `GetStoreAppReviewsResponse{Reviews, TotalCount, Average, Distribution}`.

**Response types:**

```go
type GetStoreAppReviewsResponse struct {
    Reviews      []neboloopsdk.Review `json:"reviews"`
    TotalCount   int                  `json:"totalCount"`
    Average      float64              `json:"average"`
    Distribution [5]int               `json:"distribution"`  // 5-star distribution array
}
```

### 2.8 Install Store App

1. Obtain NeboLoop client.
2. Call `client.InstallApp(ctx, id)` -- returns `*neboloopsdk.InstallResponse`.
3. Create local `plugin_registry` row via `createLocalPlugin(ctx, svcCtx, result, "app")`.
4. **Background download:** If `svcCtx.AppRegistry()` implements the `installer` interface (`InstallFromURL(ctx, downloadURL) error`):
   - Build download URL: `client.APIServer() + "/api/v1/apps/" + id + "/download"` (with optional `?version=` param).
   - Spawn a goroutine with 5-minute timeout to call `inst.InstallFromURL()`.
   - Errors are logged but do not fail the HTTP response.
5. Return `InstallStoreAppResponse{PluginID, Message: "app installed"}`.

### 2.9 Uninstall Store App

1. Obtain NeboLoop client.
2. Call `client.UninstallApp(ctx, id)`.
3. Remove local `plugin_registry` row via `removeLocalPluginByStoreID()` -- returns the `app_id` from metadata.
4. If `app_id` is non-empty and `svcCtx.AppRegistry()` implements `uninstaller` interface (`Uninstall(appID string) error`):
   - Call `u.Uninstall(appID)` to stop the running process and remove the app directory.
   - Errors are logged as warnings but do not fail the HTTP response.
5. Return `{"message": "app uninstalled"}`.

### 2.10 Install Store Skill

1. Obtain NeboLoop client.
2. Call `client.InstallSkill(ctx, id)`. If the error contains `"409"` (already installed), continue without failing.
3. Fetch skill detail via `client.GetSkill(ctx, id)` to get the SKILL.md content.
4. Extract SKILL.md content via `extractSkillContent(detail)`:
   - NeboLoop stores manifest as `base64(JSON-string)`.
   - Decoding chain: JSON unmarshal to get base64 string -> base64 decode -> JSON unmarshal to get raw markdown.
   - Graceful fallback at each step (not JSON? try raw bytes; not base64? try plain text).
5. Determine slug: use `detail.Slug` if available, otherwise `slugify(detail.Name)`.
6. Write SKILL.md to `<NeboDir>/skills/<slug>/SKILL.md` (creates directory if needed).
   - **fsnotify will hot-reload it** -- no explicit reload call needed.
7. Create local DB tracking row:
   - On fresh install (non-409): `createLocalPlugin(ctx, svcCtx, result, "skill")`.
   - On 409 re-install: `createLocalPluginFromDetail(ctx, svcCtx, id, detail.SkillItem)`.
8. Return `InstallStoreSkillResponse{PluginID, Message: "skill installed"}`.

### 2.11 Uninstall Store Skill

1. Find the slug before removing the DB row: `findPluginSlugByStoreID(ctx, svcCtx, id)`.
2. Call `client.UninstallSkill(ctx, id)`.
3. Remove skill directory from disk: `os.RemoveAll(<NeboDir>/skills/<slug>)`.
4. Remove local DB row: `removeLocalPluginByStoreID(ctx, svcCtx, id)`.
5. Return `{"message": "skill uninstalled"}`.

### 2.12 Helper Functions

```go
func createLocalPlugin(ctx context.Context, svcCtx *svc.ServiceContext, result *neboloopsdk.InstallResponse, pluginType string) (string, error)
```
- Extracts `App` or `Skill` item from `InstallResponse` (tries App first, then Skill).
- Generates new UUID for plugin ID.
- Stores `store_install_id` and `store_app_id` in metadata JSON.
- Creates DB row with `IsEnabled=1`, `IsInstalled=1`, `SettingsManifest="{}"`.

```go
func createLocalPluginFromDetail(ctx context.Context, svcCtx *svc.ServiceContext, storeID string, item neboloopsdk.SkillItem) (string, error)
```
- Used when install returns 409 (already installed on NeboLoop side).
- Creates local tracking row from the skill detail instead of install response.
- Metadata contains only `store_app_id`.

```go
func removeLocalPluginByStoreID(ctx context.Context, svcCtx *svc.ServiceContext, storeID string) string
```
- Lists ALL plugins, scans metadata JSON for matching `store_app_id` or `store_install_id`.
- Deletes the matching row.
- Returns the `app_id` from metadata (for process cleanup).

```go
func findPluginSlugByStoreID(ctx context.Context, svcCtx *svc.ServiceContext, storeID string) string
```
- Same scan logic as remove, but returns `p.Name` (the slug) without deleting.

```go
func extractSkillContent(detail *neboloopsdk.SkillDetail) (string, error)
```
- Decodes SKILL.md from NeboLoop's nested encoding: `base64(JSON-string)`.
- Three-layer fallback: JSON unmarshal -> base64 decode -> JSON unmarshal again.

```go
func slugify(name string) string
```
- Lowercases, trims, replaces spaces/underscores with hyphens.
- Strips non `[a-z0-9-]` characters via regex.
- Collapses consecutive hyphens.

```go
func installedPluginNames(ctx context.Context, svcCtx *svc.ServiceContext) map[string]bool
```
- Lists all plugins, returns set of names where `IsInstalled != 0`.

```go
func enrichApp(a neboloopsdk.AppItem, installed map[string]bool) neboloopsdk.AppItem
func enrichAppDetail(d *neboloopsdk.AppDetail, installed map[string]bool) types.StoreAppDetail
func enrichSkill(s neboloopsdk.SkillItem, installed map[string]bool) neboloopsdk.SkillItem
```
- Marks `IsInstalled` as true if the item's slug exists in the local installed set.
- `enrichAppDetail` additionally strips `ManifestURL`/`Manifest` from the frontend response (security).

---

## 3. NeboLoop Connection Code

**File:** `internal/handler/plugins/handler.go` (lines 628-746)

### 3.1 Route Map

| Method | Path | Handler | Description |
|--------|------|---------|-------------|
| POST | `/neboloop/connect` | `NeboLoopConnectHandler` | Redeem connection code, store MQTT credentials |
| GET | `/neboloop/status` | `NeboLoopStatusHandler` | Get current NeboLoop connection status |

### 3.2 Connect Flow (Code Redemption)

```go
func NeboLoopConnectHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```

**Request:**
```go
type NeboLoopConnectRequest struct {
    Code    string `json:"code"`
    Name    string `json:"name"`
    Purpose string `json:"purpose,omitempty"`
}
```

**Response:**
```go
type NeboLoopConnectResponse struct {
    BotID   string `json:"botId"`
    BotName string `json:"botName"`
    BotSlug string `json:"botSlug"`
    Message string `json:"message"`
}
```

**Logic:**

1. Parse request. Validate `Code` and `Name` are non-empty.
2. Verify `PluginStore` is initialized.
3. **Resolve API server:**
   - First try existing setting from `PluginStore.GetSettingsByName(ctx, "neboloop")["api_server"]`.
   - Fallback to `svcCtx.Config.NeboLoop.ApiURL`.
4. **Resolve bot_id (immutable UUID):**
   - Priority: `defaults.ReadBotID()` (file) -> `settings["bot_id"]` (DB) -> `uuid.New().String()` (generate).
   - If file doesn't have it, persist via `defaults.WriteBotID(botID)`.
5. **Redeem code:** Call `neboloopsdk.RedeemCode(ctx, apiServer, code, name, purpose, botID)`.
   - `purpose` defaults to `"AI companion"` if empty.
   - Bot ID is sent to the server so it registers the bot.
6. **Store connection settings:** Look up "neboloop" plugin, then call `PluginStore.UpdateSettings()` with `{"api_server": apiServer, "bot_id": botID}`.
7. Return response with `BotID`, `BotName`, `BotSlug`, and `"Connected to NeboLoop"`.

### 3.3 Status Check

```go
func NeboLoopStatusHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```

**Response:**
```go
type NeboLoopStatusResponse struct {
    Connected bool   `json:"connected"`
    BotID     string `json:"botId,omitempty"`
    BotName   string `json:"botName,omitempty"`
    APIServer string `json:"apiServer,omitempty"`
}
```

**Logic:**

1. If `PluginStore` is nil, return `{connected: false}`.
2. Load settings via `PluginStore.GetSettingsByName(ctx, "neboloop")`. On error, return `{connected: false}`.
3. `connected = botID != ""` (presence of bot_id means connected).
4. If connected, also try to get bot name from plugin's `DisplayName`.

---

## 4. User OAuth (Login/Signup)

Two separate subsystems handle user-facing OAuth:

### 4.1 OAuth Handler (Direct Callbacks)

**File:** `internal/oauth/handler.go`
**Package:** `oauth`
**Routes:** Registered at root level (not under `/api/v1/`)

```go
type Handler struct {
    svcCtx *svc.ServiceContext
}

func NewHandler(svcCtx *svc.ServiceContext) *Handler
func (h *Handler) RegisterRoutes(r chi.Router)
```

**Registered routes:**
| Method | Path | Handler Method |
|--------|------|----------------|
| GET | `/oauth/google/callback` | `googleCallback` -> `handleCallback(w, r, "google")` |
| GET | `/oauth/github/callback` | `githubCallback` -> `handleCallback(w, r, "github")` |

**Supported Providers:** Google, GitHub

### 4.2 Struct: OAuthUserInfo

```go
type OAuthUserInfo struct {
    ProviderUserID string
    Email          string
    Name           string
    AvatarURL      string
    AccessToken    string
    RefreshToken   string
}
```

### 4.3 Callback Handling (`handleCallback`)

**Full flow:**

1. **Guard checks:** `svcCtx.Config.IsOAuthEnabled()` and `svcCtx.UseLocal()`.
2. **Extract params:** `code`, `state`, `error` from query string.
3. **Error handling:** If `error` param present, redirect to `/login?error=...`.
4. **Provider dispatch:**
   - Google: Check `IsGoogleOAuthEnabled()`, call `exchangeGoogleCode(code)`.
   - GitHub: Check `IsGitHubOAuthEnabled()`, call `exchangeGitHubCode(code)`.
5. **User resolution (3-tier):**
   - **Tier 1: Existing OAuth connection.** Look up via `DB.Queries.GetOAuthConnectionByProvider(ctx, {provider, providerUserID})`. If found, use that `UserID`. Update the connection's email, name, avatar, tokens.
   - **Tier 2: Existing user by email (no OAuth connection yet).** Call `svcCtx.Auth.GetUserByEmail(ctx, email)`. If found, link the OAuth connection to this user.
   - **Tier 3: New user.** Call `DB.Queries.CreateUserFromOAuth()` with UUID, email, name, avatar. Also create user preferences via `DB.Queries.CreateUserPreferences()`. Create the OAuth connection.
6. **Token generation:** Call `svcCtx.Auth.GenerateTokensForUser(ctx, userID, email)`.
7. **Redirect:** `307 Temporary Redirect` to `/auth/callback?token=...&refresh=...&expires=...&new=...&state=...`.
8. **Error redirect:** On any failure, redirect to `/login?error=...`.

### 4.4 Google Code Exchange

```go
func (h *Handler) exchangeGoogleCode(code string) (*OAuthUserInfo, error)
```

1. **Resolve callback base:** `Config.OAuth.CallbackBaseURL` or fallback to `Config.App.BaseURL`.
2. **Token exchange:** POST to `https://oauth2.googleapis.com/token` with:
   - `client_id`, `client_secret`, `code`, `grant_type=authorization_code`, `redirect_uri=<base>/oauth/google/callback`.
3. **User info:** GET `https://www.googleapis.com/oauth2/v2/userinfo` with Bearer token.
4. Returns `OAuthUserInfo{ProviderUserID: id, Email, Name, AvatarURL: picture, AccessToken, RefreshToken}`.

### 4.5 GitHub Code Exchange

```go
func (h *Handler) exchangeGitHubCode(code string) (*OAuthUserInfo, error)
```

1. **Token exchange:** POST to `https://github.com/login/oauth/access_token` with `client_id`, `client_secret`, `code`. Headers: `Accept: application/json`.
2. **User info:** GET `https://api.github.com/user` with Bearer token.
3. **Email fallback:** If `email` is empty from the user endpoint, call `getGitHubPrimaryEmail(accessToken)`.
4. **Name fallback:** If `name` is empty, use `login`.
5. `ProviderUserID` = string of `userData.ID` (integer to string conversion).
6. No refresh token for GitHub (GitHub tokens don't expire the same way).

### 4.6 GitHub Primary Email Resolution

```go
func (h *Handler) getGitHubPrimaryEmail(accessToken string) (string, error)
```

1. GET `https://api.github.com/user/emails` with Bearer token.
2. Priority: primary + verified > any verified > error.

### 4.7 OAuth API Handlers (Protected Routes)

**File:** `internal/handler/oauth/`
**Package:** `oauth` (different from `internal/oauth`)
**Route prefix:** `/api/v1/`

| Method | Path | Handler | Auth Required |
|--------|------|---------|---------------|
| POST | `/oauth/{provider}/callback` | `OAuthCallbackHandler` | Yes (protected) |
| GET | `/oauth/{provider}/url` | `GetOAuthUrlHandler` | Yes (protected) |
| DELETE | `/oauth/{provider}` | `DisconnectOAuthHandler` | Yes (user-scoped) |
| GET | `/oauth/providers` | `ListOAuthProvidersHandler` | Yes (user-scoped) |

### 4.8 Get OAuth URL

```go
func GetOAuthUrlHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```

**Request:** `GetOAuthUrlRequest{Provider string, RedirectUrl string}`
**Response:** `GetOAuthUrlResponse{Url string, State string}`

1. Guard checks: `IsOAuthEnabled()`, `UseLocal()`.
2. Generate CSRF state: 16 random bytes, hex-encoded.
3. Resolve callback base URL.
4. Build authorization URL per provider:
   - **Google:** `https://accounts.google.com/o/oauth2/v2/auth` with scopes `openid email profile`, `access_type=offline`, `prompt=consent`.
   - **GitHub:** `https://github.com/login/oauth/authorize` with scope `user:email`.

### 4.9 Disconnect OAuth

```go
func DisconnectOAuthHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```

**Request:** `DisconnectOAuthRequest{Provider string}` (from path)

1. Guard checks.
2. Get user ID from JWT context.
3. Verify the OAuth connection exists for this user+provider.
4. **Safety check:** If user has no password AND this is their only OAuth connection, reject with `"cannot disconnect your only login method; please set a password first"`.
5. Delete the OAuth connection.

### 4.10 List OAuth Providers

```go
func ListOAuthProvidersHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```

**Response:** `ListOAuthProvidersResponse{Providers []OAuthProvider}`

```go
type OAuthProvider struct {
    Name      string `json:"name"`
    Connected bool   `json:"connected"`
    Email     string `json:"email,omitempty"`
}
```

1. Get user ID from JWT context.
2. List user's OAuth connections via `DB.Queries.ListUserOAuthConnections(ctx, userID)`.
3. Build map of `provider -> email`.
4. For each enabled provider (Google, GitHub), create an `OAuthProvider` entry with `Connected` and `Email`.

### 4.11 OAuth Callback (API -- Deprecated)

```go
func OAuthCallbackHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```

Returns an error telling the caller to use `/oauth/{provider}/callback` (the browser redirect route) instead. This endpoint exists only for API compatibility.

---

## 5. OAuth Broker (App OAuth)

**File:** `internal/oauth/broker/broker.go`
**File:** `internal/oauth/broker/providers.go`
**File:** `internal/oauth/broker/refresh.go`
**Package:** `broker`

This is a **separate OAuth system** from user login OAuth. It manages OAuth tokens for **Nebo apps** (not users). Apps request OAuth tokens to access third-party APIs on behalf of the Nebo owner.

### 5.1 Provider Configuration

```go
type OAuthProvider struct {
    Name                  string
    AuthorizationEndpoint string
    TokenEndpoint         string
    ClientID              string
    ClientSecret          string
    TenantID              string   // Microsoft only
    SupportsPKCE          bool
}
```

**Builtin providers (`BuiltinProviders()`):**

| Provider | Auth Endpoint | Token Endpoint | PKCE |
|----------|---------------|----------------|------|
| `google` | `https://accounts.google.com/o/oauth2/v2/auth` | `https://oauth2.googleapis.com/token` | Yes |
| `microsoft` | `https://login.microsoftonline.com/{tenant}/oauth2/v2/authorize` | `https://login.microsoftonline.com/{tenant}/oauth2/v2/token` | Yes |
| `github` | `https://github.com/login/oauth/authorize` | `https://github.com/login/oauth/access_token` | No |

Note: Client credentials are injected from `config.yaml` (under `AppOAuth` section), not hardcoded.

### 5.2 Broker Struct

```go
type Broker struct {
    db            *db.Store
    encryptionKey []byte
    providers     map[string]OAuthProvider
    appReceiver   AppTokenReceiver
    baseURL       string                    // e.g. "http://localhost:27895"
    httpClient    *http.Client              // 30s timeout
    mu            sync.RWMutex
}

type Config struct {
    DB            *db.Store
    EncryptionKey []byte
    BaseURL       string
    Providers     map[string]OAuthProvider
}

func New(cfg Config) *Broker
```

### 5.3 AppTokenReceiver Interface

```go
type AppTokenReceiver interface {
    PushOAuthTokens(appID, provider string, tokens map[string]string) error
}
```

Set after AppRegistry is initialized to avoid circular init:

```go
func (b *Broker) SetAppReceiver(receiver AppTokenReceiver)
```

### 5.4 Grant Struct (Public View)

```go
type Grant struct {
    Provider         string     `json:"provider"`
    Scopes           string     `json:"scopes"`
    ConnectionStatus string     `json:"connection_status"`
    ExpiresAt        *time.Time `json:"expires_at,omitempty"`
}
```

### 5.5 StartFlow -- Initiate OAuth for an App

```go
func (b *Broker) StartFlow(ctx context.Context, appID, providerName, scopes string) (string, error)
```

1. Look up provider from `b.providers` map. Error if not found.
2. Validate `ClientID` is configured. Error if empty with helpful message.
3. **Generate PKCE:** `mcpclient.GeneratePKCE()` returns `(verifier, challenge, error)`.
4. **Generate state:** `mcpclient.GenerateState()`.
5. **Encrypt verifier:** `credential.Encrypt(verifier)` before storing.
6. **Upsert grant:** `db.UpsertAppOAuthGrant()` with `connection_status = "pending"`.
7. **Build auth URL:**
   - Resolve endpoint (replace `{tenant}` for Microsoft, default `"common"`).
   - Set params: `response_type=code`, `client_id`, `redirect_uri`, `state`, `scope`, `access_type=offline`, `prompt=consent`.
   - If provider supports PKCE: add `code_challenge` and `code_challenge_method=S256`.
8. Return the full authorization URL.

**Redirect URI:** `<baseURL>/api/v1/apps/oauth/callback` (single callback for all app OAuth).

### 5.6 HandleCallback -- Process OAuth Return

```go
func (b *Broker) HandleCallback(ctx context.Context, state, code string) error
```

1. Look up grant by state: `db.GetAppOAuthGrantByState(ctx, state)`.
2. Look up provider from grant.
3. **Decrypt PKCE verifier:** `credential.Decrypt(grant.PkceVerifier)`.
4. **Token exchange:** POST to token endpoint with `grant_type=authorization_code`, `code`, `redirect_uri`, `client_id`, `client_secret` (if present), `code_verifier` (if present).
5. Parse response into `tokenResponse{AccessToken, TokenType, ExpiresIn, RefreshToken, Scope}`.
6. Call `storeAndPushTokens()`.

### 5.7 storeAndPushTokens

```go
func (b *Broker) storeAndPushTokens(ctx context.Context, appID, providerName string, tokenResp *tokenResponse, existingRefreshToken string) error
```

1. **Refresh token preservation:** If the new response has no refresh token, keep `existingRefreshToken`.
2. **Encrypt tokens:** Both access and refresh tokens encrypted via `credential.Encrypt()`.
3. **Calculate expiry:** `time.Now().Add(ExpiresIn * time.Second)`.
4. **Default token type:** `"Bearer"` if empty.
5. **Store in DB:** `db.UpdateAppOAuthTokens()` with encrypted values and expiry.
6. **Push to app:** If `appReceiver` is set, push **plaintext** tokens via `PushOAuthTokens()`:
   - Key format: `oauth:<provider>:access_token`, `oauth:<provider>:token_type`, `oauth:<provider>:expires_at`.
   - Expires_at formatted as RFC3339.
   - Errors logged but not returned.

### 5.8 GetGrants

```go
func (b *Broker) GetGrants(ctx context.Context, appID string) ([]Grant, error)
```

- Lists all grants for an app from DB.
- Returns public view (no tokens, just provider, scopes, status, expiry).

### 5.9 Disconnect

```go
func (b *Broker) Disconnect(ctx context.Context, appID, providerName string) error
```

1. Delete the grant from DB: `db.DeleteAppOAuthGrant()`.
2. Push empty tokens to the app so it knows the connection is gone:
   - `oauth:<provider>:access_token = ""`, `token_type = ""`, `expires_at = ""`.

### 5.10 PushExistingTokens -- On App Launch

```go
func (b *Broker) PushExistingTokens(ctx context.Context, appID string) error
```

Called when an app starts so it immediately has its OAuth tokens:

1. List all grants for the app.
2. Skip grants that aren't `"connected"` or have no access token.
3. Decrypt the access token.
4. Push via `receiver.PushOAuthTokens()` with the same key format.

### 5.11 Token Refresh Background Task

**File:** `internal/oauth/broker/refresh.go`

```go
func (b *Broker) StartRefreshLoop(ctx context.Context)
func (b *Broker) RefreshExpiring(ctx context.Context) error
func (b *Broker) refreshGrant(ctx context.Context, appID, providerName, encryptedRefreshToken string) error
```

**StartRefreshLoop:**
- Spawns a goroutine with a 60-second ticker.
- Calls `RefreshExpiring()` on each tick.
- Stops when context is cancelled.

**RefreshExpiring:**
- Queries `db.ListExpiringOAuthGrants(ctx, "5")` -- grants expiring within 5 minutes.
- For each, calls `refreshGrant()`. Errors are logged per-grant but don't stop the sweep.

**refreshGrant:**
1. Look up provider config.
2. Decrypt the encrypted refresh token via `mcpclient.DecryptString(encryptedRefreshToken, b.encryptionKey)`.
3. POST to token endpoint with `grant_type=refresh_token`, `refresh_token`, `client_id`, `client_secret`.
4. Parse response.
5. Call `storeAndPushTokens()` passing the old refresh token as `existingRefreshToken` (in case the provider doesn't return a new one).

---

## 6. App OAuth Handlers

**File:** `internal/handler/appoauth/handler.go`
**Package:** `appoauth`

Thin HTTP handler layer over the `broker.Broker`.

### 6.1 Route Map

| Method | Path | Handler | Description |
|--------|------|---------|-------------|
| GET | `/apps/{appId}/oauth/{provider}/connect` | `ConnectHandler` | Start OAuth flow for an app |
| GET | `/apps/oauth/callback` | `CallbackHandler` | OAuth callback (single endpoint) |
| GET | `/apps/{appId}/oauth/grants` | `GrantsHandler` | List OAuth grants for an app |
| DELETE | `/apps/{appId}/oauth/{provider}` | `DisconnectHandler` | Revoke OAuth grant |

### 6.2 Handler Signatures

```go
func ConnectHandler(b *broker.Broker) http.HandlerFunc
func CallbackHandler(b *broker.Broker) http.HandlerFunc
func GrantsHandler(b *broker.Broker) http.HandlerFunc
func DisconnectHandler(b *broker.Broker) http.HandlerFunc
```

Note: These take `*broker.Broker` directly, not `*svc.ServiceContext`. The broker is accessed via `svcCtx.OAuthBroker` at route registration time. If the broker is nil, these routes are not registered (conditional in server.go).

### 6.3 ConnectHandler

1. Extract `appId` and `provider` from path vars.
2. Read optional `scopes` query param.
3. Call `b.StartFlow(ctx, appID, providerName, scopes)`.
4. **HTTP 302 redirect** to the authorization URL (browser is redirected to the OAuth provider).

### 6.4 CallbackHandler

1. Extract `state` and `code` from query params.
2. If missing, check for `error` query param and return it.
3. Call `b.HandleCallback(ctx, state, code)`.
4. **On success:** Returns HTML that calls `window.close()` -- the OAuth popup window closes itself. Fallback text: "Connected successfully. You may close this window."
5. **On failure:** Returns HTTP 502 with error message.

### 6.5 GrantsHandler

1. Extract `appId` from path var.
2. Call `b.GetGrants(ctx, appID)`.
3. Return `{"grants": [...]}`.

### 6.6 DisconnectHandler

1. Extract `appId` and `provider` from path vars.
2. Call `b.Disconnect(ctx, appID, providerName)`.
3. Return `{"ok": true}`.

---

## 7. Developer Routes

**Files:**
- `internal/handler/dev/handler.go` -- main handlers
- `internal/handler/dev/grpc.go` -- gRPC stream inspection
- `internal/handler/dev/tools.go` -- tool listing and execution
- `internal/handler/dev/logs.go` -- log streaming

**Package:** `dev`
**Route prefix:** `/api/v1/dev/` (protected)

### 7.1 Route Map

| Method | Path | Handler | File | Description |
|--------|------|---------|------|-------------|
| POST | `/dev/sideload` | `SideloadHandler` | handler.go | Add project directory to dev workspace |
| DELETE | `/dev/sideload/{appId}` | `UnsideloadHandler` | handler.go | Remove sideloaded app |
| GET | `/dev/apps` | `ListDevAppsHandler` | handler.go | List all dev apps with status |
| POST | `/dev/apps/{appId}/relaunch` | `RelaunchDevAppHandler` | handler.go | Build and (re)launch a dev app |
| GET | `/dev/apps/{appId}/logs` | `LogStreamHandler` | logs.go | Stream logs via SSE |
| GET | `/dev/apps/{appId}/grpc` | `GrpcStreamHandler` | grpc.go | Stream gRPC traffic via SSE |
| GET | `/dev/apps/{appId}/context` | `ProjectContextHandler` | handler.go | Get full project context |
| GET | `/dev/tools` | `ListToolsHandler` | tools.go | List all registered tools |
| POST | `/dev/tools/execute` | `ToolExecuteHandler` | tools.go | Execute a tool directly |
| POST | `/dev/browse-directory` | `BrowseDirectoryHandler` | handler.go | Open native directory picker |
| POST | `/dev/open-window` | `OpenDevWindowHandler` | handler.go | Open dev window |

### 7.2 Types

```go
type SideloadRequest struct {
    Path string `json:"path"`
}

type SideloadResponse struct {
    AppID   string `json:"appId"`
    Name    string `json:"name"`
    Version string `json:"version"`
    Path    string `json:"path"`
}

type DevAppItem struct {
    AppID    string `json:"appId"`
    Name     string `json:"name"`
    Version  string `json:"version"`
    Path     string `json:"path"`
    Running  bool   `json:"running"`
    LoadedAt int64  `json:"loadedAt"`
}

type ListDevAppsResponse struct {
    Apps []DevAppItem `json:"apps"`
}

type ToolExecuteRequest struct {
    Tool  string          `json:"tool"`
    Input json.RawMessage `json:"input"`
}

type ToolExecuteResponse struct {
    Content string `json:"content"`
    IsError bool   `json:"isError"`
}

type ToolDefinitionItem struct {
    Name        string          `json:"name"`
    Description string          `json:"description"`
    Schema      json.RawMessage `json:"schema"`
}

type ListToolsResponse struct {
    Tools []ToolDefinitionItem `json:"tools"`
}

type BrowseDirectoryResponse struct {
    Path string `json:"path"`
}

type OpenDevWindowResponse struct {
    Opened bool `json:"opened"`
}

type ProjectContext struct {
    Path        string   `json:"path"`
    AppID       string   `json:"appId,omitempty"`
    Name        string   `json:"name,omitempty"`
    Version     string   `json:"version,omitempty"`
    Files       []string `json:"files"`
    ManifestRaw string   `json:"manifestRaw,omitempty"`
    HasMakefile bool     `json:"hasMakefile"`
    BinaryPath  string   `json:"binaryPath,omitempty"`
    Running     bool     `json:"running"`
    RecentLogs  string   `json:"recentLogs,omitempty"`
}
```

### 7.3 Helper: getRegistry

```go
func getRegistry(svcCtx *svc.ServiceContext) *apps.AppRegistry
```

- Calls `svcCtx.AppRegistry()` and type-asserts to `*apps.AppRegistry`.
- Returns nil if not available (agent not connected).
- Used by multiple handlers to check running state, sideload/unsideload, etc.

### 7.4 Helper: getToolRegistry

```go
func getToolRegistry(svcCtx *svc.ServiceContext) *tools.Registry
```

- Calls `svcCtx.ToolRegistry()` and type-asserts to `*tools.Registry`.
- Returns nil if agent not connected.

### 7.5 Sideload Flow

```go
func SideloadHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```

**What it does:** Registers a local project directory as a dev app. Does NOT build or launch.

1. Parse `SideloadRequest{Path}`.
2. **Validation:**
   - Path must be non-empty.
   - Path must exist (`os.Stat`).
   - Path must be a directory.
3. **Identity resolution:**
   - Try `apps.LoadManifest(req.Path)`. If manifest exists, use `manifest.ID` and `manifest.Name`.
   - Otherwise, use `filepath.Base(req.Path)` as both app ID and name.
4. **Persist:** Insert into `dev_sideloaded_apps` table via `queries.InsertDevSideloadedApp(ctx, {AppID, Path})`.
5. Return `SideloadResponse{AppID, Name, Path}`.

**Security:** No path restriction is enforced on the sideload path -- any directory the server process can access is accepted. The directory picker (BrowseDirectoryHandler) uses a native OS dialog, but the API itself accepts any path.

### 7.6 Unsideload

```go
func UnsideloadHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```

1. Extract `appId` from path.
2. Get AppRegistry (required -- returns 503 if unavailable).
3. Call `registry.Unsideload(appID)` to stop the process and remove the symlink.
4. Remove from `dev_sideloaded_apps` table.

### 7.7 List Dev Apps

```go
func ListDevAppsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```

1. Query `dev_sideloaded_apps` table.
2. For each row, build `DevAppItem`:
   - Base name from path.
   - Try loading manifest for name/version (may not exist for new projects).
   - Check running status via `registry.IsRunning(appID)`.
3. Return `ListDevAppsResponse{Apps}`.

### 7.8 Relaunch Dev App

```go
func RelaunchDevAppHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```

Handles both first launch and subsequent restarts.

1. Extract `appId`.
2. Get AppRegistry (required).
3. Look up project path from `dev_sideloaded_apps` table.
4. **Stop if running:** `registry.Unsideload(appID)`.
5. **Build and launch:** `registry.Sideload(ctx, row.Path)` -- this validates the manifest, runs `make build`, finds the binary, and launches it.
6. **Update DB:** If manifest ID changed (e.g., first build after scaffolding), delete old row and insert new one with the manifest ID.
7. Return `SideloadResponse{AppID, Name, Version, Path}`.

### 7.9 Project Context

```go
func ProjectContextHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```

Returns full project state for the Dev Assistant system prompt.

1. Look up project path from DB.
2. **File listing:** `listProjectFiles(projectPath)` -- top-level + one level deep. Skips hidden files (except `.gitignore`). Directories get trailing `/`.
3. **Manifest:** Read `manifest.json`, extract `id`, `name`, `version` fields.
4. **Makefile:** Check for existence of `Makefile`.
5. **Binary:** `apps.FindBinary(projectPath)`.
6. **Running status:** `registry.IsRunning(checkID)` (uses manifest appID if available).
7. **Recent logs:** `readRecentLogs(projectPath, 50)` -- last 50 lines from `logs/stderr.log` and `logs/stdout.log`.
8. Return full `ProjectContext` struct.

**`listProjectFiles` security:**
- Only reads top-level + one level deep (no recursive traversal).
- Skips all dotfiles except `.gitignore`.
- No path traversal risk since it only reads relative to the project directory.

**`readRecentLogs` behavior:**
- Reads from `<projectPath>/logs/stderr.log` and `<projectPath>/logs/stdout.log`.
- Takes last `maxLines` lines from each.
- Prefixes each file's content with `=== logs/stderr.log ===` header.
- Returns empty string if no logs exist.

### 7.10 Open Dev Window

```go
func OpenDevWindowHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```

1. Call `svcCtx.OpenDevWindow()` -- returns a `func()` or nil.
2. If nil, return 501 Not Implemented.
3. Call the function (opens native OS window).
4. Return `OpenDevWindowResponse{Opened: true}`.

### 7.11 Browse Directory

```go
func BrowseDirectoryHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```

1. Call `svcCtx.BrowseDirectory()` -- returns a `func() (string, error)` or nil.
2. If nil, return 501 Not Implemented.
3. Call the function (opens native OS directory picker).
4. If path is empty (user cancelled), return `BrowseDirectoryResponse{Path: ""}` (not an error).
5. Return the selected path.

**Security:** The directory browser uses the native OS file dialog, which inherently limits paths to what the user can see. No server-side path whitelist.

### 7.12 Dev Log Streaming (SSE)

**File:** `internal/handler/dev/logs.go`

```go
func LogStreamHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```

**Query params:**
- `stream`: `stdout` or `stderr` (default: `stdout`).
- `lines`: Not actually used (initial content comes from seeking to last ~32KB).

**Protocol:** Server-Sent Events (SSE).

**Flow:**

1. Validate `appId` and `stream` params.
2. Look up project path from `dev_sideloaded_apps` table.
3. Construct log path: `<projectPath>/logs/<stream>.log`.
4. Verify log file exists.
5. **Set SSE headers:** `Content-Type: text/event-stream`, `Cache-Control: no-cache`, `Connection: keep-alive`, `X-Accel-Buffering: no`.
6. **Initial content (backfill):**
   - If file > 32KB, seek to last 32KB and skip the first (partial) line.
   - Scan and send all remaining lines as `data: <line>\n\n`.
   - Flush.
7. **Tail (live):**
   - 200ms poll ticker.
   - On each tick, scan for new lines and send.
   - Continues until client disconnects (`r.Context().Done()`).

**Line format:** `data: <raw line text>\n\n` (standard SSE format, no event name, no ID).
**Buffer:** 64KB per line (handles long lines).

### 7.13 gRPC Stream Inspection (SSE)

**File:** `internal/handler/dev/grpc.go`

```go
func GrpcStreamHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```

**Protocol:** Server-Sent Events (SSE).
**Security restriction:** Only works for **sideloaded (dev) apps** -- production apps are never inspectable. Enforced via `registry.IsSideloaded(appID)`.

**Flow:**

1. Validate `appId`.
2. Get AppRegistry (required).
3. **Security check:** `registry.IsSideloaded(appID)` -- returns 403 if not a dev app.
4. Get inspector: `registry.Inspector()`.
5. **Set SSE headers** (same as log streaming).
6. **Backfill:** Send up to 200 recent events for this app via `ins.Recent(appID, 200)`.
7. **Live stream:**
   - Subscribe to inspector events: `ins.Subscribe()` returns `(chan event, unsubscribe func)`.
   - Filter events to only the requested app ID.
   - JSON-marshal each event and send as `data: <json>\n\n`.
   - Unsubscribe on disconnect.

### 7.14 Dev Tools Listing

**File:** `internal/handler/dev/tools.go`

```go
func ListToolsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```

1. Get tool registry via `getToolRegistry(svcCtx)`.
2. Call `reg.List()` to get all tool definitions.
3. Map each to `ToolDefinitionItem{Name, Description, Schema: d.InputSchema}`.
4. Return `ListToolsResponse{Tools}`.

### 7.15 Dev Tool Execution

```go
func ToolExecuteHandler(svcCtx *svc.ServiceContext) http.HandlerFunc
```

**Bypasses the agent** -- executes tools directly.

1. Parse `ToolExecuteRequest{Tool, Input}`.
2. Get tool registry.
3. Call `reg.Execute(ctx, &ai.ToolCall{ID: "dev-tool-test", Name: req.Tool, Input: req.Input})`.
4. Return `ToolExecuteResponse{Content: result.Content, IsError: result.IsError}`.

The tool call ID is hardcoded to `"dev-tool-test"` since this bypasses the normal agent loop.

---

## Appendix A: Cross-Cutting Concerns

### Secret Handling

| Layer | Secret Treatment |
|-------|-----------------|
| Plugin handler response | Masked with `"--------"` |
| PluginStore.GetSettings | Decrypted via `credential.Decrypt()` |
| PluginStore.UpdateSettings | Encrypted via `credential.Encrypt()` before DB write |
| OAuth Broker tokens | Encrypted via `credential.Encrypt()` before DB write |
| OAuth Broker PKCE verifier | Encrypted via `credential.Encrypt()` before DB write |
| OAuth Broker push to app | Plaintext (over local gRPC) |

### Error Response Patterns

All handlers use `httputil` helpers:
- `httputil.Error(w, err)` -- 400 with error message
- `httputil.BadRequest(w, msg)` -- 400
- `httputil.InternalError(w, msg)` -- 500
- `httputil.NotFound(w, msg)` -- 404
- `httputil.ErrorWithCode(w, code, msg)` -- custom status code

### Authentication

- Plugin, Store, and Dev routes are behind JWT auth middleware (registered in `registerProtectedRoutes`).
- OAuth callback routes at `/oauth/{provider}/callback` are registered as public routes (no auth needed -- the user is redirecting back from the OAuth provider).
- App OAuth routes are protected (require JWT).

### Conditional Route Registration

In `server.go`, app OAuth routes are only registered if `svcCtx.OAuthBroker` is non-nil:

```go
if svcCtx.OAuthBroker != nil {
    r.Get("/apps/{appId}/oauth/{provider}/connect", appoauth.ConnectHandler(svcCtx.OAuthBroker))
    r.Get("/apps/oauth/callback", appoauth.CallbackHandler(svcCtx.OAuthBroker))
    r.Get("/apps/{appId}/oauth/grants", appoauth.GrantsHandler(svcCtx.OAuthBroker))
    r.Delete("/apps/{appId}/oauth/{provider}", appoauth.DisconnectHandler(svcCtx.OAuthBroker))
}
```

---

## Appendix B: Database Tables Referenced

| Table | Used By | Purpose |
|-------|---------|---------|
| `plugin_registry` | Plugin handlers, Store | Plugin/app metadata, enabled state, connection status |
| `plugin_settings` | Plugin handlers, Store | Key-value settings per plugin (with secret flag) |
| `oauth_connections` | User OAuth handler | User OAuth provider links (Google, GitHub) |
| `users` | User OAuth handler | User accounts (created from OAuth) |
| `user_preferences` | User OAuth handler | Created alongside new OAuth users |
| `app_oauth_grants` | OAuth Broker | App-level OAuth grants with encrypted tokens |
| `dev_sideloaded_apps` | Dev handlers | Tracks sideloaded dev app paths |
| `auth_profiles` | NeboLoop client (indirect) | Stores OAuth JWT for NeboLoop API access |

---

## Appendix C: NeboLoop SDK Types Referenced

The plugin handler uses `neboloopsdk` (imported as `github.com/NeboLoop/neboloop-go-sdk`) with these types:

- `neboloopsdk.APIClient` -- HTTP client for NeboLoop API
- `neboloopsdk.AppItem` -- App listing item (has `Slug`, `Name`, `Version`, `IsInstalled`, etc.)
- `neboloopsdk.AppDetail` -- Full app detail (extends `AppItem` with `AgeRating`, `Platforms`, `Size`, `Language`, `Screenshots`, `Changelog`, `WebsiteURL`, `PrivacyURL`, `SupportURL`, `ManifestURL`, `Manifest`)
- `neboloopsdk.SkillItem` -- Skill listing item (has `Slug`, `Name`, `Version`, `Description`, `Icon`, `IsInstalled`)
- `neboloopsdk.SkillDetail` -- Full skill detail (has `Manifest` as `json.RawMessage`)
- `neboloopsdk.InstallResponse` -- Install result with `ID`, `App` (or `Skill`) item
- `neboloopsdk.Review` -- Single review
- `neboloopsdk.ChangelogEntry` -- Changelog entry
- `neboloopsdk.RedeemCode()` -- Static function for code redemption

SDK client methods used:
- `client.ListApps(ctx, query, category, page, pageSize)`
- `client.GetApp(ctx, id)`
- `client.GetAppReviews(ctx, id, page, pageSize)`
- `client.InstallApp(ctx, id)`
- `client.UninstallApp(ctx, id)`
- `client.ListSkills(ctx, query, category, page, pageSize)`
- `client.GetSkill(ctx, id)`
- `client.InstallSkill(ctx, id)`
- `client.UninstallSkill(ctx, id)`
- `client.APIServer()` -- returns the base URL
