/**
 * Voice conversation session store — 6-state machine.
 *
 * States: idle → connecting → listening → processing → speaking → idle
 *                                                    → error
 *
 * Manages the full lifecycle of a voice conversation:
 * 1. User starts → state: connecting
 * 2. Open WebSocket to /ws/voice/conversation, acquire mic, start PCM capture
 * 3. Server sends session_initialized → state: listening
 * 4. User speaks → PCM chunks stream to server
 * 5. Server sends transcription_end → state: processing
 * 6. Server sends playback_start + binary audio → state: speaking
 * 7. Server sends playback_end → state: listening (loop)
 * 8. User calls interrupt() → stop TTS, send interrupt, → listening
 * 9. User calls stop() → cleanup everything → idle
 *
 * Based on Claude Desktop's VoiceSession (zMt) pattern.
 */

import { writable, derived } from 'svelte/store';
import { backendWsBase } from '$lib/api/base';
import { startPcmCapture, type AudioCaptureHandle } from '$lib/stores/audio';
import { deviceManager } from '$lib/stores/devices';
import { storage } from '$lib/storage';
import { logger } from '$lib/monitoring';

const log = logger.child({ component: 'VoiceSession' });

/**
 * Cloud-mic consent. Conversation mode streams raw microphone audio to xAI
 * (directly or via Janus) — the single largest data egress in the product, so
 * it never starts silently: the first start() requires an explicit, recorded
 * consent naming xAI. Base-scoped storage, one grant per install.
 */
const CONSENT_KEY = 'nebo_voice_cloud_consent';
export function hasVoiceCloudConsent(): boolean {
	return storage.get(CONSENT_KEY) === 'granted';
}
export function grantVoiceCloudConsent(): void {
	storage.set(CONSENT_KEY, 'granted');
}

// --- Types ---

export type VoiceSessionStatus =
	| 'idle'
	| 'connecting'
	| 'listening'
	| 'processing'
	| 'speaking'
	| 'error';

export interface VoiceSessionState {
	status: VoiceSessionStatus;
	isMuted: boolean;
	audioLevel: number;
	transcripts: Array<{ speaker: 'user' | 'agent'; text: string }>;
	interimTranscript: string;
	agentId: string | null;
	conversationId: string | null;
	errorMessage: string | null;
}

const initialState: VoiceSessionState = {
	status: 'idle',
	isMuted: false,
	audioLevel: 0,
	transcripts: [],
	interimTranscript: '',
	agentId: null,
	conversationId: null,
	errorMessage: null
};

// --- Constants ---

const KEEPALIVE_MS = 4_000;
const ERROR_DISPLAY_MS = 5_000;

// --- Store ---

