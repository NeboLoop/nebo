import { writable, derived, get } from 'svelte/store';
import { speakTTS, transcribeAudio } from '$lib/api/index';
import { logger } from '$lib/monitoring';

const log = logger.child({ component: 'VoiceStore' });

/**
 * Voice pipeline state
 */
export interface VoiceState {
	isRecording: boolean;
	isPlaying: boolean;
	isSpeaking: boolean;
	error: string | null;
	currentVoice: string;
	volume: number;
}

const initialState: VoiceState = {
	isRecording: false,
	isPlaying: false,
	isSpeaking: false,
	error: null,
	currentVoice: 'default',
	volume: 1.0
};

// Ring buffer size: ~5 seconds of 24kHz mono audio
const RING_BUFFER_SAMPLES = 24000 * 5;
// SharedArrayBuffer layout: 3 Int32 meta fields + float samples
const SAB_BYTE_SIZE = 3 * Int32Array.BYTES_PER_ELEMENT + RING_BUFFER_SAMPLES * Float32Array.BYTES_PER_ELEMENT;

function createVoiceStore() {
	const { subscribe, set, update } = writable<VoiceState>(initialState);

	let audioContext: AudioContext | null = null;
	let workletNode: AudioWorkletNode | null = null;
	let ringBufferSab: SharedArrayBuffer | null = null;
	let mediaStream: MediaStream | null = null;
	let mediaRecorder: MediaRecorder | null = null;
	let recordedChunks: Blob[] = [];
	let workletReady = false;

	/**
	 * Write Float32 samples into the SharedArrayBuffer ring buffer.
	 * Mirrors the RingBuffer.push() logic from audioSinkWorklet.js.
	 */
	function pushSamples(sab: SharedArrayBuffer, input: Float32Array): number {
		const meta = new Int32Array(sab, 0, 3);
		const sampleByteOffset = 3 * Int32Array.BYTES_PER_ELEMENT;
		const capacity = Atomics.load(meta, 2);
		const storage = new Float32Array(sab, sampleByteOffset, capacity);

		// Calculate available write space
		const w = Atomics.load(meta, 0);
		const r = Atomics.load(meta, 1);
		const used = w >= r ? w - r : capacity - r + w;
		const available = capacity - 1 - used;
		const toWrite = Math.min(input.length, available);

		if (toWrite === 0) return 0;

		let writeHead = w;
		for (let i = 0; i < toWrite; i++) {
			storage[writeHead] = input[i];
			writeHead = (writeHead + 1) % capacity;
		}

		Atomics.store(meta, 0, writeHead);
		return toWrite;
	}

	return {
		subscribe,

		/**
		 * Initialize the AudioContext, load the worklet, and set up the ring buffer.
		 */
		async initAudio(): Promise<void> {
			if (workletReady) return;

			try {
				audioContext = new AudioContext({ sampleRate: 24000 });

				// Load the worklet processor from static/
				await audioContext.audioWorklet.addModule('/audioSinkWorklet.js');

				// Create SharedArrayBuffer for the ring buffer
				ringBufferSab = new SharedArrayBuffer(SAB_BYTE_SIZE);

				// Initialize the capacity meta field
				const meta = new Int32Array(ringBufferSab, 0, 3);
				Atomics.store(meta, 0, 0); // writeHead
				Atomics.store(meta, 1, 0); // readHead
				Atomics.store(meta, 2, RING_BUFFER_SAMPLES); // capacity

				// Create the AudioWorkletNode
				workletNode = new AudioWorkletNode(audioContext, 'audio-sink-processor', {
					processorOptions: { sab: ringBufferSab }
				});

				workletNode.connect(audioContext.destination);

				// Listen for drained messages from the worklet
				workletNode.port.onmessage = (e) => {
					if (e.data?.type === 'drained') {
						update((s) => ({ ...s, isPlaying: false, isSpeaking: false }));
						log.debug('Audio playback drained');
					}
				};

				workletReady = true;
				log.info('Audio pipeline initialized');
			} catch (err) {
				const msg = err instanceof Error ? err.message : 'Failed to initialize audio';
				update((s) => ({ ...s, error: msg }));
				log.error('Audio init failed', err);
				throw err;
			}
		},

		/**
		 * Start recording audio from the microphone.
		 */
		async startRecording(): Promise<void> {
			try {
				update((s) => ({ ...s, error: null }));

				mediaStream = await navigator.mediaDevices.getUserMedia({
					audio: {
						echoCancellation: true,
						noiseSuppression: true,
						sampleRate: 16000
					}
				});

				recordedChunks = [];

				mediaRecorder = new MediaRecorder(mediaStream, {
					mimeType: MediaRecorder.isTypeSupported('audio/webm;codecs=opus')
						? 'audio/webm;codecs=opus'
						: 'audio/webm'
				});

				mediaRecorder.ondataavailable = (e) => {
					if (e.data.size > 0) {
						recordedChunks.push(e.data);
					}
				};

				mediaRecorder.start(100); // Collect chunks every 100ms
				update((s) => ({ ...s, isRecording: true }));
				log.info('Recording started');
			} catch (err) {
				const msg = err instanceof Error ? err.message : 'Failed to start recording';
				update((s) => ({ ...s, error: msg }));
				log.error('Recording start failed', err);
				throw err;
			}
		},

		/**
		 * Stop recording and return the recorded audio as a Blob.
		 */
		async stopRecording(): Promise<Blob> {
			return new Promise((resolve, reject) => {
				if (!mediaRecorder || mediaRecorder.state === 'inactive') {
					update((s) => ({ ...s, isRecording: false }));
					reject(new Error('No active recording'));
					return;
				}

				mediaRecorder.onstop = () => {
					const blob = new Blob(recordedChunks, {
						type: mediaRecorder?.mimeType || 'audio/webm'
					});
					recordedChunks = [];

					// Stop all media stream tracks
					if (mediaStream) {
						mediaStream.getTracks().forEach((t) => t.stop());
						mediaStream = null;
					}

					update((s) => ({ ...s, isRecording: false }));
					log.info('Recording stopped, blob size: ' + blob.size);
					resolve(blob);
				};

				mediaRecorder.onerror = (e) => {
					update((s) => ({ ...s, isRecording: false, error: 'Recording error' }));
					log.error('MediaRecorder error', e);
					reject(new Error('Recording error'));
				};

				mediaRecorder.stop();
			});
		},

		/**
		 * Call the TTS API, decode the audio, and push samples into the ring buffer for playback.
		 */
		async playTTS(text: string, voice?: string): Promise<void> {
			try {
				// Ensure audio is initialized
				if (!workletReady) {
					await this.initAudio();
				}

				// Resume AudioContext if it was suspended (browser autoplay policy)
				if (audioContext?.state === 'suspended') {
					await audioContext.resume();
				}

				update((s) => ({
					...s,
					isPlaying: true,
					isSpeaking: true,
					error: null,
					currentVoice: voice || s.currentVoice
				}));

				// Fetch TTS audio from backend
				const audioBlob = await speakTTS({
					text,
					voice: voice || get({ subscribe }).currentVoice
				});

				// Decode audio blob to PCM samples
				const arrayBuffer = await audioBlob.arrayBuffer();
				const audioBuffer = await audioContext!.decodeAudioData(arrayBuffer);

				// Get mono channel data (use first channel)
				const samples = audioBuffer.getChannelData(0);

				// Push samples into the ring buffer
				if (ringBufferSab) {
					// Push in chunks to avoid overflowing the ring buffer
					const chunkSize = 4096;
					for (let offset = 0; offset < samples.length; offset += chunkSize) {
						const end = Math.min(offset + chunkSize, samples.length);
						const chunk = samples.subarray(offset, end);
						pushSamples(ringBufferSab, chunk);
					}
				}

				log.info('TTS playback started, samples: ' + samples.length);
			} catch (err) {
				const msg = err instanceof Error ? err.message : 'TTS playback failed';
				update((s) => ({ ...s, isPlaying: false, isSpeaking: false, error: msg }));
				log.error('TTS playback failed', err);
				throw err;
			}
		},

		/**
		 * Stop playback by flushing the ring buffer.
		 */
		stopPlayback(): void {
			if (workletNode) {
				workletNode.port.postMessage({ type: 'flush' });
			}
			update((s) => ({ ...s, isPlaying: false, isSpeaking: false }));
			log.debug('Playback stopped');
		},

		/**
		 * Transcribe a recorded audio blob to text via the backend.
		 */
		async transcribe(audioBlob: Blob): Promise<string> {
			try {
				const result = await transcribeAudio(audioBlob);
				return result.text;
			} catch (err) {
				const msg = err instanceof Error ? err.message : 'Transcription failed';
				update((s) => ({ ...s, error: msg }));
				log.error('Transcription failed', err);
				throw err;
			}
		},

		/**
		 * Clean up all audio resources.
		 */
		cleanup(): void {
			// Stop recording if active
			if (mediaRecorder && mediaRecorder.state !== 'inactive') {
				mediaRecorder.stop();
			}
			mediaRecorder = null;

			// Stop media stream tracks
			if (mediaStream) {
				mediaStream.getTracks().forEach((t) => t.stop());
				mediaStream = null;
			}

			// Disconnect and close audio worklet
			if (workletNode) {
				workletNode.disconnect();
				workletNode = null;
			}

			// Close AudioContext
			if (audioContext) {
				audioContext.close();
				audioContext = null;
			}

			ringBufferSab = null;
			workletReady = false;
			recordedChunks = [];

			set(initialState);
			log.info('Voice pipeline cleaned up');
		},

		/**
		 * Set or clear the error message.
		 */
		setError(error: string | null): void {
			update((s) => ({ ...s, error }));
		}
	};
}

// Export the voice store singleton
export const voiceStore = createVoiceStore();

// Derived stores for convenience
export const isRecording = derived(voiceStore, ($v) => $v.isRecording);
export const isPlaying = derived(voiceStore, ($v) => $v.isPlaying);
export const isSpeaking = derived(voiceStore, ($v) => $v.isSpeaking);
export const voiceError = derived(voiceStore, ($v) => $v.error);
