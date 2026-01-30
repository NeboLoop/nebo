import webapi from "./gocliRequest"
import * as components from "./gobotComponents"
export * from "./gobotComponents"

/**
 * @description "Health check endpoint"
 */
export function healthCheck() {
	return webapi.get<components.HealthResponse>(`/health`)
}

/**
 * @description "List agent sessions"
 */
export function listAgentSessions() {
	return webapi.get<components.ListAgentSessionsResponse>(`/api/v1/agent/sessions`)
}

/**
 * @description "Delete agent session"
 * @param params
 */
export function deleteAgentSession(params: components.DeleteAgentSessionRequestParams, id: string) {
	return webapi.delete<components.MessageResponse>(`/api/v1/agent/sessions/${id}`, params)
}

/**
 * @description "Get session messages"
 * @param params
 */
export function getAgentSessionMessages(params: components.GetAgentSessionRequestParams, id: string) {
	return webapi.get<components.GetAgentSessionMessagesResponse>(`/api/v1/agent/sessions/${id}/messages`, params)
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
	return webapi.put<components.GetAgentSettingsResponse>(`/api/v1/agent/settings`, req)
}

/**
 * @description "Get simple agent status (single agent model)"
 */
export function getSimpleAgentStatus() {
	return webapi.get<components.SimpleAgentStatusResponse>(`/api/v1/agent/status`)
}

/**
 * @description "List connected agents"
 */
export function listAgents() {
	return webapi.get<components.ListAgentsResponse>(`/api/v1/agents`)
}

/**
 * @description "Get agent status"
 * @param params
 */
export function getAgentStatus(params: components.AgentStatusRequestParams, agentId: string) {
	return webapi.get<components.AgentStatusResponse>(`/api/v1/agents/${agentId}/status`, params)
}

/**
 * @description "Get auth configuration (OAuth providers enabled)"
 */
export function getAuthConfig() {
	return webapi.get<components.AuthConfigResponse>(`/api/v1/auth/config`)
}

/**
 * @description "Dev auto-login (local development only)"
 */
export function devLogin() {
	return webapi.get<components.LoginResponse>(`/api/v1/auth/dev-login`)
}

/**
 * @description "Request password reset"
 * @param req
 */
export function forgotPassword(req: components.ForgotPasswordRequest) {
	return webapi.post<components.MessageResponse>(`/api/v1/auth/forgot-password`, req)
}

/**
 * @description "User login"
 * @param req
 */
export function login(req: components.LoginRequest) {
	return webapi.post<components.LoginResponse>(`/api/v1/auth/login`, req)
}

/**
 * @description "Refresh authentication token"
 * @param req
 */
export function refreshToken(req: components.RefreshTokenRequest) {
	return webapi.post<components.RefreshTokenResponse>(`/api/v1/auth/refresh`, req)
}

/**
 * @description "Register new user"
 * @param req
 */
export function register(req: components.RegisterRequest) {
	return webapi.post<components.LoginResponse>(`/api/v1/auth/register`, req)
}

/**
 * @description "Resend email verification"
 * @param req
 */
export function resendVerification(req: components.ResendVerificationRequest) {
	return webapi.post<components.MessageResponse>(`/api/v1/auth/resend-verification`, req)
}

/**
 * @description "Reset password with token"
 * @param req
 */
export function resetPassword(req: components.ResetPasswordRequest) {
	return webapi.post<components.MessageResponse>(`/api/v1/auth/reset-password`, req)
}

/**
 * @description "Verify email address with token"
 * @param req
 */
export function verifyEmail(req: components.EmailVerificationRequest) {
	return webapi.post<components.MessageResponse>(`/api/v1/auth/verify-email`, req)
}

/**
 * @description "List user chats"
 * @param params
 */
export function listChats(params: components.ListChatsRequestParams) {
	return webapi.get<components.ListChatsResponse>(`/api/v1/chats`, params)
}

/**
 * @description "Create new chat"
 * @param req
 */
export function createChat(req: components.CreateChatRequest) {
	return webapi.post<components.CreateChatResponse>(`/api/v1/chats`, req)
}

/**
 * @description "Get chat with messages"
 * @param params
 */
export function getChat(params: components.GetChatRequestParams, id: string) {
	return webapi.get<components.GetChatResponse>(`/api/v1/chats/${id}`, params)
}

/**
 * @description "Update chat title"
 * @param params
 * @param req
 */
export function updateChat(params: components.UpdateChatRequestParams, req: components.UpdateChatRequest, id: string) {
	return webapi.put<components.Chat>(`/api/v1/chats/${id}`, params, req)
}

