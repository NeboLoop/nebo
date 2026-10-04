/**
 * Record a clip: the owner's own voice, from the microphone he chose, as an
 * audio file on the message he is writing — the desktop's voice note.
 *
 * It arrives the way the phone's does (nebo-mobile voice_recorder_sheet.dart):
 * a `recording-YYYYMMDD-HHMMSS.m4a` typed `audio/mp4`, staged in the composer
 * and uploaded through the ONE upload path like any picked file. Where the
 * webview cannot write MP4 it is `.webm` typed `audio/webm` (Opus).
 *
 * Only the owner's click starts it. Nothing an employee does reaches here.
 *
 * Time and the level meter move on the recorder's own `dataavailable` events
 * (one every SLICE_MS), so nothing ticks on a timer.
 */

import { writable, type Readable } from 'svelte/store';
import { deviceManager } from '$lib/stores/devices';

/** Recordings stop here and are attached as they stand. */
export const CLIP_CAP_MS = 10 * 60 * 1000;

/** How often the recorder hands over audio, and so how often the clock and
 *  the meter move. */
export const SLICE_MS = 200;

/** In order of preference. MP4/AAC first: it is what the phone sends, and
 *  what WebKit (Tauri on macOS) records. Chromium (WebView2 on Windows)
 *  records Opus in WebM, and newer builds MP4 as well. */
export const CLIP_TYPES = [
	'audio/mp4;codecs=mp4a.40.2',
	'audio/mp4',
	'audio/webm;codecs=opus',
	'audio/webm',
	'audio/ogg;codecs=opus'
];

export type ClipStage = 'idle' | 'starting' | 'recording';

/** Why the composer is saying something. Each maps to one line of copy. */
export type ClipNotice = '' | 'micBlocked' | 'noMic' | 'unsupported' | 'failed' | 'capReached';

export interface ClipState {
	stage: ClipStage;
	elapsedMs: number;
	/** Microphone level, 0..1. */
	level: number;
	notice: ClipNotice;
}

/** The part of MediaRecorder this uses — a test hands in a stand-in. */
export interface ClipRecorderLike {
	readonly mimeType: string;
	start(timeslice?: number): void;
	stop(): void;
	ondataavailable: ((e: { data: Blob }) => void) | null;
	onstop: (() => void) | null;
	onerror: ((e: unknown) => void) | null;
}

export interface ClipRecorderCtor {
	new (stream: MediaStream, options?: { mimeType?: string }): ClipRecorderLike;
	isTypeSupported?: (type: string) => boolean;
}

/** Reads the microphone level while recording. */
export interface ClipMeter {
	level(): number;
	close(): void;
}

export interface ClipDeps {
	/** The finished recording, for the draft. */
	onClip: (file: File) => void;
	/** The chosen microphone, or the default. */
	acquire?: () => Promise<MediaStream>;
	Recorder?: ClipRecorderCtor;
	now?: () => number;
	meter?: (stream: MediaStream) => ClipMeter | null;
}

/** The first type this webview can record, or '' to let it choose. */
export function pickClipType(isTypeSupported?: (type: string) => boolean): string {
	if (!isTypeSupported) return '';
	for (const t of CLIP_TYPES) {
		try {
			if (isTypeSupported(t)) return t;
		} catch {
			/* a webview that throws on a codec string does not record it */
		}
	}
	return '';
}

const EXT_FOR: Record<string, string> = {
	'audio/mp4': 'm4a',
	'video/mp4': 'm4a',
	'audio/aac': 'aac',
	'audio/webm': 'webm',
	'video/webm': 'webm',
	'audio/ogg': 'ogg',
	'audio/mpeg': 'mp3',
	'audio/wav': 'wav'
};

/** The finished recording as a file: named like the phone's, typed by what
 *  the recorder actually wrote (codecs dropped — the upload, the bot and the
 *  audio card read the base type). A sound-only WebM or MP4 is audio. */
export function clipFile(blob: Blob, mimeType: string, at: Date): File {
	const base = (mimeType || blob.type || 'audio/mp4').split(';')[0].trim().toLowerCase();
	const ext = EXT_FOR[base] ?? 'm4a';
	const type = base.startsWith('video/') ? base.replace('video/', 'audio/') : base;
	const two = (n: number) => String(n).padStart(2, '0');
	const stamp =
		`${at.getFullYear()}${two(at.getMonth() + 1)}${two(at.getDate())}` +
		`-${two(at.getHours())}${two(at.getMinutes())}${two(at.getSeconds())}`;
	return new File([blob], `recording-${stamp}.${ext}`, { type });
}

