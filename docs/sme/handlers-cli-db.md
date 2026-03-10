# Handlers, CLI Commands, and Database Layer -- Deep Dive

This document provides a comprehensive logic deep-dive covering every HTTP handler, CLI command, and database layer component in the Nebo Go codebase. It is intended as a subject-matter-expert reference for the Rust rewrite.

> **Rust implementation status (2026-03-10):**
> Handlers are in `crates/server/src/handlers/` with 18 modules: `agent`, `auth`, `chat`, `files`, `integrations`, `mcp_server`, `memory`, `neboloop`, `notification`, `provider`, `roles`, `setup`, `skills`, `tasks`, `user`, `workflows`, `ws`, `mod`. All use Axum extractors (`State`, `Path`, `Query`, `Json`).
>
> Key handler difference from Go as of commit 9f6dafc (2026-03-09):
> - Chat: `build_message_metadata()` added to reconstruct tool call/result metadata on refresh. Runs on `get_companion_chat`, `get_chat_history_by_day`, and `get_chat_messages`. It collects tool results from `role="tool"` messages and rebuilds `toolCalls` + `contentBlocks` metadata in-place on assistant messages.
> - Companion chat default message limit is 20 (Go used 200), configurable via `?limit=` query param.
> - Markdown-to-HTML rendering is NOT done server-side in Rust (handled by frontend).
> - `role="tool"` messages are NOT filtered out in Rust responses.

---

## Table of Contents

