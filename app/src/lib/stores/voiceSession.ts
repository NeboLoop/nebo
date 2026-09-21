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
import { finishUserTranscript } from './voiceTranscript';
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
 * consent that audio leaves the machine. The UI names no provider (product
 * decision 2026-09-15). Base-scoped storage, one grant per install.
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
	/** The line dropped mid-call and the client is redialing the same thread. */
	| 'reconnecting'
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
	/** Chat the server bound this call to — announced via `chat_bound` only
	 *  once a turn actually persisted, so no frame means no chat was created. */
	boundChatId: string | null;
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
	boundChatId: null,
	errorMessage: null
};

// --- Constants ---

const KEEPALIVE_MS = 4_000;
const ERROR_DISPLAY_MS = 5_000;
const CONNECT_TIMEOUT_MS = 5_000;

/**
 * Reconnect window and backoff.
 *
 * A bot's carrier tunnel resets at arbitrary intervals — a hub deploy severs
 * it, and an overnight log shows resets minutes apart — so a dropped socket is
 * not the end of the call. The server rejoins a voice session by THREAD ID:
 * hand the same `chat_id` back on the redial and it binds that thread again
 * and feeds its history to the employee, and it counts a thread as the live
 * one for VOICE_RESUME_WINDOW — 30 minutes
 * (crates/server/src/handlers/voice.rs). The client redials for exactly that
 * long before it admits the call is over.
 *
 * Backoff doubles from half a second to eight, jittered ±25% so a hub deploy
 * that dropped every bot at once doesn't bring them all back in lockstep.
 */
const RESUME_WINDOW_MS = 30 * 60_000;
const RECONNECT_BASE_MS = 500;
const RECONNECT_MAX_MS = 8_000;
const RECONNECT_JITTER = 0.25;
/** Close 1012 — the server said it is restarting, so it answers again shortly. */
const SERVER_RESTART_CODE = 1012;
const SERVER_RESTART_DELAY_MS = 250;

// --- Store ---

