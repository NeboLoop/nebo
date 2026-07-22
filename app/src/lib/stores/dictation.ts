/**
 * Dictation state machine — 4-state: idle → connecting → recording → error.
 *
 * Manages the full lifecycle:
 * 1. User clicks dictate → state: connecting
 * 2. Open WebSocket to /ws/voice/dictation, acquire mic, start PCM capture
 * 3. Audio flows: mic → ScriptProcessor → Int16 → WebSocket → whisper-rs
 * 4. Transcript events flow back: WebSocket → store → callbacks → TipTap
 * 5. User clicks stop → send CloseStream, drain events, close
 *
 * Two routing modes:
 * - "editor": transcript to client only (quick composer input)
 * - "agent": transcript to client AND agent (long-form dictation)
 */

import { writable, derived } from 'svelte/store';
import { backendWsBase } from '$lib/api/base';
import { startPcmCapture, type AudioCaptureHandle } from '$lib/stores/audio';
import { deviceManager } from '$lib/stores/devices';
import { logger } from '$lib/monitoring';

const log = logger.child({ component: 'DictationStore' });

// --- Types ---

export type DictationStatus = 'idle' | 'connecting' | 'recording' | 'error';

export type DictationRoute =
	| { type: 'editor' }
	| { type: 'agent'; agentId: string };

export interface DictationState {
	status: DictationStatus;
	transcript: string;
	interimTranscript: string;
	audioLevel: number;
	error: string | null;
	ownerId: string | null;
	isPushToTalk: boolean;
	holdToRecordEnabled: boolean;
	route: DictationRoute;
}

const HOLD_TO_RECORD_KEY = 'voice:hold-to-record';

const initialState: DictationState = {
	status: 'idle',
	transcript: '',
	interimTranscript: '',
	audioLevel: 0,
	error: null,
	ownerId: null,
	isPushToTalk: false,
	holdToRecordEnabled: typeof localStorage !== 'undefined' && localStorage.getItem(HOLD_TO_RECORD_KEY) === 'true',
	route: { type: 'editor' }
};

// --- Constants ---

const WATCHDOG_MS = 2_500;
const ERROR_CLEAR_MS = 1_500;
const KEEPALIVE_MS = 4_000;

// --- Store ---