function createVoiceSessionStore() {
	const { subscribe, set, update } = writable<VoiceSessionState>(initialState);

	// Active session resources
	let ws: WebSocket | null = null;
	let captureHandle: AudioCaptureHandle | null = null;
	let keepAliveInterval: ReturnType<typeof setInterval> | null = null;
	let errorClearTimer: ReturnType<typeof setTimeout> | null = null;

	// TTS playback resources
	let playbackCtx: AudioContext | null = null;
	let pendingAudioChunks: Float32Array[] = [];
	let currentSource: AudioBufferSourceNode | null = null;
	let isPlayingAudio = false;
	// Whether the current trailing transcript entry is the agent's in-progress
	// streamed response (deltas append to it; playback_end closes it).
	let agentEntryOpen = false;
	// Mic chunks captured before the WS finishes opening (parallel init).
	let preOpenAudio: ArrayBuffer[] = [];

	function readState(): VoiceSessionState {
		let state = initialState;
		const unsub = subscribe((s) => {
			state = s;
		});
		unsub();
		return state;
	}

	function clearTimers() {
		if (keepAliveInterval) {
			clearInterval(keepAliveInterval);
			keepAliveInterval = null;
		}
		if (errorClearTimer) {
			clearTimeout(errorClearTimer);
			errorClearTimer = null;
		}
	}

	/** Convert Int16 PCM (from server) to Float32 for AudioContext playback. */
	function int16ToFloat32(int16: Int16Array): Float32Array {
		const float32 = new Float32Array(int16.length);
		for (let i = 0; i < int16.length; i++) {
			float32[i] = int16[i] / (int16[i] < 0 ? 0x8000 : 0x7fff);
		}
		return float32;
	}

	/** Play queued audio chunks through AudioContext. */
	function flushPlaybackQueue() {
		if (isPlayingAudio || pendingAudioChunks.length === 0) return;

		// Concatenate all pending chunks into one buffer
		const totalLength = pendingAudioChunks.reduce((sum, c) => sum + c.length, 0);
		const merged = new Float32Array(totalLength);
		let offset = 0;
		for (const chunk of pendingAudioChunks) {
			merged.set(chunk, offset);
			offset += chunk.length;
		}
		pendingAudioChunks = [];

		if (!playbackCtx) return;

		const audioBuffer = playbackCtx.createBuffer(1, merged.length, 24000);
		audioBuffer.copyToChannel(merged, 0);

		const source = playbackCtx.createBufferSource();
		source.buffer = audioBuffer;
		source.connect(playbackCtx.destination);

		isPlayingAudio = true;
		currentSource = source;

		source.onended = () => {
			isPlayingAudio = false;
			currentSource = null;
			// If more chunks arrived during playback, flush again
			if (pendingAudioChunks.length > 0) {
				flushPlaybackQueue();
			}
		};

		source.start();
	}

	/** Stop any in-progress TTS playback. */
	function stopPlayback() {
		if (currentSource) {
			try {
				currentSource.stop();
			} catch {
				// Already stopped
			}
			currentSource = null;
		}
		pendingAudioChunks = [];
		isPlayingAudio = false;
	}

	/** Full cleanup of all resources. */
	function cleanup() {
		clearTimers();
		stopPlayback();

		if (captureHandle) {
			captureHandle.stop();
			captureHandle = null;
		}

		if (ws) {
			if (ws.readyState === WebSocket.OPEN) {
				ws.send(JSON.stringify({ type: 'Stop' }));
			}
			ws.close();
			ws = null;
		}

		if (playbackCtx) {
			playbackCtx.close();
			playbackCtx = null;
		}
	}

	function transitionToError(message: string) {
		cleanup();
		update((s) => ({
			...s,
			status: 'error',
			errorMessage: message,
			audioLevel: 0
		}));
		log.error('Voice session error: ' + message);

		errorClearTimer = setTimeout(() => {
			set(initialState);
		}, ERROR_DISPLAY_MS);
	}

	function handleWsMessage(event: MessageEvent) {
		// Binary data = TTS audio chunk (Int16 PCM at 24kHz)
		if (event.data instanceof ArrayBuffer) {
			const int16 = new Int16Array(event.data);
			const float32 = int16ToFloat32(int16);
			pendingAudioChunks.push(float32);
			flushPlaybackQueue();
			return;
		}

		if (typeof event.data !== 'string') return;

		try {
			const msg = JSON.parse(event.data);

			switch (msg.type) {
				case 'session_initialized':
					update((s) => ({
						...s,
						status: 'listening',
						conversationId: msg.conversationId ?? s.conversationId
					}));
					log.info('Voice session initialized');
					break;

				case 'transcription_start':
					// User started speaking — if we're currently playing TTS, interrupt it
					if (readState().status === 'speaking') {
						stopPlayback();
						if (ws && ws.readyState === WebSocket.OPEN) {
							ws.send(JSON.stringify({ type: 'interrupt' }));
						}
						update((s) => ({ ...s, status: 'listening' }));
					}
					break;

				case 'transcription_text':
					// Cumulative transcript (includes upstream corrections) —
					// REPLACE, never append.
					update((s) => ({
						...s,
						interimTranscript: msg.text ?? ''
					}));
					break;

				case 'conversation_id':
					// Resumption handle from the realtime engine (30 min expiry).
					update((s) => ({ ...s, conversationId: msg.id ?? s.conversationId }));
					break;

				case 'transcription_end':
					// Finalize the user's transcript and transition to processing
					update((s) => {
						const userText = s.interimTranscript || msg.text || '';
						const newTranscripts = userText
							? [...s.transcripts, { speaker: 'user' as const, text: userText }]
							: s.transcripts;
						return {
							...s,
							status: 'processing',
							transcripts: newTranscripts,
							interimTranscript: ''
						};
					});
					break;

				case 'playback_start':
					update((s) => ({ ...s, status: 'speaking' }));
					break;

				case 'playback_end':
					agentEntryOpen = false;
					update((s) => ({ ...s, status: 'listening' }));
					break;

				case 'response_text':
					// Agent transcript arrives as DELTAS from the realtime engine —
					// append to the current agent entry (opened by playback_start),
					// creating it if the delta beats the playback frame.
					if (msg.text) {
						update((s) => {
							const t = [...s.transcripts];
							const last = t[t.length - 1];
							if (last && last.speaker === 'agent' && agentEntryOpen) {
								t[t.length - 1] = { speaker: 'agent', text: last.text + msg.text };
							} else {
								agentEntryOpen = true;
								t.push({ speaker: 'agent', text: msg.text });
							}
							return { ...s, transcripts: t };
						});
					}
					break;

				case 'Error':
					transitionToError(msg.message || 'Unknown server error');
					break;

				default:
					log.debug('Unknown voice session message type: ' + msg.type);
			}
		} catch {
			log.warn('Failed to parse voice session WS message');
		}
	}

	const store = {
		subscribe,

		/**
		 * Start a voice conversation session.
		 * Connects WebSocket, acquires mic, starts PCM capture.
		 */
		async start(agentId: string) {
			const current = readState();
			if (current.status !== 'idle') {
				log.warn('Cannot start voice session — status is ' + current.status);
				return;
			}

			// Cloud-mic consent gate: conversation audio leaves the machine
			// (xAI, directly or via Janus). No consent, no socket.
			if (!hasVoiceCloudConsent()) {
				transitionToError(
					'Voice conversation sends your microphone audio to xAI for processing. Enable it in the voice panel to consent.'
				);
				return;
			}

			update((s) => ({
				...s,
				status: 'connecting',
				agentId,
				transcripts: [],
				interimTranscript: '',
				audioLevel: 0,
				errorMessage: null,
				isMuted: false,
				conversationId: null
			}));
			agentEntryOpen = false;
			preOpenAudio = [];

			log.info('Voice session connecting for agent: ' + agentId);

			try {
				// Playback AudioContext (24kHz — matches the realtime output rate)
				playbackCtx = new AudioContext({ sampleRate: 24000 });

				// PARALLEL INIT: open the WebSocket and acquire the mic at the same
				// time (serializing them wastes the slower of the two); mic chunks
				// captured before the socket opens are buffered and flushed on open.
				const wsUrl = `${backendWsBase()}/ws/voice/conversation`;
				ws = new WebSocket(wsUrl);
				ws.binaryType = 'arraybuffer';

				const wsOpen = new Promise<void>((resolve, reject) => {
					if (!ws) return reject(new Error('WebSocket is null'));
					ws.onopen = () => resolve();
					ws.onerror = () => reject(new Error('WebSocket connection failed'));
					setTimeout(() => reject(new Error('WebSocket connection timeout')), 5000);
				});

				const sendOrBuffer = (buffer: ArrayBuffer) => {
					const state = readState();
					if (state.isMuted) return;
					if (ws && ws.readyState === WebSocket.OPEN) {
						ws.send(buffer);
					} else if (preOpenAudio.length < 50) {
						// ≤ ~5s of early audio; beyond that the connection is the problem
						preOpenAudio.push(buffer);
					}
				};

				const micReady = (async () => {
					const stream = await deviceManager.acquireMicStream();
					// Bail if session was stopped while awaiting mic permission
					const s = readState().status;
					if (s === 'idle' || s === 'error') {
						stream.getTracks().forEach((t) => t.stop());
						return;
					}
					// 24kHz capture — the realtime engine's native input rate, so
					// nothing resamples anywhere in the chain.
					captureHandle = await startPcmCapture(
						stream,
						{
							onAudioChunk: sendOrBuffer,
							onAudioLevel: (level) => {
								update((st) => ({
									...st,
									audioLevel: Math.min(1, Math.max(0, level))
								}));
							}
						},
						24000
					);
				})();

				await wsOpen;

				// Wire up handlers and flush any early mic audio
				ws.send(JSON.stringify({ type: 'Start', agentId }));
				ws.onmessage = handleWsMessage;
				ws.onclose = () => {
					const state = readState();
					if (state.status !== 'idle' && state.status !== 'error') {
						log.warn('Voice session WebSocket closed unexpectedly');
						transitionToError('Connection lost');
					}
				};
				for (const chunk of preOpenAudio) ws.send(chunk);
				preOpenAudio = [];

				await micReady;

				// Keepalive
				keepAliveInterval = setInterval(() => {
					if (ws && ws.readyState === WebSocket.OPEN) {
						ws.send(JSON.stringify({ type: 'KeepAlive' }));
					}
				}, KEEPALIVE_MS);

				log.info('Voice session mic capture started, waiting for session_initialized');
			} catch (err) {
				const msg = err instanceof Error ? err.message : 'Failed to start voice session';
				transitionToError(msg);
			}
		},

		/**
		 * Stop the voice conversation session. Cleans up everything.
		 */
		stop() {
			const current = readState();
			if (current.status === 'idle') return;

			log.info('Voice session stopped');
			cleanup();
			set(initialState);
		},

		/**
		 * Interrupt TTS playback and notify the server.
		 */
		interrupt() {
			const current = readState();
			if (current.status !== 'speaking') return;

			stopPlayback();

			if (ws && ws.readyState === WebSocket.OPEN) {
				ws.send(JSON.stringify({ type: 'interrupt' }));
			}

			update((s) => ({ ...s, status: 'listening' }));
			log.info('Voice session interrupted');
		},

		/**
		 * Toggle microphone mute/unmute.
		 */
		toggleMute() {
			update((s) => {
				const newMuted = !s.isMuted;
				log.info('Voice session mic ' + (newMuted ? 'muted' : 'unmuted'));
				return {
					...s,
					isMuted: newMuted,
					audioLevel: newMuted ? 0 : s.audioLevel
				};
			});
		}
	};

	return store;
}

// Export singleton
export const voiceSession = createVoiceSessionStore();

// Derived convenience stores
export const voiceSessionStatus = derived(voiceSession, ($s) => $s.status);
export const voiceSessionActive = derived(
	voiceSession,
	($s) => $s.status !== 'idle' && $s.status !== 'error'
);
export const voiceSessionTranscripts = derived(voiceSession, ($s) => $s.transcripts);
export const voiceSessionAudioLevel = derived(voiceSession, ($s) => $s.audioLevel);
