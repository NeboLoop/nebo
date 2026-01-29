import { browser } from '$app/environment';
import { writable, derived, get } from 'svelte/store';

// WebSocket message types
export interface WSMessage {
	type: string;
	channel?: string;
	data?: Record<string, unknown>;
	timestamp?: string;
	userId?: string;
}

export interface WebSocketState {
	connected: boolean;
	reconnecting: boolean;
	error: string | null;
}

type MessageHandler = (msg: WSMessage) => void;

// Store for WebSocket state
const state = writable<WebSocketState>({
	connected: false,
	reconnecting: false,
	error: null
});

// Derived stores for convenience
export const wsConnected = derived(state, ($state) => $state.connected);
export const wsReconnecting = derived(state, ($state) => $state.reconnecting);
export const wsError = derived(state, ($state) => $state.error);

class WebSocketService {
	private ws: WebSocket | null = null;
	private handlers: Map<string, Set<MessageHandler>> = new Map();
	private reconnectAttempts = 0;
	private maxReconnectAttempts = 5;
	private reconnectDelay = 1000;
	private pingInterval: ReturnType<typeof setInterval> | null = null;
	private clientId: string;
	private userId: string = '';

	constructor() {
		this.clientId = this.generateClientId();
	}

	get state() {
		return state;
	}

	private generateClientId(): string {
		return `client_${Date.now()}_${Math.random().toString(36).substring(2, 9)}`;
	}

	connect(userId?: string) {
		if (!browser) return;

		// Don't create new connection if one exists and is open or connecting
		if (this.ws && (this.ws.readyState === WebSocket.OPEN || this.ws.readyState === WebSocket.CONNECTING)) {
			return;
		}

		this.userId = userId || '';
		state.update((s) => ({ ...s, error: null, reconnecting: this.reconnectAttempts > 0 }));

		const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
		const wsUrl = `${protocol}//${window.location.host}/ws?clientId=${this.clientId}&userId=${this.userId}`;

		try {
			this.ws = new WebSocket(wsUrl);
			this.setupEventHandlers();
		} catch (err) {
			state.update((s) => ({
				...s,
				error: err instanceof Error ? err.message : 'Failed to connect'
			}));
			this.scheduleReconnect();
		}
	}

	private setupEventHandlers() {
		if (!this.ws) return;

		this.ws.onopen = () => {
			state.set({ connected: true, reconnecting: false, error: null });
			this.reconnectAttempts = 0;
			this.startPingInterval();
			console.log('[WS] Connected');
		};

		this.ws.onclose = (event) => {
			state.update((s) => ({ ...s, connected: false }));
			this.stopPingInterval();
			console.log('[WS] Disconnected', event.code, event.reason);

			if (!event.wasClean) {
				this.scheduleReconnect();
			}
		};

		this.ws.onerror = (error) => {
			console.error('[WS] Error:', error);
			state.update((s) => ({ ...s, error: 'WebSocket error' }));
		};

		this.ws.onmessage = (event) => {
			try {
				// Handle multiple messages separated by newlines
				const messages = event.data.split('\n').filter((m: string) => m.trim());
				for (const msgStr of messages) {
					const msg: WSMessage = JSON.parse(msgStr);
					this.dispatchMessage(msg);
				}
			} catch (err) {
				console.error('[WS] Failed to parse message:', err);
			}
		};
	}

	private dispatchMessage(msg: WSMessage) {
		// Dispatch to type-specific handlers
		const typeHandlers = this.handlers.get(msg.type);
		if (typeHandlers) {
			typeHandlers.forEach((handler) => handler(msg));
		}

		// Dispatch to wildcard handlers
		const wildcardHandlers = this.handlers.get('*');
		if (wildcardHandlers) {
			wildcardHandlers.forEach((handler) => handler(msg));
		}
	}

	private startPingInterval() {
		this.stopPingInterval();
		this.pingInterval = setInterval(() => {
			this.send({ type: 'ping' });
		}, 30000);
	}

	private stopPingInterval() {
		if (this.pingInterval) {
			clearInterval(this.pingInterval);
			this.pingInterval = null;
		}
	}

	private scheduleReconnect() {
		if (this.reconnectAttempts >= this.maxReconnectAttempts) {
			state.update((s) => ({
				...s,
				error: 'Max reconnection attempts reached',
				reconnecting: false
			}));
			return;
		}

		state.update((s) => ({ ...s, reconnecting: true }));
		this.reconnectAttempts++;
		const delay = this.reconnectDelay * Math.pow(2, this.reconnectAttempts - 1);

		console.log(`[WS] Reconnecting in ${delay}ms (attempt ${this.reconnectAttempts})`);
		setTimeout(() => this.connect(this.userId), delay);
	}

	disconnect() {
		this.stopPingInterval();
		if (this.ws) {
			this.ws.close(1000, 'Client disconnect');
			this.ws = null;
		}
		state.set({ connected: false, reconnecting: false, error: null });
	}

	send(msg: WSMessage) {
		if (this.ws?.readyState === WebSocket.OPEN) {
			this.ws.send(JSON.stringify(msg));
		}
	}

	on(type: string, handler: MessageHandler): () => void {
		if (!this.handlers.has(type)) {
			this.handlers.set(type, new Set());
		}
		this.handlers.get(type)!.add(handler);

		// Return unsubscribe function
		return () => {
			this.handlers.get(type)?.delete(handler);
		};
	}

	off(type: string, handler: MessageHandler) {
		this.handlers.get(type)?.delete(handler);
	}

	isConnected(): boolean {
		return get(state).connected;
	}
}

// Singleton instance
export const ws = new WebSocketService();
