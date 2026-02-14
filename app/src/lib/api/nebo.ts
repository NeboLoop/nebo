import webapi from "./gocliRequest"
import * as components from "./neboComponents"
export * from "./neboComponents"

/**
 * @description "List advisors"
 */
export function listAdvisors() {
	return webapi.get<components.ListAdvisorsResponse>(`/api/v1/agent/advisors`)
}

/**
 * @description "Create advisor"
 * @param req
 */
export function createAdvisor(req: components.CreateAdvisorRequest) {
	return webapi.post<components.GetAdvisorResponse>(`/api/v1/agent/advisors`, req)
}

/**
 * @description "Delete advisor"
 * @param req
 */
export function deleteAdvisor(name: string) {
	return webapi.delete<components.DeleteAdvisorResponse>(`/api/v1/agent/advisors/${name}`)
}

/**
 * @description "Get advisor"
 * @param req
 */
export function getAdvisor(name: string) {
	return webapi.get<components.GetAdvisorResponse>(`/api/v1/agent/advisors/${name}`)
}

/**
 * @description "Update advisor"
 * @param req
 */
export function updateAdvisor(req: components.UpdateAdvisorRequest, name: string) {
	return webapi.put<components.GetAdvisorResponse>(`/api/v1/agent/advisors/${name}`, req)
}

/**
 * @description "Get heartbeat"
 */
export function getHeartbeat() {
	return webapi.get<components.GetHeartbeatResponse>(`/api/v1/agent/heartbeat`)
}

/**
 * @description "Update heartbeat"
 * @param req
 */
export function updateHeartbeat(req: components.UpdateHeartbeatRequest) {
	return webapi.put<components.UpdateHeartbeatResponse>(`/api/v1/agent/heartbeat`, req)
}

/**
 * @description "Get lanes"
 */
export function getLanes() {
	return webapi.get<components.MessageResponse>(`/api/v1/agent/lanes`)
}

/**
 * @description "List personality presets"
 */
export function listPersonalityPresets() {
	return webapi.get<components.ListPersonalityPresetsResponse>(`/api/v1/agent/personality-presets`)
}

/**
 * @description "Get agent profile"
 */
export function getAgentProfile() {
	return webapi.get<components.AgentProfileResponse>(`/api/v1/agent/profile`)
}

/**
 * @description "Update agent profile"
 * @param req
 */
export function updateAgentProfile(req: components.UpdateAgentProfileRequest) {
	return webapi.put<components.MessageResponse>(`/api/v1/agent/profile`, req)
}

/**
 * @description "List agent sessions"
 */
export function listAgentSessions() {
	return webapi.get<components.ListAgentSessionsResponse>(`/api/v1/agent/sessions`)
}

/**
 * @description "Delete agent session"
 * @param req
 */
export function deleteAgentSession(id: string) {
	return webapi.delete<components.MessageResponse>(`/api/v1/agent/sessions/${id}`)
}

/**
 * @description "Get agent session messages"
 */
export function getAgentSessionMessages(id: string) {
	return webapi.get<components.GetAgentSessionMessagesResponse>(`/api/v1/agent/sessions/${id}/messages`)
}

/**
 * @description "Get agent settings"
 */
export function getAgentSettings() {
	return webapi.get<components.GetAgentSettingsResponse>(`/api/v1/agent/settings`)
}

/**
 * @description "Update agent settings"
 * @param req
 */
export function updateAgentSettings(req: components.UpdateAgentSettingsRequest) {
	return webapi.put<components.MessageResponse>(`/api/v1/agent/settings`, req)
}

/**
 * @description "Get simple agent status"
 */
export function getSimpleAgentStatus() {
	return webapi.get<components.SimpleAgentStatusResponse>(`/api/v1/agent/status`)
}

/**
 * @description "Get system info"
 */
export function getSystemInfo() {
	return webapi.get<components.SystemInfoResponse>(`/api/v1/agent/system-info`)
}

/**
 * @description "List agents"
 * @param req
 */
export function listAgents() {
	return webapi.get<components.ListAgentsResponse>(`/api/v1/agents`)
}

/**
 * @description "Get agent status"
 */
