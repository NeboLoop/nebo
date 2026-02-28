/**
 * VoiceSession manages a full-duplex voice WebSocket connection.
 * Binary audio frames flow in both directions alongside JSON control messages.
 *
 * Browser Mic → AudioWorklet (Float32→Int16LE) → /ws/voice Binary
 * /ws/voice Binary → PlaybackProcessor → Speakers
 */

export type VoiceState = 'idle' | 'connecting' | 'listening' | 'processing' | 'speaking' | 'interrupting' | 'error';

export interface VoiceSessionCallbacks {
	onStateChange?: (state: VoiceState) => void;
	onTranscript?: (text: string) => void;
	onVadState?: (isSpeech: boolean) => void;
	onError?: (message: string) => void;
	onWakeWord?: () => void;
}

export class VoiceSession {
	private ws: WebSocket | null = null;
	private audioContext: AudioContext | null = null;
	private captureNode: AudioWorkletNode | null = null;
	private playbackNode: AudioWorkletNode | null = null;
	private mediaStream: MediaStream | null = null;
	private state: VoiceState = 'idle';
	private callbacks: VoiceSessionCallbacks;
	private reconnectTimer: ReturnType<typeof setTimeout> | null = null;

	constructor(callbacks: VoiceSessionCallbacks = {}) {
		this.callbacks = callbacks;
	}

	/** Connect to the duplex voice WebSocket and start audio capture/playback. */
	async connect(voice?: string): Promise<void> {
		if (this.ws) {
			this.disconnect();
		}

		this.setState('connecting');

		try {
			// Request microphone access
			this.mediaStream = await navigator.mediaDevices.getUserMedia({
				audio: {
					sampleRate: 16000,
					channelCount: 1,
					echoCancellation: true,
					noiseSuppression: true,
					autoGainControl: true
				}
			});

			// Create AudioContext at 16kHz
			this.audioContext = new AudioContext({ sampleRate: 16000 });

			// Load AudioWorklet processors
			await this.audioContext.audioWorklet.addModule('/voice/capture-processor.js');
			await this.audioContext.audioWorklet.addModule('/voice/playback-processor.js');

			// Create capture worklet (mic → WS)
			this.captureNode = new AudioWorkletNode(this.audioContext, 'capture-processor');
			const source = this.audioContext.createMediaStreamSource(this.mediaStream);
			source.connect(this.captureNode);

			// Create playback worklet (WS → speakers)
			this.playbackNode = new AudioWorkletNode(this.audioContext, 'playback-processor', {
				outputChannelCount: [1]
			});
			this.playbackNode.connect(this.audioContext.destination);

			// Connect WebSocket
			const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
			const wsUrl = `${protocol}//${window.location.host}/ws/voice`;
			this.ws = new WebSocket(wsUrl);
			this.ws.binaryType = 'arraybuffer';

			this.ws.onopen = () => {
				// Send config message
				if (voice) {
					this.sendControl({ type: 'config', voice });
				}
				this.setState('listening');
			};

			this.ws.onmessage = (event) => {
				if (event.data instanceof ArrayBuffer) {
					// Binary audio from server → playback
					this.playbackNode?.port.postMessage(event.data, [event.data]);
				} else {
					// JSON control message
					this.handleControl(JSON.parse(event.data));
				}
			};

			this.ws.onclose = () => {
				this.setState('idle');
				this.cleanup();
			};

			this.ws.onerror = () => {
				this.callbacks.onError?.('WebSocket connection failed');
				this.setState('error');
				this.cleanup();
			};

			// Wire capture worklet → WebSocket
			this.captureNode.port.onmessage = (event) => {
				if (this.ws?.readyState === WebSocket.OPEN) {
					this.ws.send(event.data);
				}
			};
		} catch (err) {
			const message = err instanceof Error ? err.message : 'Failed to connect';
			this.callbacks.onError?.(message);
			this.setState('error');
			this.cleanup();
			throw err;
		}
	}

