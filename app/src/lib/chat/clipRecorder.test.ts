/**
 * Record a clip, against a stand-in MediaRecorder: start, stop, cancel, the
 * ten-minute cap, a refused microphone, and the file the draft receives.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('$lib/stores/devices', () => ({
	deviceManager: { acquireMicStream: async () => ({ getTracks: () => [] }) }
}));

import {
	CLIP_CAP_MS,
	SLICE_MS,
	clipClock,
	clipFile,
	createClipRecorder,
	pickClipType,
	type ClipRecorderLike
} from './clipRecorder';

/** A recorder driven by hand: `tick` is one `dataavailable`. */
class FakeRecorder implements ClipRecorderLike {
	static instances: FakeRecorder[] = [];
	static supported = new Set(['audio/mp4;codecs=mp4a.40.2', 'audio/mp4']);
	static isTypeSupported(t: string) {
		return FakeRecorder.supported.has(t);
	}
	mimeType: string;
	slice = 0;
	stopped = false;
	ondataavailable: ((e: { data: Blob }) => void) | null = null;
	onstop: (() => void) | null = null;
	onerror: ((e: unknown) => void) | null = null;
	constructor(
		public stream: MediaStream,
		opts?: { mimeType?: string }
	) {
		this.mimeType = opts?.mimeType ?? 'audio/webm;codecs=opus';
		FakeRecorder.instances.push(this);
	}
	start(timeslice?: number) {
		this.slice = timeslice ?? 0;
	}
	tick(bytes = 'aaaa') {
		this.ondataavailable?.({ data: new Blob([bytes]) });
	}
	stop() {
		if (this.stopped) throw new Error('already stopped');
		this.stopped = true;
		this.tick('zz'); // the last slice
		this.onstop?.();
	}
}

function fakeStream() {
	const track = { stop: vi.fn() };
	return { stream: { getTracks: () => [track] } as unknown as MediaStream, track };
}

let clock = 0;
let clips: File[] = [];

function recorderWith(acquire: () => Promise<MediaStream>) {
	return createClipRecorder({
		onClip: (f) => clips.push(f),
		acquire,
		Recorder: FakeRecorder,
		now: () => clock,
		meter: () => ({ level: () => 0.5, close: () => {} })
	});
}

beforeEach(() => {
	FakeRecorder.instances = [];
	FakeRecorder.supported = new Set(['audio/mp4;codecs=mp4a.40.2', 'audio/mp4']);
	clock = 1_000;
	clips = [];
});