/**
 * @description "Delete chat"
 * @param params
 */
export function deleteChat(params: components.DeleteChatRequestParams, id: string) {
	return webapi.delete<components.MessageResponse>(`/api/v1/chats/${id}`, params)
}

/**
 * @description "Get companion chat (auto-creates if needed)"
 */
export function getCompanionChat() {
	return webapi.get<components.GetChatResponse>(`/api/v1/chats/companion`)
}

/**
 * @description "List days with messages for history browsing"
 * @param params
 */
export function listChatDays(params: components.ListChatDaysRequestParams) {
	return webapi.get<components.ListChatDaysResponse>(`/api/v1/chats/days`, params)
}

/**
 * @description "Get messages for a specific day"
 * @param params
 */
export function getHistoryByDay(params: components.GetHistoryByDayRequestParams, day: string) {
	return webapi.get<components.GetHistoryByDayResponse>(`/api/v1/chats/history/${day}`, params)
}

/**
 * @description "Send message (creates chat if needed)"
 * @param req
 */
export function sendMessage(req: components.SendMessageRequest) {
	return webapi.post<components.SendMessageResponse>(`/api/v1/chats/message`, req)
}

/**
 * @description "Search chat messages"
 * @param params
 */
export function searchChatMessages(params: components.SearchChatMessagesRequestParams) {
	return webapi.get<components.SearchChatMessagesResponse>(`/api/v1/chats/search`, params)
}

/**
 * @description "List all extensions (tools, skills, plugins)"
 */
export function listExtensions() {
	return webapi.get<components.ListExtensionsResponse>(`/api/v1/extensions`)
}

/**
 * @description "Get single skill details"
 * @param params
 */
export function getSkill(params: components.GetSkillRequestParams, name: string) {
	return webapi.get<components.GetSkillResponse>(`/api/v1/skills/${name}`, params)
}

/**
 * @description "Toggle skill enabled/disabled"
 * @param params
 */
export function toggleSkill(params: components.ToggleSkillRequestParams, name: string) {
	return webapi.post<components.ToggleSkillResponse>(`/api/v1/skills/${name}/toggle`, params)
}

/**
 * @description "List user notifications"
 * @param params
 */
export function listNotifications(params: components.ListNotificationsRequestParams) {
	return webapi.get<components.ListNotificationsResponse>(`/api/v1/notifications`, params)
}

/**
 * @description "Delete notification"
 * @param params
 */
export function deleteNotification(params: components.DeleteNotificationRequestParams, id: string) {
	return webapi.delete<components.MessageResponse>(`/api/v1/notifications/${id}`, params)
}

/**
 * @description "Mark notification as read"
 * @param params
 */
export function markNotificationRead(params: components.MarkNotificationReadRequestParams, id: string) {
	return webapi.put<components.MessageResponse>(`/api/v1/notifications/${id}/read`, params)
}

/**
 * @description "Mark all notifications as read"
 */
export function markAllNotificationsRead() {
	return webapi.put<components.MessageResponse>(`/api/v1/notifications/read-all`)
}

/**
 * @description "Get unread notification count"
 */
export function getUnreadCount() {
	return webapi.get<components.GetUnreadCountResponse>(`/api/v1/notifications/unread-count`)
}

/**
 * @description "OAuth callback - exchange code for tokens"
 * @param params
 * @param req
 */
export function oAuthCallback(params: components.OAuthLoginRequestParams, req: components.OAuthLoginRequest, provider: string) {
	return webapi.post<components.OAuthLoginResponse>(`/api/v1/oauth/${provider}/callback`, params, req)
}

/**
 * @description "Get OAuth authorization URL"
 * @param params
 */
export function getOAuthUrl(params: components.GetOAuthUrlRequestParams, provider: string) {
	return webapi.get<components.GetOAuthUrlResponse>(`/api/v1/oauth/${provider}/url`, params)
}

/**
 * @description "Disconnect OAuth provider"
 * @param params
 */
export function disconnectOAuth(params: components.DisconnectOAuthRequestParams, provider: string) {
	return webapi.delete<components.MessageResponse>(`/api/v1/oauth/${provider}`, params)
}

/**
 * @description "List connected OAuth providers"
 */
export function listOAuthProviders() {
	return webapi.get<components.ListOAuthProvidersResponse>(`/api/v1/oauth/providers`)
}

/**
 * @description "List all available models from YAML cache"
 */
