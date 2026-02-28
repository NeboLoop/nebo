/**
 * AudioWorklet processor that plays back PCM audio from a ring buffer.
 * Receives Int16LE ArrayBuffer messages via port.postMessage().
 * Outputs Float32 samples. Zero-fills on underrun for glitch-free playback.
 *
 * Usage: new AudioWorkletNode(ctx, 'playback-processor', { outputChannelCount: [1] })
 */
class PlaybackProcessor extends AudioWorkletProcessor {
	constructor() {
		super();
		// Ring buffer: ~500ms at 16kHz = 8000 samples
		this.bufferSize = 8000;
		this.buffer = new Float32Array(this.bufferSize);
		this.writePos = 0;
		this.readPos = 0;
		this.count = 0; // samples available

		this.port.onmessage = (e) => {
			const data = e.data;
			if (data instanceof ArrayBuffer) {
				this._enqueue(data);
			} else if (data === 'clear') {
				this._clear();
			}
		};
	}

	_enqueue(arrayBuffer) {
		const int16 = new Int16Array(arrayBuffer);
		for (let i = 0; i < int16.length; i++) {
			if (this.count >= this.bufferSize) {
				break; // buffer full, drop oldest would be complex — just stop
			}
			this.buffer[this.writePos] = int16[i] / 32768.0;
			this.writePos = (this.writePos + 1) % this.bufferSize;
			this.count++;
		}
	}

	_clear() {
		this.writePos = 0;
		this.readPos = 0;
		this.count = 0;
	}

	process(inputs, outputs) {
		const output = outputs[0];
		if (!output || !output[0]) return true;

		const channel = output[0];
		for (let i = 0; i < channel.length; i++) {
			if (this.count > 0) {
				channel[i] = this.buffer[this.readPos];
				this.readPos = (this.readPos + 1) % this.bufferSize;
				this.count--;
			} else {
				channel[i] = 0; // zero-fill underrun
			}
		}

		return true;
	}
}

registerProcessor('playback-processor', PlaybackProcessor);