	/** Disconnect and clean up all resources. */
	disconnect(): void {
		if (this.ws) {
			this.ws.close(1000, 'User disconnected');
			this.ws = null;
		}
		this.cleanup();
		this.setState('idle');
	}

	/** Send an interrupt signal to stop TTS playback. */
	interrupt(): void {
		this.sendControl({ type: 'interrupt' });
		// Clear local playback buffer
		this.playbackNode?.port.postMessage('clear');
	}

	/** Get the current voice state. */
	getState(): VoiceState {
		return this.state;
	}

	/** Check if the session is active (connected and not idle/error). */
	isActive(): boolean {
		return this.state !== 'idle' && this.state !== 'error';
	}

	private setState(state: VoiceState): void {
		this.state = state;
		this.callbacks.onStateChange?.(state);
	}

	private sendControl(msg: Record<string, unknown>): void {
		if (this.ws?.readyState === WebSocket.OPEN) {
			this.ws.send(JSON.stringify(msg));
		}
	}

	private handleControl(msg: { type: string; state?: string; text?: string; is_speech?: boolean }): void {
		switch (msg.type) {
			case 'state':
				if (msg.state) {
					this.setState(msg.state as VoiceState);
				}
				break;
			case 'transcript':
				if (msg.text) {
					this.callbacks.onTranscript?.(msg.text);
				}
				break;
			case 'vad_state':
				this.callbacks.onVadState?.(msg.is_speech ?? false);
				break;
			case 'error':
				this.callbacks.onError?.(msg.text ?? 'Unknown error');
				break;
		}
	}

	/** Start listening for "Hey Nebo" wake word. Low-power mic → server-side detection. */
	async startWakeWordListening(): Promise<void> {
		if (this.ws) {
			this.disconnect();
		}

		this.setState('connecting');

		try {
			this.mediaStream = await navigator.mediaDevices.getUserMedia({
				audio: {
					sampleRate: 16000,
					channelCount: 1,
					echoCancellation: true,
					noiseSuppression: true,
					autoGainControl: true
				}
			});

			this.audioContext = new AudioContext({ sampleRate: 16000 });
			await this.audioContext.audioWorklet.addModule('/voice/capture-processor.js');

			this.captureNode = new AudioWorkletNode(this.audioContext, 'capture-processor');
			const source = this.audioContext.createMediaStreamSource(this.mediaStream);
			source.connect(this.captureNode);

			const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
			const wsUrl = `${protocol}//${window.location.host}/ws/voice/wake`;
			this.ws = new WebSocket(wsUrl);
			this.ws.binaryType = 'arraybuffer';

			this.ws.onopen = () => {
				this.setState('listening');
			};

			this.ws.onmessage = (event) => {
				if (typeof event.data === 'string') {
					const msg = JSON.parse(event.data);
					if (msg.type === 'wake') {
						this.callbacks.onWakeWord?.();
					}
				}
			};

			this.ws.onclose = () => {
				this.setState('idle');
				this.cleanup();
			};

			this.ws.onerror = () => {
				this.callbacks.onError?.('Wake word connection failed');
				this.setState('error');
				this.cleanup();
			};

			this.captureNode.port.onmessage = (event) => {
				if (this.ws?.readyState === WebSocket.OPEN) {
					this.ws.send(event.data);
				}
			};
		} catch (err) {
			const message = err instanceof Error ? err.message : 'Failed to start wake word';
			this.callbacks.onError?.(message);
			this.setState('error');
			this.cleanup();
			throw err;
		}
	}

	private cleanup(): void {
		if (this.reconnectTimer) {
			clearTimeout(this.reconnectTimer);
			this.reconnectTimer = null;
		}

		if (this.captureNode) {
			this.captureNode.disconnect();
			this.captureNode = null;
		}

		if (this.playbackNode) {
			this.playbackNode.disconnect();
			this.playbackNode = null;
		}

		if (this.mediaStream) {
			this.mediaStream.getTracks().forEach((t) => t.stop());
			this.mediaStream = null;
		}

		if (this.audioContext) {
			this.audioContext.close().catch(() => {});
			this.audioContext = null;
		}
	}
}
