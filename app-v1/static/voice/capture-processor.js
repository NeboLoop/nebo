/**
 * AudioWorklet processor that captures microphone audio.
 * Converts Float32 samples to Int16LE and posts buffers to the main thread.
 *
 * Usage: new AudioWorkletNode(ctx, 'capture-processor')
 * Receives ArrayBuffer messages via port.onmessage.
 */
class CaptureProcessor extends AudioWorkletProcessor {
	process(inputs) {
		const input = inputs[0];
		if (!input || !input[0] || input[0].length === 0) {
			return true;
		}

		const float32 = input[0]; // mono channel
		const int16 = new Int16Array(float32.length);

		for (let i = 0; i < float32.length; i++) {
			// Clamp to [-1, 1] and convert to Int16
			const s = Math.max(-1, Math.min(1, float32[i]));
			int16[i] = s < 0 ? s * 0x8000 : s * 0x7fff;
		}

		this.port.postMessage(int16.buffer, [int16.buffer]);
		return true;
	}
}

registerProcessor('capture-processor', CaptureProcessor);
