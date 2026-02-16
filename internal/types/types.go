package types

import "encoding/json"

type AgentConnectRequest struct {
	AgentId string `json:"agentId"`
}

type AgentConnectResponse struct {
	Connected bool   `json:"connected"`
	AgentId   string `json:"agentId"`
}

type AgentInfo struct {
	AgentId   string `json:"agentId"`
	Connected bool   `json:"connected"`
	CreatedAt string `json:"createdAt"`
}

type AgentSession struct {
	Id           string `json:"id"`
	Name         string `json:"name,omitempty"`
	Summary      string `json:"summary,omitempty"`
	MessageCount int    `json:"messageCount"`
	CreatedAt    string `json:"createdAt"`
	UpdatedAt    string `json:"updatedAt"`
}

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

type AgentStatusRequest struct {
	AgentId string `path:"agentId"`
}

type AgentStatusResponse struct {
	AgentId   string `json:"agentId"`
	Connected bool   `json:"connected"`
	Uptime    int64  `json:"uptime"`
}

type GetHeartbeatResponse struct {
	Content string `json:"content"`
}

type UpdateHeartbeatRequest struct {
	Content string `json:"content"`
}

type UpdateHeartbeatResponse struct {
	Success bool `json:"success"`
}

type AuthConfigResponse struct {
	GoogleEnabled bool `json:"googleEnabled"`
	GitHubEnabled bool `json:"githubEnabled"`
}

type AuthProfile struct {
	Id        string `json:"id"`
	Name      string `json:"name"`
	Provider  string `json:"provider"` // anthropic, openai, google, ollama
	Model     string `json:"model,omitempty"`
	BaseUrl   string `json:"baseUrl,omitempty"`
	Priority  int    `json:"priority"`
	IsActive  bool   `json:"isActive"`
	CreatedAt string `json:"createdAt"`
	UpdatedAt string `json:"updatedAt"`
}

type CLIAvailability struct {
	Claude bool `json:"claude"`
	Codex  bool `json:"codex"`
	Gemini bool `json:"gemini"`
}

type CLIStatus struct {
	Installed     bool   `json:"installed"`
	Authenticated bool   `json:"authenticated"`
	Version       string `json:"version,omitempty"`
}

type CLIStatusMap struct {
	Claude CLIStatus `json:"claude"`
	Codex  CLIStatus `json:"codex"`
	Gemini CLIStatus `json:"gemini"`
}

type ChangePasswordRequest struct {
	CurrentPassword string `json:"currentPassword"`
	NewPassword     string `json:"newPassword"`
}

type Chat struct {
	Id        string `json:"id"`
	Title     string `json:"title"`
	CreatedAt string `json:"createdAt"`
	UpdatedAt string `json:"updatedAt"`
}

type ChatMessage struct {
	Id        string `json:"id"`
	ChatId    string `json:"chatId"`
	Role      string `json:"role"`
	Content   string `json:"content"`
	Metadata  string `json:"metadata,omitempty"`
	CreatedAt string `json:"createdAt"`
}

type CompleteSetupResponse struct {
	Success bool `json:"success"`
}

type CreateAdminRequest struct {
	Email    string `json:"email"`
	Password string `json:"password"`
	Name     string `json:"name"`
}

type CreateAdminResponse struct {
	Token        string `json:"token"`
	RefreshToken string `json:"refreshToken"`
	ExpiresAt    int64  `json:"expiresAt"`
	User         User   `json:"user"`
}

type CreateAuthProfileRequest struct {
	Name     string `json:"name"`
	Provider string `json:"provider"`
	ApiKey   string `json:"apiKey"`
	Model    string `json:"model,optional"`
	BaseUrl  string `json:"baseUrl,optional"`
	Priority int    `json:"priority,optional"`
}

type CreateAuthProfileResponse struct {
	Profile AuthProfile `json:"profile"`
}

type CreateChatRequest struct {
	Title string `json:"title,optional"`
}

type CreateChatResponse struct {
	Chat Chat `json:"chat"`
}

type DayInfo struct {
	Day          string `json:"day"`
	MessageCount int    `json:"messageCount"`
}

type DeleteAccountRequest struct {
	Password string `json:"password"`
}

type DeleteAgentSessionRequest struct {
	Id string `path:"id"`
}

type DeleteAuthProfileRequest struct {
	Id string `path:"id"`
}

type DeleteChatRequest struct {
	Id string `path:"id"`
}

type DeleteNotificationRequest struct {
	Id string `path:"id"`
}

type DisconnectOAuthRequest struct {
	Provider string `path:"provider"`
}

type EmailVerificationRequest struct {
	Token string `json:"token"`
}

type Empty struct {
}

type ExtensionChannel struct {
	Id   string `json:"id"`
	Path string `json:"path"`
}

type ExtensionSkill struct {
	Name         string   `json:"name"`
	Description  string   `json:"description"`
	Version      string   `json:"version"`
	Tags         []string `json:"tags"`
	Dependencies []string `json:"dependencies"`
	Tools        []string `json:"tools"`
	Priority     int      `json:"priority"`
	Enabled      bool     `json:"enabled"`
	FilePath     string   `json:"filePath"`
	Source       string   `json:"source"`   // "bundled" or "user"
	Editable     bool     `json:"editable"` // true for user skills
}