export function getAgentStatus(agentId: string) {
	return webapi.get<components.MessageResponse>(`/api/v1/agents/${agentId}/status`)
}

/**
 * @description "List u i apps"
 */
export function listUIApps() {
	return webapi.get<components.ListUIAppsResponse>(`/api/v1/apps/ui`)
}

/**
 * @description "Grants"
 */
export function getAppOAuthGrants(appId: string) {
	return webapi.get<components.GetAppOAuthGrantsResponse>(`/api/v1/apps/${appId}/oauth/grants`)
}

/**
 * @description "Disconnect"
 */
export function disconnectAppOAuth(appId: string, provider: string) {
	return webapi.delete<components.MessageResponse>(`/api/v1/apps/${appId}/oauth/${provider}`)
}

/**
 * @description "Connect"
 */
export function getAppOAuthConnectUrl(appId: string, provider: string): string {
	return `/api/v1/apps/${appId}/oauth/${provider}/connect`
}

/**
 * @description "Get u i view"
 */
export function getUIView(id: string) {
	return webapi.get<components.UIView>(`/api/v1/apps/${id}/ui`)
}

/**
 * @description "Send u i event"
 * @param req
 */
export function sendUIEvent(req: components.SendUIEventRequest, id: string) {
	return webapi.post<components.SendUIEventResponse>(`/api/v1/apps/${id}/ui/event`, req)
}

/**
 * @description "Get auth config"
 */
export function getAuthConfig() {
	return webapi.get<components.AuthConfigResponse>(`/api/v1/auth/config`)
}

/**
 * @description "Dev login"
 */
export function devLogin() {
	return webapi.get<components.MessageResponse>(`/api/v1/auth/dev-login`)
}

/**
 * @description "Forgot password"
 * @param req
 */
export function forgotPassword(req: components.ForgotPasswordRequest) {
	return webapi.post<components.MessageResponse>(`/api/v1/auth/forgot-password`, req)
}

/**
 * @description "Login"
 * @param req
 */
export function login(req: components.LoginRequest) {
	return webapi.post<components.LoginResponse>(`/api/v1/auth/login`, req)
}

/**
 * @description "Refresh token"
 * @param req
 */
export function refreshToken(req: components.RefreshTokenRequest) {
	return webapi.post<components.RefreshTokenResponse>(`/api/v1/auth/refresh`, req)
}

/**
 * @description "Register"
 * @param req
 */
export function register(req: components.RegisterRequest) {
	return webapi.post<components.LoginResponse>(`/api/v1/auth/register`, req)
}

/**
 * @description "Resend verification"
 * @param req
 */
export function resendVerification(req: components.ResendVerificationRequest) {
	return webapi.post<components.MessageResponse>(`/api/v1/auth/resend-verification`, req)
}

/**
 * @description "Reset password"
 * @param req
 */
export function resetPassword(req: components.ResetPasswordRequest) {
	return webapi.post<components.MessageResponse>(`/api/v1/auth/reset-password`, req)
}

/**
 * @description "Verify email"
 */
export function verifyEmail() {
	return webapi.post<components.MessageResponse>(`/api/v1/auth/verify-email`)
}

/**
 * @description "List chats"
 * @param req
 */
export function listChats(params: components.ListChatsRequestParams) {
	return webapi.get<components.ListChatsResponse>(`/api/v1/chats`, params)
}

/**
 * @description "Create chat"
 * @param req
 */
export function createChat(req: components.CreateChatRequest) {
	return webapi.post<components.CreateChatResponse>(`/api/v1/chats`, req)
}

/**
 * @description "Get companion chat"
 */
export function getCompanionChat() {
	return webapi.get<components.GetChatResponse>(`/api/v1/chats/companion`)
}

/**
 * @description "List chat days"
 * @param req
 */
export function listChatDays(params: components.ListChatDaysRequestParams) {
	return webapi.get<components.ListChatDaysResponse>(`/api/v1/chats/days`, params)
}

/**
 * @description "Get history by day"
 * @param req
 */
export function getHistoryByDay(day: string) {
	return webapi.get<components.GetHistoryByDayResponse>(`/api/v1/chats/history/${day}`)
}

/**
 * @description "Send message"
 * @param req
 */
