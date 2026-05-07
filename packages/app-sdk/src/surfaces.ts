/**
 * nebo.surfaces — agent-to-app surface events.
 *
 * Lets apps receive structured UI updates from their agent over WebSocket.
 * The agent pushes events; the app decides how to render them using its own
 * framework (React, Svelte, Vue, etc.).
 *
 * This bridges A2UI/AG-UI concepts into the app SDK without requiring
 * apps to use Nebo's component library. Apps subscribe to events and
 * render however they want.
 *
 * Event types follow the AG-UI protocol where applicable:
 * - State events (snapshot, delta) for shared agent↔app state
 * - Text events (start, content, end) for streaming responses
 * - Tool events for visibility into agent tool execution
 * - Surface events for A2UI component updates
 * - Custom events for app-specific communication
 */

import { getAppId, getBaseUrl } from './config';

// ─── Event Types ─────────────────────────────────────────────────────

/** Base event — all events carry a type and optional timestamp. */
export interface SurfaceEvent {
	type: string;
	timestamp?: string;
	[key: string]: unknown;
}

/** Agent run lifecycle */
export interface RunStartedEvent extends SurfaceEvent {
	type: 'run_started';
	runId: string;
	threadId?: string;
}

export interface RunFinishedEvent extends SurfaceEvent {
	type: 'run_finished';
	runId: string;
}

export interface RunErrorEvent extends SurfaceEvent {
	type: 'run_error';
	runId: string;
	message: string;
	code?: string;
}

/** Streaming text from agent */
export interface TextStartEvent extends SurfaceEvent {
	type: 'text_start';
	messageId: string;
}

export interface TextContentEvent extends SurfaceEvent {
	type: 'text_content';
	messageId: string;
	delta: string;
}

export interface TextEndEvent extends SurfaceEvent {
	type: 'text_end';
	messageId: string;
}

/** Tool execution visibility */
export interface ToolCallStartEvent extends SurfaceEvent {
	type: 'tool_call_start';
	toolCallId: string;
	toolName: string;
}

export interface ToolCallEndEvent extends SurfaceEvent {
	type: 'tool_call_end';
	toolCallId: string;
	result?: unknown;
}

/** State management — shared agent↔app state */
export interface StateSnapshotEvent extends SurfaceEvent {
	type: 'state_snapshot';
	snapshot: Record<string, unknown>;
}

export interface StateDeltaEvent extends SurfaceEvent {
	type: 'state_delta';
	/** RFC 6902 JSON Patch operations */
	delta: Array<{
		op: 'add' | 'replace' | 'remove' | 'move' | 'copy' | 'test';
		path: string;
		value?: unknown;
		from?: string;
	}>;
}

/** A2UI surface updates — agent pushes component trees */
export interface SurfaceCreateEvent extends SurfaceEvent {
	type: 'surface_create';
	surfaceId: string;
	components: unknown[];
	data?: Record<string, unknown>;
}

export interface SurfaceUpdateEvent extends SurfaceEvent {
	type: 'surface_update';
	surfaceId: string;
	components?: unknown[];
	data?: Record<string, unknown>;
}

export interface SurfaceDeleteEvent extends SurfaceEvent {
	type: 'surface_delete';
	surfaceId: string;
}

/** Data model update (partial update to a surface's data) */
export interface DataUpdateEvent extends SurfaceEvent {
	type: 'data_update';
	surfaceId?: string;
	path?: string;
	value: unknown;
}

/** Custom app-specific events */
export interface CustomEvent extends SurfaceEvent {
	type: 'custom';
	name: string;
	value: unknown;
}

// ─── All Event Types Union ───────────────────────────────────────────

export type NeboSurfaceEvent =
	| RunStartedEvent
	| RunFinishedEvent
	| RunErrorEvent
	| TextStartEvent
	| TextContentEvent
	| TextEndEvent
	| ToolCallStartEvent
	| ToolCallEndEvent
	| StateSnapshotEvent
	| StateDeltaEvent
	| SurfaceCreateEvent
	| SurfaceUpdateEvent
	| SurfaceDeleteEvent
	| DataUpdateEvent
	| CustomEvent
	| SurfaceEvent;

// ─── Event Map for typed listeners ───────────────────────────────────

export interface SurfaceEventMap {
	run_started: RunStartedEvent;
	run_finished: RunFinishedEvent;
	run_error: RunErrorEvent;
	text_start: TextStartEvent;
	text_content: TextContentEvent;
	text_end: TextEndEvent;
	tool_call_start: ToolCallStartEvent;
	tool_call_end: ToolCallEndEvent;
	state_snapshot: StateSnapshotEvent;
	state_delta: StateDeltaEvent;
	surface_create: SurfaceCreateEvent;
	surface_update: SurfaceUpdateEvent;
	surface_delete: SurfaceDeleteEvent;
	data_update: DataUpdateEvent;
	custom: CustomEvent;
	'*': NeboSurfaceEvent;
}

type EventHandler<T> = (event: T) => void;

// ─── Surface Manager ─────────────────────────────────────────────────

/**
 * Manages the WebSocket connection for agent↔app surface events.
 *
 * Usage:
 * ```ts
 * const surfaces = new NeboSurfaces();
 * surfaces.connect();
 *
 * // Listen for streaming text from agent
 * surfaces.on('text_content', (e) => {
 *   output.textContent += e.delta;
 * });
 *
 * // Listen for state snapshots
 * surfaces.on('state_snapshot', (e) => {
 *   appState = e.snapshot;
 *   rerender();
 * });
 *
 * // Listen for all events
 * surfaces.on('*', (e) => console.log('event:', e));
 *
 * // Send action to agent
 * surfaces.send('button_click', { buttonId: 'analyze' });
 * ```
 */
