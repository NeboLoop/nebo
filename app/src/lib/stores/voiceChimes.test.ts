/**
 * The call's two sounds are the bundled WAV files, decoded for the call's
 * playback context. The files are held to what was chosen (mono 16-bit PCM,
 * the connect sound 0.35 s, the disconnect sound 0.30 s, two different
 * sounds), and the loader to decoding exactly those two on the context it is
 * given.
 */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { loadVoiceChimes } from './voiceChimes';

function wav(name: string) {
	const bytes = readFileSync(
		fileURLToPath(new URL(`../assets/sounds/${name}.wav`, import.meta.url))
	);
	const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
	const tag = (at: number) => String.fromCharCode(...bytes.subarray(at, at + 4));
	let format = 0;
	let channels = 0;
	let rate = 0;
	let bits = 0;
	let frames = 0;
	for (let at = 12; at + 8 <= bytes.length; ) {
		const size = view.getUint32(at + 4, true);
		if (tag(at) === 'fmt ') {
			format = view.getUint16(at + 8, true);
			channels = view.getUint16(at + 10, true);
			rate = view.getUint32(at + 12, true);
			bits = view.getUint16(at + 22, true);
		}
		if (tag(at) === 'data') frames = size / 2;
		at += 8 + size + (size & 1);
	}
	return { riff: tag(0) + tag(8), format, channels, rate, bits, seconds: frames / rate, bytes };
}

describe('the bundled call sounds', () => {
	for (const [name, seconds] of [
		['nebo-voice-connect', 0.35],
		['nebo-voice-disconnect', 0.3]
	] as const) {
		it(`${name} is the chosen mono 16-bit WAV, ${seconds} s long`, () => {
			const w = wav(name);
			expect(w.riff).toBe('RIFFWAVE');
			expect([w.format, w.channels, w.bits]).toEqual([1, 1, 16]);
			expect(w.seconds).toBeCloseTo(seconds, 2);
		});
	}

	it('are two different sounds', () => {
		expect(wav('nebo-voice-connect').bytes.equals(wav('nebo-voice-disconnect').bytes)).toBe(false);
	});
});

describe('loadVoiceChimes', () => {
	afterEach(() => vi.unstubAllGlobals());

	it('decodes both bundled files on the call context', async () => {
		const fetched: string[] = [];
		vi.stubGlobal('fetch', async (url: string) => {
			fetched.push(url);
			return { arrayBuffer: async () => ({ url }) };
		});
		const ctx = {
			decodeAudioData: async (data: { url: string }) => ({ decoded: data.url })
		} as unknown as BaseAudioContext;

		const chimes = await loadVoiceChimes(ctx);

		expect(fetched).toHaveLength(2);
		expect(fetched[0]).toContain('nebo-voice-connect');
		expect(fetched[1]).toContain('nebo-voice-disconnect');
		expect(chimes.connect).toEqual({ decoded: fetched[0] });
		expect(chimes.disconnect).toEqual({ decoded: fetched[1] });
	});
});
