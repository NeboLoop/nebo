// Re-export all generated API functions
export * from './nebo';

// Import all functions and components
import * as api from './nebo';
import webapi from './gocliRequest';

// Export api object containing all API methods
export const nebo = api;

// API Configuration - base URL is loaded from browser origin
export const API_CONFIG = {
	get baseURL() {
		// In browser, use the current origin; in SSR, default to localhost
		if (typeof window !== 'undefined') {
			return window.location.origin;
		}
		return 'http://localhost:8847';
	}
};

// Re-export types
export type * from './neboComponents';

// Custom API functions that return binary data (not auto-generated)
export interface TTSRequest {
	text: string;
	voice?: string;
	speed?: number;
}

export function speakTTS(req: TTSRequest): Promise<Blob> {
	return webapi.postBlob('/api/v1/voice/tts', req);
}

// Voice transcription via backend (local whisper-cli or OpenAI fallback)
export interface TranscribeResponse {
	text: string;
}

export async function transcribeAudio(audioBlob: Blob): Promise<TranscribeResponse> {
	const formData = new FormData();
	formData.append('audio', audioBlob, 'recording.webm');

	const baseUrl = typeof window !== 'undefined' ? window.location.origin : '';
	const token = typeof window !== 'undefined' ? localStorage.getItem('nebo_token') : null;

	const headers: Record<string, string> = {};
	if (token) {
		headers['Authorization'] = `Bearer ${token}`;
	}

	const response = await fetch(`${baseUrl}/api/v1/voice/transcribe`, {
		method: 'POST',
		credentials: 'include',
		headers,
		body: formData
	});

	if (!response.ok) {
		const text = await response.text();
		throw new Error(text || `HTTP ${response.status}`);
	}

	return response.json();
}

// Plugin Settings API
export interface PluginItem {
	id: string;
	name: string;
	pluginType: string;
	displayName: string;
	description: string;
	icon: string;
	version: string;
	isEnabled: boolean;
	isInstalled: boolean;
	connectionStatus: string;
	lastConnectedAt: string;
	lastError: string;
	settings?: Record<string, string>;
	createdAt: string;
	updatedAt: string;
}

export interface ListPluginsResponse {
	plugins: PluginItem[];
}

export interface GetPluginResponse {
	plugin: PluginItem;
}

export interface UpdatePluginSettingsRequest {
	settings: Record<string, string>;
	secrets?: Record<string, boolean>;
}

export interface UpdatePluginSettingsResponse {
	plugin: PluginItem;
}

export interface TogglePluginRequest {
	isEnabled: boolean;
}

export function listPlugins(type?: string): Promise<ListPluginsResponse> {
	return webapi.get<ListPluginsResponse>('/api/v1/plugins', type ? { type } : undefined);
}

export function getPlugin(id: string): Promise<GetPluginResponse> {
	return webapi.get<GetPluginResponse>(`/api/v1/plugins/${id}`);
}

export function updatePluginSettings(id: string, req: UpdatePluginSettingsRequest): Promise<UpdatePluginSettingsResponse> {
	return webapi.put<UpdatePluginSettingsResponse>(`/api/v1/plugins/${id}/settings`, req);
}

export function togglePlugin(id: string, req: TogglePluginRequest): Promise<GetPluginResponse> {
	return webapi.put<GetPluginResponse>(`/api/v1/plugins/${id}/toggle`, req);
}

// NeboLoop Marketplace

export interface StoreAuthor {
	id: string;
	name: string;
	verified: boolean;
}

export interface StoreApp {
	id: string;
	name: string;
	slug: string;
	description: string;
	icon: string;
	category: string;
	version: string;
	author: StoreAuthor;
	installCount: number;
	rating: number;
	reviewCount: number;
	isInstalled: boolean;
	status: string;
}

export interface StoreSkill {
	id: string;
	name: string;
	slug: string;
	description: string;
	icon: string;
	category: string;
	version: string;
	author: StoreAuthor;
	installCount: number;
	rating: number;
	reviewCount: number;
	isInstalled: boolean;
	status: string;
}

export interface StoreAppsResponse {
	apps: StoreApp[];
	totalCount: number;
	page: number;
	pageSize: number;
}

export interface StoreSkillsResponse {
	skills: StoreSkill[];
	totalCount: number;
	page: number;
	pageSize: number;
}

export interface StoreInstallResponse {
	pluginId: string;
	message: string;
}

export function listStoreApps(params?: {
	q?: string;
	category?: string;
	page?: number;
	pageSize?: number;
}): Promise<StoreAppsResponse> {
	return webapi.get<StoreAppsResponse>('/api/v1/store/apps', params);
}

export function listStoreSkills(params?: {
	q?: string;
	category?: string;
	page?: number;
	pageSize?: number;
}): Promise<StoreSkillsResponse> {
	return webapi.get<StoreSkillsResponse>('/api/v1/store/skills', params);
}

export function installStoreApp(id: string): Promise<StoreInstallResponse> {
	return webapi.post<StoreInstallResponse>(`/api/v1/store/apps/${id}/install`);
}

export function uninstallStoreApp(id: string): Promise<{ message: string }> {
	return webapi.delete<{ message: string }>(`/api/v1/store/apps/${id}/install`);
}

export function installStoreSkill(id: string): Promise<StoreInstallResponse> {
	return webapi.post<StoreInstallResponse>(`/api/v1/store/skills/${id}/install`);
}

export function uninstallStoreSkill(id: string): Promise<{ message: string }> {
	return webapi.delete<{ message: string }>(`/api/v1/store/skills/${id}/install`);
}

// NeboLoop OAuth with Janus opt-in
import type * as components from './neboComponents';

export function neboLoopOAuthStartWithJanus(janus: boolean) {
	return webapi.get<components.NeboLoopOAuthStartResponse>(
		'/api/v1/neboloop/oauth/start',
		janus ? { janus: 'true' } : undefined
	);
}


