/**
 * WebSocket Client for Nebo
 *
 * Handles real-time communication with the backend.
 */

import { logger } from '$lib/monitoring/logger';

const log = logger.child({ component: 'WebSocket' });

export type ConnectionStatus = 'connecting' | 'connected' | 'disconnected' | 'error';

export interface WebSocketMessage<T = any> {
	type: string;
	channel?: string;
	data?: T;
	timestamp?: string;
}

export interface RewriteRequest {
	content: string;
	targetStage: string;
	formality?: string;
	tone?: string;
	brandVoice?: string;
	requestId: string;
}

export interface RewriteChunk {
	requestId: string;
	chunk: string;
}

export interface RewriteComplete {
	requestId: string;
	stage: string;
}

export interface RewriteError {
	requestId: string;
	error: string;
}

type MessageHandler<T = any> = (data: T) => void | Promise<void>;

/**
 * Get WebSocket URL based on current page origin
 */
function getWebSocketUrl(): string {
	if (typeof window === 'undefined') return '';

	const origin = window.location.origin;
	const wsProtocol = origin.startsWith('https:') ? 'wss:' : 'ws:';
	const host = origin.replace(/^https?:/, '');
	return `${wsProtocol}${host}/ws`;
}

class WebSocketClient {
	private ws: WebSocket | null = null;
	private listeners = new Map<string, Set<MessageHandler>>();
	private statusListeners = new Set<(status: ConnectionStatus) => void>();
	private messageQueue: string[] = [];
	private currentStatus: ConnectionStatus = 'disconnected';
	private closedByUser = false;
	private reconnectTimeout: ReturnType<typeof setTimeout> | null = null;
	private reconnectAttempts = 0;
	private authToken: string | null = null;

	private setStatus(status: ConnectionStatus) {
		if (this.currentStatus === status) return;
		this.currentStatus = status;
		this.statusListeners.forEach((fn) => fn(status));
	}

	/**
	 * Connect to the WebSocket server.
	 * Auth is handled post-connect: sends JWT token as first message after upgrade,
	 * waits for auth_ok before setting status to 'connected'.
	 * @param token - JWT token (passed from auth store on first connect; cached for reconnects)
	 */
	connect(token?: string): void {
		if (token) {
			this.authToken = token;
		}
		if (this.ws?.readyState === WebSocket.OPEN) {
			return;
		}

		if (this.ws?.readyState === WebSocket.CONNECTING) {
			return;
		}

		this.closedByUser = false;
		this.setStatus('connecting');

		const url = getWebSocketUrl();
		log.info('Connecting to WS at: ' + url);

		try {
			this.ws = new WebSocket(url);
			log.info('WebSocket object created, readyState: ' + this.ws.readyState);

			this.ws.onopen = () => {
				log.info('WS onopen fired, readyState: ' + this.ws?.readyState);

				// Send auth token if available, otherwise just connect.
				// Server allows unauthenticated local connections (all HTTP API routes are public).
				const authToken = this.authToken || localStorage.getItem('nebo_token');
				const msg = authToken
					? { type: 'auth', data: { token: authToken }, timestamp: new Date().toISOString() }
					: { type: 'connect', timestamp: new Date().toISOString() };
				const payload = JSON.stringify(msg);
				log.info('WS sending handshake (' + msg.type + '): ' + payload.substring(0, 120));
				try {
					this.ws!.send(payload);
					log.info('WS handshake sent OK');
				} catch (err) {
					log.error('WS handshake send FAILED', err);
				}
			};

			this.ws.onclose = (event) => {
				log.info('WS onclose: code=' + event.code + ' reason=' + event.reason + ' wasClean=' + event.wasClean);
				this.setStatus('disconnected');
				this.ws = null;

				// Auto-reconnect if not closed by user
				if (!this.closedByUser) {
					const delay = Math.min(2000 * Math.pow(2, this.reconnectAttempts), 30000);
					this.reconnectTimeout = setTimeout(() => {
						this.reconnectAttempts++;
						this.connect();
					}, delay);
				}
			};

			this.ws.onerror = (event) => {
				log.error('WS onerror fired', event);
				this.setStatus('error');
			};

			this.ws.onmessage = async (event) => {
				log.info('WS onmessage received, data type: ' + typeof event.data + ', length: ' + (event.data as string).length);
				// Backend may batch multiple JSON messages into one frame separated by newlines
				const rawData = event.data as string;
				log.info('WS raw message: ' + rawData.substring(0, 200));
				const lines = rawData.split('\n').filter((line) => line.trim());

				for (const line of lines) {
					try {
						const message: WebSocketMessage = JSON.parse(line);

						// Handle auth_ok — server confirmed our token
						if (message.type === 'auth_ok') {
							log.info('WebSocket authenticated');
							this.setStatus('connected');
							this.reconnectAttempts = 0;

							// Flush queued messages
							while (this.messageQueue.length > 0) {
								const msg = this.messageQueue.shift();
								if (msg && this.ws?.readyState === WebSocket.OPEN) {
									this.ws.send(msg);
								}
							}
							continue;
						}

						// Handle ping/pong
						if (message.type === 'ping') {
							this.send('pong', { timestamp: new Date().toISOString() });
							continue;
						}

						if (message.type === 'pong') {
							continue;
						}

						// Route to listeners by message type
						const handlers = this.listeners.get(message.type);
						if (handlers) {
							for (const handler of handlers) {
								try {
									await handler(message.data);
								} catch (err) {
									log.error('Error in message handler', err);
								}
							}
						}
					} catch (err) {
						log.error('Failed to parse message: ' + line.substring(0, 100), err);
					}
				}
			};
		} catch (err) {
			log.error('Connection error', err);
			this.setStatus('error');
		}
	}