export function sendMessage(req: components.SendMessageRequest) {
	return webapi.post<components.SendMessageResponse>(`/api/v1/chats/message`, req)
}

/**
 * @description "Search chat messages"
 * @param req
 */
export function searchChatMessages(params: components.SearchChatMessagesRequestParams) {
	return webapi.get<components.SearchChatMessagesResponse>(`/api/v1/chats/search`, params)
}

/**
 * @description "Delete chat"
 * @param req
 */
export function deleteChat(id: string) {
	return webapi.delete<components.MessageResponse>(`/api/v1/chats/${id}`)
}

/**
 * @description "Get chat"
 * @param req
 */
export function getChat(id: string) {
	return webapi.get<components.GetChatResponse>(`/api/v1/chats/${id}`)
}

/**
 * @description "Update chat"
 * @param req
 */
export function updateChat(req: components.UpdateChatRequest, id: string) {
	return webapi.put<components.MessageResponse>(`/api/v1/chats/${id}`, req)
}

/**
 * @description "List dev apps"
 */
export function listDevApps() {
	return webapi.get<components.ListDevAppsResponse>(`/api/v1/dev/apps`)
}

/**
 * @description "Project context"
 */
export function projectContext(appId: string) {
	return webapi.get<components.ProjectContext>(`/api/v1/dev/apps/${appId}/context`)
}

/**
 * @description "Grpc stream"
 */
export function grpcStream(appId: string) {
	return webapi.get<components.MessageResponse>(`/api/v1/dev/apps/${appId}/grpc`)
}

/**
 * @description "Log stream"
 */
export function logStream(appId: string) {
	return webapi.get<components.MessageResponse>(`/api/v1/dev/apps/${appId}/logs`)
}

/**
 * @description "Relaunch dev app"
 */
export function relaunchDevApp(appId: string) {
	return webapi.post<components.MessageResponse>(`/api/v1/dev/apps/${appId}/relaunch`)
}

/**
 * @description "Browse directory"
 */
export function browseDirectory() {
	return webapi.post<components.BrowseDirectoryResponse>(`/api/v1/dev/browse-directory`)
}

/**
 * @description "Open dev window"
 */
export function openDevWindow() {
	return webapi.post<components.OpenDevWindowResponse>(`/api/v1/dev/open-window`)
}

/**
 * @description "Sideload"
 * @param req
 */
export function sideload(req: components.SideloadRequest) {
	return webapi.post<components.SideloadResponse>(`/api/v1/dev/sideload`, req)
}

/**
 * @description "Unsideload"
 */
export function unsideload(appId: string) {
	return webapi.delete<components.MessageResponse>(`/api/v1/dev/sideload/${appId}`)
}

/**
 * @description "List tools"
 */
export function listTools() {
	return webapi.get<components.ListToolsResponse>(`/api/v1/dev/tools`)
}

/**
 * @description "Tool execute"
 * @param req
 */
export function toolExecute(req: components.ToolExecuteRequest) {
	return webapi.post<components.ToolExecuteResponse>(`/api/v1/dev/tools/execute`, req)
}

/**
 * @description "List extensions"
 */
export function listExtensions() {
	return webapi.get<components.ListExtensionsResponse>(`/api/v1/extensions`)
}

/**
 * @description "List m c p integrations"
 */
export function listMCPIntegrations() {
	return webapi.get<components.ListMCPIntegrationsResponse>(`/api/v1/integrations`)
}

/**
 * @description "Create m c p integration"
 * @param req
 */
export function createMCPIntegration(req: components.CreateMCPIntegrationRequest) {
	return webapi.post<components.CreateMCPIntegrationResponse>(`/api/v1/integrations`, req)
}

/**
 * @description "List m c p server registry"
 */
export function listMCPServerRegistry() {
	return webapi.get<components.ListMCPServerRegistryResponse>(`/api/v1/integrations/registry`)
}

/**
 * @description "List m c p tools"
 */
export function listMCPTools() {
	return webapi.get<components.ListMCPToolsResponse>(`/api/v1/integrations/tools`)
}

/**
 * @description "Delete m c p integration"
 * @param req
 */
