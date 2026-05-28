// Re-export all generated API functions
export * from './nebo';

// Import all functions and components
import * as api from './nebo';
import webapi from './gocliRequest';
import type * as components from './neboComponents';

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
	const baseUrl = typeof window !== 'undefined' ? window.location.origin : '';
	const token = typeof window !== 'undefined' ? localStorage.getItem('nebo_token') : null;

	const headers: Record<string, string> = {
		'Content-Type': audioBlob.type || 'audio/webm'
	};
	if (token) {
		headers['Authorization'] = `Bearer ${token}`;
	}

	const response = await fetch(`${baseUrl}/api/v1/voice/transcribe`, {
		method: 'POST',
		credentials: 'include',
		headers,
		body: audioBlob
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
	capabilities?: string[];
	permissions?: string[];
	appId?: string;
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

// NeboAI Marketplace — query-param variants (generated API lacks param support)

export function listStoreProducts(params?: Record<string, string | number>): Promise<unknown> {
	return webapi.get<unknown>('/api/v1/store/products', params);
}

export function getStoreProduct(id: string): Promise<unknown> {
	return webapi.get<unknown>(`/api/v1/store/products/${id}`);
}

export function getStoreProductReviews(id: string): Promise<unknown> {
	return webapi.get<unknown>(`/api/v1/store/products/${id}/reviews`);
}

// NeboAI OAuth with Janus opt-in
export function neboAIOAuthStartWithJanus(janus: boolean) {
	return webapi.get<components.OAuthStartResponse>(
		'/api/v1/neboai/oauth/start',
		janus ? { janus: 'true' } : undefined
	);
}

// NeboAI OAuth status polling (generated API lacks the state param)
export function neboAIOAuthStatus(state: string) {
	return webapi.get<components.OAuthStatusResponse>(
		'/api/v1/neboai/oauth/status',
		{ state }
	);
}

// Marketplace subscription API wrappers

export interface MarketplaceSubscriptionInfo {
	id: string;
	targetId: string;
	targetType: string;
	artifactName?: string;
	tierName?: string;
	priceCents?: number;
	billingInterval?: string;
	status: string;
	currentPeriodEnd?: string;
	cancelledAt?: string;
}

export function createMarketplaceSubscription(params: {
	targetId: string;
	targetType: string;
	botCount?: number;
}): Promise<{ checkoutUrl?: string; subscriptionId?: string }> {
	return webapi.post('/api/v1/neboai/marketplace/subscriptions', params);
}

export function listMarketplaceSubscriptions(): Promise<{
	subscriptions: MarketplaceSubscriptionInfo[];
}> {
	return webapi.get('/api/v1/neboai/marketplace/subscriptions');
}

export function cancelMarketplaceSubscription(
	id: string
): Promise<{ success: boolean }> {
	return webapi.post(`/api/v1/neboai/marketplace/subscriptions/${id}/cancel`, {});
}