describe('recording a clip', () => {
	it('starts on the chosen microphone in MP4 and shows time and level', async () => {
		const { stream } = fakeStream();
		const acquire = vi.fn(async () => stream);
		const rec = recorderWith(acquire);
		expect(get(rec).stage).toBe('idle');
		expect(acquire).not.toHaveBeenCalled(); // nothing until the click

		await rec.start();
		expect(acquire).toHaveBeenCalledOnce();
		const r = FakeRecorder.instances[0];
		expect(r.stream).toBe(stream);
		expect(r.mimeType).toBe('audio/mp4;codecs=mp4a.40.2');
		expect(r.slice).toBe(SLICE_MS);
		expect(get(rec).stage).toBe('recording');

		clock += 42_000;
		r.tick();
		expect(get(rec).elapsedMs).toBe(42_000);
		expect(get(rec).level).toBe(0.5);
	});

	it('stop attaches the recording and lets the microphone go', async () => {
		const { stream, track } = fakeStream();
		const rec = recorderWith(async () => stream);
		await rec.start();
		FakeRecorder.instances[0].tick();
		rec.stop();

		expect(clips).toHaveLength(1);
		expect(clips[0].type).toBe('audio/mp4');
		expect(clips[0].name).toMatch(/^recording-\d{8}-\d{6}\.m4a$/);
		expect(clips[0].size).toBe(6);
		expect(track.stop).toHaveBeenCalled();
		expect(get(rec)).toEqual({ stage: 'idle', elapsedMs: 0, level: 0, notice: '' });
	});

	it('cancel throws the recording away', async () => {
		const { stream, track } = fakeStream();
		const rec = recorderWith(async () => stream);
		await rec.start();
		FakeRecorder.instances[0].tick();
		rec.cancel();

		expect(clips).toHaveLength(0);
		expect(track.stop).toHaveBeenCalled();
		expect(get(rec).stage).toBe('idle');
		expect(get(rec).notice).toBe('');
	});

	it('cancel while the microphone is being asked for never records', async () => {
		const { stream, track } = fakeStream();
		let grant: (s: MediaStream) => void = () => {};
		const rec = recorderWith(() => new Promise((res) => (grant = res)));
		const starting = rec.start();
		expect(get(rec).stage).toBe('starting');
		rec.cancel();
		grant(stream);
		await starting;

		expect(FakeRecorder.instances).toHaveLength(0);
		expect(track.stop).toHaveBeenCalled();
		expect(get(rec).stage).toBe('idle');
	});

	it('stops itself at ten minutes, attaches what it has, and says so', async () => {
		const { stream } = fakeStream();
		const rec = recorderWith(async () => stream);
		await rec.start();
		const r = FakeRecorder.instances[0];
		clock += CLIP_CAP_MS - 1;
		r.tick();
		expect(get(rec).stage).toBe('recording');
		clock += 1;
		r.tick();

		expect(r.stopped).toBe(true);
		expect(clips).toHaveLength(1);
		expect(get(rec).stage).toBe('idle');
		expect(get(rec).notice).toBe('capReached');
	});

	it('a refused microphone says so plainly and records nothing', async () => {
		const rec = recorderWith(async () => {
			throw new DOMException('denied', 'NotAllowedError');
		});
		await rec.start();
		expect(get(rec).stage).toBe('idle');
		expect(get(rec).notice).toBe('micBlocked');
		expect(FakeRecorder.instances).toHaveLength(0);

		rec.dismiss();
		expect(get(rec).notice).toBe('');
	});

	it('no microphone at all is its own message', async () => {
		const rec = recorderWith(async () => {
			throw new DOMException('none', 'NotFoundError');
		});
		await rec.start();
		expect(get(rec).notice).toBe('noMic');
	});

	it('a webview without MP4 records WebM/Opus', async () => {
		FakeRecorder.supported = new Set(['audio/webm;codecs=opus', 'audio/webm']);
		const { stream } = fakeStream();
		const rec = recorderWith(async () => stream);
		await rec.start();
		FakeRecorder.instances[0].tick();
		rec.stop();
		expect(clips[0].type).toBe('audio/webm');
		expect(clips[0].name).toMatch(/\.webm$/);
	});

	it('a second click while recording does not start another', async () => {
		const { stream } = fakeStream();
		const rec = recorderWith(async () => stream);
		await rec.start();
		await rec.start();
		expect(FakeRecorder.instances).toHaveLength(1);
	});
});

describe('the attachment', () => {
	it('is named like the phone\'s and typed without codecs', () => {
		const at = new Date(2026, 9, 4, 9, 5, 7);
		const f = clipFile(new Blob(['x']), 'audio/mp4;codecs=mp4a.40.2', at);
		expect(f.name).toBe('recording-20261004-090507.m4a');
		expect(f.type).toBe('audio/mp4');
	});

	it('a sound-only WebM is audio, not video', () => {
		const f = clipFile(new Blob(['x']), 'video/webm;codecs=opus', new Date(2026, 0, 1, 0, 0, 0));
		expect(f.name).toBe('recording-20260101-000000.webm');
		expect(f.type).toBe('audio/webm');
	});

	it('picks MP4 first, then WebM, else lets the webview choose', () => {
		expect(pickClipType((t) => t.startsWith('audio/'))).toBe('audio/mp4;codecs=mp4a.40.2');
		expect(pickClipType((t) => t === 'audio/webm;codecs=opus')).toBe('audio/webm;codecs=opus');
		expect(pickClipType(() => false)).toBe('');
		expect(pickClipType(undefined)).toBe('');
	});

	it('reads the time as m:ss', () => {
		expect(clipClock(0)).toBe('0:00');
		expect(clipClock(65_400)).toBe('1:05');
		expect(clipClock(CLIP_CAP_MS)).toBe('10:00');
	});
});
