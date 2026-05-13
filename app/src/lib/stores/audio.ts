/**
 * Shared audio infrastructure for dictation and voice conversation.
 *
 * PCM capture: ScriptProcessorNode at 16kHz mono, Float32→Int16 conversion.
 * Audio level analysis: AnalyserNode, RMS, 100ms interval.
 * Used by both dictation WebSocket and voice conversation WebSocket.
 */

import { logger } from '$lib/monitoring';

const log = logger.child({ component: 'AudioCapture' });

/** Convert Float32 PCM samples to Int16 PCM (whisper-rs expects Int16 at 16kHz). */
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
 * Start capturing PCM audio from the microphone.
 *
 * Creates an AudioContext at 16kHz, connects a ScriptProcessorNode for
 * PCM capture, and an AnalyserNode for level monitoring.
 *
 * @param stream - MediaStream from getUserMedia
 * @param callbacks - onAudioChunk and onAudioLevel handlers
 * @returns Handle to stop capture
 */
export function startPcmCapture(
	stream: MediaStream,
	callbacks: AudioCaptureCallbacks
): AudioCaptureHandle {
	const audioCtx = new AudioContext({ sampleRate: 16000 });
	const source = audioCtx.createMediaStreamSource(stream);

	// ScriptProcessorNode for raw PCM capture
	// Buffer size 4096 at 16kHz = 256ms chunks
	const processor = audioCtx.createScriptProcessor(4096, 1, 1);
	processor.onaudioprocess = (e) => {
		const float32 = e.inputBuffer.getChannelData(0);
		const int16 = float32ToInt16(float32);
		callbacks.onAudioChunk(int16.buffer as ArrayBuffer);
	};

	source.connect(processor);
	processor.connect(audioCtx.destination);

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

	log.info('PCM audio capture started (16kHz mono)');

	return {
		stop() {
			clearInterval(levelInterval);
			processor.disconnect();
			analyser.disconnect();
			source.disconnect();
			audioCtx.close();
			stream.getTracks().forEach((t) => t.stop());
			log.info('PCM audio capture stopped');
		},
		stream
	};
}