export function deleteMCPIntegration(id: string) {
	return webapi.delete<components.MessageResponse>(`/api/v1/integrations/${id}`)
}

/**
 * @description "Get m c p integration"
 * @param req
 */
export function getMCPIntegration(id: string) {
	return webapi.get<components.GetMCPIntegrationResponse>(`/api/v1/integrations/${id}`)
}

/**
 * @description "Update m c p integration"
 * @param req
 */
export function updateMCPIntegration(req: components.UpdateMCPIntegrationRequest, id: string) {
	return webapi.put<components.UpdateMCPIntegrationResponse>(`/api/v1/integrations/${id}`, req)
}

/**
 * @description "Disconnect m c p integration"
 * @param req
 */
export function disconnectMCPIntegration(id: string) {
	return webapi.post<components.DisconnectMCPIntegrationResponse>(`/api/v1/integrations/${id}/disconnect`)
}

/**
 * @description "Get m c p o auth u r l"
 * @param req
 */
export function getMCPOAuthURL(id: string) {
	return webapi.get<components.GetMCPOAuthURLResponse>(`/api/v1/integrations/${id}/oauth-url`)
}

/**
 * @description "Test m c p integration"
 * @param req
 */
export function testMCPIntegration(id: string) {
	return webapi.post<components.TestMCPIntegrationResponse>(`/api/v1/integrations/${id}/test`)
}

/**
 * @description "List memories"
 * @param req
 */
export function listMemories(params: components.ListMemoriesRequestParams) {
	return webapi.get<components.ListMemoriesResponse>(`/api/v1/memories`, params)
}

/**
 * @description "Search memories"
 * @param req
 */
export function searchMemories(params: components.SearchMemoriesRequestParams) {
	return webapi.get<components.SearchMemoriesResponse>(`/api/v1/memories/search`, params)
}

/**
 * @description "Get memory stats"
 */
export function getMemoryStats() {
	return webapi.get<components.MemoryStatsResponse>(`/api/v1/memories/stats`)
}

/**
 * @description "Delete memory"
 * @param req
 */
export function deleteMemory(id: string) {
	return webapi.delete<components.MessageResponse>(`/api/v1/memories/${id}`)
}

/**
 * @description "Get memory"
 * @param req
 */
export function getMemory(id: string) {
	return webapi.get<components.GetMemoryResponse>(`/api/v1/memories/${id}`)
}

/**
 * @description "Update memory"
 * @param req
 */
export function updateMemory(req: components.UpdateMemoryRequest, id: string) {
	return webapi.put<components.GetMemoryResponse>(`/api/v1/memories/${id}`, req)
}

/**
 * @description "List models"
 */
export function listModels() {
	return webapi.get<components.ListModelsResponse>(`/api/v1/models`)
}

/**
 * @description "Update model config"
 * @param req
 */
export function updateModelConfig(req: components.UpdateModelConfigRequest) {
	return webapi.put<components.UpdateModelConfigResponse>(`/api/v1/models/config`, req)
}

/**
 * @description "Update task routing"
 * @param req
 */
export function updateTaskRouting(req: components.UpdateTaskRoutingRequest) {
	return webapi.put<components.MessageResponse>(`/api/v1/models/task-routing`, req)
}

/**
 * @description "Update model"
 * @param req
 */
export function updateModel(req: components.UpdateModelRequest, provider: string, modelId: string) {
	return webapi.put<components.MessageResponse>(`/api/v1/models/${provider}/${modelId}`, req)
}

/**
 * @description "Nebo loop disconnect"
 */
export function neboLoopDisconnect() {
	return webapi.delete<components.NeboLoopDisconnectResponse>(`/api/v1/neboloop/account`)
}

/**
 * @description "Nebo loop account status"
 */
export function neboLoopAccountStatus() {
	return webapi.get<components.NeboLoopAccountStatusResponse>(`/api/v1/neboloop/account`)
}

/**
 * @description "Nebo loop connect"
 * @param req
 */
export function neboLoopConnect(req: components.NeboLoopConnectRequest) {
	return webapi.post<components.NeboLoopConnectResponse>(`/api/v1/neboloop/connect`, req)
}