type ExtensionTool struct {
	Name             string `json:"name"`
	Description      string `json:"description"`
	Schema           string `json:"schema,omitempty"`
	RequiresApproval bool   `json:"requiresApproval"`
	IsPlugin         bool   `json:"isPlugin"`
	Path             string `json:"path,omitempty"`
}

type ForgotPasswordRequest struct {
	Email string `json:"email"`
}

type GetAgentSessionMessagesResponse struct {
	Messages []SessionMessage `json:"messages"`
	Total    int              `json:"total"`
}

type GetAgentSessionRequest struct {
	Id string `path:"id"`
}

type GetAgentSettingsResponse struct {
	Settings AgentSettings `json:"settings"`
}

type GetAuthProfileRequest struct {
	Id string `path:"id"`
}

type GetAuthProfileResponse struct {
	Profile AuthProfile `json:"profile"`
}

type GetChatRequest struct {
	Id string `path:"id"`
}

type GetChatResponse struct {
	Chat          Chat          `json:"chat"`
	Messages      []ChatMessage `json:"messages"`
	TotalMessages int           `json:"totalMessages"` // Total messages in chat (may be more than returned)
}

type GetHistoryByDayRequest struct {
	Day string `path:"day"`
}

type GetHistoryByDayResponse struct {
	Day      string        `json:"day"`
	Messages []ChatMessage `json:"messages"`
}

type GetOAuthUrlRequest struct {
	Provider    string `path:"provider"`
	RedirectUrl string `form:"redirectUrl,optional"`
}

type GetOAuthUrlResponse struct {
	Url   string `json:"url"`
	State string `json:"state"`
}

type GetPersonalityResponse struct {
	Content string `json:"content"`
}

type GetPreferencesResponse struct {
	Preferences UserPreferences `json:"preferences"`
}

type GetSkillRequest struct {
	Name string `path:"name"`
}

type GetSkillResponse struct {
	Skill ExtensionSkill `json:"skill"`
}

type GetUnreadCountResponse struct {
	Count int `json:"count"`
}

type GetUserResponse struct {
	User User `json:"user"`
}

type HealthResponse struct {
	Status    string `json:"status"`
	Version   string `json:"version"`
	Timestamp string `json:"timestamp"`
}

type ListAgentSessionsResponse struct {
	Sessions []AgentSession `json:"sessions"`
	Total    int            `json:"total"`
}

type ListAgentsRequest struct {
}

type ListAgentsResponse struct {
	Agents []AgentInfo `json:"agents"`
	Total  int         `json:"total"`
}

type ListAuthProfilesResponse struct {
	Profiles []AuthProfile `json:"profiles"`
}

type ListChatDaysRequest struct {
	Page     int `form:"page,optional"`
	PageSize int `form:"pageSize,optional"`
}

type ListChatDaysResponse struct {
	Days []DayInfo `json:"days"`
}

type ListChatsRequest struct {
	Page     int `form:"page,optional"`
	PageSize int `form:"pageSize,optional"`
}

type ListChatsResponse struct {
	Chats []Chat `json:"chats"`
	Total int    `json:"total"`
}

type ListExtensionsResponse struct {
	Tools    []ExtensionTool    `json:"tools"`
	Skills   []ExtensionSkill   `json:"skills"`
	Channels []ExtensionChannel `json:"channels"`
}

// CLIProviderInfo describes a CLI provider (from models.yaml cli_providers)
type CLIProviderInfo struct {
	ID           string   `json:"id"`
	DisplayName  string   `json:"displayName"`
	Command      string   `json:"command"`
	InstallHint  string   `json:"installHint"`
	Models       []string `json:"models"`
	DefaultModel string   `json:"defaultModel"`
}

type ListModelsResponse struct {
	Models        map[string][]ModelInfo `json:"models"`
	TaskRouting   *TaskRouting           `json:"taskRouting,omitempty"`
	Aliases       []ModelAlias           `json:"aliases,omitempty"`
	AvailableCLIs *CLIAvailability       `json:"availableCLIs,omitempty"`
	CLIStatuses   *CLIStatusMap          `json:"cliStatuses,omitempty"`
	CLIProviders  []CLIProviderInfo      `json:"cliProviders,omitempty"`
}

type ListNotificationsRequest struct {
	Page     int  `form:"page,optional"`
	PageSize int  `form:"pageSize,optional"`
	Unread   bool `form:"unread,optional"`
}

type ListNotificationsResponse struct {
	Notifications []Notification `json:"notifications"`
	UnreadCount   int            `json:"unreadCount"`
	TotalCount    int            `json:"totalCount"`
}

type ListOAuthProvidersResponse struct {
	Providers []OAuthProvider `json:"providers"`
}

type LoginRequest struct {
	Email    string `json:"email"`
	Password string `json:"password"`
}

type LoginResponse struct {
	Token        string `json:"token"`
	RefreshToken string `json:"refreshToken"`
	ExpiresAt    int64  `json:"expiresAt"`
}

type MarkNotificationReadRequest struct {
	Id string `path:"id"`
}

type MessageResponse struct {
	Message string `json:"message"`
}

type ModelAlias struct {
	Alias   string `json:"alias"`
	ModelId string `json:"modelId"`
}

