/**
 * WebSocket Client for GoBot
 *
 * Handles real-time communication with the backend.
 */

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
	private clientId: string;
	private userId: string;

	constructor() {
		this.clientId = this.generateClientId();
		this.userId = 'anonymous';
	}

	private generateClientId(): string {
		return `client_${Date.now()}_${Math.random().toString(36).substring(2, 9)}`;
	}

	private setStatus(status: ConnectionStatus) {
		if (this.currentStatus === status) return;
		this.currentStatus = status;
		this.statusListeners.forEach((fn) => fn(status));
	}

	/**
	 * Connect to the WebSocket server
	 */
	connect(userId?: string): void {
		if (userId) {
			this.userId = userId;
		}

		if (this.ws?.readyState === WebSocket.OPEN) {
			return;
		}

		if (this.ws?.readyState === WebSocket.CONNECTING) {
			return;
		}

		this.closedByUser = false;
		this.setStatus('connecting');

		const baseUrl = getWebSocketUrl();
		const url = `${baseUrl}?clientId=${this.clientId}&userId=${this.userId}`;
		console.log('[WebSocket] Connecting to:', url);

		try {
			this.ws = new WebSocket(url);

			this.ws.onopen = () => {
				console.log('[WebSocket] Connection opened');
				this.setStatus('connected');
				this.reconnectAttempts = 0;

				// Send queued messages
				console.log('[WebSocket] Sending', this.messageQueue.length, 'queued messages');
				while (this.messageQueue.length > 0) {
					const msg = this.messageQueue.shift();
					if (msg && this.ws?.readyState === WebSocket.OPEN) {
						console.log('[WebSocket] Sending queued message');
						this.ws.send(msg);
					}
				}
			};

			this.ws.onclose = (event) => {
				console.log('[WebSocket] Connection closed:', event.code, event.reason);
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
				console.error('[WebSocket] Error:', event);
				this.setStatus('error');
			};

			this.ws.onmessage = async (event) => {
				// Backend may batch multiple JSON messages into one frame separated by newlines
				const rawData = event.data as string;
				console.log('[WebSocket] Raw message received:', rawData.substring(0, 200));
				const lines = rawData.split('\n').filter((line) => line.trim());

				for (const line of lines) {
					try {
						const message: WebSocketMessage = JSON.parse(line);
						console.log('[WebSocket] Parsed message:', message.type, message.data);

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
						console.log('[WebSocket] Handlers for', message.type, ':', handlers?.size ?? 0);
						if (handlers) {
							for (const handler of handlers) {
								try {
									await handler(message.data);
								} catch (err) {
									console.error('Error in message handler:', err);
								}
							}
						}
					} catch (err) {
						console.error('Failed to parse WebSocket message:', err, 'Raw:', line);
					}
				}
			};
		} catch (err) {
			console.error('WebSocket connection error:', err);
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
		console.log('[WebSocket] Sending message:', type, 'readyState:', this.ws?.readyState);

		if (this.ws?.readyState === WebSocket.OPEN) {
			this.ws.send(payload);
			console.log('[WebSocket] Message sent successfully');
		} else {
			console.log('[WebSocket] Queuing message, WS not open');
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