/**
 * @description "Nebo loop login"
 * @param req
 */
export function neboLoopLogin(req: components.NeboLoopLoginRequest) {
	return webapi.post<components.NeboLoopLoginResponse>(`/api/v1/neboloop/login`, req)
}

/**
 * @description "Nebo loop register"
 * @param req
 */
export function neboLoopRegister(req: components.NeboLoopRegisterRequest) {
	return webapi.post<components.NeboLoopRegisterResponse>(`/api/v1/neboloop/register`, req)
}

/**
 * @description "Nebo loop status"
 */
export function neboLoopStatus() {
	return webapi.get<components.NeboLoopStatusResponse>(`/api/v1/neboloop/status`)
}

/**
 * @description "List notifications"
 * @param req
 */
export function listNotifications(params: components.ListNotificationsRequestParams) {
	return webapi.get<components.ListNotificationsResponse>(`/api/v1/notifications`, params)
}

/**
 * @description "Mark all notifications read"
 */
export function markAllNotificationsRead() {
	return webapi.put<components.MessageResponse>(`/api/v1/notifications/read-all`)
}

/**
 * @description "Get unread count"
 */
export function getUnreadCount() {
	return webapi.get<components.GetUnreadCountResponse>(`/api/v1/notifications/unread-count`)
}

/**
 * @description "Delete notification"
 * @param req
 */
export function deleteNotification(id: string) {
	return webapi.delete<components.MessageResponse>(`/api/v1/notifications/${id}`)
}

/**
 * @description "Mark notification read"
 * @param req
 */
export function markNotificationRead(id: string) {
	return webapi.put<components.MessageResponse>(`/api/v1/notifications/${id}/read`)
}

/**
 * @description "List o auth providers"
 */
export function listOAuthProviders() {
	return webapi.get<components.ListOAuthProvidersResponse>(`/api/v1/oauth/providers`)
}

/**
 * @description "Disconnect o auth"
 * @param req
 */
export function disconnectOAuth(provider: string) {
	return webapi.delete<components.MessageResponse>(`/api/v1/oauth/${provider}`)
}

/**
 * @description "O auth callback"
 */
export function oAuthCallback(provider: string) {
	return webapi.post<components.MessageResponse>(`/api/v1/oauth/${provider}/callback`)
}

/**
 * @description "Get o auth url"
 * @param req
 */
export function getOAuthUrl(params: components.GetOAuthUrlRequestParams, provider: string) {
	return webapi.get<components.GetOAuthUrlResponse>(`/api/v1/oauth/${provider}/url`, params)
}

/**
 * @description "List plugins"
 */
export function listPlugins() {
	return webapi.get<components.ListPluginsResponse>(`/api/v1/plugins`)
}

/**
 * @description "Get plugin"
 * @param req
 */
export function getPlugin(id: string) {
	return webapi.get<components.GetPluginResponse>(`/api/v1/plugins/${id}`)
}

/**
 * @description "Update plugin settings"
 * @param req
 */
export function updatePluginSettings(req: components.UpdatePluginSettingsRequest, id: string) {
	return webapi.put<components.UpdatePluginSettingsResponse>(`/api/v1/plugins/${id}/settings`, req)
}

/**
 * @description "Toggle plugin"
 * @param req
 */
export function togglePlugin(req: components.TogglePluginRequest, id: string) {
	return webapi.put<components.MessageResponse>(`/api/v1/plugins/${id}/toggle`, req)
}

/**
 * @description "List auth profiles"
 */
export function listAuthProfiles() {
	return webapi.get<components.ListAuthProfilesResponse>(`/api/v1/providers`)
}

/**
 * @description "Create auth profile"
 * @param req
 */
export function createAuthProfile(req: components.CreateAuthProfileRequest) {
	return webapi.post<components.CreateAuthProfileResponse>(`/api/v1/providers`, req)
}

/**
 * @description "Delete auth profile"
 * @param req
 */
export function deleteAuthProfile(id: string) {
	return webapi.delete<components.MessageResponse>(`/api/v1/providers/${id}`)
}

/**
 * @description "Get auth profile"
 * @param req
 */
export function getAuthProfile(id: string) {
	return webapi.get<components.GetAuthProfileResponse>(`/api/v1/providers/${id}`)
}