/** `m:ss`, as the recording bar shows it. */
export function clipClock(ms: number): string {
	const s = Math.max(0, Math.floor(ms / 1000));
	return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

/** What a failed getUserMedia means, in the composer's words. */
function noticeFor(err: unknown): ClipNotice {
	const name = (err as { name?: string } | null)?.name ?? '';
	if (name === 'NotAllowedError' || name === 'SecurityError' || name === 'PermissionDeniedError') {
		return 'micBlocked';
	}
	if (name === 'NotFoundError' || name === 'OverconstrainedError' || name === 'DevicesNotFoundError') {
		return 'noMic';
	}
	return 'failed';
}

/** The level from an AnalyserNode on the stream, read when asked. */
function analyserMeter(stream: MediaStream): ClipMeter | null {
	try {
		const ctx = new AudioContext();
		const source = ctx.createMediaStreamSource(stream);
		const analyser = ctx.createAnalyser();
		analyser.fftSize = 1024;
		source.connect(analyser);
		const buf = new Uint8Array(analyser.fftSize);
		return {
			level() {
				analyser.getByteTimeDomainData(buf);
				let sum = 0;
				for (const v of buf) sum += ((v - 128) / 128) ** 2;
				// Speech sits around 0.05–0.2 RMS; scale so it fills the bar.
				return Math.min(1, Math.sqrt(sum / buf.length) * 4);
			},
			close() {
				source.disconnect();
				void ctx.close();
			}
		};
	} catch {
		return null; // no meter is fine; the recording still works
	}
}

export interface ClipRecorder extends Readable<ClipState> {
	/** Start recording. Only ever called from the owner's click. */
	start(): Promise<void>;
	/** Stop and attach the recording. */
	stop(): void;
	/** Stop and throw the recording away. */
	cancel(): void;
	/** Clear the notice line. */
	dismiss(): void;
}

const IDLE: ClipState = { stage: 'idle', elapsedMs: 0, level: 0, notice: '' };

export function createClipRecorder(deps: ClipDeps): ClipRecorder {
	const acquire = deps.acquire ?? (() => deviceManager.acquireMicStream('clip'));
	const now = deps.now ?? (() => Date.now());
	const makeMeter = deps.meter ?? analyserMeter;

	const state = writable<ClipState>({ ...IDLE });
	let stage: ClipStage = 'idle';
	let take = 0; // bumps on every start/cancel, so a late permission answer is ignored
	let recorder: ClipRecorderLike | null = null;
	let stream: MediaStream | null = null;
	let meter: ClipMeter | null = null;
	let chunks: Blob[] = [];
	let startedAt = 0;
	let keep = false;
	let capped = false;
	let stopping = false;

	function set(next: Partial<ClipState>) {
		if (next.stage) stage = next.stage;
		state.update((s) => ({ ...s, ...next }));
	}

	function release() {
		meter?.close();
		meter = null;
		stream?.getTracks().forEach((t) => t.stop());
		stream = null;
		if (recorder) {
			recorder.ondataavailable = null;
			recorder.onerror = null;
		}
	}

	function finish(keepIt: boolean) {
		if (stage !== 'recording' || !recorder || stopping) return;
		stopping = true;
		keep = keepIt;
		try {
			recorder.stop(); // the last data, then onstop
		} catch {
			recorder.onstop?.();
		}
	}

	async function start() {
		if (stage !== 'idle') return;
		const Recorder = deps.Recorder ?? (globalThis as { MediaRecorder?: ClipRecorderCtor }).MediaRecorder;
		if (!Recorder) {
			set({ ...IDLE, notice: 'unsupported' });
			return;
		}
		const mine = ++take;
		set({ ...IDLE, stage: 'starting' });

		let s: MediaStream;
		try {
			s = await acquire();
		} catch (err) {
			if (mine !== take) return;
			set({ ...IDLE, notice: noticeFor(err) });
			return;
		}
		if (mine !== take) {
			s.getTracks().forEach((t) => t.stop()); // cancelled while asking
			return;
		}
		stream = s;

		const type = pickClipType(Recorder.isTypeSupported?.bind(Recorder));
		let r: ClipRecorderLike;
		try {
			r = new Recorder(s, type ? { mimeType: type } : undefined);
		} catch {
			release();
			set({ ...IDLE, notice: 'unsupported' });
			return;
		}
		recorder = r;
		chunks = [];
		keep = false;
		capped = false;
		stopping = false;
		meter = makeMeter(s);

		r.ondataavailable = (e) => {
			if (e.data && e.data.size > 0) chunks.push(e.data);
			if (stage !== 'recording' || stopping) return;
			const elapsedMs = now() - startedAt;
			set({ elapsedMs: Math.min(elapsedMs, CLIP_CAP_MS), level: meter?.level() ?? 0 });
			if (elapsedMs >= CLIP_CAP_MS) {
				capped = true;
				finish(true);
			}
		};
		r.onerror = () => {
			r.onstop = null;
			keep = false;
			release();
			recorder = null;
			chunks = [];
			set({ ...IDLE, notice: 'failed' });
		};
		r.onstop = () => {
			const kept = keep;
			const recorded = chunks;
			const written = r.mimeType || type;
			chunks = [];
			release();
			recorder = null;
			if (!kept) {
				set({ ...IDLE });
				return;
			}
			const blob = new Blob(recorded, { type: written.split(';')[0] });
			if (blob.size === 0) {
				set({ ...IDLE, notice: 'failed' });
				return;
			}
			set({ ...IDLE, notice: capped ? 'capReached' : '' });
			deps.onClip(clipFile(blob, written, new Date()));
		};

		try {
			r.start(SLICE_MS);
		} catch {
			release();
			recorder = null;
			set({ ...IDLE, notice: 'failed' });
			return;
		}
		startedAt = now();
		set({ stage: 'recording', elapsedMs: 0, level: 0, notice: '' });
	}

	return {
		subscribe: state.subscribe,
		start,
		stop: () => finish(true),
		cancel() {
			if (stage === 'starting') {
				take++;
				set({ ...IDLE });
				return;
			}
			finish(false);
		},
		dismiss: () => set({ notice: '' })
	};
}
