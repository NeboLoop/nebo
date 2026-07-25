/**
 * Audio capture for voice conversation.
 *
 * PCM capture: AudioWorklet (off the main thread — ScriptProcessorNode was
 * deprecated and glitched under load), Float32→Int16 conversion at 24kHz —
 * xAI realtime's native rate, so nothing resamples anywhere in the chain.
 * Audio level analysis: AnalyserNode, RMS, 100ms interval.
 */

import { logger } from '$lib/monitoring';

const log = logger.child({ component: 'AudioCapture' });

/** Convert Float32 PCM samples to Int16 PCM. */
export function float32ToInt16(float32: Float32Array): Int16Array {
	const int16 = new Int16Array(float32.length);
	for (let i = 0; i < float32.length; i++) {
		const s = Math.max(-1, Math.min(1, float32[i]));
		int16[i] = s < 0 ? s * 0x8000 : s * 0x7fff;
	}
	return int16;
}

export interface AudioCaptureCallbacks {
	/** Called with PCM Int16 audio chunks (ready to send over WebSocket as ArrayBuffer). */
	onAudioChunk: (buffer: ArrayBuffer) => void;
	/** Called every 100ms with audio level (0–1). */
	onAudioLevel: (level: number) => void;
}

export interface AudioCaptureHandle {
	/** Stop capture and release all resources. */
	stop: () => void;
	/** The underlying MediaStream (for device info, etc.). */
	stream: MediaStream;
}

/**
 * Worklet processor source. Accumulates 128-frame render quanta into ~100ms
 * chunks and posts the Float32 block to the main thread, which converts to
 * Int16 and hands it to the WebSocket. Loaded from a Blob URL so it needs no
 * static-asset routing (works identically under the tunnel base path).
 */
const WORKLET_SOURCE = `
class PcmCaptureProcessor extends AudioWorkletProcessor {
	constructor(options) {
		super();
		this.chunkSize = (options.processorOptions && options.processorOptions.chunkSize) || 2400;
		this.buffer = new Float32Array(this.chunkSize);
		this.offset = 0;
	}
	process(inputs) {
		const input = inputs[0] && inputs[0][0];
		if (!input) return true;
		let read = 0;
		while (read < input.length) {
			const n = Math.min(input.length - read, this.chunkSize - this.offset);
			this.buffer.set(input.subarray(read, read + n), this.offset);
			this.offset += n;
			read += n;
			if (this.offset === this.chunkSize) {
				this.port.postMessage(this.buffer.slice(0));
				this.offset = 0;
			}
		}
		return true;
	}
}
registerProcessor('pcm-capture', PcmCaptureProcessor);
`;

let workletUrl: string | null = null;
function getWorkletUrl(): string {
	if (!workletUrl) {
		workletUrl = URL.createObjectURL(new Blob([WORKLET_SOURCE], { type: 'application/javascript' }));
	}
	return workletUrl;
}

/**
 * Start capturing PCM audio from the microphone.
 *
 * Creates an AudioContext at the requested rate, loads the capture worklet,
 * and wires an AnalyserNode for level monitoring.
 *
 * @param stream - MediaStream from getUserMedia
 * @param callbacks - onAudioChunk and onAudioLevel handlers
 * @param sampleRate - capture rate (24000 = xAI realtime's native rate)
 * @returns Handle to stop capture
 */
export async function startPcmCapture(
	stream: MediaStream,
	callbacks: AudioCaptureCallbacks,
	sampleRate: number = 24000
): Promise<AudioCaptureHandle> {
	const audioCtx = new AudioContext({ sampleRate });
	const source = audioCtx.createMediaStreamSource(stream);

	await audioCtx.audioWorklet.addModule(getWorkletUrl());
	// ~100ms chunks at the capture rate.
	const worklet = new AudioWorkletNode(audioCtx, 'pcm-capture', {
		numberOfInputs: 1,
		numberOfOutputs: 0,
		processorOptions: { chunkSize: Math.round(sampleRate / 10) }
	});
	worklet.port.onmessage = (e: MessageEvent<Float32Array>) => {
		const int16 = float32ToInt16(e.data);
		callbacks.onAudioChunk(int16.buffer as ArrayBuffer);
	};
	source.connect(worklet);

	// AnalyserNode for audio level visualization
	const analyser = audioCtx.createAnalyser();
	analyser.fftSize = 2048;
	analyser.smoothingTimeConstant = 0.3;
	source.connect(analyser);

	const frequencyData = new Uint8Array(analyser.frequencyBinCount);
	const levelInterval = setInterval(() => {
		analyser.getByteFrequencyData(frequencyData);
		const rms = Math.sqrt(
			frequencyData.reduce((sum, v) => sum + v * v, 0) / frequencyData.length
		);
		const level = Math.min(1, rms / 128);
		callbacks.onAudioLevel(level);
	}, 100);

	log.info(`PCM audio capture started (${sampleRate}Hz mono, AudioWorklet)`);

	return {
		stop() {
			clearInterval(levelInterval);
			worklet.port.onmessage = null;
			worklet.disconnect();
			analyser.disconnect();
			source.disconnect();
			audioCtx.close();
			stream.getTracks().forEach((t) => t.stop());
			log.info('PCM audio capture stopped');
		},
		stream
	};
}