export function listModels() {
	return webapi.get<components.ListModelsResponse>(`/api/v1/models`)
}

/**
 * @description "Update model settings (active, kind, preferred)"
 * @param params
 * @param req
 */
export function updateModel(params: components.UpdateModelRequestParams, req: components.UpdateModelRequest, provider: string, modelId: string) {
	return webapi.put<components.MessageResponse>(`/api/v1/models/${provider}/${modelId}`, params, req)
}

/**
 * @description "Update task routing configuration"
 * @param req
 */
export function updateTaskRouting(req: components.UpdateTaskRoutingRequest) {
	return webapi.put<components.MessageResponse>(`/api/v1/models/task-routing`, req)
}

/**
 * @description "List all auth profiles (API keys)"
 */
export function listAuthProfiles() {
	return webapi.get<components.ListAuthProfilesResponse>(`/api/v1/providers`)
}

/**
 * @description "Create a new auth profile"
 * @param req
 */
export function createAuthProfile(req: components.CreateAuthProfileRequest) {
	return webapi.post<components.CreateAuthProfileResponse>(`/api/v1/providers`, req)
}

/**
 * @description "Get auth profile by ID"
 * @param params
 */
export function getAuthProfile(params: components.GetAuthProfileRequestParams, id: string) {
	return webapi.get<components.GetAuthProfileResponse>(`/api/v1/providers/${id}`, params)
}

/**
 * @description "Update auth profile"
 * @param params
 * @param req
 */
export function updateAuthProfile(params: components.UpdateAuthProfileRequestParams, req: components.UpdateAuthProfileRequest, id: string) {
	return webapi.put<components.GetAuthProfileResponse>(`/api/v1/providers/${id}`, params, req)
}

/**
 * @description "Delete auth profile"
 * @param params
 */
export function deleteAuthProfile(params: components.DeleteAuthProfileRequestParams, id: string) {
	return webapi.delete<components.MessageResponse>(`/api/v1/providers/${id}`, params)
}

/**
 * @description "Test auth profile (verify API key works)"
 * @param params
 */
export function testAuthProfile(params: components.TestAuthProfileRequestParams, id: string) {
	return webapi.post<components.TestAuthProfileResponse>(`/api/v1/providers/${id}/test`, params)
}

/**
 * @description "Create the first admin user (only works when no admin exists)"
 * @param req
 */
export function createAdmin(req: components.CreateAdminRequest) {
	return webapi.post<components.CreateAdminResponse>(`/api/v1/setup/admin`, req)
}

/**
 * @description "Mark initial setup as complete"
 */
export function completeSetup() {
	return webapi.post<components.CompleteSetupResponse>(`/api/v1/setup/complete`)
}

/**
 * @description "Get AI personality configuration"
 */
export function getPersonality() {
	return webapi.get<components.GetPersonalityResponse>(`/api/v1/setup/personality`)
}

/**
 * @description "Update AI personality configuration"
 * @param req
 */
export function updatePersonality(req: components.UpdatePersonalityRequest) {
	return webapi.put<components.UpdatePersonalityResponse>(`/api/v1/setup/personality`, req)
}

/**
 * @description "Check if setup is required (no admin exists)"
 */
export function setupStatus() {
	return webapi.get<components.SetupStatusResponse>(`/api/v1/setup/status`)
}

/**
 * @description "Get current user profile"
 */
export function getCurrentUser() {
	return webapi.get<components.GetUserResponse>(`/api/v1/user/me`)
}

/**
 * @description "Update current user profile"
 * @param req
 */
export function updateCurrentUser(req: components.UpdateUserRequest) {
	return webapi.put<components.GetUserResponse>(`/api/v1/user/me`, req)
}

/**
 * @description "Delete current user account"
 * @param req
 */
export function deleteAccount(req: components.DeleteAccountRequest) {
	return webapi.delete<components.MessageResponse>(`/api/v1/user/me`, req)
}

/**
 * @description "Change password for authenticated user"
 * @param req
 */
export function changePassword(req: components.ChangePasswordRequest) {
	return webapi.post<components.MessageResponse>(`/api/v1/user/me/change-password`, req)
}

/**
 * @description "Get user preferences"
 */
export function getPreferences() {
	return webapi.get<components.GetPreferencesResponse>(`/api/v1/user/me/preferences`)
}

/**
 * @description "Update user preferences"
 * @param req
 */
export function updatePreferences(req: components.UpdatePreferencesRequest) {
	return webapi.put<components.GetPreferencesResponse>(`/api/v1/user/me/preferences`, req)
}