function createVoiceSessionStore() {
	const { subscribe, set, update } = writable<VoiceSessionState>(initialState);

	// Active session resources
	let ws: WebSocket | null = null;
	let captureHandle: AudioCaptureHandle | null = null;
	let keepAliveInterval: ReturnType<typeof setInterval> | null = null;
	let errorClearTimer: ReturnType<typeof setTimeout> | null = null;
	// What this call dialed with, kept so a drop can redial the same session.
	let sessionAgentId = '';
	let sessionChatId: string | undefined;
	let sessionTeamId: string | undefined;
	// Reconnect bookkeeping: the pending retry, how many redials this outage
	// has cost (the backoff step), when the resume window closes, and whether a
	// socket is already opening — the guard against a second one.
	let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
	let reconnectAttempt = 0;
	let resumeDeadline = 0;
	let socketPending = false;

	// TTS playback resources
	let playbackCtx: AudioContext | null = null;
	let pendingAudioChunks: Float32Array[] = [];
	let currentSource: AudioBufferSourceNode | null = null;
	let isPlayingAudio = false;
	// Whether the current trailing transcript entry is the agent's in-progress
	// streamed response (deltas append to it; playback_end closes it).
	let agentEntryOpen = false;
	// Whether the trailing entry is the user's just-finished utterance, which a
	// late correction may still replace (see finishUserTranscript).
	let userEntryOpen = false;
	// Mic chunks captured before the WS finishes opening (parallel init).
	let preOpenAudio: ArrayBuffer[] = [];
	// Screen wake lock held for the duration of a call. Without it, mobile
	// browsers auto-lock the screen mid-conversation and SUSPEND the page —
	// freezing JS, killing the mic, and closing the WebSocket (close 1001
	// "closed due to suspension"). Re-acquired on visibility return because
	// the OS silently releases it whenever the page is hidden.
	let wakeLock: { release(): Promise<void> } | null = null;
	let wakeLockVisibilityHandler: (() => void) | null = null;

	async function acquireWakeLock() {
		try {
			const nav = navigator as Navigator & {
				wakeLock?: { request(type: 'screen'): Promise<{ release(): Promise<void> }> };
			};
			if (!nav.wakeLock) return; // unsupported: old browsers keep old behavior
			wakeLock = await nav.wakeLock.request('screen');
			if (!wakeLockVisibilityHandler) {
				wakeLockVisibilityHandler = () => {
					if (document.visibilityState === 'visible' && readState().status !== 'idle') {
						acquireWakeLock();
					}
				};
				document.addEventListener('visibilitychange', wakeLockVisibilityHandler);
			}
		} catch {
			// Denied (low battery mode etc.) — the call still works, the screen
			// just isn't protected from auto-lock.
		}
	}

	function releaseWakeLock() {
		if (wakeLockVisibilityHandler) {
			document.removeEventListener('visibilitychange', wakeLockVisibilityHandler);
			wakeLockVisibilityHandler = null;
		}
		wakeLock?.release().catch(() => {});
		wakeLock = null;
	}

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
		if (reconnectTimer) {
			clearTimeout(reconnectTimer);
			reconnectTimer = null;
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
		releaseWakeLock();

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
		// Nulling ws above is what keeps a closing socket from redialing: every
		// close handler bails on a socket the store no longer holds.
		socketPending = false;

		if (playbackCtx) {
			playbackCtx.close();
			playbackCtx = null;
		}
	}

	/**
	 * The one voice URL, built fresh for every dial.
	 *
	 * On a redial the chat id is the resume handle: `boundChatId` is the thread
	 * the server announced (`chat_bound`), so handing it back rejoins that
	 * transcript instead of letting the server resolve a thread afresh. Before
	 * any turn has persisted there is no bound thread, so the id the call was
	 * opened with rides instead, and a call opened with neither lets the server
	 * pick the same way it did the first time.
	 */
	function dialUrl(): string {
		const params = new URLSearchParams();
		if (sessionTeamId) {
			params.set('team_id', sessionTeamId);
		} else {
			if (sessionAgentId) params.set('agent_id', sessionAgentId);
			const chat = readState().boundChatId ?? sessionChatId;
			if (chat) params.set('chat_id', chat);
		}
		const qs = params.size > 0 ? `?${params.toString()}` : '';
		return `${backendWsBase()}/ws/voice/conversation${qs}`;
	}

	/**
	 * Open the call's WebSocket — the ONE socket path, used by the first dial
	 * and by every redial. Resolves once the socket is open, its handlers are
	 * wired and the session is announced; rejects if it never opens.
	 */
	function openSocket(): Promise<void> {
		const sock = new WebSocket(dialUrl());
		ws = sock;
		sock.binaryType = 'arraybuffer';
		socketPending = true;
		return new Promise<void>((resolve, reject) => {
			const settle = (err?: Error) => {
				socketPending = false;
				if (err) reject(err);
				else resolve();
			};
			sock.onopen = () => {
				sock.onmessage = handleWsMessage;
				sock.onclose = (ev: CloseEvent) => handleWsClose(sock, ev);
				sock.send(JSON.stringify({ type: 'Start', agentId: sessionAgentId }));
				for (const chunk of preOpenAudio) sock.send(chunk);
				preOpenAudio = [];
				startKeepAlive();
				settle();
			};
			sock.onerror = () => settle(new Error('WebSocket connection failed'));
			setTimeout(() => {
				if (sock.readyState === WebSocket.OPEN) return;
				sock.close();
				settle(new Error('WebSocket connection timeout'));
			}, CONNECT_TIMEOUT_MS);
		});
	}

	function startKeepAlive() {
		if (keepAliveInterval) clearInterval(keepAliveInterval);
		keepAliveInterval = setInterval(() => {
			if (ws && ws.readyState === WebSocket.OPEN) {
				ws.send(JSON.stringify({ type: 'KeepAlive' }));
			}
		}, KEEPALIVE_MS);
	}

	/**
	 * A socket died with the call still up. Anything the dead socket owned goes
	 * — the keepalive, the tail of a sentence the employee will not finish, the
	 * half-heard utterance — but the transcript and the screen wake lock stay:
	 * this is the same call, and it is about to be rejoined.
	 */
	function handleWsClose(sock: WebSocket, ev: CloseEvent) {
		// A socket the store has already moved on from (cleanup, or superseded
		// by a redial) is nobody's business.
		if (ws !== sock) return;
		ws = null;
		const status = readState().status;
		if (status === 'idle' || status === 'error') return;
		log.warn('Voice session WebSocket closed unexpectedly (code ' + ev.code + ')');
		if (keepAliveInterval) {
			clearInterval(keepAliveInterval);
			keepAliveInterval = null;
		}
		stopPlayback();
		agentEntryOpen = false;
		userEntryOpen = false;
		// Audio spoken into a dead line belongs to no session — dropped, never
		// replayed into the rejoined one.
		preOpenAudio = [];
		if (status !== 'reconnecting') {
			resumeDeadline = Date.now() + RESUME_WINDOW_MS;
		}
		update((s) => ({ ...s, status: 'reconnecting', interimTranscript: '', audioLevel: 0 }));
		scheduleReconnect(ev.code === SERVER_RESTART_CODE);
	}

	/** Arm the next redial, or give up once the resume window has closed. */
	function scheduleReconnect(serverRestarting: boolean) {
		// Never a second socket: one retry armed, one dial in flight.
		if (reconnectTimer || socketPending) return;
		if (Date.now() >= resumeDeadline) {
			transitionToError(
				'The call dropped and could not be rejoined within the 30-minute window. Everything said before that is saved in the thread.'
			);
			return;
		}
		reconnectAttempt++;
		const base =
			serverRestarting && reconnectAttempt === 1
				? SERVER_RESTART_DELAY_MS
				: Math.min(RECONNECT_BASE_MS * 2 ** (reconnectAttempt - 1), RECONNECT_MAX_MS);
		const delay = Math.round(base * (1 + (Math.random() * 2 - 1) * RECONNECT_JITTER));
		log.info('Voice session redialing in ' + delay + 'ms (attempt ' + reconnectAttempt + ')');
		reconnectTimer = setTimeout(() => {
			reconnectTimer = null;
			void attemptReconnect();
		}, delay);
	}

	async function attemptReconnect() {
		const status = readState().status;
		if (status !== 'reconnecting' || socketPending) return;
		try {
			await openSocket();
			// Still 'reconnecting' until the server answers with
			// session_initialized — an open socket is not yet a rejoined call.
		} catch {
			scheduleReconnect(false);
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
					// The call is live again (or for the first time) — the backoff
					// starts from scratch for whatever the next outage is.
					if (readState().status === 'reconnecting') {
						log.info('Voice session rejoined after ' + reconnectAttempt + ' redial(s)');
					} else {
						log.info('Voice session initialized');
					}
					reconnectAttempt = 0;
					update((s) => ({
						...s,
						status: 'listening',
						conversationId: msg.conversationId ?? s.conversationId
					}));
					break;

				case 'transcription_start':
					// Barge-in: the user's voice always wins. Kill local playback
					// immediately — including tail audio still buffered after the
					// server finished generating (status already 'listening') —
					// and cancel upstream only mid-response (the server drops the
					// cancel when nothing is in flight, so the race is harmless).
					stopPlayback();
					if (readState().status === 'speaking') {
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

				case 'chat_bound':
					update((s) => ({ ...s, boundChatId: msg.chatId ?? s.boundChatId }));
					break;

				case 'conversation_id':
					// Resumption handle from the realtime engine (30 min expiry).
					update((s) => ({ ...s, conversationId: msg.id ?? s.conversationId }));
					break;

				case 'transcription_end':
					// Finalize the user's transcript and transition to processing
					update((s) => {
						const userText = s.interimTranscript || msg.text || '';
						const newTranscripts = finishUserTranscript(s.transcripts, userText, userEntryOpen);
						if (userText) userEntryOpen = true;
						return {
							...s,
							status: 'processing',
							transcripts: newTranscripts,
							interimTranscript: ''
						};
					});
					break;

				case 'playback_start':
					userEntryOpen = false;
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
					// xAI emits sentence-level segments with NO separator between
					// them ("...asking!What can I..."), so restore the space when a
					// segment ends a sentence and the next opens one. Uppercase
					// check keeps mid-token continuations (e.g. "3." + "14") intact.
					if (msg.text) {
						update((s) => {
							const t = [...s.transcripts];
							const last = t[t.length - 1];
							if (last && last.speaker === 'agent' && agentEntryOpen) {
								const needsSpace =
									/[.!?…]["')\]]?$/.test(last.text) && /^["'([]?[A-Z]/.test(msg.text);
								t[t.length - 1] = {
									speaker: 'agent',
									text: last.text + (needsSpace ? ' ' : '') + msg.text
								};
							} else {
								agentEntryOpen = true;
								userEntryOpen = false;
								t.push({ speaker: 'agent', text: msg.text });
							}
							return { ...s, transcripts: t };
						});
					}
					break;

				case 'Error':
					// A refusal that lands mid-redial is the end of the resuming:
					// the server has told us why it will not take this call back.
					transitionToError(
						readState().status === 'reconnecting'
							? 'Could not rejoin the call: ' + (msg.message || 'the bot refused the session')
							: msg.message || 'Unknown server error'
					);
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
		/**
		 * @param agentId - the employee this call belongs to (tool/session scope)
		 * @param chatId - the chat thread the transcript persists into; every
		 *   finished turn lands there as a normal message, so closing the call
		 *   leaves the whole exchange in the chat window.
		 * @param teamId - set when the call is opened from a team thread: the
		 *   server picks the team's lead to speak and posts every finished turn
		 *   into the team thread (owner's words from the owner, the lead's
		 *   reply from the lead); agentId and chatId are not sent.
		 */
		async start(agentId: string, chatId?: string, teamId?: string) {
			const current = readState();
			if (current.status !== 'idle') {
				log.warn('Cannot start voice session — status is ' + current.status);
				return;
			}

			// Cloud-mic consent gate: conversation audio leaves the machine
			// (xAI, directly or via Janus). No consent, no socket.
			if (!hasVoiceCloudConsent()) {
				transitionToError(
					'Voice conversation sends your microphone audio to a cloud voice service for processing. Enable it in the voice panel to consent.'
				);
				return;
			}

			sessionAgentId = agentId;
			sessionChatId = chatId;
			sessionTeamId = teamId;
			reconnectAttempt = 0;
			resumeDeadline = 0;

			update((s) => ({
				...s,
				status: 'connecting',
				agentId,
				transcripts: [],
				interimTranscript: '',
				audioLevel: 0,
				errorMessage: null,
				isMuted: false,
				conversationId: null,
				boundChatId: null
			}));
			agentEntryOpen = false;
			userEntryOpen = false;
			preOpenAudio = [];

			// Keep the screen awake for the whole call — auto-lock suspends the
			// page and kills the session (fire-and-forget; failure is benign).
			acquireWakeLock();

			log.info('Voice session connecting for agent: ' + agentId);

			try {
				// Playback AudioContext (24kHz — matches the realtime output rate)
				playbackCtx = new AudioContext({ sampleRate: 24000 });

				// PARALLEL INIT: open the WebSocket and acquire the mic at the same
				// time (serializing them wastes the slower of the two); mic chunks
				// captured before the socket opens are buffered and flushed on open.
				const wsOpen = openSocket();

				const sendOrBuffer = (buffer: ArrayBuffer) => {
					const state = readState();
					if (state.isMuted) return;
					if (ws && ws.readyState === WebSocket.OPEN) {
						ws.send(buffer);
					} else if (state.status !== 'reconnecting' && preOpenAudio.length < 50) {
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
				await micReady;

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
			// boundChatId records what the call produced, so it has to outlive the
			// call — the closer reads it to land in the thread voice just created.
			// `start()` clears it for the next session.
			set({ ...initialState, boundChatId: current.boundChatId });
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