type ModelInfo struct {
	Id            string        `json:"id"`
	DisplayName   string        `json:"displayName"`
	ContextWindow int           `json:"contextWindow,omitempty"`
	Pricing       *ModelPricing `json:"pricing,omitempty"`
	Capabilities  []string      `json:"capabilities,omitempty"`
	Kind          []string      `json:"kind,omitempty"`
	Preferred     bool          `json:"preferred,omitempty"`
	IsActive      bool          `json:"isActive"`
}

type ModelPricing struct {
	Input       float64 `json:"input,omitempty"`
	Output      float64 `json:"output,omitempty"`
	CachedInput float64 `json:"cachedInput,omitempty"`
}

type Notification struct {
	Id        string `json:"id"`
	Type      string `json:"type"`
	Title     string `json:"title"`
	Body      string `json:"body,omitempty"`
	ActionUrl string `json:"actionUrl,omitempty"`
	Icon      string `json:"icon,omitempty"`
	ReadAt    string `json:"readAt,omitempty"`
	CreatedAt string `json:"createdAt"`
}

type OAuthLoginRequest struct {
	Provider string `path:"provider"`
	Code     string `json:"code"`
	State    string `json:"state,optional"`
}

type OAuthLoginResponse struct {
	Token        string `json:"token"`
	RefreshToken string `json:"refreshToken"`
	ExpiresAt    int64  `json:"expiresAt"`
	IsNewUser    bool   `json:"isNewUser"`
}

type OAuthProvider struct {
	Name      string `json:"name"`
	Connected bool   `json:"connected"`
	Email     string `json:"email,omitempty"`
}

type AppOAuthGrant struct {
	Provider         string `json:"provider"`
	Scopes           string `json:"scopes"`
	ConnectionStatus string `json:"connectionStatus"`
	ExpiresAt        string `json:"expiresAt,omitempty"`
}

type GetAppOAuthGrantsResponse struct {
	Grants []AppOAuthGrant `json:"grants"`
}

type RefreshTokenRequest struct {
	RefreshToken string `json:"refreshToken"`
}

type RefreshTokenResponse struct {
	Token        string `json:"token"`
	RefreshToken string `json:"refreshToken"`
	ExpiresAt    int64  `json:"expiresAt"`
}

type RegisterRequest struct {
	Email    string `json:"email"`
	Password string `json:"password"`
	Name     string `json:"name"`
}

type ResendVerificationRequest struct {
	Email string `json:"email"`
}

type ResetPasswordRequest struct {
	Token       string `json:"token"`
	NewPassword string `json:"newPassword"`
}

type SearchChatMessagesRequest struct {
	Query    string `form:"query"`
	Page     int    `form:"page,optional"`
	PageSize int    `form:"pageSize,optional"`
}

type SearchChatMessagesResponse struct {
	Messages []ChatMessage `json:"messages"`
	Total    int           `json:"total"`
}

type SendMessageRequest struct {
	ChatId  string `json:"chatId"`
	Content string `json:"content"`
	Role    string `json:"role,optional"`
}

type SendMessageResponse struct {
	Message ChatMessage `json:"message"`
	ChatId  string      `json:"chatId"`
}

type SessionMessage struct {
	Id        int    `json:"id"`
	Role      string `json:"role"`
	Content   string `json:"content,omitempty"`
	CreatedAt string `json:"createdAt"`
}

type SetupStatusResponse struct {
	SetupRequired bool `json:"setupRequired"`
	HasAdmin      bool `json:"hasAdmin"`
	SetupComplete bool `json:"setupComplete"`
}

type SimpleAgentStatusResponse struct {
	Connected bool   `json:"connected"`
	AgentId   string `json:"agentId,omitempty"`
	Uptime    int64  `json:"uptime,omitempty"`
}

type TaskRouting struct {
	Vision    string              `json:"vision,omitempty"`
	Audio     string              `json:"audio,omitempty"`
	Reasoning string              `json:"reasoning,omitempty"`
	Code      string              `json:"code,omitempty"`
	General   string              `json:"general,omitempty"`
	Fallbacks map[string][]string `json:"fallbacks,omitempty"`
}

type TestAuthProfileRequest struct {
	Id string `path:"id"`
}

type TestAuthProfileResponse struct {
	Success bool   `json:"success"`
	Message string `json:"message"`
	Model   string `json:"model,omitempty"`
}

type ToggleSkillRequest struct {
	Name string `path:"name"`
}

type ToggleSkillResponse struct {
	Name    string `json:"name"`
	Enabled bool   `json:"enabled"`
}

type CreateSkillRequest struct {
	Content string `json:"content"`          // Full SKILL.md text (YAML frontmatter + markdown body)
	Slug    string `json:"slug,omitempty"`   // Optional directory name override
}

type CreateSkillResponse struct {
	Skill ExtensionSkill `json:"skill"`
}

type UpdateSkillRequest struct {
	Name    string `path:"name"`
	Content string `json:"content"`
}

type UpdateSkillResponse struct {
	Skill ExtensionSkill `json:"skill"`
}

type DeleteSkillRequest struct {
	Name string `path:"name"`
}

type GetSkillContentRequest struct {
	Name string `path:"name"`
}