	/**
	 * Disconnect from the WebSocket server
	 */
	disconnect(): void {
		this.closedByUser = true;

		if (this.reconnectTimeout) {
			clearTimeout(this.reconnectTimeout);
			this.reconnectTimeout = null;
		}

		if (this.ws) {
			this.ws.close();
			this.ws = null;
		}

		this.setStatus('disconnected');
	}

	/**
	 * Subscribe to a message type
	 */
	on<T = any>(type: string, handler: MessageHandler<T>): () => void {
		if (!this.listeners.has(type)) {
			this.listeners.set(type, new Set());
		}
		this.listeners.get(type)!.add(handler as MessageHandler);

		return () => {
			const handlers = this.listeners.get(type);
			if (handlers) {
				handlers.delete(handler as MessageHandler);
				if (handlers.size === 0) {
					this.listeners.delete(type);
				}
			}
		};
	}

	/**
	 * Subscribe to connection status changes
	 */
	onStatus(handler: (status: ConnectionStatus) => void): () => void {
		this.statusListeners.add(handler);
		handler(this.currentStatus); // Call immediately with current value

		return () => {
			this.statusListeners.delete(handler);
		};
	}

	/**
	 * Send a message through the WebSocket
	 */
	send<T = any>(type: string, data?: T): void {
		const message: WebSocketMessage<T> = {
			type,
			data,
			timestamp: new Date().toISOString()
		};
		const payload = JSON.stringify(message);
		if (this.ws?.readyState === WebSocket.OPEN) {
			this.ws.send(payload);
		} else {
			log.debug('Queuing message, WS not open: ' + type);
			this.messageQueue.push(payload);
		}
	}

	/**
	 * Request a content rewrite via WebSocket
	 */
	requestRewrite(request: RewriteRequest): void {
		this.send('rewrite', request);
	}

	/**
	 * Get current connection status
	 */
	getStatus(): ConnectionStatus {
		return this.currentStatus;
	}

	/**
	 * Check if connected
	 */
	isConnected(): boolean {
		return this.currentStatus === 'connected';
	}
}

// Singleton instance
let instance: WebSocketClient | null = null;

/**
 * Get the singleton WebSocket client instance
 */
export function getWebSocketClient(): WebSocketClient {
	if (!instance) {
		instance = new WebSocketClient();
	}
	return instance;
}

/**
 * Request a content rewrite with streaming response
 *
 * @param content - The content to rewrite
 * @param targetStage - The awareness stage to target
 * @param options - Voice/tone options
 * @param callbacks - Callbacks for streaming events
 * @returns A function to cancel the request
 */
export function streamRewrite(
	content: string,
	targetStage: string,
	options: {
		formality?: string;
		tone?: string;
		brandVoice?: string;
	} = {},
	callbacks: {
		onStart?: () => void;
		onChunk?: (chunk: string) => void;
		onComplete?: () => void;
		onError?: (error: string) => void;
	} = {}
): () => void {
	const client = getWebSocketClient();
	const requestId = `rewrite_${Date.now()}_${Math.random().toString(36).substring(2, 9)}`;

	// Ensure connected
	if (!client.isConnected()) {
		client.connect();
	}

	// Set up event listeners for this request
	const unsubscribers: (() => void)[] = [];

	const unsubStart = client.on('rewrite_start', (data: { requestId: string }) => {
		if (data.requestId === requestId) {
			callbacks.onStart?.();
		}
	});
	unsubscribers.push(unsubStart);

	const unsubChunk = client.on('rewrite_chunk', (data: RewriteChunk) => {
		if (data.requestId === requestId) {
			callbacks.onChunk?.(data.chunk);
		}
	});
	unsubscribers.push(unsubChunk);

	const unsubComplete = client.on('rewrite_complete', (data: RewriteComplete) => {
		if (data.requestId === requestId) {
			callbacks.onComplete?.();
			cleanup();
		}
	});
	unsubscribers.push(unsubComplete);

	const unsubError = client.on('rewrite_error', (data: RewriteError) => {
		if (data.requestId === requestId) {
			callbacks.onError?.(data.error);
			cleanup();
		}
	});
	unsubscribers.push(unsubError);

	function cleanup() {
		unsubscribers.forEach((unsub) => unsub());
	}

	// Send the rewrite request
	client.requestRewrite({
		content,
		targetStage,
		formality: options.formality,
		tone: options.tone,
		brandVoice: options.brandVoice,
		requestId
	});

	// Return cleanup function
	return cleanup;
}