function createDictationStore() {
	const { subscribe, set, update } = writable<DictationState>(initialState);

	let watchdogTimer: ReturnType<typeof setTimeout> | null = null;
	let errorClearTimer: ReturnType<typeof setTimeout> | null = null;
	let keepAliveInterval: ReturnType<typeof setInterval> | null = null;
	let audioDetected = false;

	// Active session resources
	let ws: WebSocket | null = null;
	let captureHandle: AudioCaptureHandle | null = null;

	// Callbacks for the owner
	let onTranscriptCallback: ((text: string) => void) | null = null;
	let onInterimCallback: ((text: string) => void) | null = null;
	let onEndpointCallback: (() => void) | null = null;
	let onStopCallback: (() => void) | null = null;

	function readStatus(): DictationStatus {
		let status: DictationStatus = 'idle';
		const unsub = subscribe(s => { status = s.status; });
		unsub();
		return status;
	}

	function clearAllTimers() {
		if (watchdogTimer) { clearTimeout(watchdogTimer); watchdogTimer = null; }
		if (errorClearTimer) { clearTimeout(errorClearTimer); errorClearTimer = null; }
		if (keepAliveInterval) { clearInterval(keepAliveInterval); keepAliveInterval = null; }
	}

	function cleanup() {
		clearAllTimers();

		if (captureHandle) {
			captureHandle.stop();
			captureHandle = null;
		}

		if (ws) {
			if (ws.readyState === WebSocket.OPEN) {
				ws.send(JSON.stringify({ type: 'CloseStream' }));
			}
			ws.close();
			ws = null;
		}
	}

	function transitionToError(message: string) {
		cleanup();
		update(s => ({ ...s, status: 'error', error: message, audioLevel: 0 }));
		log.error('Dictation error: ' + message);
		errorClearTimer = setTimeout(() => {
			set(initialState);
		}, ERROR_CLEAR_MS);
	}

	function transitionToIdle() {
		cleanup();
		const currentOnStop = onStopCallback;
		set(initialState);
		currentOnStop?.();
	}

	function handleWsMessage(event: MessageEvent) {
		if (typeof event.data !== 'string') return;

		try {
			const msg = JSON.parse(event.data);
			switch (msg.type) {
				case 'TranscriptInterim':
					audioDetected = true;
					update(s => ({ ...s, interimTranscript: msg.text }));
					onInterimCallback?.(msg.text);
					break;

				case 'TranscriptText':
					audioDetected = true;
					update(s => ({
						...s,
						transcript: s.transcript + (s.transcript ? ' ' : '') + msg.text,
						interimTranscript: ''
					}));
					onTranscriptCallback?.(msg.text);
					break;

				case 'TranscriptEndpoint':
					onEndpointCallback?.();
					break;

				case 'Error':
					log.error('Server dictation error: ' + msg.message);
					transitionToError(msg.message);
					break;
			}
		} catch {
			log.warn('Failed to parse dictation WS message');
		}
	}

	const store = {
		subscribe,

		/**
		 * Start dictation for a given owner.
		 * Connects WebSocket, acquires mic, starts PCM capture.
		 */
		async start(ownerId: string, route: DictationRoute = { type: 'editor' }) {
			let current: DictationState = initialState;
			const unsub = subscribe(s => { current = s; });
			unsub();

			if (current.status !== 'idle') {
				log.warn('Cannot start dictation — status is ' + current.status);
				return;
			}

			update(s => ({
				...s,
				status: 'connecting',
				ownerId,
				route,
				transcript: '',
				interimTranscript: '',
				audioLevel: 0,
				error: null,
				isPushToTalk: false
			}));

			log.info('Dictation connecting for owner: ' + ownerId);

			try {
				// 1. Open WebSocket (same base derivation as the chat WS — carries
				// the tunnel prefix, and the Vite proxy still applies in dev)
				const wsUrl = `${backendWsBase()}/ws/voice/dictation`;
				ws = new WebSocket(wsUrl);
				ws.binaryType = 'arraybuffer';

				await new Promise<void>((resolve, reject) => {
					if (!ws) return reject(new Error('WebSocket is null'));
					ws.onopen = () => resolve();
					ws.onerror = () => reject(new Error('WebSocket connection failed'));
					// Timeout after 5s
					setTimeout(() => reject(new Error('WebSocket connection timeout')), 5000);
				});

				// 2. Send Start message with routing
				const startMsg = route.type === 'agent'
					? { type: 'Start', route: 'agent', agentId: route.agentId }
					: { type: 'Start', route: 'editor' };
				ws.send(JSON.stringify(startMsg));

				// 3. Wire up message handler
				ws.onmessage = handleWsMessage;
				ws.onclose = () => {
					let current: DictationState = initialState;
					const unsub = subscribe(s => { current = s; });
					unsub();
					if (current.status === 'recording') {
						log.warn('Dictation WebSocket closed unexpectedly');
						transitionToError('Connection lost');
					}
				};

				// 4. Acquire mic
				const stream = await deviceManager.acquireMicStream();

				// Bail if an error arrived while we were awaiting mic permission
				if (readStatus() !== 'connecting') {
					stream.getTracks().forEach(t => t.stop());
					return;
				}

				// 5. Start PCM capture → feed to WebSocket
				captureHandle = startPcmCapture(stream, {
					onAudioChunk: (buffer) => {
						if (ws && ws.readyState === WebSocket.OPEN) {
							ws.send(buffer);
						}
					},
					onAudioLevel: (level) => {
						audioDetected = true;
						update(s => ({ ...s, audioLevel: Math.min(1, Math.max(0, level)) }));
					}
				});

				// 6. Start keepalive
				keepAliveInterval = setInterval(() => {
					if (ws && ws.readyState === WebSocket.OPEN) {
						ws.send(JSON.stringify({ type: 'KeepAlive' }));
					}
				}, KEEPALIVE_MS);

				// 7. Transition to recording
				audioDetected = false;
				update(s => ({ ...s, status: 'recording' }));
				log.info('Dictation recording started');

				// Watchdog
				watchdogTimer = setTimeout(() => {
					if (!audioDetected) {
						log.warn('No audio detected after watchdog period — mic may be muted');
					}
				}, WATCHDOG_MS);

			} catch (err) {
				const msg = err instanceof Error ? err.message : 'Failed to start dictation';
				transitionToError(msg);
			}
		},

		/**
		 * Stop dictation (user-initiated or tab-hidden).
		 */
		stop() {
			let current: DictationState = initialState;
			const unsub = subscribe(s => { current = s; });
			unsub();

			if (current.status === 'idle' || current.status === 'error') return;

			log.info('Dictation stopped');
			transitionToIdle();
		},

		/**
		 * Mark as push-to-talk mode.
		 */
		setPushToTalk(enabled: boolean) {
			update(s => ({ ...s, isPushToTalk: enabled }));
		},

		/**
		 * Toggle hold-to-record preference (persisted to localStorage).
		 */
		setHoldToRecordEnabled(enabled: boolean) {
			localStorage.setItem(HOLD_TO_RECORD_KEY, String(enabled));
			update(s => ({ ...s, holdToRecordEnabled: enabled }));
		},

		/**
		 * Check if a given owner currently owns the dictation session.
		 */
		isOwner(ownerId: string): boolean {
			let current: DictationState = initialState;
			const unsub = subscribe(s => { current = s; });
			unsub();
			return current.ownerId === ownerId;
		},

		/**
		 * Register callbacks for transcript events.
		 * Returns an unsubscribe function.
		 */
		onEvents(callbacks: {
			onTranscript?: (text: string) => void;
			onInterim?: (text: string) => void;
			onEndpoint?: () => void;
			onStop?: () => void;
		}) {
			onTranscriptCallback = callbacks.onTranscript ?? null;
			onInterimCallback = callbacks.onInterim ?? null;
			onEndpointCallback = callbacks.onEndpoint ?? null;
			onStopCallback = callbacks.onStop ?? null;

			return () => {
				onTranscriptCallback = null;
				onInterimCallback = null;
				onEndpointCallback = null;
				onStopCallback = null;
			};
		},

		/**
		 * Handle tab visibility change — stop if tab becomes hidden.
		 */
		handleVisibilityChange() {
			if (document.hidden) {
				let current: DictationState = initialState;
				const unsub = subscribe(s => { current = s; });
				unsub();
				if (current.status === 'recording') {
					log.info('Tab hidden — stopping dictation');
					store.stop();
				}
			}
		}
	};

	return store;
}

export const dictationStore = createDictationStore();

// Derived convenience stores
export const dictationStatus = derived(dictationStore, $d => $d.status);
export const dictationAudioLevel = derived(dictationStore, $d => $d.audioLevel);
export const isDictating = derived(dictationStore, $d => $d.status === 'recording');
export const dictationError = derived(dictationStore, $d => $d.error);

/** Combined transcript: finalized + interim with smart spacing. */
export const combinedTranscript = derived(dictationStore, $d => {
	const { transcript, interimTranscript } = $d;
	if (!interimTranscript) return transcript;
	if (!transcript) return interimTranscript;
	const needsSpace = !transcript.endsWith(' ') && !interimTranscript.startsWith(' ');
	return transcript + (needsSpace ? ' ' : '') + interimTranscript;
});