type GetSkillContentResponse struct {
	Content  string `json:"content"`
	Editable bool   `json:"editable"`
}

type UpdateAgentSettingsRequest struct {
	AutonomousMode           bool   `json:"autonomousMode"`
	AutoApproveRead          bool   `json:"autoApproveRead"`
	AutoApproveWrite         bool   `json:"autoApproveWrite"`
	AutoApproveBash          bool   `json:"autoApproveBash"`
	HeartbeatIntervalMinutes int    `json:"heartbeatIntervalMinutes"`
	CommEnabled              bool   `json:"commEnabled"`
	CommPlugin               string `json:"commPlugin,omitempty"`
	DeveloperMode            bool   `json:"developerMode"`
}

type UpdateAuthProfileRequest struct {
	Id       string `path:"id"`
	Name     string `json:"name,optional"`
	ApiKey   string `json:"apiKey,optional"`
	Model    string `json:"model,optional"`
	BaseUrl  string `json:"baseUrl,optional"`
	Priority int    `json:"priority,optional"`
	IsActive bool   `json:"isActive,optional"`
}

type UpdateChatRequest struct {
	Id    string `path:"id"`
	Title string `json:"title"`
}

type UpdateModelRequest struct {
	Provider  string   `path:"provider"`
	ModelId   string   `path:"modelId"`
	Active    *bool    `json:"active,optional"`
	Kind      []string `json:"kind,optional"`
	Preferred *bool    `json:"preferred,optional"`
}

type UpdatePersonalityRequest struct {
	Content string `json:"content"`
}

type UpdatePersonalityResponse struct {
	Success bool `json:"success"`
}

type UpdatePreferencesRequest struct {
	EmailNotifications bool   `json:"emailNotifications,optional"`
	MarketingEmails    bool   `json:"marketingEmails,optional"`
	Timezone           string `json:"timezone,optional"`
	Language           string `json:"language,optional"`
	Theme              string `json:"theme,optional"`
}

type UpdateTaskRoutingRequest struct {
	Vision    string              `json:"vision,omitempty"`
	Audio     string              `json:"audio,omitempty"`
	Reasoning string              `json:"reasoning,omitempty"`
	Code      string              `json:"code,omitempty"`
	General   string              `json:"general,omitempty"`
	Fallbacks map[string][]string `json:"fallbacks,omitempty"`
	Aliases   []ModelAlias        `json:"aliases,omitempty"`
}

type UpdateModelConfigRequest struct {
	Primary   string   `json:"primary,omitempty"`
	Fallbacks []string `json:"fallbacks,omitempty"`
}

type UpdateModelConfigResponse struct {
	Success bool   `json:"success"`
	Primary string `json:"primary"`
}

type UpdateUserRequest struct {
	Name      string `json:"name,optional"`
	AvatarUrl string `json:"avatarUrl,optional"`
}

type User struct {
	Id            string `json:"id"`
	Email         string `json:"email"`
	Name          string `json:"name"`
	AvatarUrl     string `json:"avatarUrl,omitempty"`
	EmailVerified bool   `json:"emailVerified"`
	CreatedAt     string `json:"createdAt"`
	UpdatedAt     string `json:"updatedAt"`
}

type UserPreferences struct {
	EmailNotifications bool   `json:"emailNotifications"`
	MarketingEmails    bool   `json:"marketingEmails"`
	Timezone           string `json:"timezone"`
	Language           string `json:"language"`
	Theme              string `json:"theme"`
}

// User Profile types

type UserProfile struct {
	UserId              string            `json:"userId"`
	DisplayName         string            `json:"displayName,omitempty"`
	Bio                 string            `json:"bio,omitempty"`
	Location            string            `json:"location,omitempty"`
	Timezone            string            `json:"timezone,omitempty"`
	Occupation          string            `json:"occupation,omitempty"`
	Interests           []string          `json:"interests,omitempty"`
	CommunicationStyle  string            `json:"communicationStyle,omitempty"`
	Goals               string            `json:"goals,omitempty"`
	Context             string            `json:"context,omitempty"`
	OnboardingCompleted bool              `json:"onboardingCompleted"`
	OnboardingStep      string            `json:"onboardingStep,omitempty"`
	ToolPermissions     map[string]bool   `json:"toolPermissions,omitempty"`
	TermsAcceptedAt     string            `json:"termsAcceptedAt,omitempty"`
	CreatedAt           string            `json:"createdAt"`
	UpdatedAt           string            `json:"updatedAt"`
}

type GetUserProfileResponse struct {
	Profile UserProfile `json:"profile"`
}

type UpdateUserProfileRequest struct {
	DisplayName         string   `json:"displayName,omitempty"`
	Bio                 string   `json:"bio,omitempty"`
	Location            string   `json:"location,omitempty"`
	Timezone            string   `json:"timezone,omitempty"`
	Occupation          string   `json:"occupation,omitempty"`
	Interests           []string `json:"interests,omitempty"`
	CommunicationStyle  string   `json:"communicationStyle,omitempty"`
	Goals               string   `json:"goals,omitempty"`
	Context             string   `json:"context,omitempty"`
	OnboardingCompleted *bool    `json:"onboardingCompleted,omitempty"`
}

type UpdateUserProfileResponse struct {
	Profile UserProfile `json:"profile"`
}