1. [HTTP Handlers](#http-handlers)
   - [Agent Handlers](#agent-handlers)
   - [Chat Handlers](#chat-handlers)
   - [Extensions Handlers](#extensions-handlers)
   - [Memory Handlers](#memory-handlers)
   - [Notification Handlers](#notification-handlers)
   - [Setup Handlers](#setup-handlers)
   - [User Handlers](#user-handlers)
   - [Tasks Handlers](#tasks-handlers)
   - [Integration Handlers](#integration-handlers)
   - [NeboLoop Handlers](#neboloop-handlers)
   - [Files Handler](#files-handler)
   - [Voice Handler](#voice-handler)
   - [Root Handlers](#root-handlers)
2. [CLI Commands](#cli-commands)
   - [vars.go -- Global Flags](#varsgo----global-flags)
   - [root.go -- RunAll](#rootgo----runall)
   - [agent.go -- Agent Loop](#agentgo----agent-loop)
   - [chat.go -- CLI Chat](#chatgo----cli-chat)
   - [config.go -- Configuration](#configgo----configuration)
   - [session.go -- Session Management](#sessiongo----session-management)
   - [message.go -- Channel Messaging](#messagego----channel-messaging)
   - [skills.go -- Skill Listing](#skillsgo----skill-listing)
   - [plugins.go -- Apps and Capabilities](#pluginsgo----apps-and-capabilities)
   - [providers.go -- Provider Loading](#providersgo----provider-loading)
   - [updates.go -- Background Updater](#updatesgo----background-updater)
   - [onboard.go -- Setup Wizard](#onboardgo----setup-wizard)
   - [doctor.go -- Diagnostics](#doctorgo----diagnostics)
3. [Database Layer](#database-layer)
   - [Connection Setup (sqlite.go)](#connection-setup-sqlitego)
   - [Store and Transactions (store.go)](#store-and-transactions-storego)
   - [Migration System (migrations/migrate.go)](#migration-system-migrationsmigratego)
   - [Session Manager (session_manager.go)](#session-manager-session_managergo)
   - [SQL Query Patterns](#sql-query-patterns)

---

## HTTP Handlers

All handlers follow the same pattern: accept `*svc.ServiceContext`, return `http.HandlerFunc`. Request parsing uses `httputil.Parse(r, &req)` for JSON body + path params + query params. Responses use `httputil.OkJSON(w, resp)` or `httputil.WriteJSON(w, status, resp)`. Errors use `httputil.Error(w, err)` or `httputil.ErrorWithCode(w, code, msg)`.

---

### Agent Handlers

Located in: `internal/handler/agent/`

#### GetHeartbeatHandler

- **Route:** `GET /api/v1/agent/heartbeat`
- **File:** `heartbeathandler.go`
- **Logic:**
  1. Resolves data directory via `defaults.DataDir()`
  2. Reads `HEARTBEAT.md` file from the data directory
  3. If file does not exist, returns empty content with 200
  4. If file exists, returns its contents
- **Response:** `{ "content": "<markdown string>" }`
- **Errors:** File read errors return 500

#### UpdateHeartbeatHandler

- **Route:** `PUT /api/v1/agent/heartbeat`
- **File:** `heartbeathandler.go`
- **Request:** `{ "content": "<markdown string>" }`
- **Logic:**
  1. Parses request body
  2. Resolves data directory
  3. Writes content to `HEARTBEAT.md` in the data directory (0644 permissions)
- **Response:** `{ "success": true }`
- **Errors:** Write failures return 500

#### GetSystemInfoHandler

- **Route:** `GET /api/v1/agent/system-info`
- **File:** `systeminfohandler.go`
- **Logic:**
  1. Gathers system information using Go `runtime` and `os` packages
  2. Returns OS (`runtime.GOOS`), architecture (`runtime.GOARCH`), hostname (`os.Hostname`), home directory (`os.UserHomeDir`), and username (`os/user.Current`)
- **Response:** `{ "os": "darwin", "arch": "arm64", "hostname": "...", "homeDir": "...", "username": "..." }`

#### GetAgentSettingsHandler

- **Route:** `GET /api/v1/agent/settings`
- **File:** `getagentsettingshandler.go`
- **Logic:**
  1. Reads settings from the local settings singleton (`local.GetAgentSettings()`)
  2. If settings unavailable, returns defaults
  3. Default heartbeat interval: 30 minutes
- **Response:** `{ "heartbeatIntervalMinutes": 30, "autonomousMode": false, ... }`

#### UpdateAgentSettingsHandler

- **Route:** `PUT /api/v1/agent/settings`
- **File:** `updateagentsettingshandler.go`
- **Request:** `{ "heartbeatIntervalMinutes": 15, "autonomousMode": true, ... }`
- **Logic:**
  1. Parses request body
  2. Validates heartbeat interval: must be between 1 and 1440 minutes
  3. Saves settings to local store via `local.GetAgentSettings().Update()`
  4. Broadcasts `settings_updated` frame to all connected agents via `AgentHub.BroadcastToAgents()`
- **Response:** `{ "success": true }`
- **Errors:** Invalid interval returns 400

#### GetAgentStatusHandler

- **Route:** `GET /api/v1/agent/status/{agentId}`
- **File:** `getagentstatushandler.go`
- **Logic:**
  1. Extracts `agentId` from path
  2. Looks up agent in `svcCtx.AgentHub.GetAgent(agentId)`
  3. Returns connected status and agent info
- **Response:** `{ "connected": true, "agentId": "...", "connectedAt": "..." }`
- **Errors:** Returns `connected: false` if agent not found (still 200)

#### GetSimpleAgentStatusHandler

- **Route:** `GET /api/v1/agent/status`
- **File:** `getsimpleagentstatushandler.go`
- **Logic:**
  1. Gets any connected agent from `svcCtx.AgentHub.GetAnyAgent()`
  2. Calculates uptime in seconds since connection
- **Response:** `{ "connected": true, "uptime": 3600 }` or `{ "connected": false }`

#### ListAgentsHandler

- **Route:** `GET /api/v1/agents`
- **File:** `listagentshandler.go`
- **Logic:**
  1. Gets all agents from `svcCtx.AgentHub.GetAllAgents()`
  2. Maps to response format with ID, connected status, and createdAt
- **Response:** `{ "agents": [{ "id": "...", "connected": true, "createdAt": "..." }] }`

#### ListAdvisorsHandler

- **Route:** `GET /api/v1/agent/advisors`
- **File:** `advisorshandler.go`
- **Logic:**
  1. Queries `ListAdvisors` from the database
  2. Returns array of advisor objects
- **Response:** `{ "advisors": [{ "name": "skeptic", "role": "critic", ... }] }`

#### GetAdvisorHandler

- **Route:** `GET /api/v1/agent/advisors/{name}`
- **File:** `advisorshandler.go`
- **Logic:**
  1. Extracts `name` from path
  2. Queries `GetAdvisorByName` from the database
- **Response:** Single advisor object
- **Errors:** 404 if not found

#### CreateAdvisorHandler

- **Route:** `POST /api/v1/agent/advisors`
- **File:** `advisorshandler.go`
- **Request:** `{ "name": "skeptic", "slug": "skeptic", "role": "critic", "description": "...", "prompt": "...", "priority": 10, "enabled": true }`
- **Logic:**
  1. Parses request
  2. Validates slug against regex `^[a-z][a-z0-9_-]{0,63}$`
  3. Generates UUID for advisor ID
  4. Inserts into database via `CreateAdvisor`
- **Response:** Created advisor object
- **Errors:** Invalid slug returns 400; duplicate name returns 409

#### UpdateAdvisorHandler

- **Route:** `PUT /api/v1/agent/advisors/{name}`
- **File:** `advisorshandler.go`
- **Request:** Partial object -- any subset of advisor fields
- **Logic:**
  1. Gets existing advisor by name
  2. Merges provided fields (only overwrites non-zero fields)
  3. Updates via `UpdateAdvisor`
- **Response:** Updated advisor object
- **Errors:** 404 if not found

#### DeleteAdvisorHandler

- **Route:** `DELETE /api/v1/agent/advisors/{name}`
- **File:** `advisorshandler.go`
- **Logic:**
  1. Gets advisor by name to verify existence
  2. Deletes via `DeleteAdvisor`
- **Response:** `{ "success": true }`
- **Errors:** 404 if not found

#### DeleteAgentSessionHandler

- **Route:** `DELETE /api/v1/agent/sessions/{id}`
- **File:** `deleteagentsessionhandler.go`
- **Logic:**
  1. Extracts session `id` from path
  2. Deletes session from database via `DeleteSession`
- **Response:** `{ "success": true }`

#### ListAgentSessionsHandler

- **Route:** `GET /api/v1/agent/sessions`
- **File:** `listagentsessionshandler.go`
- **Logic:**
  1. Queries `ListSessions` with limit=100, offset=0
  2. For each session, queries message count
  3. Maps to response format
- **Response:** `{ "sessions": [{ "id": "...", "key": "...", "scope": "agent", "messageCount": 42, ... }] }`

#### GetAgentSessionMessagesHandler

- **Route:** `GET /api/v1/agent/sessions/{id}/messages`
- **File:** `getagentsessionmessageshandler.go`
- **Logic:**
  1. Extracts session `id` from path
  2. Resolves session ID to session name (chat_id) via database lookup
  3. Reads messages from `chat_messages` table using the resolved chat_id
  4. Has fallback query if primary lookup fails
- **Response:** `{ "messages": [{ "role": "user", "content": "...", "timestamp": "..." }] }`

#### GetLanesHandler

- **Route:** `GET /api/v1/agent/lanes`
- **File:** `laneshandler.go`
- **Logic:**
  1. Gets any connected agent from hub
  2. If no agent, returns 503 Service Unavailable
  3. Sends a sync `get_lanes` request to the agent via hub with 5-second timeout
  4. Returns lane stats directly from agent response
- **Response:** `{ "main": { "active": 1, "queued": 0, "maxConcurrency": 1 }, ... }`
- **Errors:** 503 if no agent connected; 504 if timeout

#### GetLoopsHandler

- **Route:** `GET /api/v1/agent/loops`
- **File:** `loopshandler.go`
- **Logic:**
  1. Gets any connected agent from hub
  2. Sends sync `get_loops` request to agent via hub with 5-second timeout
  3. Returns loops with their channels
- **Response:** `{ "loops": [{ "id": "...", "name": "...", "channels": [...] }] }`
- **Errors:** 503 if no agent; 504 if timeout

#### GetChannelMessagesHandler

- **Route:** `GET /api/v1/agent/channels/{channelId}/messages`
- **File:** `loopshandler.go`
- **Logic:**
  1. Extracts `channelId` from path
  2. Reads `limit` query param (default: 50)
  3. Sends sync `get_channel_messages` request to agent via hub
  4. Returns rendered messages with HTML
- **Response:** `{ "messages": [{ "content": "...", "contentHtml": "...", "from": "..." }], "members": [...] }`

#### SendChannelMessageHandler

- **Route:** `POST /api/v1/agent/channels/{channelId}/messages`
- **File:** `loopshandler.go`
- **Request:** `{ "text": "Hello" }`
- **Logic:**
  1. Extracts `channelId` from path
  2. Sends sync `send_channel_message` request to agent via hub
  3. Marks message as human-injected so gateway attributes it to owner, not bot
- **Response:** `{ "success": true }`

#### GetAgentProfileHandler

- **Route:** `GET /api/v1/agent/profile`
- **File:** `profilehandler.go`
- **Logic:**
  1. Queries `GetAgentProfile` from database (singleton row, id=1)
  2. If not found, creates with defaults via `EnsureAgentProfileExists`
  3. Returns profile with 15+ nullable fields
- **Response:** `{ "name": "Nebo", "role": "assistant", "personality": "helpful", ... }`

#### UpdateAgentProfileHandler

- **Route:** `PUT /api/v1/agent/profile`
- **File:** `profilehandler.go`
- **Request:** Partial profile object (15 nullable fields: name, role, personality, quietHoursStart, quietHoursEnd, bio, interests, communication_style, proactivity_level, formality_level, humor_level, expertise_areas, boundaries, avatar_url, soul_override)
- **Logic:**
  1. Gets existing profile
  2. Merges non-zero fields from request
  3. Updates via `UpdateAgentProfile`
  4. Fire-and-forget sync to NeboLoop identity API via `syncIdentityToNeboLoop()` (name + role only)
- **Response:** Updated profile object

#### ListPersonalityPresetsHandler

- **Route:** `GET /api/v1/agent/personality-presets`
- **File:** `profilehandler.go`
- **Logic:**
  1. Queries `ListPersonalityPresets` from database
  2. Returns array of preset objects
- **Response:** `{ "presets": [{ "name": "Professional", "personality": "...", ... }] }`

---

### Chat Handlers

Located in: `internal/handler/chat/`

**Key concept: Single Bot Paradigm** -- Nebo has ONE companion chat, not multiple independent chats. The companion chat is the primary conversation surface shared between web UI, DMs, and CLI.

#### CreateChatHandler

- **Route:** `POST /api/v1/chats`
- **File:** `createchathandler.go`
- **Logic:**
  1. **Single Bot Paradigm:** Always returns the companion chat
  2. Calls `GetOrCreateCompanionChat` with user ID `"companion-default"`
  3. If chat does not exist, creates it with a new UUID
  4. Uses `ON CONFLICT` SQL pattern for atomic upsert
- **Response:** `{ "id": "...", "title": "Chat", "createdAt": "..." }`

#### DeleteChatHandler

- **Route:** `DELETE /api/v1/chats/{id}`
- **File:** `deletechathandler.go`
- **Logic:**
  1. Extracts chat `id` from path
  2. Deletes chat via `DeleteChat` (cascade removes messages)
- **Response:** `{ "success": true }`

#### GetChatHandler

- **Route:** `GET /api/v1/chats/{id}`
- **File:** `getchathandler.go`
- **Logic:**
  1. Gets chat by ID
  2. Loads all messages for the chat
  3. Renders markdown to HTML via `markdown.Render()` for each message
- **Response:** `{ "id": "...", "title": "...", "messages": [{ "content": "...", "contentHtml": "..." }] }`

#### GetCompanionChatHandler

- **Route:** `GET /api/v1/chats/companion`
- **File:** `getcompanionchathandler.go`
- **Constants:** `companionUserIDFallback = "companion-default"`, `defaultContextMessageLimit = 200`
- **Logic:**
  1. Gets or creates companion chat for `"companion-default"` user
  2. Loads up to 200 messages
  3. Builds a tool result map: maps `tool_call_id` to tool result content
  4. Filters out messages with `role = "tool"` from the response
  5. Reconstructs metadata for frontend: converts `toolCalls` and `contentBlocks` format
  6. Renders markdown to HTML for all messages
  7. Returns the chat with processed messages
- **Response:** Full chat object with messages, tool results mapped into toolCalls metadata
- **Key detail:** This handler is the primary entry point for the web UI chat. The tool result mapping is critical for the frontend to display tool call/result pairs correctly.

#### GetHistoryByDayHandler

- **Route:** `GET /api/v1/chats/{id}/history/{day}`
- **File:** `gethistorybydayhandler.go`
- **Logic:**
  1. Extracts chat `id` and `day` (YYYY-MM-DD format) from path
  2. Queries `GetMessagesByDay` using the `day_marker` column
  3. Renders markdown to HTML
- **Response:** `{ "messages": [...], "day": "2026-01-15" }`

#### ListChatDaysHandler

- **Route:** `GET /api/v1/chats/{id}/days`
- **File:** `listchatdayshandler.go`
- **Query params:** `page` (default 1), `pageSize` (default 30)
- **Logic:**
  1. Queries `ListChatDays` with pagination
  2. Returns distinct days with message counts
- **Response:** `{ "days": [{ "day": "2026-01-15", "messageCount": 12 }], "total": 45 }`

#### ListChatsHandler

- **Route:** `GET /api/v1/chats`
- **File:** `listchatshandler.go`
- **Logic:**
  1. **Single Bot Paradigm:** Returns only the companion chat
  2. Gets or creates companion chat for `"companion-default"`
- **Response:** `{ "chats": [{ "id": "...", "title": "Chat" }] }`

#### SearchChatMessagesHandler

- **Route:** `GET /api/v1/chats/search`
- **File:** `searchchatmessageshandler.go`
- **Query params:** `q` (search query), `page` (default 1), `pageSize` (default 20)
- **Logic:**
  1. Searches messages in the companion chat using `LIKE '%query%'` pattern
  2. Applies pagination
  3. Renders markdown to HTML
- **Response:** `{ "messages": [...], "total": 5, "page": 1, "pageSize": 20 }`

#### SendMessageHandler

- **Route:** `POST /api/v1/chats/{id}/messages`
- **File:** `sendmessagehandler.go`
- **Request:** `{ "content": "Hello", "role": "user" }`
- **Logic:**
  1. If no `chatId` in path, creates a new chat first
  2. Generates title from first 50 characters of content
  3. Creates message with UUID, role, content
  4. Updates chat's `updated_at` timestamp
- **Response:** `{ "id": "...", "role": "user", "content": "...", "createdAt": "..." }`

#### UpdateChatHandler

- **Route:** `PUT /api/v1/chats/{id}`
- **File:** `updatechathandler.go`
- **Request:** `{ "title": "New Title" }`
- **Logic:**
  1. Extracts chat `id` from path
  2. Updates chat title via `UpdateChat`
- **Response:** Updated chat object

---

### Extensions Handlers

Located in: `internal/handler/extensions/`

#### ListExtensionsHandler

- **Route:** `GET /api/v1/extensions`
- **File:** `listextensionshandler.go`
- **Logic:**
  1. Builds a hardcoded list of built-in tools (file, shell, web, agent/bot, event, app, loop, msg)
  2. Loads bundled skills from embedded `extensions/skills/` directory
  3. Loads user skills from `<data_dir>/skills/` directory
  4. User skills override bundled skills on name collision
  5. Loads plugin tools from `svcCtx.PluginStore` (app platform tools)
  6. Loads channel plugins
  7. Merges enabled/disabled state from `svcCtx.SkillSettings`
- **Response:** `{ "tools": [...], "skills": [...], "channels": [...] }`

#### GetSkillHandler

- **Route:** `GET /api/v1/extensions/skills/{name}`
- **File:** `getskillhandler.go`
- **Logic:**
  1. Extracts skill `name` from path
  2. Searches user skills directory first (priority)
  3. Falls back to bundled skills
- **Response:** `{ "name": "...", "description": "...", "template": "...", "triggers": [...] }`
- **Errors:** 404 if not found in either location

#### CreateSkillHandler

- **Route:** `POST /api/v1/extensions/skills`
- **File:** `skillhandlers.go`
- **Request:** `{ "name": "my-skill", "content": "<SKILL.md content>" }`
- **Logic:**
  1. Validates that content contains `SKILL.md` YAML frontmatter
  2. Derives slug from name
  3. Checks that slug does not already exist
  4. Creates directory at `<data_dir>/skills/<slug>/`
  5. Writes `SKILL.md` file to the directory
- **Response:** `{ "name": "my-skill", "slug": "my-skill" }`
- **Errors:** Invalid content returns 400; duplicate slug returns 409

#### UpdateSkillHandler

- **Route:** `PUT /api/v1/extensions/skills/{name}`
- **File:** `skillhandlers.go`
- **Request:** `{ "content": "<updated SKILL.md content>" }`
- **Logic:**
  1. **Forbids editing bundled skills** -- returns 403
  2. Validates content
  3. Overwrites `SKILL.md` in the user skills directory
- **Response:** Updated skill object
- **Errors:** 403 if bundled; 404 if not found

#### DeleteSkillHandler

- **Route:** `DELETE /api/v1/extensions/skills/{name}`
- **File:** `skillhandlers.go`
- **Logic:**
  1. **Forbids deleting bundled skills** -- returns 403
  2. Removes the entire skill directory from user skills
- **Response:** `{ "success": true }`
- **Errors:** 403 if bundled; 404 if not found

#### GetSkillContentHandler

- **Route:** `GET /api/v1/extensions/skills/{name}/content`
- **File:** `skillhandlers.go`
- **Logic:**
  1. Searches user skills first, then bundled
  2. Reads and returns raw `SKILL.md` file content
- **Response:** `{ "content": "<raw SKILL.md content>" }`

#### ToggleSkillHandler

- **Route:** `POST /api/v1/extensions/skills/{name}/toggle`
- **File:** `toggleskillhandler.go`
- **Request:** `{ "enabled": true }`
- **Logic:**
  1. Extracts skill `name` from path
  2. Toggles via `svcCtx.SkillSettings.Toggle(name)`
  3. The toggle fires an `OnChange` callback that propagates to the agent
- **Response:** `{ "success": true, "enabled": true }`

---

### Memory Handlers

Located in: `internal/handler/memory/handlers.go` (single file, 6 handlers)

#### ListMemoriesHandler

- **Route:** `GET /api/v1/memories`
- **Query params:** `page` (default 1), `pageSize` (default 20, max 100), `namespace` (optional filter)
- **Logic:**
  1. Queries `ListMemories` with pagination
  2. Optionally filters by namespace
- **Response:** `{ "memories": [...], "total": 100, "page": 1, "pageSize": 20 }`

#### GetMemoryHandler

- **Route:** `GET /api/v1/memories/{id}`
- **Logic:**
  1. Gets memory by ID
  2. **Increments access count** via `IncrementMemoryAccessCount`
- **Response:** Single memory object

#### UpdateMemoryHandler

- **Route:** `PUT /api/v1/memories/{id}`
- **Request:** `{ "value": "updated content", "tags": "tag1,tag2" }`
- **Logic:**
  1. Gets existing memory
  2. Updates value and tags
  3. Updates `updated_at` timestamp
- **Response:** Updated memory object

#### DeleteMemoryHandler

- **Route:** `DELETE /api/v1/memories/{id}`
- **Logic:** Deletes memory by ID
- **Response:** `{ "success": true }`

#### SearchMemoriesHandler

- **Route:** `GET /api/v1/memories/search`
- **Query params:** `q` (search query), `page`, `pageSize` (default 20)
- **Logic:**
  1. Searches across key, value, and tags columns using `LIKE '%query%'`
  2. Returns paginated results
- **Response:** `{ "memories": [...], "total": 5 }`

#### GetMemoryStatsHandler

- **Route:** `GET /api/v1/memories/stats`
- **Logic:**
  1. Counts total memories
  2. Counts memories per layer (tacit, daily, entity)
  3. Lists distinct namespaces
- **Response:** `{ "total": 150, "byLayer": { "tacit": 80, "daily": 50, "entity": 20 }, "namespaces": ["user", "system"] }`

---

### Notification Handlers

Located in: `internal/handler/notification/`

All notification handlers check two guards before proceeding:
1. `svcCtx.Config.IsNotificationsEnabled()` -- returns 404 if disabled
2. `svcCtx.UseLocal()` -- ensures local mode is active

The `userID` is typically `"default-user"` in single-user mode.

#### ListNotificationsHandler

- **Route:** `GET /api/v1/notifications`
- **Query params:** `page` (default 1), `pageSize` (default 20, max 100), `unreadOnly` (optional boolean)
- **Logic:**
  1. Lists notifications for user with pagination
  2. Optionally filters to unread only
- **Response:** `{ "notifications": [...], "total": 10, "page": 1, "pageSize": 20, "unreadCount": 3 }`

#### GetUnreadCountHandler

- **Route:** `GET /api/v1/notifications/unread-count`
- **Logic:** Counts unread notifications for user
- **Response:** `{ "count": 3 }`

#### MarkNotificationReadHandler

- **Route:** `PUT /api/v1/notifications/{id}/read`
- **Logic:** Marks single notification as read by ID + userID
- **Response:** `{ "success": true }`

#### MarkAllNotificationsReadHandler

- **Route:** `PUT /api/v1/notifications/read-all`
- **Logic:** Marks all notifications as read for userID
- **Response:** `{ "success": true }`

#### DeleteNotificationHandler

- **Route:** `DELETE /api/v1/notifications/{id}`
- **Logic:** Deletes notification by ID + userID
- **Response:** `{ "success": true }`

---

### Setup Handlers

Located in: `internal/handler/setup/`

#### SetupStatusHandler

- **Route:** `GET /api/v1/setup/status`
- **File:** `setupstatushandler.go`
- **Logic:**
  1. Checks if admin user exists via `svcCtx.Auth.GetAdminCount()`
  2. Checks if `.setup-complete` file exists via `defaults.IsSetupComplete()`
  3. Returns combined status
- **Response:** `{ "setupRequired": true, "hasAdmin": false, "setupComplete": false }`

#### CreateAdminHandler

- **Route:** `POST /api/v1/setup/admin`
- **File:** `createadminhandler.go`
- **Request:** `{ "email": "admin@example.com", "password": "...", "name": "Admin" }`
- **Logic:**
  1. Checks if admin already exists -- returns 400 if so
  2. Checks if email is already in use
  3. Hashes password with bcrypt
  4. Creates user with `role = "admin"`
  5. Creates default user preferences
  6. Generates JWT access token + refresh token
- **Response:** `{ "user": { "id": "...", "email": "...", "name": "..." }, "accessToken": "...", "refreshToken": "..." }`
- **Errors:** 400 if admin exists or email taken

#### GetPersonalityHandler

- **Route:** `GET /api/v1/setup/personality`
- **File:** `getpersonalityhandler.go`
- **Logic:**
  1. Reads `SOUL.md` from data directory
  2. If file does not exist, falls back to embedded default personality
- **Response:** `{ "content": "<SOUL.md content>" }`

#### UpdatePersonalityHandler

- **Route:** `PUT /api/v1/setup/personality`
- **File:** `updatepersonalityhandler.go`
- **Request:** `{ "content": "<new SOUL.md content>" }`
- **Logic:** Writes content to `SOUL.md` in data directory
- **Response:** `{ "success": true }`

#### CompleteSetupHandler

- **Route:** `POST /api/v1/setup/complete`
- **File:** `completesetuphandler.go`
- **Logic:** Creates `.setup-complete` marker file via `defaults.MarkSetupComplete()`
- **Response:** `{ "success": true }`

---

### User Handlers

Located in: `internal/handler/user/`

#### GetCurrentUserHandler

- **Route:** `GET /api/v1/users/me`
- **File:** `getcurrentuserhandler.go`
- **Logic:**
  1. Extracts email from JWT context via `auth.GetEmailFromContext(ctx)`
  2. Looks up user by email via `svcCtx.Auth.GetUserByEmail()`
- **Response:** `{ "user": { "id": "...", "email": "...", "name": "...", "createdAt": "..." } }`

#### UpdateCurrentUserHandler

- **Route:** `PUT /api/v1/users/me`
- **File:** `updatecurrentuserhandler.go`
- **Request:** `{ "name": "New Name" }`
- **Logic:**
  1. Gets current user from JWT context
  2. Updates user name (only if non-empty in request)
  3. Saves via `svcCtx.Auth.UpdateUser()`
- **Response:** Updated user object

#### GetPreferencesHandler

- **Route:** `GET /api/v1/users/me/preferences`
- **File:** `getpreferenceshandler.go`
- **Logic:** Returns hardcoded defaults
- **Response:** `{ "theme": "system", "language": "en", "notifications": true, "soundEnabled": true }`
- **Note:** No persistence -- always returns defaults

#### UpdatePreferencesHandler

- **Route:** `PUT /api/v1/users/me/preferences`
- **File:** `updatepreferenceshandler.go`
- **Request:** Preferences object
- **Logic:** Echoes back the submitted preferences (no actual persistence)
- **Response:** The submitted preferences
- **Note:** This is effectively a no-op that just validates the request format

#### ChangePasswordHandler

- **Route:** `PUT /api/v1/users/me/password`
- **File:** `changepasswordhandler.go`
- **Request:** `{ "currentPassword": "...", "newPassword": "..." }`
- **Logic:** Delegates to `svcCtx.Auth.ChangePassword()`
- **Response:** `{ "success": true }`
- **Errors:** 401 if current password wrong; 400 if new password invalid

#### DeleteAccountHandler

- **Route:** `DELETE /api/v1/users/me`
- **File:** `deleteaccounthandler.go`
- **Request:** `{ "password": "..." }`
- **Logic:**
  1. Verifies password by calling `svcCtx.Auth.Login(email, password)`
  2. If login succeeds, deletes user via `svcCtx.Auth.DeleteUser()`
- **Response:** `{ "success": true }`
- **Errors:** 401 if password wrong

#### GetToolPermissionsHandler

- **Route:** `GET /api/v1/users/me/permissions`
- **File:** `permissionshandler.go`
- **Logic:**
  1. Reads `tool_permissions` JSON from `user_profiles` table
  2. Uses `defaultUserID = "default-user"` in single-user mode
  3. If no profile exists, returns default permissions
- **Default permissions:** `{ "chat": true, "file": true, "web": true, "desktop": true, "system": true, "shell": false, "contacts": false, "media": false }`
- **Response:** `{ "permissions": { ... } }`

#### UpdateToolPermissionsHandler

- **Route:** `PUT /api/v1/users/me/permissions`
- **File:** `permissionshandler.go`
- **Request:** `{ "permissions": { "shell": true, "contacts": true } }`
- **Logic:**
  1. Ensures default user exists via `ensureDefaultUserExists()`
  2. Serializes permissions to JSON
  3. Updates `user_profiles.tool_permissions` column
- **Response:** `{ "success": true }`

#### AcceptTermsHandler

- **Route:** `POST /api/v1/users/me/accept-terms`
- **File:** `permissionshandler.go`
- **Logic:**
  1. Ensures default user exists
  2. Sets `terms_accepted_at` to current timestamp on user profile
- **Response:** `{ "success": true }`

#### GetUserProfileHandler

- **Route:** `GET /api/v1/users/me/profile`
- **File:** `profilehandler.go`
- **Logic:**
  1. Queries `user_profiles` table for `defaultUserID`
  2. If no profile exists, creates one with `ensureDefaultUserExists()`
- **Response:** Profile object with name, bio, avatar, preferences JSON

#### UpdateUserProfileHandler

- **Route:** `PUT /api/v1/users/me/profile`
- **File:** `profilehandler.go`
- **Request:** Profile fields (name, bio, avatar_url, etc.)
- **Logic:**
  1. Ensures default user exists
  2. Upserts profile data
- **Response:** Updated profile object

---

### Tasks Handlers

Located in: `internal/handler/tasks/handlers.go` (single file, 8 handlers)

All task handlers interact with the `svcCtx.Scheduler` interface, which wraps `SchedulerManager` (cron system).

#### ListTasksHandler

- **Route:** `GET /api/v1/tasks`
- **Query params:** `page` (default 1), `pageSize` (default 20, max 100)
- **Logic:** Lists all cron jobs with pagination via `Scheduler.ListJobs()`
- **Response:** `{ "tasks": [...], "total": 5 }`

#### GetTaskHandler

- **Route:** `GET /api/v1/tasks/{name}`
- **Logic:** Gets task by name via `Scheduler.GetJob(name)`
- **Response:** Single task object
- **Errors:** 404 if not found

#### CreateTaskHandler

- **Route:** `POST /api/v1/tasks`
- **Request:** `{ "name": "daily-report", "schedule": "0 9 * * *", "message": "...", "instructions": "..." }`
- **Logic:**
  1. Validates: `name` and `schedule` are required
  2. Creates via `Scheduler.CreateJob()`
- **Response:** Created task object
- **Errors:** 400 if missing required fields; 409 if name exists

#### UpdateTaskHandler

- **Route:** `PUT /api/v1/tasks/{name}`
- **Request:** Partial task object
- **Logic:** Partial merge update via `Scheduler.UpdateJob()`
- **Response:** Updated task object

#### DeleteTaskHandler

- **Route:** `DELETE /api/v1/tasks/{name}`
- **Logic:** Deletes by name via `Scheduler.DeleteJob(name)`
- **Response:** `{ "success": true }`

#### ToggleTaskHandler

- **Route:** `POST /api/v1/tasks/{name}/toggle`
- **Request:** `{ "enabled": true }`
- **Logic:** Toggles enabled state via `Scheduler.ToggleJob(name, enabled)`
- **Response:** `{ "success": true, "enabled": true }`

#### RunTaskHandler

- **Route:** `POST /api/v1/tasks/{name}/run`
- **Logic:** Triggers immediate execution via `Scheduler.RunJob(name)`
- **Response:** `{ "success": true }`

#### ListTaskHistoryHandler

- **Route:** `GET /api/v1/tasks/{name}/history`
- **Query params:** `page`, `pageSize` (default 20)
- **Logic:** Lists execution history for task via `Scheduler.ListHistory(name)`
- **Response:** `{ "history": [{ "startedAt": "...", "completedAt": "...", "status": "success", "output": "..." }] }`

---

### Integration Handlers

Located in: `internal/handler/integration/`

#### ListIntegrationsHandler

- **Route:** `GET /api/v1/integrations`
- **File:** `handler.go`
- **Logic:**
  1. Queries `ListMCPIntegrations` from database
  2. Maps to response format (strips encrypted credentials)
- **Response:** `{ "integrations": [{ "id": "...", "name": "...", "url": "...", "toolCount": 5, "status": "connected" }] }`

#### ListServerRegistryHandler

- **Route:** `GET /api/v1/integrations/registry`
- **File:** `handler.go`
- **Logic:**
  1. Queries `ListMCPServerRegistry` from database
  2. Returns known MCP server definitions with their URLs and descriptions
- **Response:** `{ "servers": [...] }`

#### GetIntegrationHandler

- **Route:** `GET /api/v1/integrations/{id}`
- **File:** `handler.go`
- **Logic:** Gets single integration by ID
- **Response:** Integration object

#### CreateIntegrationHandler

- **Route:** `POST /api/v1/integrations`
- **File:** `handler.go`
- **Request:** `{ "url": "https://mcp-server.example.com", "apiKey": "sk-..." }`
- **Logic:**
  1. Derives `serverType` and `name` from URL (e.g., "github" from github.com)
  2. Encrypts API key via `credential.Encrypt()`
  3. Generates UUID for integration ID
  4. Inserts into database via `CreateMCPIntegration`
- **Response:** Created integration object
- **Errors:** 400 if URL invalid

#### UpdateIntegrationHandler

- **Route:** `PUT /api/v1/integrations/{id}`
- **File:** `handler.go`
- **Request:** Partial integration object
- **Logic:**
  1. Gets existing integration
  2. Partial merge of provided fields
  3. Re-encrypts API key if changed
  4. Updates via `UpdateMCPIntegration`
- **Response:** Updated integration object

#### DeleteIntegrationHandler

- **Route:** `DELETE /api/v1/integrations/{id}`
- **File:** `handler.go`
- **Logic:**
  1. Deletes stored credentials
  2. Deletes integration record
- **Response:** `{ "success": true }`

#### TestIntegrationHandler

- **Route:** `POST /api/v1/integrations/{id}/test`
- **File:** `handler.go`
- **Logic:**
  1. Gets integration by ID
  2. Decrypts API key
  3. Creates MCP client and calls `ListTools()` to verify connectivity
  4. Updates integration's `connection_status` and `tool_count` in database
  5. Broadcasts `integrations_changed` event to agents (triggers MCP bridge re-sync)
- **Response:** `{ "connected": true, "toolCount": 8, "tools": ["tool1", "tool2"] }`
- **Errors:** Returns `connected: false` with error message on failure

#### MCP OAuth Handlers

- **File:** `oauth.go`
- **Routes:**
  - `GET /api/v1/integrations/{id}/oauth/start` -- Initiates MCP OAuth flow
  - `GET /api/v1/integrations/oauth/callback` -- Handles OAuth callback
- **Logic:** Standard OAuth authorization code flow for MCP servers that require OAuth authentication

---

### NeboLoop Handlers

Located in: `internal/handler/neboloop/`

#### RegisterHandler

- **Route:** `POST /api/v1/neboloop/register`
- **File:** `handlers.go`
- **Request:** `{ "email": "...", "password": "...", "name": "..." }`
- **Logic:**
  1. Proxies registration request to NeboLoop API
  2. On success, stores NeboLoop credentials in `auth_profiles` table via `storeNeboLoopProfile()`
  3. `storeNeboLoopProfile()` maintains exactly ONE active neboloop profile (deactivates old ones)
  4. Activates NeboLoop comm via `activateNeboLoopComm()` (persists settings + broadcasts to agent)
- **Response:** `{ "success": true, "user": { ... } }`

#### LoginHandler

- **Route:** `POST /api/v1/neboloop/login`
- **File:** `handlers.go`
- **Request:** `{ "email": "...", "password": "..." }`
- **Logic:** Same flow as Register -- proxies to NeboLoop API, stores profile, activates comm
- **Response:** `{ "success": true, "token": "..." }`

#### AccountStatusHandler

- **Route:** `GET /api/v1/neboloop/status`
- **File:** `handlers.go`
- **Logic:**
  1. Checks `auth_profiles` table for active `provider = "neboloop"` profiles
  2. Returns connected status and account info
- **Response:** `{ "connected": true, "email": "...", "plan": "pro" }`

#### DisconnectHandler

- **Route:** `POST /api/v1/neboloop/disconnect`
- **File:** `handlers.go`
- **Logic:**
  1. Deactivates all neboloop auth profiles
  2. Clears stored token
  3. Broadcasts `settings_updated` to agent (triggers comm plugin disconnect)
- **Response:** `{ "success": true }`

#### OpenHandler

- **Route:** `POST /api/v1/neboloop/open`
- **File:** `handlers.go`
- **Request:** `{ "url": "https://neboloop.com/..." }`
- **Logic:** Opens the specified URL in the default system browser
- **Response:** `{ "success": true }`

#### JanusUsageHandler

- **Route:** `GET /api/v1/neboloop/janus/usage`
- **File:** `handlers.go`
- **Logic:**
  1. Reads Janus rate limit info from `svcCtx.JanusUsage` (in-memory, persisted to `janus_usage.json`)
  2. Returns session and weekly token usage
- **Response:** `{ "sessionTokens": 1500, "weeklyTokens": 50000, "weeklyLimit": 100000, "resetAt": "..." }`

#### NeboLoopOAuthStartHandler

- **Route:** `POST /api/v1/neboloop/oauth/start`
- **File:** `oauth.go`
- **Logic:**
  1. Generates PKCE code verifier + challenge
  2. Generates random state parameter
  3. Stores flow state in memory (10-minute TTL with cleanup goroutine)
  4. Builds OAuth authorization URL
  5. Opens URL in system browser
- **Response:** `{ "state": "...", "url": "https://neboloop.com/oauth/authorize?..." }`

#### NeboLoopOAuthCallbackHandler

- **Route:** `GET /api/v1/neboloop/oauth/callback`
- **File:** `oauth.go`
- **Query params:** `code`, `state`
- **Logic:**
  1. Validates `state` against stored flows
  2. Exchanges authorization code for tokens using PKCE verifier
  3. Fetches user info from NeboLoop `/api/v1/auth/userinfo`
  4. Stores profile in `auth_profiles` table
  5. Activates NeboLoop comm
  6. Returns HTML page that auto-closes the browser window
- **Response:** HTML page with `window.close()` script

#### NeboLoopOAuthStatusHandler

- **Route:** `GET /api/v1/neboloop/oauth/status`
- **File:** `oauth.go`
- **Query params:** `state`
- **Logic:** Polling endpoint -- frontend polls this until OAuth flow completes
- **Response:** `{ "complete": true, "email": "..." }` or `{ "complete": false }`

#### RefreshNeboLoopToken (helper)

- **Not an HTTP handler** -- utility function called by the agent
- **File:** `oauth.go`
- **Logic:**
  1. Takes refresh_token and API URL
  2. Posts to NeboLoop `/api/v1/auth/refresh` endpoint
  3. Returns new access_token and refresh_token
- **Used by:** `tryRefreshNeboLoopToken()` in agent.go

---

### Files Handler

Located in: `internal/handler/files/filehandler.go`

#### ServeFileHandler

- **Route:** `GET /api/v1/files/*`
- **Logic:**
  1. Extracts file path from the wildcard portion of the URL
  2. **Path traversal protection:** rejects paths containing `..`
  3. Determines content type by file extension
  4. Sets 1-hour cache headers
  5. Serves the file via `http.ServeFile()`
- **Response:** File content with appropriate Content-Type
- **Errors:** 403 if path traversal detected; 404 if file not found

#### BrowseFilesHandler

- **Route:** `POST /api/v1/files/browse`
- **Logic:** Delegates to `svcCtx.BrowseFiles()` which opens a native file picker dialog
- **Response:** `{ "paths": ["/path/to/selected/file"] }`
- **Note:** Only works in desktop mode (native window required)

---

### Voice Handler

Located in: `internal/handler/voice/modelshandler.go`

#### ModelsStatusHandler

- **Route:** `GET /api/v1/voice/models/status`
- **Logic:**
  1. Checks status of voice models (ASR + TTS)
  2. Returns whether models are ready and individual model status
- **Response:** `{ "ready": true, "models": { "whisper": { "status": "ready" }, "piper": { "status": "ready" } } }`

#### ModelsDownloadHandler

- **Route:** `POST /api/v1/voice/models/download`
- **Logic:**
  1. Initiates model download
  2. Returns SSE (Server-Sent Events) stream of download progress
  3. Each event contains: `{ "model": "whisper", "progress": 45, "status": "downloading" }`
  4. Stream ends with `{ "status": "complete" }` or `{ "status": "error", "error": "..." }`
- **Response:** SSE stream (Content-Type: text/event-stream)

---

### Root Handlers

Located in: `internal/handler/`

#### HealthCheckHandler

- **Route:** `GET /api/v1/health`
- **File:** `healthcheckhandler.go`
- **Logic:** Returns static health status
- **Response:** `{ "status": "healthy", "version": "1.2.3", "timestamp": "2026-01-15T10:30:00Z" }`

#### UpdateCheckHandler

- **Route:** `GET /api/v1/update/check`
- **File:** `updatecheckhandler.go`
- **Logic:**
  1. Checks for available updates via `updater.Check(currentVersion)`
  2. Detects install method (direct, homebrew, etc.)
  3. Determines if auto-update is possible (direct installs only)
- **Response:** `{ "available": true, "currentVersion": "1.2.3", "latestVersion": "1.3.0", "releaseUrl": "...", "installMethod": "direct", "canAutoUpdate": true }`

#### UpdateApplyHandler

- **Route:** `POST /api/v1/update/apply`
- **File:** `updateapplyhandler.go`
- **Logic:**
  1. Checks for staged binary from UpdateManager (`svcCtx.UpdateManager().GetPending()`)
  2. If no staged binary, downloads latest version first
  3. Verifies checksum of downloaded binary
  4. Responds to client with "restarting" status
  5. After 500ms delay, applies the update (replaces binary and restarts process)
- **Response:** `{ "status": "restarting", "version": "1.3.0" }`
- **Errors:** 400 if no update available; 500 if download/verify fails

---

## CLI Commands

Located in: `cmd/nebo/`

### vars.go -- Global Flags

**File:** `cmd/nebo/vars.go`

**Global variables:**
- `cfgFile` -- Config file path (default: auto-detected)
- `sessionKey` -- Session key for agent sessions (default: `"default"`)
- `providerArg` -- Provider override for CLI chat
- `verbose` -- Enable verbose logging
- `headless` -- Run in headless mode (default for CLI)
- `dangerouslyAll` -- Default: `true` (autonomous mode for all commands)
- `ServerConfig` -- Shared server configuration
- `AppVersion` -- Application version string

**SetupRootCmd function:**
- Registers all subcommands: `serve`, `agent`, `chat`, `config`, `session`, `message`, `skills`, `apps`, `capabilities`, `doctor`, `onboard`
- Sets up persistent flags on root command

---

### root.go -- RunAll

**File:** `cmd/nebo/root.go`

**Function:** `RunAll()` -- Entry point for the combined server+agent mode (default)

**Execution flow:**
1. `ensureUserPath()` -- Augments PATH with common CLI tool locations (/opt/homebrew/bin, ~/.local/bin, ~/go/bin, etc.) so CLI providers (claude, gemini, codex) are discoverable even when launched as a macOS .app
2. Enable quiet mode for migrations and app initialization
3. `defaults.EnsureDataDir()` -- Creates data directory with default files
4. `logging.Init()` -- Initialize unified logging (tint console + file at `<data_dir>/logs/agent.log`)
5. `acquireLock(dataDir)` -- Enforce single instance with lock file
6. Set up pre-apply hook to release lock before binary restart
7. Create context with cancellation, wire up SIGINT/SIGTERM
8. `svc.NewServiceContext(*c)` -- Initialize shared ServiceContext (owns database connection)
9. `startBackgroundUpdater(ctx, svcCtx)` -- Start 6-hour update check cycle
10. Create shared components: `AgentMCPProxy` and `VoiceDuplexProxy`
11. Start server goroutine via `server.Run(ctx, *c, opts)` with shared ServiceContext
12. Wait for server readiness by polling `/api/v1/csrf-token` (10s timeout)
13. Load agent config, set shared DB reference
14. Start agent goroutine via `runAgent(ctx, agentCfg, serverURL, agentOpts)`
15. Register lifecycle hooks:
    - `OnAgentConnected` -- Signal readiness, start heartbeat daemon (once), wire settings changes to heartbeat interval
    - `OnAgentDisconnected` -- Silent (no console spam)
16. Wait for agent connection (5s timeout, continues anyway)
17. `printStartupBanner()` -- Print clean startup message with URL
18. `openBrowser()` -- Auto-open browser (skip if opened within 8 hours or in dev mode)
19. Wait for shutdown signal or error

**`openBrowser()` details:**
- Skips if `NEBO_NO_BROWSER=1` or `AIR_TMP_DIR` is set (dev mode)
- Checks `browser_opened` file -- skips if modified within 8 hours
- Uses platform-specific commands: `open` (macOS), `xdg-open` (Linux), `rundll32` (Windows)
- Writes timestamp to `browser_opened` file on success

**`waitForServer()` details:**
- Polls `GET /api/v1/csrf-token` every 100ms
- Returns true when 200 OK received
- Returns false after timeout

---

### agent.go -- Agent Loop

**File:** `cmd/nebo/agent.go` (largest file in the codebase)

#### agentState struct

Core state container for the agent WebSocket connection:
- `conn` -- gorilla/websocket connection
- `connMu` -- Mutex for connection writes (only used for ping)
- `outbound` -- Buffered channel (512) for outgoing frames
- `pendingApproval` -- Map of request_id -> approval info (for tool approval flow)
- `pendingAsk` -- Map of request_id -> response channel (for interactive prompts)
- `policy` -- Tool execution policy reference
- `lanes` -- LaneManager for concurrent work queues
- `recovery` -- Recovery manager for crash-safe task persistence
- `commManager` -- CommPluginManager for inter-agent communication
- `mcpBridge` -- MCP bridge for external tool integrations
- `sqlDB` -- Raw database connection
- `botID` -- Immutable bot UUID (generated on first startup)
- `companionChatID` -- Cached companion chat session ID
- `heartbeat` -- Pointer-to-pointer to heartbeat daemon
- `appRegistry` -- App registry for installed apps
- `registry` -- Tool registry
- `skillLoader` -- Skill loader

#### Key methods

**`sendFrame(data map[string]any) error`:**
- Non-blocking send via buffered channel
- All 56+ call sites use this instead of direct writes
- If channel full, logs warning and returns error (never blocks)

**`writeLoop()`:**
- Single goroutine that drains the outbound channel
- Serializes all writes to the WebSocket connection
- Eliminates mutex contention from concurrent senders

**`requestApproval(ctx, requestID, toolName, input) (bool, error)`:**
- Sends `approval_request` frame to server
- Blocks waiting for `approval_response` frame
- On "always" approval, adds tool to policy allowlist permanently
- Uses per-request channel in `pendingApproval` map

**`requestAsk(ctx, requestID, prompt, widgets) (string, error)`:**
- Sends `ask_request` frame to server (interactive prompt for user)
- Blocks waiting for `ask_response` frame
- Returns user's text response

#### runAgent() -- Main agent initialization

The `runAgent()` function is the single code path for all agent modes. Execution flow:

1. **WebSocket connection:** Constructs WS URL, generates short-lived JWT, dials connection
2. **State initialization:** Creates agentState with buffered outbound channel, starts writeLoop goroutine
3. **Lane configuration:** Applies lane concurrency from config (main, events, subagent, nested, heartbeat, comm)
4. **Lane event forwarding:** Forwards `lane_update` events to server for UI
5. **Database:** Uses shared database if provided (RunAll mode), otherwise opens its own
6. **Session manager:** Creates session manager, purges ghost messages from failed runs
7. **Crash logger:** Initializes crash logger for persistent error tracking
8. **Recovery manager:** Initializes for task persistence across restarts
9. **Providers:** Creates AI providers via `createProviders(cfg)`
10. **Settings singleton:** Initializes local settings from DB
11. **Tool policy:** Creates policy from config, wires live autonomous mode check, sets approval callback
12. **Tool registry:** Creates registry with permissions, registers default tools
13. **Browser manager:** Starts headless browser for web automation
14. **Embedding service:** Creates for hybrid memory search (OpenAI preferred, Ollama fallback)
15. **Memory tool:** Creates for auto-extraction (not registered as standalone -- only via agent STRAP tool)
16. **Advisors:** Loads from files + DB (DB overrides file-based on name collision)
17. **Cron tool:** Creates scheduler manager for reminders
18. **STRAP tools:** Registers BotTool, EventTool, AppTool, SkillTool
19. **MCP server:** Creates agent MCP server for CLI provider loopback
20. **Runner:** Creates agentic loop runner, wires up:
    - Background function (memory extraction via events lane)
    - Model selector + fuzzy matcher
    - Memory tool, profile tracker, rate limit store
    - Warning handler, MCP server, vision tool
    - Subagent persistence + recovery
21. **Cron wiring:** Connects cron tool to agent execution via message bus
22. **Comm system:** Initializes CommPluginManager, registers loopback + neboloop plugins
23. **Token refresher:** Wires NeboLoop token refresh (serialized, coalesced)
24. **Skill tool:** Creates unified skill domain tool, registers standalone skills
25. **App registry:** Discovers and launches apps, starts supervisor
26. **Provider loader:** Sets dynamic provider reload (includes gateway providers from apps)
27. **Fuzzy matcher:** Registers gateway app names as aliases
28. **Comm handler:** Wires runner + lanes into comm handler
29. **NeboLoop message wiring:**
    - Loop channel messages -> per-channel lanes via message bus
    - Owner DMs -> main lane (shares companion chat session with web UI)
    - External DMs -> comm lane
    - Voice messages -> full-duplex voice pipeline
    - History requests -> companion chat messages
    - Install events -> app registry
    - Account events -> frontend notification
30. **Local channel apps:** Wires through message bus
31. **ServiceContext wiring:** Registers app UI, registry, tool registry, scheduler with HTTP layer
32. **MCP bridge:** Wires external tool integrations with periodic re-sync (15 min)
33. **Comm plugin connection:** Connects neboloop plugin with JWT + bot_id
34. **Background update checker:** 6-hour interval, auto-downloads for direct installs
35. **Agent keepalive:** 30-second ping interval
36. **Message loop:** Reads WebSocket messages, dispatches to goroutines

#### handleAgentMessageWithState() -- Message dispatcher

Handles incoming WebSocket frames from the server:

| Frame Type | Method | Action |
|-----------|--------|--------|
| `approval_response` | -- | Routes to `handleApprovalResponse()` |
| `ask_response` | -- | Routes to `handleAskResponse()` |
| `req` | `ping` | Responds with `pong` |
| `req` | `introduce` | Enqueues introduction to main lane |
| `req` | `get_lanes` | Returns lane stats directly |
| `req` | `get_loops` | Returns loops + channels from comm plugin |
| `req` | `get_channel_messages` | Returns rendered channel messages |
| `req` | `send_channel_message` | Sends human-injected message to channel |
| `req` | `cancel` | Cancels active task in main lane |
| `req` | `run` / `generate_title` | Main chat execution (see below) |
| `event` | `ready` | Recovers pending tasks from previous session |
| `event` | `settings_updated` | Reloads providers, handles comm settings |
| `event` | `integrations_changed` | Re-syncs MCP bridge |

**`run` method processing:**
1. Intercept special codes before LLM: NEBO-xxxx (connection), LOOP-xxxx (invite), SKIL-xxxx (install), APP-xxxx (install)
2. Determine lane by session key prefix: `heartbeat-` -> heartbeat, `reminder-`/`routine-` -> events, `comm-` -> comm, `dev-` -> dev, else -> main
3. Cache companion chat ID for DM handler
4. Derive origin from lane: system for heartbeat/cron, comm for comm messages, user for everything else
5. Resolve lane model override from config (`LaneRouting`)
6. Build hub frame sink with tool result forwarding
7. If cron job, wire result into heartbeat on completion
8. Emit to message bus

#### Code interceptors

Four special code patterns are intercepted before the LLM:

| Pattern | Length | Example | Handler |
|---------|--------|---------|---------|
| `NEBO-XXXX-XXXX-XXXX` | 19 | `NEBO-AB12-CD34-EF56` | `handleNeboLoopCode` -- redeems connection code, activates comm |
| `LOOP-XXXX-XXXX-XXXX` | 19 | `LOOP-AB12-CD34-EF56` | `handleLoopCode` -- joins bot to a loop |
| `SKIL-XXXX-XXXX-XXXX` | 19 | `SKIL-AB12-CD34-EF56` | `handleSkillCode` -- installs skill from NeboLoop |
| `APP-XXXX-XXXX-XXXX` | 18 | `APP-AB12-CD34-EF56` | `handleAppCode` -- installs app from NeboLoop |

All interceptors emit tool-call-style events for UI feedback.

#### Bot ID management

`ensureBotID()` resolves with file-first priority:
1. **File** (`<data_dir>/bot_id`) -- source of truth, survives DB deletion
2. **DB** (`plugin_settings`) -- backward compat, migrated to file on first read
3. **Generate** -- new UUID, persisted to both file and DB

#### Token refresh

`tryRefreshNeboLoopToken()`:
- Serialized via mutex -- concurrent callers coalesce
- Coalesce window: 30 seconds (if refreshed within 30s, read from DB instead)
- Persists new tokens atomically in a transaction
- Deletes stale inactive neboloop profiles
- Called by the neboloop plugin on auth failure, post-reconnect, and plan changes

---

### chat.go -- CLI Chat

**File:** `cmd/nebo/chat.go`

**Command:** `nebo chat [prompt]`

**Flags:**
- `-i, --interactive` -- Interactive mode (REPL)
- `--dangerously` -- Autonomous mode (skip approval prompts)
- `--voice` -- Voice input mode

**Execution flow:**
1. Opens database from config path
2. Creates session manager
3. Creates AI providers via `createProviders(cfg)`
4. Creates tool policy + registry
5. Creates runner with provider loader + model selector
6. If `--voice` flag, enables voice input via microphone
7. If interactive mode or no prompt given, enters `runInteractive()`
8. If prompt given as args, enters `runOnce()` with single prompt

**Interactive mode commands:**
| Command | Action |
|---------|--------|
| `/help` | Print help message |
| `/clear` | Clear current session |
| `/sessions` | List all sessions |
| `/quit` or `/exit` | Exit REPL |

**Event handling (`handleEvent`):**
- Streams text chunks to stdout in real-time
- Displays tool calls and results
- Handles errors with formatting

---

### config.go -- Configuration

**File:** `cmd/nebo/config.go`

**Command:** `nebo config`

**Subcommands:**
- `nebo config` (no subcommand) -- Shows current configuration:
  - Data directory path
  - Database path
  - Configured providers (names only)
  - Tool policy level and ask mode
- `nebo config init` -- Creates default `config.yaml` in the data directory

---

### session.go -- Session Management

**File:** `cmd/nebo/session.go`

**Command:** `nebo session`

**Subcommands:**
- `nebo session list` -- Lists all sessions with their keys, scopes, and message counts
- `nebo session clear [key]` -- Clears messages from a specific session by key

---

### message.go -- Channel Messaging

**File:** `cmd/nebo/message.go`

**Command:** `nebo message`

**Subcommands:**
- `nebo message send [text]` -- Send a message to a channel
  - `--to` or `--channel` flag in `type:id` format (e.g., `telegram:12345`)
  - Posts to the gateway API endpoint
- `nebo message channels` -- Lists available channels (GET from gateway API)

---

### skills.go -- Skill Listing

**File:** `cmd/nebo/skills.go`

**Command:** `nebo skills`

**Subcommands:**
- `nebo skills list` -- Lists all skills (loads user + bundled, user overrides on collision)
- `nebo skills show [name]` -- Shows details of a specific skill (description, triggers, tools)

---

### plugins.go -- Apps and Capabilities

**File:** `cmd/nebo/plugins.go`

**Command:** `nebo apps`

**Subcommands:**
- `nebo apps list` -- Scans apps directory for `manifest.json` files, displays name/version/description
- `nebo apps uninstall [name]` -- Removes app directory

**Command:** `nebo capabilities`

**Logic:** Lists platform capabilities grouped by category. Each capability shows:
- Name, description, required tools
- Platform availability (darwin, linux, windows)

---

### providers.go -- Provider Loading

**File:** `cmd/nebo/providers.go`

**Function:** `createProviders(cfg) []ai.Provider`

**Provider detection priority (cascading):**

1. **Database** -- API keys from UI (Settings > Providers) stored in `auth_profiles` table
   - Queries active profiles grouped by provider
   - Priority order: OAuth > Token > API Key (`auth_type` column)
   - Supports: Anthropic, OpenAI, Gemini, Ollama, NeboLoop/Janus
   - Decrypts API keys via `credential.Decrypt()`
   - Tracks usage and error counts per profile

2. **Config file** -- `models.yaml` credentials section
   - Supports environment variable expansion via `os.ExpandEnv()`
   - Example: `api_key: ${ANTHROPIC_API_KEY}`

3. **CLI auto-discovery** -- If `models.yaml` `defaults.primary` starts with `claude-code`/`codex-cli`/`gemini-cli`
   - Checks PATH for the CLI binary
   - Creates CLI provider wrapper

**Function:** `loadToolPermissions(sqlDB) map[string]bool`

- Reads from `user_profiles.tool_permissions` for `"default-user"`
- Migrates old defaults format (adds new permission keys)
- Backfills missing keys with defaults
- Default permissions: chat, file, web, desktop, system = true; shell, contacts, media = false

---

### updates.go -- Background Updater

**File:** `cmd/nebo/updates.go`

**Function:** `startBackgroundUpdater(ctx, svcCtx)`

**Logic:**
1. Checks for updates every 6 hours
2. Broadcasts events to browser clients via AgentHub:
   - `update_available` -- New version found
   - `update_progress` -- Download progress (downloaded, total, percent)
   - `update_ready` -- Binary downloaded and verified
   - `update_error` -- Download or verification failure
3. Auto-downloads for "direct" installs only (not homebrew/package manager)
4. Verifies checksum after download
5. Stages binary via `UpdateManager.SetPending()`

---

### onboard.go -- Setup Wizard

**File:** `cmd/nebo/onboard.go`

**Command:** `nebo onboard`

**Interactive wizard flow:**
1. Creates data directory
2. Prompts to choose AI provider (1-5): Anthropic, OpenAI, Google Gemini, Ollama, Skip
3. Prompts for API key (if applicable)
4. Writes `config.yaml` with provider credentials
5. Optional: channel setup (Telegram, Discord, Slack) with bot token prompts

---

### doctor.go -- Diagnostics

**File:** `cmd/nebo/doctor.go`

**Command:** `nebo doctor`

**Flag:** `--fix` -- Attempts automatic repairs

**Diagnostic checks:**
1. **Config directory** -- Exists and is writable
2. **Config file** -- Exists and is valid YAML
3. **Server config** -- Port and domain are configured
4. **API keys** -- Providers have valid credentials
5. **Gateway connectivity** -- Can reach NeboLoop gateway (if configured)
6. **Database** -- Can open and query SQLite database
7. **System** -- Platform info, checks for required tools (claude, codex, gemini CLI)
8. **Channels** -- Configured channels have valid tokens

Each check outputs PASS/FAIL with details. `--fix` mode attempts to create missing directories, write default configs, etc.

---

## Database Layer

Located in: `internal/db/`

### Connection Setup (sqlite.go)

**File:** `internal/db/sqlite.go`

**Function:** `NewSQLite(path string) (*Store, error)`

**SQLite configuration (pragmas):**
```sql
PRAGMA journal_mode = WAL;       -- Write-Ahead Logging for concurrent reads
PRAGMA synchronous = NORMAL;     -- Balance between safety and performance
PRAGMA cache_size = -1073741824; -- 1GB cache (negative = bytes)
PRAGMA foreign_keys = ON;        -- Enforce foreign key constraints
```

**Connection pool settings:**
- `MaxOpenConns = 1` -- Single writer (SQLite limitation)
- `MaxIdleConns = 1` -- Keep connection alive
- Connection lifetime: unlimited

**Initialization sequence:**
1. Opens SQLite database at the given path
2. Applies pragmas
3. Runs goose migrations via `migrations.Run(db)`
4. Returns `Store` wrapping the connection

### Store and Transactions (store.go)

**File:** `internal/db/store.go`

**`Store` struct:**
```go
type Store struct {
    *Queries  // sqlc-generated query methods
    db *sql.DB
}
```

**Key methods:**
- `GetDB() *sql.DB` -- Returns raw database connection
- `Ping() error` -- Health check
- `Close() error` -- Close database connection

**Transaction support:**
- `ExecTx(ctx, fn func(*Queries) error) error` -- Execute function within a transaction
- `ExecTxWithResult[T](ctx, fn func(*Queries) (T, error)) (T, error)` -- Transaction with return value
- Both methods: begin transaction, call function with transaction-scoped Queries, commit on success, rollback on error

### Migration System (migrations/migrate.go)

**File:** `internal/db/migrations/migrate.go`

**Technology:** [goose](https://github.com/pressly/goose/v3) with embedded SQL migrations

**Key features:**
- Migrations are embedded in the Go binary via `//go:embed *.sql`
- Sequential numbering: `0001_description.sql`, `0002_description.sql`, etc.
- `QuietMode` flag suppresses migration logging during normal operation
- Supports `Run()` (apply pending), `Status()` (check state), `Down()` (rollback last)

**Migration file format (goose):**
```sql
-- +goose Up
CREATE TABLE ...;

-- +goose Down
DROP TABLE ...;
```

### Session Manager (session_manager.go)

**File:** `internal/db/session_manager.go`

**`SessionManager` struct:** Manages agent sessions and message persistence.

**Core session operations:**
- `GetOrCreate(key, scope string) (*Session, error)` -- Gets existing session by key or creates new one. Uses `sync.Map` cache (`sessionKeys`) to avoid repeated DB lookups.
- `ListSessions() ([]Session, error)` -- Lists all sessions (limit 100)
- `DeleteSession(id string) error` -- Deletes session and its messages
- `Reset(sessionID string) error` -- Clears all messages in a session

**Message operations:**
- `GetMessages(sessionID string, limit int) ([]Message, error)` -- Reads from `chat_messages` table
- `AppendMessage(sessionID string, msg Message) error` -- Guards against empty messages (no-op if content empty). Creates chat_messages entry with UUID, role, content, metadata.
- `sanitizeAgentMessages(messages []Message) []Message` -- Strips orphaned tool_results (tool results without matching tool_use in the conversation)

**Task tracking:**
- `GetActiveTask(sessionID string) (string, error)` / `SetActiveTask(sessionID, task string) error`
- `GetWorkTasks(sessionID string) (string, error)` / `SetWorkTasks(sessionID, tasks string) error`

**Compaction tracking:**
- `GetSummary(sessionID string) (string, error)` / `UpdateSummary(sessionID, summary string) error`
- `GetLastSummarizedCount(sessionID string) (int, error)` / `SetLastSummarizedCount(sessionID string, count int) error`

**Session resolution:** `resolveSessionKey(key string) string` transforms human-readable keys into database session IDs. Uses cached lookup in `sessionKeys` sync.Map.

### SQL Query Patterns

Located in: `internal/db/queries/`

Each `.sql` file corresponds to one entity/domain and is processed by [sqlc](https://sqlc.dev) to generate type-safe Go code.

#### chats.sql (18 queries)

| Query | Type | Description |
|-------|------|-------------|
| `GetChat` | one | Get chat by ID |
| `ListChats` | many | List all chats with pagination |
| `CreateChat` | exec | Create new chat |
| `UpdateChat` | exec | Update chat title |
| `DeleteChat` | exec | Delete chat (cascade removes messages) |
| `GetOrCreateCompanionChat` | one | Atomic upsert -- `INSERT ... ON CONFLICT DO NOTHING` then `SELECT` |
| `GetCompanionChat` | one | Get companion chat by user_id |
| `CreateChatMessage` | exec | Create new message |
| `GetChatMessages` | many | Get messages for a chat (ordered by created_at) |
| `GetRecentChatMessages` | many | Get N most recent messages (for history API) |
| `GetMessagesByDay` | many | Get messages filtered by `day_marker` column |
| `ListChatDays` | many | List distinct days with message counts |
| `SearchChatMessages` | many | LIKE search across message content |
| `DeleteChatMessages` | exec | Delete all messages for a chat |
| `CountChatMessages` | one | Count messages in a chat |
| `CreateRunnerMessage` | exec | Create message with `metadata` JSON column |
| `GetRunnerMessages` | many | Get messages with metadata (used by runner/session) |
| `UpdateChatTimestamp` | exec | Touch chat's `updated_at` |

#### sessions.sql (25 queries)

| Query | Type | Description |
|-------|------|-------------|
| `GetSession` | one | Get session by ID |
| `GetSessionByKey` | one | Get session by human-readable key |
| `GetSessionByScope` | one | Get session by key + scope (agent/user) |
| `CreateSession` | exec | Create new session with key, scope, user_id |
| `UpdateSession` | exec | Update session metadata |
| `DeleteSession` | exec | Delete session by ID |
| `ListSessions` | many | List sessions with limit/offset |
| `CountSessions` | one | Count total sessions |
| `GetSessionSummary` | one | Get compaction summary for session |
| `UpdateSessionSummary` | exec | Update compaction summary |
| `GetSessionActiveTask` | one | Get active task for session |
| `SetSessionActiveTask` | exec | Set active task |
| `GetSessionWorkTasks` | one | Get work tasks JSON |
| `SetSessionWorkTasks` | exec | Set work tasks JSON |
| `GetLastSummarizedCount` | one | Get last summarized message count (for incremental compaction) |
| `SetLastSummarizedCount` | exec | Set last summarized count |
| `GetSessionPolicyOverrides` | one | Get model/provider/auth profile overrides |
| `SetSessionModel` | exec | Override model for session |
| `SetSessionProvider` | exec | Override provider for session |
| `SetSessionAuthProfile` | exec | Override auth profile for session |
| `ClearSessionOverrides` | exec | Reset all overrides to NULL |
| `GetSessionEmbeddingModel` | one | Get embedding model used for session |
| `SetSessionEmbeddingModel` | exec | Set embedding model |
| `PurgeEmptyMessages` | exec | Delete ghost messages (empty content from failed runs) |
| `GetSessionUserID` | one | Get user_id for a session |

#### memories.sql (22 queries)

| Query | Type | Description |
|-------|------|-------------|
| `CreateMemory` | one | Create new memory with layer, namespace, key, value, tags |
| `GetMemory` | one | Get memory by ID |
| `UpdateMemory` | exec | Update memory value + tags |
| `DeleteMemory` | exec | Delete memory by ID |
| `ListMemories` | many | List with pagination |
| `ListMemoriesByNamespace` | many | Filter by namespace with pagination |
| `SearchMemories` | many | LIKE search across key, value, tags |
| `CountMemories` | one | Total count |
| `CountMemoriesByLayer` | one | Count by layer (tacit/daily/entity) |
| `ListNamespaces` | many | Distinct namespace values |
| `UpsertMemory` | one | Upsert by key+namespace -- `ON CONFLICT(key, namespace) DO UPDATE` |
| `UpsertMemoryWithScope` | one | Upsert with user_id scope |
| `GetMemoryByKey` | one | Lookup by key |
| `GetMemoryByKeyAndNamespace` | one | Lookup by key + namespace |
| `IncrementMemoryAccessCount` | exec | Increment access_count + update last_accessed_at |
| `ListMemoriesForContext` | many | Get memories for context building (ordered by priority) |
| `GetMemoryEmbedding` | one | Get embedding vector for a memory |
| `SetMemoryEmbedding` | exec | Store embedding vector |
| `ListMemoriesWithoutEmbeddings` | many | Find memories needing embedding backfill |
| `ListMemoriesWithEmbeddings` | many | Get memories that have embeddings |
| `ClearEmbeddingsByModel` | exec | Clear embeddings from a specific model (migration) |
| `CountEmbeddings` | one | Count memories with embeddings |

**Memory 3-tier system:**
- `tacit` -- Long-term preferences and learned behaviors (e.g., "user prefers dark mode")
- `daily` -- Day-specific facts keyed by date (e.g., "2026-01-15/weather: sunny")
- `entity` -- Information about people, places, things (e.g., "person/alice: colleague, works in marketing")

#### notifications.sql (10 queries)

| Query | Type | Description |
|-------|------|-------------|
| `CreateNotification` | one | Create with type, title, message, user_id, metadata JSON |
| `GetNotification` | one | Get by ID |
| `ListNotifications` | many | List for user with pagination |
| `ListUnreadNotifications` | many | List unread only |
| `CountUnreadNotifications` | one | Count unread for user |
| `MarkNotificationRead` | exec | Mark single as read |
| `MarkAllNotificationsRead` | exec | Mark all as read for user |
| `DeleteNotification` | exec | Delete by ID + user_id |
| `DeleteOldNotifications` | exec | Cleanup notifications older than N days |
| `DeleteAllNotifications` | exec | Delete all for user |

#### advisors.sql (7 queries)

| Query | Type | Description |
|-------|------|-------------|
| `CreateAdvisor` | one | Create advisor with name, slug, role, description, prompt, priority, enabled |
| `GetAdvisorByName` | one | Get by name (unique) |
| `GetAdvisorBySlug` | one | Get by slug (unique) |
| `ListAdvisors` | many | List all advisors ordered by priority |
| `ListEnabledAdvisors` | many | List only enabled advisors |
| `UpdateAdvisor` | exec | Update all fields by ID |
| `DeleteAdvisor` | exec | Delete by ID |

#### cron_jobs.sql (19 queries)

| Query | Type | Description |
|-------|------|-------------|
| `CreateCronJob` | one | Create with name, schedule (cron expression), message, instructions |
| `GetCronJob` | one | Get by ID |
| `GetCronJobByName` | one | Get by name (unique) |
| `ListCronJobs` | many | List with pagination |
| `UpdateCronJob` | exec | Update all fields |
| `DeleteCronJob` | exec | Delete by ID |
| `DeleteCronJobByName` | exec | Delete by name |
| `ToggleCronJob` | exec | Enable/disable by ID |
| `UpsertCronJob` | one | Upsert by name -- `ON CONFLICT(name) DO UPDATE` |
| `ListEnabledCronJobs` | many | List only enabled jobs |
| `UpdateLastRun` | exec | Update last_run_at timestamp |
| `UpdateNextRun` | exec | Update next_run_at timestamp |
| `CreateCronJobHistory` | exec | Record execution in history table |
| `ListCronJobHistory` | many | Get execution history for a job |
| `CountCronJobs` | one | Total count |
| `GetCronJobWithNextRun` | one | Get job with calculated next run time |
| `UpdateCronJobSchedule` | exec | Update schedule expression only |
| `UpdateCronJobMessage` | exec | Update message only |
| `UpdateCronJobInstructions` | exec | Update instructions only |

#### mcp_integrations.sql (17 queries)

| Query | Type | Description |
|-------|------|-------------|
| `CreateMCPIntegration` | one | Create with URL, server_type, name, config JSON |
| `GetMCPIntegration` | one | Get by ID |
| `ListMCPIntegrations` | many | List all integrations |
| `UpdateMCPIntegration` | exec | Update fields by ID |
| `DeleteMCPIntegration` | exec | Delete by ID |
| `UpdateMCPConnectionStatus` | exec | Update connection_status + tool_count |
| `GetMCPCredentials` | one | Get encrypted API key for integration |
| `UpdateMCPCredentials` | exec | Update encrypted API key |
| `DeleteMCPCredentials` | exec | Delete credentials |
| `CreateMCPCredentials` | exec | Store encrypted credentials |
| `ListMCPServerRegistry` | many | List known MCP server definitions |
| `GetMCPServerByType` | one | Get server by type |
| `SetMCPOAuthTokens` | exec | Store OAuth tokens (access + refresh) |
| `GetMCPOAuthTokens` | one | Get OAuth tokens for integration |
| `ClearMCPOAuthTokens` | exec | Clear OAuth tokens |
| `SetMCPOAuthState` | exec | Store OAuth flow state (PKCE verifier + state) |
| `GetMCPOAuthState` | one | Get OAuth flow state |

#### agent_profile.sql (7 queries)

| Query | Type | Description |
|-------|------|-------------|
| `GetAgentProfile` | one | Get singleton profile (id=1) |
| `EnsureAgentProfileExists` | exec | Insert default profile if not exists |
| `UpdateAgentProfile` | exec | Update 15+ nullable fields (name, role, personality, quiet hours, bio, etc.) |
| `ListPersonalityPresets` | many | List personality preset templates |
| `CreatePersonalityPreset` | exec | Create new preset |
| `UpdatePersonalityPreset` | exec | Update preset |
| `DeletePersonalityPreset` | exec | Delete preset |

#### auth_profiles.sql (13 queries)

| Query | Type | Description |
|-------|------|-------------|
| `CreateAuthProfile` | one | Create with provider, name, api_key, base_url, auth_type, metadata JSON |
| `GetAuthProfile` | one | Get by ID |
| `GetActiveAuthProfileByProvider` | one | Get highest-priority active profile for provider |
| `ListActiveAuthProfilesByProvider` | many | List all active profiles for provider (sorted by auth_type priority: oauth > token > apikey) |
| `ListAllActiveAuthProfilesByProvider` | many | List active profiles (no user_id filter -- for agent use) |
| `ListAuthProfiles` | many | List all profiles |
| `UpdateAuthProfile` | exec | Update fields by ID |
| `DeleteAuthProfile` | exec | Delete by ID |
| `DeactivateAuthProfilesByProvider` | exec | Set is_active=0 for all profiles of a provider |
| `DeleteInactiveAuthProfilesByProvider` | exec | Purge inactive profiles |
| `IncrementAuthProfileUsage` | exec | Increment usage_count |
| `IncrementAuthProfileErrors` | exec | Increment error_count + set last_error |
| `SetAuthProfileCooldown` | exec | Set cooldown_until timestamp (for rate limiting / error backoff) |

**Auth profile priority system:**
- OAuth profiles have highest priority (auth_type = "oauth")
- Token profiles are next (auth_type = "token")
- API Key profiles are lowest (auth_type = "apikey")
- The `GetActiveAuthProfileByProvider` query uses `ORDER BY` on auth_type to implement this
- Cooldown system: providers with errors get `cooldown_until` set, and are skipped until cooldown expires

---

## Cross-Cutting Patterns

### Single Bot Paradigm

Throughout the codebase, the "Single Bot Paradigm" is enforced:
- Only ONE companion chat exists (keyed by `companion-default` user ID)
- `CreateChat`, `ListChats` always return/create the companion chat
- Owner DMs share the companion chat session
- The web UI and NeboLoop DMs see the same conversation

### Message Bus Architecture

All inbound messages (web UI, DMs, cron, recovery, channels) flow through the unified message bus (`msgbus.Bus`):
- `bus.Emit(InboundMessage)` enqueues to the appropriate lane
- Each message source has a `Sink` that handles the output:
  - `HubFrameSink` -- streams back via WebSocket frames
  - `OwnerDMSink` -- streams to web UI + sends DM reply
  - `ExternalDMSink` -- sends DM reply only
  - `LoopChannelSink` -- sends to loop channel
  - `CronSink` -- sends notification
  - `RecoverySink` -- tracks task completion status
  - `LocalChannelSink` -- sends to local channel app

### ServiceContext Pattern

`svc.ServiceContext` is the central dependency injection container:
- Created once in `RunAll()`
- Contains: DB store, Auth service, AgentHub, PluginStore, SkillSettings, MCPClient, OAuthBroker
- Lazy-set fields: AppRegistry, ToolRegistry, Scheduler, NeboLoopClient
- Passed to all HTTP handlers
- Shared between server and agent goroutines

### Credential Encryption

All API keys and tokens stored in the database are encrypted:
- `credential.Encrypt(plaintext)` -- Encrypts before storage
- `credential.Decrypt(ciphertext)` -- Decrypts before use
- Fallback: if decryption fails, assumes plaintext (migration window for existing installs)

*Last updated: 2026-03-10*