/**
 * @description "Update auth profile"
 * @param req
 */
export function updateAuthProfile(req: components.UpdateAuthProfileRequest, id: string) {
	return webapi.put<components.MessageResponse>(`/api/v1/providers/${id}`, req)
}

/**
 * @description "Test auth profile"
 * @param req
 */
export function testAuthProfile(id: string) {
	return webapi.post<components.TestAuthProfileResponse>(`/api/v1/providers/${id}/test`)
}

/**
 * @description "Create admin"
 * @param req
 */
export function createAdmin(req: components.CreateAdminRequest) {
	return webapi.post<components.CreateAdminResponse>(`/api/v1/setup/admin`, req)
}

/**
 * @description "Complete setup"
 */
export function completeSetup() {
	return webapi.post<components.CompleteSetupResponse>(`/api/v1/setup/complete`)
}

/**
 * @description "Get personality"
 */
export function getPersonality() {
	return webapi.get<components.GetPersonalityResponse>(`/api/v1/setup/personality`)
}

/**
 * @description "Update personality"
 * @param req
 */
export function updatePersonality(req: components.UpdatePersonalityRequest) {
	return webapi.put<components.UpdatePersonalityResponse>(`/api/v1/setup/personality`, req)
}

/**
 * @description "Setup status"
 */
export function setupStatus() {
	return webapi.get<components.SetupStatusResponse>(`/api/v1/setup/status`)
}

/**
 * @description "Create skill"
 * @param req
 */
export function createSkill(req: components.CreateSkillRequest) {
	return webapi.post<components.CreateSkillResponse>(`/api/v1/skills`, req)
}

/**
 * @description "Delete skill"
 * @param req
 */
export function deleteSkill(name: string) {
	return webapi.delete<components.MessageResponse>(`/api/v1/skills/${name}`)
}

/**
 * @description "Get skill"
 * @param req
 */
export function getSkill(name: string) {
	return webapi.get<components.GetSkillResponse>(`/api/v1/skills/${name}`)
}

/**
 * @description "Update skill"
 * @param req
 */
export function updateSkill(req: components.UpdateSkillRequest, name: string) {
	return webapi.put<components.UpdateSkillResponse>(`/api/v1/skills/${name}`, req)
}

/**
 * @description "Get skill content"
 * @param req
 */
export function getSkillContent(name: string) {
	return webapi.get<components.GetSkillContentResponse>(`/api/v1/skills/${name}/content`)
}

/**
 * @description "Toggle skill"
 * @param req
 */
export function toggleSkill(name: string) {
	return webapi.post<components.ToggleSkillResponse>(`/api/v1/skills/${name}/toggle`)
}

/**
 * @description "List store apps"
 */
export function listStoreApps() {
	return webapi.get<components.ListStoreAppsResponse>(`/api/v1/store/apps`)
}

/**
 * @description "Get store app"
 */
export function getStoreApp(id: string) {
	return webapi.get<components.GetStoreAppResponse>(`/api/v1/store/apps/${id}`)
}

/**
 * @description "Uninstall store app"
 */
export function uninstallStoreApp(id: string) {
	return webapi.delete<components.MessageResponse>(`/api/v1/store/apps/${id}/install`)
}

/**
 * @description "Install store app"
 */
export function installStoreApp(id: string) {
	return webapi.post<components.InstallStoreAppResponse>(`/api/v1/store/apps/${id}/install`)
}

/**
 * @description "Get store app reviews"
 */
export function getStoreAppReviews(id: string) {
	return webapi.get<components.GetStoreAppReviewsResponse>(`/api/v1/store/apps/${id}/reviews`)
}

/**
 * @description "List store skills"
 */
export function listStoreSkills() {
	return webapi.get<components.ListStoreSkillsResponse>(`/api/v1/store/skills`)
}

/**
 * @description "Uninstall store skill"
 */
export function uninstallStoreSkill(id: string) {
	return webapi.delete<components.MessageResponse>(`/api/v1/store/skills/${id}/install`)
}

/**
 * @description "Install store skill"
 */