// Agent Profile types

type AgentProfileResponse struct {
	Name              string `json:"name"`
	PersonalityPreset string `json:"personalityPreset"`
	CustomPersonality string `json:"customPersonality,omitempty"`
	VoiceStyle        string `json:"voiceStyle"`
	ResponseLength    string `json:"responseLength"`
	EmojiUsage        string `json:"emojiUsage"`
	Formality         string `json:"formality"`
	Proactivity       string `json:"proactivity"`
	Emoji             string `json:"emoji,omitempty"`
	Creature          string `json:"creature,omitempty"`
	Vibe              string `json:"vibe,omitempty"`
	Role              string `json:"role,omitempty"`
	Avatar            string `json:"avatar,omitempty"`
	AgentRules        string `json:"agentRules,omitempty"`
	ToolNotes         string `json:"toolNotes,omitempty"`
	CreatedAt         string `json:"createdAt"`
	UpdatedAt         string `json:"updatedAt"`
}

type UpdateAgentProfileRequest struct {
	Name              string `json:"name,omitempty"`
	PersonalityPreset string `json:"personalityPreset,omitempty"`
	CustomPersonality string `json:"customPersonality,omitempty"`
	VoiceStyle        string `json:"voiceStyle,omitempty"`
	ResponseLength    string `json:"responseLength,omitempty"`
	EmojiUsage        string `json:"emojiUsage,omitempty"`
	Formality         string `json:"formality,omitempty"`
	Proactivity       string `json:"proactivity,omitempty"`
	Emoji             string `json:"emoji,omitempty"`
	Creature          string `json:"creature,omitempty"`
	Vibe              string `json:"vibe,omitempty"`
	Role              string `json:"role,omitempty"`
	Avatar            string `json:"avatar,omitempty"`
	AgentRules        string `json:"agentRules,omitempty"`
	ToolNotes         string `json:"toolNotes,omitempty"`
}

type SystemInfoResponse struct {
	OS       string `json:"os"`
	Arch     string `json:"arch"`
	Hostname string `json:"hostname"`
	HomeDir  string `json:"homeDir"`
	Username string `json:"username"`
}

type PersonalityPreset struct {
	Id           string `json:"id"`
	Name         string `json:"name"`
	Description  string `json:"description,omitempty"`
	SystemPrompt string `json:"systemPrompt"`
	Icon         string `json:"icon,omitempty"`
	DisplayOrder int    `json:"displayOrder"`
}

type ListPersonalityPresetsResponse struct {
	Presets []PersonalityPreset `json:"presets"`
}

// Memory types

type MemoryItem struct {
	Id          int64    `json:"id"`
	Namespace   string   `json:"namespace"`
	Key         string   `json:"key"`
	Value       string   `json:"value"`
	Tags        []string `json:"tags,omitempty"`
	AccessCount int64    `json:"accessCount"`
	CreatedAt   string   `json:"createdAt"`
	UpdatedAt   string   `json:"updatedAt"`
	AccessedAt  string   `json:"accessedAt,omitempty"`
}

type ListMemoriesRequest struct {
	Namespace string `form:"namespace,omitempty"`
	Page      int    `form:"page,omitempty"`
	PageSize  int    `form:"pageSize,omitempty"`
}

type ListMemoriesResponse struct {
	Memories []MemoryItem `json:"memories"`
	Total    int64        `json:"total"`
}

type GetMemoryRequest struct {
	Id int64 `path:"id"`
}

type GetMemoryResponse struct {
	Memory MemoryItem `json:"memory"`
}

type UpdateMemoryRequest struct {
	Id    int64    `path:"id"`
	Value string   `json:"value,omitempty"`
	Tags  []string `json:"tags,omitempty"`
}

type DeleteMemoryRequest struct {
	Id int64 `path:"id"`
}

type SearchMemoriesRequest struct {
	Query    string `form:"query"`
	Page     int    `form:"page,omitempty"`
	PageSize int    `form:"pageSize,omitempty"`
}

type SearchMemoriesResponse struct {
	Memories []MemoryItem `json:"memories"`
	Total    int64        `json:"total"`
}

type MemoryStatsResponse struct {
	TotalCount  int64            `json:"totalCount"`
	LayerCounts map[string]int64 `json:"layerCounts"`
	Namespaces  []string         `json:"namespaces"`
}

// Task/Cron types

type TaskItem struct {
	Id        int64  `json:"id"`
	Name      string `json:"name"`
	Schedule  string `json:"schedule"`
	Command   string `json:"command,omitempty"`
	TaskType  string `json:"taskType"`
	Message   string `json:"message,omitempty"`
	Deliver   string `json:"deliver,omitempty"`
	Enabled   bool   `json:"enabled"`
	LastRun   string `json:"lastRun,omitempty"`
	RunCount  int64  `json:"runCount"`
	LastError string `json:"lastError,omitempty"`
	CreatedAt string `json:"createdAt"`
}

type ListTasksRequest struct {
	Page     int `form:"page,omitempty"`
	PageSize int `form:"pageSize,omitempty"`
}

type ListTasksResponse struct {
	Tasks []TaskItem `json:"tasks"`
	Total int64      `json:"total"`
}

type GetTaskRequest struct {
	Name string `path:"name"`
}