export class NeboSurfaces {
	private ws: WebSocket | null = null;
	private listeners = new Map<string, Set<EventHandler<any>>>();
	private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
	private reconnectDelay = 1000;
	private maxReconnectDelay = 30000;
	private _connected = false;

	/** @internal A2UI message handler — set by NeboSDK to forward a2ui_message events */
	_a2uiHandler: ((message: unknown) => void) | null = null;

	/** Current shared state (updated by state_snapshot and state_delta events) */
	state: Record<string, unknown> = {};

	/** Whether the WebSocket is connected */
	get connected(): boolean {
		return this._connected;
	}

	/** Connect to the app's surface WebSocket */
	connect(): void {
		if (this.ws && this.ws.readyState <= WebSocket.OPEN) return;

		const appId = getAppId();
		const base = getBaseUrl().replace(/^http/, 'ws');
		const url = `${base}/ws/app/${appId}`;

		this.ws = new WebSocket(url);

		this.ws.onopen = () => {
			this._connected = true;
			this.reconnectDelay = 1000;
		};

		this.ws.onmessage = (msg) => {
			try {
				const parsed = JSON.parse(msg.data);

				// Forward A2UI v0.9 messages to the a2ui processor
				if (parsed.type === 'a2ui_message' && parsed.data?.message) {
					this._a2uiHandler?.(parsed.data.message);
					return;
				}

				this.handleEvent(parsed as NeboSurfaceEvent);
			} catch {
				// Non-JSON message — ignore
			}
		};

		this.ws.onclose = () => {
			this._connected = false;
			this.scheduleReconnect();
		};

		this.ws.onerror = () => {
			this._connected = false;
		};
	}

	/** Disconnect and stop reconnecting */
	disconnect(): void {
		if (this.reconnectTimer) {
			clearTimeout(this.reconnectTimer);
			this.reconnectTimer = null;
		}
		if (this.ws) {
			this.ws.close();
			this.ws = null;
		}
		this._connected = false;
	}

	/** Subscribe to a specific event type (or '*' for all) */
	on<K extends keyof SurfaceEventMap>(
		type: K,
		handler: EventHandler<SurfaceEventMap[K]>
	): () => void {
		if (!this.listeners.has(type)) {
			this.listeners.set(type, new Set());
		}
		this.listeners.get(type)!.add(handler);

		// Return unsubscribe function
		return () => {
			this.listeners.get(type)?.delete(handler);
		};
	}

	/** Remove a specific listener */
	off<K extends keyof SurfaceEventMap>(
		type: K,
		handler: EventHandler<SurfaceEventMap[K]>
	): void {
		this.listeners.get(type)?.delete(handler);
	}

	/** Send an action/event to the agent */
	send(name: string, payload?: Record<string, unknown>): void {
		if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;
		this.ws.send(JSON.stringify({ type: 'action', name, ...payload }));
	}

	/** Request a state snapshot from the agent */
	requestState(): void {
		this.send('request_state');
	}

	/** @internal Send raw data through the WebSocket (used by a2ui module) */
	_rawSend(data: string): void {
		if (this.ws && this.ws.readyState === WebSocket.OPEN) {
			this.ws.send(data);
		}
	}

	// ─── Internal ────────────────────────────────────────────────────

	private handleEvent(event: NeboSurfaceEvent): void {
		// Update local state for state events
		if (event.type === 'state_snapshot') {
			this.state = { ...(event as StateSnapshotEvent).snapshot };
		} else if (event.type === 'state_delta') {
			this.applyDelta((event as StateDeltaEvent).delta);
		}

		// Dispatch to typed listeners
		const handlers = this.listeners.get(event.type);
		if (handlers) {
			for (const handler of handlers) {
				try {
					handler(event);
				} catch {
					// Don't let listener errors break the event loop
				}
			}
		}

		// Dispatch to wildcard listeners
		const wildcardHandlers = this.listeners.get('*');
		if (wildcardHandlers) {
			for (const handler of wildcardHandlers) {
				try {
					handler(event);
				} catch {
					// Swallow
				}
			}
		}
	}

	private applyDelta(
		ops: StateDeltaEvent['delta']
	): void {
		for (const op of ops) {
			const parts = op.path.split('/').filter(Boolean);
			if (parts.length === 0) continue;

			const parent = this.resolveParent(parts);
			if (!parent) continue;

			const key = parts[parts.length - 1];

			switch (op.op) {
				case 'add':
				case 'replace':
					(parent as Record<string, unknown>)[key] = op.value;
					break;
				case 'remove':
					delete (parent as Record<string, unknown>)[key];
					break;
			}
		}
	}

	private resolveParent(parts: string[]): unknown {
		let current: unknown = this.state;
		for (let i = 0; i < parts.length - 1; i++) {
			if (current == null || typeof current !== 'object') return null;
			current = (current as Record<string, unknown>)[parts[i]];
		}
		return current;
	}

	private scheduleReconnect(): void {
		if (this.reconnectTimer) return;
		this.reconnectTimer = setTimeout(() => {
			this.reconnectTimer = null;
			this.reconnectDelay = Math.min(this.reconnectDelay * 2, this.maxReconnectDelay);
			this.connect();
		}, this.reconnectDelay);
	}
}

/** Singleton instance */
export const surfaces = new NeboSurfaces();
