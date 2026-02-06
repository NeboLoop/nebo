package types

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
	AutonomousMode           bool `json:"autonomousMode"`
	AutoApproveRead          bool `json:"autoApproveRead"`
	AutoApproveWrite         bool `json:"autoApproveWrite"`
	AutoApproveBash          bool `json:"autoApproveBash"`
	HeartbeatIntervalMinutes int  `json:"heartbeatIntervalMinutes"`
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
	Name        string   `json:"name"`
	Description string   `json:"description"`
	Version     string   `json:"version"`
	Triggers    []string `json:"triggers"`
	Tools       []string `json:"tools"`
	Priority    int      `json:"priority"`
	Enabled     bool     `json:"enabled"`
	FilePath    string   `json:"filePath"`
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

type ListModelsResponse struct {
	Models        map[string][]ModelInfo `json:"models"`
	TaskRouting   *TaskRouting           `json:"taskRouting,omitempty"`
	Aliases       []ModelAlias           `json:"aliases,omitempty"`
	AvailableCLIs *CLIAvailability       `json:"availableCLIs,omitempty"`
	CLIStatuses   *CLIStatusMap          `json:"cliStatuses,omitempty"`
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

type UpdateAgentSettingsRequest struct {
	AutonomousMode           bool `json:"autonomousMode"`
	AutoApproveRead          bool `json:"autoApproveRead"`
	AutoApproveWrite         bool `json:"autoApproveWrite"`
	AutoApproveBash          bool `json:"autoApproveBash"`
	HeartbeatIntervalMinutes int  `json:"heartbeatIntervalMinutes"`
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
	UserId              string   `json:"userId"`
	DisplayName         string   `json:"displayName,omitempty"`
	Bio                 string   `json:"bio,omitempty"`
	Location            string   `json:"location,omitempty"`
	Timezone            string   `json:"timezone,omitempty"`
	Occupation          string   `json:"occupation,omitempty"`
	Interests           []string `json:"interests,omitempty"`
	CommunicationStyle  string   `json:"communicationStyle,omitempty"`
	Goals               string   `json:"goals,omitempty"`
	Context             string   `json:"context,omitempty"`
	OnboardingCompleted bool     `json:"onboardingCompleted"`
	OnboardingStep      string   `json:"onboardingStep,omitempty"`
	CreatedAt           string   `json:"createdAt"`
	UpdatedAt           string   `json:"updatedAt"`
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
	Id int64 `path:"id"`
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
	Id int64 `path:"id"`
}

type ToggleTaskRequest struct {
	Id int64 `path:"id"`
}

type ToggleTaskResponse struct {
	Enabled bool `json:"enabled"`
}

type RunTaskRequest struct {
	Id int64 `path:"id"`
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
	Id       int64 `path:"id"`
	Page     int   `form:"page,omitempty"`
	PageSize int   `form:"pageSize,omitempty"`
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
	Name       string `json:"name"`
	ServerType string `json:"serverType"`
	ServerUrl  string `json:"serverUrl,omitempty"`
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
	Success bool   `json:"success"`
	Message string `json:"message"`
}

// Channel types

type ChannelItem struct {
	Id               string            `json:"id"`
	Name             string            `json:"name"`
	ChannelType      string            `json:"channelType"`
	IsEnabled        bool              `json:"isEnabled"`
	ConnectionStatus string            `json:"connectionStatus"`
	LastConnectedAt  string            `json:"lastConnectedAt,omitempty"`
	LastError        string            `json:"lastError,omitempty"`
	MessageCount     int64             `json:"messageCount"`
	Config           map[string]string `json:"config,omitempty"`
	CreatedAt        string            `json:"createdAt"`
	UpdatedAt        string            `json:"updatedAt"`
}

type ChannelRegistryItem struct {
	Id                  string   `json:"id"`
	Name                string   `json:"name"`
	Description         string   `json:"description,omitempty"`
	Icon                string   `json:"icon,omitempty"`
	SetupInstructions   string   `json:"setupInstructions,omitempty"`
	RequiredCredentials []string `json:"requiredCredentials"`
	OptionalCredentials []string `json:"optionalCredentials,omitempty"`
	DisplayOrder        int      `json:"displayOrder"`
}

type ListChannelsResponse struct {
	Channels []ChannelItem `json:"channels"`
}

type ListChannelRegistryResponse struct {
	Channels []ChannelRegistryItem `json:"channels"`
}

type GetChannelRequest struct {
	Id string `path:"id"`
}

type GetChannelResponse struct {
	Channel ChannelItem `json:"channel"`
}

type CreateChannelRequest struct {
	Name        string            `json:"name"`
	ChannelType string            `json:"channelType"`
	Credentials map[string]string `json:"credentials"`
	Config      map[string]string `json:"config,omitempty"`
}

type CreateChannelResponse struct {
	Channel ChannelItem `json:"channel"`
}

type UpdateChannelRequest struct {
	Id          string            `path:"id"`
	Name        string            `json:"name,omitempty"`
	IsEnabled   *bool             `json:"isEnabled,omitempty"`
	Credentials map[string]string `json:"credentials,omitempty"`
	Config      map[string]string `json:"config,omitempty"`
}

type UpdateChannelResponse struct {
	Channel ChannelItem `json:"channel"`
}

type DeleteChannelRequest struct {
	Id string `path:"id"`
}

type TestChannelRequest struct {
	Id string `path:"id"`
}

type TestChannelResponse struct {
	Success bool   `json:"success"`
	Message string `json:"message"`
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
