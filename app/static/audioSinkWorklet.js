/**
 * AudioSinkWorklet — ring buffer + AudioWorkletProcessor
 *
 * Loaded from static/ so it can be referenced by URL in AudioWorklet.addModule().
 * Uses SharedArrayBuffer + Atomics for lock-free producer/consumer communication
 * between the main thread and the audio rendering thread.
 */

// ── RingBuffer ────────────────────────────────────────────────────────────────

class RingBuffer {
  /**
   * @param {SharedArrayBuffer} sab - Backing buffer (must be pre-allocated)
   *
   * Layout (all values are Int32):
   *   [0] = writeHead
   *   [1] = readHead
   *   [2] = capacity  (set once by producer)
   *
   * Float32 sample storage starts at byte offset 12 (3 * 4).
   */
  constructor(sab) {
    this._sab = sab;
    this._meta = new Int32Array(sab, 0, 3);
    // Sample storage occupies the rest of the buffer
    const sampleByteOffset = 3 * Int32Array.BYTES_PER_ELEMENT;
    const sampleCount = (sab.byteLength - sampleByteOffset) / Float32Array.BYTES_PER_ELEMENT;
    this._capacity = sampleCount | 0;
    this._storage = new Float32Array(sab, sampleByteOffset, this._capacity);
    // Store capacity in meta so both sides agree
    Atomics.store(this._meta, 2, this._capacity);
    this._underrunCount = 0;
    this._playedSamples = 0;
  }

  /** Number of samples available to read. */
  availableRead() {
    const w = Atomics.load(this._meta, 0);
    const r = Atomics.load(this._meta, 1);
    if (w >= r) return w - r;
    return this._capacity - r + w;
  }

  /** Number of samples that can be written before overrun. */
  availableWrite() {
    return this._capacity - 1 - this.availableRead();
  }

  /**
   * Push samples into the ring buffer (producer side).
   * @param {Float32Array} input
   * @returns {number} Number of samples actually written.
   */
  push(input) {
    const toWrite = Math.min(input.length, this.availableWrite());
    if (toWrite === 0) return 0;

    let w = Atomics.load(this._meta, 0);

    for (let i = 0; i < toWrite; i++) {
      this._storage[w] = input[i];
      w = (w + 1) % this._capacity;
    }

    Atomics.store(this._meta, 0, w);
    return toWrite;
  }

  /**
   * Pop samples from the ring buffer (consumer side).
   * @param {Float32Array} output
   * @returns {number} Number of samples actually read.
   */
  pop(output) {
    const toRead = Math.min(output.length, this.availableRead());

    if (toRead === 0) {
      // Fill with silence
      output.fill(0);
      this._underrunCount++;
      return 0;
    }

    let r = Atomics.load(this._meta, 1);

    for (let i = 0; i < toRead; i++) {
      output[i] = this._storage[r];
      r = (r + 1) % this._capacity;
    }

    // Fill remainder with silence if output is larger than available
    for (let i = toRead; i < output.length; i++) {
      output[i] = 0;
    }

    Atomics.store(this._meta, 1, r);
    this._playedSamples += toRead;
    return toRead;
  }

  /** Reset read and write heads to zero. */
  clear() {
    Atomics.store(this._meta, 0, 0);
    Atomics.store(this._meta, 1, 0);
  }

  /**
   * Drain all remaining samples into a new Float32Array.
   * @returns {Float32Array}
   */
  drainAll() {
    const avail = this.availableRead();
    if (avail === 0) return new Float32Array(0);
    const out = new Float32Array(avail);
    this.pop(out);
    return out;
  }

  /** Number of underrun events (consumer found nothing to read). */
  get underrunCount() {
    return this._underrunCount;
  }

  /** Total samples played since creation. */
  get playedSamples() {
    return this._playedSamples;
  }
}

// ── AudioSinkProcessor ────────────────────────────────────────────────────────

class AudioSinkProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();

    const sab = options.processorOptions?.sab;
    if (!sab) {
      throw new Error('AudioSinkProcessor requires processorOptions.sab (SharedArrayBuffer)');
    }

    this._ringBuffer = new RingBuffer(sab);
    this._wasPlaying = false;
    this._flushing = false;

    this.port.onmessage = (e) => {
      const msg = e.data;
      if (msg === 'flush' || msg?.type === 'flush') {
        this._ringBuffer.clear();
        this._flushing = true;
      } else if (msg === 'turn_end' || msg?.type === 'turn_end') {
        // Mark that no more data is coming — once buffer drains, post "drained"
        this._flushing = false;
      }
    };
  }

  process(inputs, outputs) {
    const output = outputs[0];
    if (!output || output.length === 0) return true;

    const channel = output[0];
    const samplesRead = this._ringBuffer.pop(channel);
    const isPlaying = samplesRead > 0;

    // Copy mono to all channels if multi-channel output
    for (let ch = 1; ch < output.length; ch++) {
      output[ch].set(channel);
    }

    // Detect transition from playing -> empty (drained)
    if (this._wasPlaying && !isPlaying) {
      this.port.postMessage({ type: 'drained' });
    }

    this._wasPlaying = isPlaying;

    // Keep processor alive
    return true;
  }
}

registerProcessor('audio-sink-processor', AudioSinkProcessor);