type GetTaskResponse struct {
	Task TaskItem `json:"task"`
}

type CreateTaskRequest struct {
	Name     string `json:"name"`
	Schedule string `json:"schedule"`
	Command  string `json:"command,omitempty"`
	TaskType string `json:"taskType"`
	Message  string `json:"message,omitempty"`
	Deliver  string `json:"deliver,omitempty"`
	Enabled  bool   `json:"enabled"`
}

type CreateTaskResponse struct {
	Task TaskItem `json:"task"`
}

type UpdateTaskRequest struct {
	Name     string `json:"name,omitempty"`
	Schedule string `json:"schedule,omitempty"`
	Command  string `json:"command,omitempty"`
	TaskType string `json:"taskType,omitempty"`
	Message  string `json:"message,omitempty"`
	Deliver  string `json:"deliver,omitempty"`
}

type DeleteTaskRequest struct {
	Name string `path:"name"`
}

type ToggleTaskRequest struct {
	Name string `path:"name"`
}

type ToggleTaskResponse struct {
	Enabled bool `json:"enabled"`
}

type RunTaskRequest struct {
	Name string `path:"name"`
}

type RunTaskResponse struct {
	Success bool   `json:"success"`
	Output  string `json:"output,omitempty"`
	Error   string `json:"error,omitempty"`
}

type TaskHistoryItem struct {
	Id         int64  `json:"id"`
	JobId      int64  `json:"jobId"`
	StartedAt  string `json:"startedAt"`
	FinishedAt string `json:"finishedAt,omitempty"`
	Success    bool   `json:"success"`
	Output     string `json:"output,omitempty"`
	Error      string `json:"error,omitempty"`
}

type ListTaskHistoryRequest struct {
	Name     string `path:"name"`
	Page     int    `form:"page,omitempty"`
	PageSize int    `form:"pageSize,omitempty"`
}

type ListTaskHistoryResponse struct {
	History []TaskHistoryItem `json:"history"`
	Total   int64             `json:"total"`
}

// MCP Integration types

type MCPIntegration struct {
	Id               string `json:"id"`
	Name             string `json:"name"`
	ServerType       string `json:"serverType"`
	ServerUrl        string `json:"serverUrl,omitempty"`
	AuthType         string `json:"authType"`
	IsEnabled        bool   `json:"isEnabled"`
	ConnectionStatus string `json:"connectionStatus"`
	ToolCount        int    `json:"toolCount"`
	LastConnectedAt  string `json:"lastConnectedAt,omitempty"`
	LastError        string `json:"lastError,omitempty"`
	CreatedAt        string `json:"createdAt"`
	UpdatedAt        string `json:"updatedAt"`
}

type MCPServerInfo struct {
	Id                string `json:"id"`
	Name              string `json:"name"`
	Description       string `json:"description,omitempty"`
	Icon              string `json:"icon,omitempty"`
	AuthType          string `json:"authType"`
	ApiKeyUrl         string `json:"apiKeyUrl,omitempty"`
	ApiKeyPlaceholder string `json:"apiKeyPlaceholder,omitempty"`
	IsBuiltin         bool   `json:"isBuiltin"`
	DisplayOrder      int    `json:"displayOrder"`
}

type ListMCPIntegrationsResponse struct {
	Integrations []MCPIntegration `json:"integrations"`
}

type ListMCPServerRegistryResponse struct {
	Servers []MCPServerInfo `json:"servers"`
}

type GetMCPIntegrationRequest struct {
	Id string `path:"id"`
}

type GetMCPIntegrationResponse struct {
	Integration MCPIntegration `json:"integration"`
}

type CreateMCPIntegrationRequest struct {
	Name       string `json:"name,omitempty"`
	ServerType string `json:"serverType,omitempty"`
	ServerUrl  string `json:"serverUrl"`
	AuthType   string `json:"authType"`
	ApiKey     string `json:"apiKey,omitempty"`
}

type CreateMCPIntegrationResponse struct {
	Integration MCPIntegration `json:"integration"`
}

type UpdateMCPIntegrationRequest struct {
	Id        string `path:"id"`
	Name      string `json:"name,omitempty"`
	ServerUrl string `json:"serverUrl,omitempty"`
	IsEnabled *bool  `json:"isEnabled,omitempty"`
	ApiKey    string `json:"apiKey,omitempty"`
}

type UpdateMCPIntegrationResponse struct {
	Integration MCPIntegration `json:"integration"`
}

type DeleteMCPIntegrationRequest struct {
	Id string `path:"id"`
}

type TestMCPIntegrationRequest struct {
	Id string `path:"id"`
}

type TestMCPIntegrationResponse struct {
	Success   bool   `json:"success"`
	Message   string `json:"message"`
	ToolCount int    `json:"tool_count,omitempty"`
}

// MCP OAuth Client types

type GetMCPOAuthURLRequest struct {
	Id string `path:"id"`
}

type GetMCPOAuthURLResponse struct {
	AuthURL string `json:"authUrl"`
}

type DisconnectMCPIntegrationRequest struct {
	Id string `path:"id"`
}

type DisconnectMCPIntegrationResponse struct {
	Success bool   `json:"success"`
	Message string `json:"message"`
}