export function installStoreSkill(id: string) {
	return webapi.post<components.InstallStoreSkillResponse>(`/api/v1/store/skills/${id}/install`)
}

/**
 * @description "List tasks"
 * @param req
 */
export function listTasks(params: components.ListTasksRequestParams) {
	return webapi.get<components.ListTasksResponse>(`/api/v1/tasks`, params)
}

/**
 * @description "Create task"
 * @param req
 */
export function createTask(req: components.CreateTaskRequest) {
	return webapi.post<components.CreateTaskResponse>(`/api/v1/tasks`, req)
}

/**
 * @description "Delete task"
 * @param req
 */
export function deleteTask(name: string) {
	return webapi.delete<components.MessageResponse>(`/api/v1/tasks/${name}`)
}

/**
 * @description "Get task"
 * @param req
 */
export function getTask(name: string) {
	return webapi.get<components.GetTaskResponse>(`/api/v1/tasks/${name}`)
}

/**
 * @description "Update task"
 * @param req
 */
export function updateTask(req: components.UpdateTaskRequest, name: string) {
	return webapi.put<components.MessageResponse>(`/api/v1/tasks/${name}`, req)
}

/**
 * @description "List task history"
 * @param req
 */
export function listTaskHistory(params: components.ListTaskHistoryRequestParams, name: string) {
	return webapi.get<components.ListTaskHistoryResponse>(`/api/v1/tasks/${name}/history`, params)
}

/**
 * @description "Run task"
 * @param req
 */
export function runTask(name: string) {
	return webapi.post<components.RunTaskResponse>(`/api/v1/tasks/${name}/run`)
}

/**
 * @description "Toggle task"
 * @param req
 */
export function toggleTask(name: string) {
	return webapi.post<components.ToggleTaskResponse>(`/api/v1/tasks/${name}/toggle`)
}

/**
 * @description "Delete account"
 * @param req
 */
export function deleteAccount(req: components.DeleteAccountRequest) {
	return webapi.delete<components.MessageResponse>(`/api/v1/user/me`)
}

/**
 * @description "Get current user"
 */
export function getCurrentUser() {
	return webapi.get<components.GetUserResponse>(`/api/v1/user/me`)
}

/**
 * @description "Update current user"
 * @param req
 */
export function updateCurrentUser(req: components.UpdateUserRequest) {
	return webapi.put<components.GetUserResponse>(`/api/v1/user/me`, req)
}

/**
 * @description "Accept terms"
 */
export function acceptTerms() {
	return webapi.post<components.AcceptTermsResponse>(`/api/v1/user/me/accept-terms`)
}

/**
 * @description "Change password"
 * @param req
 */
export function changePassword(req: components.ChangePasswordRequest) {
	return webapi.post<components.MessageResponse>(`/api/v1/user/me/change-password`, req)
}

/**
 * @description "Get tool permissions"
 */
export function getToolPermissions() {
	return webapi.get<components.GetToolPermissionsResponse>(`/api/v1/user/me/permissions`)
}

/**
 * @description "Update tool permissions"
 * @param req
 */
export function updateToolPermissions(req: components.UpdateToolPermissionsRequest) {
	return webapi.put<components.UpdateToolPermissionsResponse>(`/api/v1/user/me/permissions`, req)
}

/**
 * @description "Get preferences"
 */
export function getPreferences() {
	return webapi.get<components.GetPreferencesResponse>(`/api/v1/user/me/preferences`)
}

/**
 * @description "Update preferences"
 * @param req
 */
export function updatePreferences(req: components.UpdatePreferencesRequest) {
	return webapi.put<components.MessageResponse>(`/api/v1/user/me/preferences`, req)
}

/**
 * @description "Get user profile"
 */
export function getUserProfile() {
	return webapi.get<components.GetUserProfileResponse>(`/api/v1/user/me/profile`)
}

/**
 * @description "Update user profile"
 * @param req
 */
export function updateUserProfile(req: components.UpdateUserProfileRequest) {
	return webapi.put<components.UpdateUserProfileResponse>(`/api/v1/user/me/profile`, req)
}

/**
 * @description "Health check"
 */
export function healthCheck() {
	return webapi.get<components.MessageResponse>(`/health`)
}