type MCPToolInfo struct {
	Name        string `json:"name"`
	Description string `json:"description,omitempty"`
	ServerType  string `json:"serverType"`
}

type ListMCPToolsResponse struct {
	Tools []MCPToolInfo `json:"tools"`
}

// Plugin Settings types

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
	SettingsManifest json.RawMessage   `json:"settingsManifest"`
	ConnectionStatus string            `json:"connectionStatus"`
	LastConnectedAt  string            `json:"lastConnectedAt,omitempty"`
	LastError        string            `json:"lastError,omitempty"`
	Settings         map[string]string `json:"settings,omitempty"`
	Capabilities     []string          `json:"capabilities,omitempty"`
	Permissions      []string          `json:"permissions,omitempty"`
	CreatedAt        string            `json:"createdAt"`
	UpdatedAt        string            `json:"updatedAt"`
}

type ListPluginsResponse struct {
	Plugins []PluginItem `json:"plugins"`
}

type GetPluginRequest struct {
	Id string `path:"id"`
}

type GetPluginResponse struct {
	Plugin PluginItem `json:"plugin"`
}

type UpdatePluginSettingsRequest struct {
	Id       string            `path:"id"`
	Settings map[string]string `json:"settings"`
	Secrets  map[string]bool   `json:"secrets,omitempty"`
}

type UpdatePluginSettingsResponse struct {
	Plugin PluginItem `json:"plugin"`
}

type TogglePluginRequest struct {
	Id        string `path:"id"`
	IsEnabled bool   `json:"isEnabled"`
}

// NeboLoop store types

type StoreAuthor struct {
	ID       string `json:"id"`
	Name     string `json:"name"`
	Verified bool   `json:"verified"`
}

type StoreApp struct {
	ID           string      `json:"id"`
	Name         string      `json:"name"`
	Slug         string      `json:"slug"`
	Description  string      `json:"description"`
	Icon         string      `json:"icon"`
	Category     string      `json:"category"`
	Version      string      `json:"version"`
	Author       StoreAuthor `json:"author"`
	InstallCount int         `json:"installCount"`
	Rating       float64     `json:"rating"`
	ReviewCount  int         `json:"reviewCount"`
	IsInstalled  bool        `json:"isInstalled"`
	Status       string      `json:"status"`
}

type StoreSkill struct {
	ID           string      `json:"id"`
	Name         string      `json:"name"`
	Slug         string      `json:"slug"`
	Description  string      `json:"description"`
	Icon         string      `json:"icon"`
	Category     string      `json:"category"`
	Version      string      `json:"version"`
	Author       StoreAuthor `json:"author"`
	InstallCount int         `json:"installCount"`
	Rating       float64     `json:"rating"`
	ReviewCount  int         `json:"reviewCount"`
	IsInstalled  bool        `json:"isInstalled"`
	Status       string      `json:"status"`
}

type ListStoreAppsResponse struct {
	Apps       []StoreApp `json:"apps"`
	TotalCount int        `json:"totalCount"`
	Page       int        `json:"page"`
	PageSize   int        `json:"pageSize"`
}

type ListStoreSkillsResponse struct {
	Skills     []StoreSkill `json:"skills"`
	TotalCount int          `json:"totalCount"`
	Page       int          `json:"page"`
	PageSize   int          `json:"pageSize"`
}

type StoreAppDetail struct {
	StoreApp
	AgeRating   string              `json:"ageRating,omitempty"`
	Platforms   []string            `json:"platforms,omitempty"`
	Size        map[string]int      `json:"size,omitempty"`
	Language    string              `json:"language,omitempty"`
	Screenshots []string            `json:"screenshots,omitempty"`
	Changelog   []StoreChangelog    `json:"changelog,omitempty"`
	WebsiteURL  string              `json:"websiteUrl,omitempty"`
	PrivacyURL  string              `json:"privacyUrl,omitempty"`
	SupportURL  string              `json:"supportUrl,omitempty"`
}

type StoreChangelog struct {
	Version string `json:"version"`
	Date    string `json:"date"`
	Notes   string `json:"notes"`
}

type StoreReview struct {
	ID        string `json:"id"`
	UserName  string `json:"userName"`
	Rating    int    `json:"rating"`
	Title     string `json:"title"`
	Body      string `json:"body"`
	CreatedAt string `json:"createdAt"`
	Helpful   int    `json:"helpful"`
}

type GetStoreAppResponse struct {
	App StoreAppDetail `json:"app"`
}

type GetStoreAppReviewsResponse struct {
	Reviews      []StoreReview `json:"reviews"`
	TotalCount   int           `json:"totalCount"`
	Average      float64       `json:"average"`
	Distribution [5]int        `json:"distribution"`
}

type InstallStoreAppResponse struct {
	PluginID string `json:"pluginId"`
	Message  string `json:"message"`
}

type InstallStoreSkillResponse struct {
	PluginID string `json:"pluginId"`
	Message  string `json:"message"`
}

// NeboLoop Connection Code types

type NeboLoopConnectRequest struct {
	Code    string `json:"code"`
	Name    string `json:"name"`
	Purpose string `json:"purpose,omitempty"`
}

type NeboLoopConnectResponse struct {
	BotID   string `json:"botId"`
	BotName string `json:"botName"`
	BotSlug string `json:"botSlug"`
	Message string `json:"message"`
}

type NeboLoopStatusResponse struct {
	Connected bool   `json:"connected"`
	BotID     string `json:"botId,omitempty"`
	BotName   string `json:"botName,omitempty"`
	APIServer string `json:"apiServer,omitempty"`
}

// NeboLoop Account types (owner registration/login)

type NeboLoopRegisterRequest struct {
	Email       string `json:"email"`
	DisplayName string `json:"displayName"`
	Password    string `json:"password"`
}

type NeboLoopRegisterResponse struct {
	ID          string `json:"id"`
	Email       string `json:"email"`
	DisplayName string `json:"displayName"`
	Token       string `json:"token"`
}

type NeboLoopLoginRequest struct {
	Email    string `json:"email"`
	Password string `json:"password"`
}

type NeboLoopLoginResponse struct {
	ID          string `json:"id"`
	Email       string `json:"email"`
	DisplayName string `json:"displayName"`
	Token       string `json:"token"`
}

type NeboLoopAccountStatusResponse struct {
	Connected   bool   `json:"connected"`
	OwnerID     string `json:"ownerId,omitempty"`
	Email       string `json:"email,omitempty"`
	DisplayName string `json:"displayName,omitempty"`
}

type NeboLoopDisconnectResponse struct {
	Disconnected bool `json:"disconnected"`
}

// Tool Permissions types

type ToolPermissions struct {
	Permissions map[string]bool `json:"permissions"`
}

type GetToolPermissionsResponse struct {
	Permissions map[string]bool `json:"permissions"`
}

type UpdateToolPermissionsRequest struct {
	Permissions map[string]bool `json:"permissions"`
}

type UpdateToolPermissionsResponse struct {
	Permissions map[string]bool `json:"permissions"`
}

type AcceptTermsResponse struct {
	AcceptedAt string `json:"acceptedAt"`
}

// Advisor types

type AdvisorItem struct {
	ID             int64  `json:"id"`
	Name           string `json:"name"`
	Role           string `json:"role"`
	Description    string `json:"description"`
	Priority       int    `json:"priority"`
	Enabled        bool   `json:"enabled"`
	MemoryAccess   bool   `json:"memoryAccess"`
	Persona        string `json:"persona"`
	TimeoutSeconds int    `json:"timeoutSeconds"`
}

type ListAdvisorsResponse struct {
	Advisors []AdvisorItem `json:"advisors"`
}

type GetAdvisorResponse struct {
	Advisor AdvisorItem `json:"advisor"`
}

type CreateAdvisorRequest struct {
	Name           string `json:"name"`
	Role           string `json:"role"`
	Description    string `json:"description"`
	Priority       int    `json:"priority"`
	MemoryAccess   bool   `json:"memoryAccess"`
	Persona        string `json:"persona"`
	TimeoutSeconds int    `json:"timeoutSeconds"`
}

type UpdateAdvisorRequest struct {
	Name           string  `path:"name"`
	Role           *string `json:"role,omitempty"`
	Description    *string `json:"description,omitempty"`
	Priority       *int    `json:"priority,omitempty"`
	Enabled        *bool   `json:"enabled,omitempty"`
	MemoryAccess   *bool   `json:"memoryAccess,omitempty"`
	Persona        *string `json:"persona,omitempty"`
	TimeoutSeconds *int    `json:"timeoutSeconds,omitempty"`
}

type DeleteAdvisorRequest struct {
	Name string `path:"name"`
}

type DeleteAdvisorResponse struct {
	Success bool `json:"success"`
}

type GetAdvisorRequest struct {
	Name string `path:"name"`
}

// Developer Mode types

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

// Tool execution types (Developer Window)

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

// ProjectContext provides full project state for the Dev Assistant system prompt.
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

// App UI types (structured template blocks)

type UIBlock struct {
	BlockID     string           `json:"block_id"`
	Type        string           `json:"type"`
	Text        string           `json:"text,omitempty"`
	Value       string           `json:"value,omitempty"`
	Placeholder string           `json:"placeholder,omitempty"`
	Hint        string           `json:"hint,omitempty"`
	Variant     string           `json:"variant,omitempty"`
	Src         string           `json:"src,omitempty"`
	Alt         string           `json:"alt,omitempty"`
	Disabled    bool             `json:"disabled,omitempty"`
	Options     []UISelectOption `json:"options,omitempty"`
	Style       string           `json:"style,omitempty"`
}

type UISelectOption struct {
	Label string `json:"label"`
	Value string `json:"value"`
}

type UIView struct {
	ViewID string    `json:"view_id"`
	Title  string    `json:"title"`
	Blocks []UIBlock `json:"blocks"`
}

type UIAppInfo struct {
	ID      string `json:"id"`
	Name    string `json:"name"`
	Version string `json:"version"`
}

type ListUIAppsResponse struct {
	Apps []UIAppInfo `json:"apps"`
}

type SendUIEventRequest struct {
	ViewID  string `json:"view_id"`
	BlockID string `json:"block_id"`
	Action  string `json:"action"`
	Value   string `json:"value"`
}

type SendUIEventResponse struct {
	View  *UIView `json:"view,omitempty"`
	Error string  `json:"error,omitempty"`
	Toast string  `json:"toast,omitempty"`
}
