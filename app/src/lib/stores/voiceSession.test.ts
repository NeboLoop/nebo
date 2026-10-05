/**
 * Reconnect behaviour of the desktop voice client, against a fake WebSocket.
 *
 * The bot's carrier tunnel resets mid-call, so a dropped socket has to be
 * redialed — same thread, same transcript, same wake lock — for as long as the
 * server would still take the call back (VOICE_RESUME_WINDOW, 30 minutes).
 *
 * None of which the owner is told about. A redial that lands inside the quiet
 * window (RECONNECT_QUIET_MS, 4s) never reaches the screen: the call keeps the
 * status it had. Only an outage that outlasts it says one line.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { get } from 'svelte/store';
import en from '$lib/i18n/locales/en.json';

vi.mock('$lib/api/base', () => ({ backendWsBase: () => 'ws://bot.test' }));
vi.mock('$lib/storage', () => ({ storage: { get: () => 'granted', set: () => {} } }));
vi.mock('$lib/monitoring', () => ({
	logger: { child: () => ({ info: () => {}, warn: () => {}, error: () => {}, debug: () => {} }) }
}));
vi.mock('$lib/stores/audio', () => ({
	startPcmCapture: async () => ({ stop: () => {} })
}));
vi.mock('$lib/stores/devices', () => ({
	deviceManager: { acquireMicStream: async () => ({ getTracks: () => [] }) }
}));

/** Every socket the store dialed, in order, driven by hand. */
class FakeSocket {
	static CONNECTING = 0;
	static OPEN = 1;
	static CLOSING = 2;
	static CLOSED = 3;
	static instances: FakeSocket[] = [];

	readyState = 0;
	binaryType = '';
	sent: string[] = [];
	onopen: (() => void) | null = null;
	onerror: (() => void) | null = null;
	onclose: ((ev: { code: number }) => void) | null = null;
	onmessage: ((ev: { data: string | ArrayBuffer }) => void) | null = null;

	constructor(public url: string) {
		FakeSocket.instances.push(this);
	}
	send(data: string) {
		this.sent.push(data);
	}
	close() {
		this.readyState = FakeSocket.CLOSED;
	}
	/** The server accepted the dial. */
	open() {
		this.readyState = FakeSocket.OPEN;
		this.onopen?.();
	}
	/** The line dropped. 1006 = abnormal, 1012 = the server is restarting. */
	drop(code = 1006) {
		this.readyState = FakeSocket.CLOSED;
		this.onclose?.({ code });
	}
	emit(msg: Record<string, unknown>) {
		this.onmessage?.({ data: JSON.stringify(msg) });
	}
	/** `ms` of the employee's 24 kHz PCM16 audio, as one binary frame. */
	audio(ms: number) {
		this.onmessage?.({ data: new Int16Array(ms * 24).buffer });
	}
	/** The JSON frames the client sent, parsed. */
	frames(): Array<Record<string, unknown>> {
		return this.sent.filter((s) => typeof s === 'string').map((s) => JSON.parse(s));
	}
}

const released = vi.fn();

/** The playback AudioContext's clock (seconds) and every source it played. */
let audioClock = 0;
let sources: Array<{
	onended: (() => void) | null;
	stopped: boolean;
	buffer: { data?: Float32Array; sound?: string } | null;
}> = [];
/** Every playback AudioContext the store made, and whether it was closed. */
let contexts: Array<{ closed: boolean }> = [];

async function startCall(chatId?: string) {
	// The store reads its sentences through the i18n layer: the module
	// registry is fresh each test, so the locale is set on that fresh copy.
	const i18n = await import('svelte-i18n');
	i18n.addMessages('en', en);
	await i18n.init({ fallbackLocale: 'en', initialLocale: 'en' });
	const { voiceSession } = await import('./voiceSession');
	const started = voiceSession.start('employee-1', chatId);
	const first = FakeSocket.instances[0];
	first.open();
	await started;
	return { voiceSession, first };
}

/** A call that is live, bound to a thread, with one agent turn on record. */
async function liveCall() {
	const { voiceSession, first } = await startCall('thread-1');
	first.emit({ type: 'session_initialized' });
	first.emit({ type: 'chat_bound', chatId: 'thread-9' });
	first.emit({ type: 'playback_start' });
	first.emit({ type: 'response_text', text: 'Good morning.' });
	first.emit({ type: 'playback_end' });
	expect(get(voiceSession).status).toBe('listening');
	return { voiceSession, first };
}

beforeEach(() => {
	vi.resetModules();
	FakeSocket.instances = [];
	released.mockClear();
	vi.useFakeTimers();
	// No jitter spread in the test: 0.5 is the midpoint, so delays are exact.
	vi.spyOn(Math, 'random').mockReturnValue(0.5);
	vi.stubGlobal('WebSocket', FakeSocket);
	audioClock = 0;
	sources = [];
	contexts = [];
	vi.stubGlobal('AudioContext', class {
		destination = {};
		closed = false;
		constructor() {
			contexts.push(this);
		}
		get currentTime() {
			return audioClock;
		}
		createBuffer() {
			return { copyToChannel: () => {} };
		}
		/** A bundled sound, named after its file. */
		async decodeAudioData(data: { url: string }) {
			return { sound: data.url.includes('disconnect') ? 'disconnect' : 'connect' };
		}
		createBufferSource() {
			const source = {
				buffer: null as { data?: Float32Array; sound?: string } | null,
				connect: () => {},
				start: () => {},
				stopped: false,
				stop() {
					source.stopped = true;
				},
				onended: null as (() => void) | null
			};
			sources.push(source);
			return source;
		}
		close() {
			this.closed = true;
		}
	});
	vi.stubGlobal('fetch', async (url: string) => ({ arrayBuffer: async () => ({ url }) }));
	vi.stubGlobal('navigator', {
		wakeLock: { request: async () => ({ release: async () => released() }) }
	});
	vi.stubGlobal('document', {
		visibilityState: 'visible',
		addEventListener: () => {},
		removeEventListener: () => {}
	});
});

afterEach(() => {
	vi.useRealTimers();
	vi.unstubAllGlobals();
	vi.restoreAllMocks();
});

describe('voiceSession reconnect', () => {
	it('says nothing at all about a drop the redial fixes inside the quiet window', async () => {
		const { voiceSession } = await liveCall();

		FakeSocket.instances[0].drop(1006);
		expect(get(voiceSession).status).toBe('listening');

		await vi.advanceTimersByTimeAsync(500);
		const second = FakeSocket.instances[1];
		second.open();
		expect(get(voiceSession).status).toBe('listening');
		second.emit({ type: 'session_initialized' });
		expect(get(voiceSession).status).toBe('listening');

		// The quiet window passes with the call long since back: the line that
		// would have announced the outage never fires.
		await vi.advanceTimersByTimeAsync(10_000);
		expect(get(voiceSession).status).toBe('listening');
	});

	it('says one line, once, when an outage outlasts the quiet window', async () => {
		const { voiceSession } = await liveCall();

		FakeSocket.instances[0].drop(1006);
		await vi.advanceTimersByTimeAsync(3_999);
		expect(get(voiceSession).status).toBe('listening');

		await vi.advanceTimersByTimeAsync(1);
		expect(get(voiceSession).status).toBe('reconnecting');
	});

	it('redials the bound thread after an unexpected close, keeping the transcript and the wake lock', async () => {
		const { voiceSession } = await liveCall();

		FakeSocket.instances[0].drop(1006);
		// Inside the quiet window the screen is untouched — the call still
		// reads as listening while the redial runs behind it.
		expect(get(voiceSession).status).toBe('listening');
		expect(get(voiceSession).transcripts).toEqual([{ speaker: 'agent', text: 'Good morning.' }]);
		expect(released).not.toHaveBeenCalled();
		expect(FakeSocket.instances).toHaveLength(1);

		await vi.advanceTimersByTimeAsync(500);
		expect(FakeSocket.instances).toHaveLength(2);
		const second = FakeSocket.instances[1];
		// The resume handshake: the same call dialed back with the thread the
		// server bound it to, so the server rejoins that transcript.
		expect(second.url).toContain('agent_id=employee-1');
		expect(second.url).toContain('chat_id=thread-9');

		second.open();
		expect(JSON.parse(second.sent[0])).toEqual({ type: 'Start', agentId: 'employee-1' });
		expect(get(voiceSession).status).toBe('listening');

		second.emit({ type: 'session_initialized' });
		expect(get(voiceSession).status).toBe('listening');
		expect(get(voiceSession).transcripts).toEqual([{ speaker: 'agent', text: 'Good morning.' }]);
	});

	it('backs off exponentially, and starts over once the call is back', async () => {
		const { voiceSession } = await liveCall();

		FakeSocket.instances[0].drop(1006);
		await vi.advanceTimersByTimeAsync(499);
		expect(FakeSocket.instances).toHaveLength(1);
		await vi.advanceTimersByTimeAsync(1);
		expect(FakeSocket.instances).toHaveLength(2);

		// Second redial only after the connect timeout, then twice the wait.
		await vi.advanceTimersByTimeAsync(5_000);
		expect(FakeSocket.instances).toHaveLength(2);
		await vi.advanceTimersByTimeAsync(1_000);
		expect(FakeSocket.instances).toHaveLength(3);

		FakeSocket.instances[2].open();
		FakeSocket.instances[2].emit({ type: 'session_initialized' });
		expect(get(voiceSession).status).toBe('listening');

		// A later drop starts from the first step again, not the third.
		FakeSocket.instances[2].drop(1006);
		await vi.advanceTimersByTimeAsync(500);
		expect(FakeSocket.instances).toHaveLength(4);
	});

	it('comes back promptly when the server says it is restarting (1012)', async () => {
		await liveCall();

		FakeSocket.instances[0].drop(1012);
		await vi.advanceTimersByTimeAsync(100);
		expect(FakeSocket.instances).toHaveLength(1);
		await vi.advanceTimersByTimeAsync(150);
		expect(FakeSocket.instances).toHaveLength(2);
	});

	it('never opens a second socket while one is still connecting', async () => {
		const { voiceSession } = await liveCall();
		const first = FakeSocket.instances[0];

		first.drop(1006);
		await vi.advanceTimersByTimeAsync(500);
		expect(FakeSocket.instances).toHaveLength(2);

		// A stale close from the socket the store already let go, and every
		// moment the redial spends connecting: still one dial in flight.
		first.drop(1006);
		await vi.advanceTimersByTimeAsync(4_000);
		expect(FakeSocket.instances).toHaveLength(2);
		expect(get(voiceSession).status).toBe('reconnecting');
	});

	it('does not redial a call the owner ended', async () => {
		const { voiceSession, first } = await liveCall();

		voiceSession.stop();
		expect(get(voiceSession).status).toBe('idle');
		expect(released).toHaveBeenCalled();
		expect(JSON.parse(first.sent[first.sent.length - 1])).toEqual({ type: 'Stop' });

		// The socket the browser closes after the owner hung up is not a drop.
		first.drop(1000);
		await vi.advanceTimersByTimeAsync(60_000);
		expect(FakeSocket.instances).toHaveLength(1);
		expect(get(voiceSession).status).toBe('idle');
	});

	it('ends in the error state, saying so, when the server refuses the resume', async () => {
		const { voiceSession } = await liveCall();

		FakeSocket.instances[0].drop(1006);
		await vi.advanceTimersByTimeAsync(500);
		const second = FakeSocket.instances[1];
		second.open();
		second.emit({ type: 'Error', message: 'Voice needs an agent to bind to.' });

		const state = get(voiceSession);
		expect(state.status).toBe('error');
		expect(state.errorMessage).toBe(
			'The call dropped and could not be rejoined. Start it again.'
		);

		// A refusal is final — nothing redials after it.
		await vi.advanceTimersByTimeAsync(4_000);
		expect(FakeSocket.instances).toHaveLength(2);
	});

	it('gives up at the end of the resume window, saying what happened', async () => {
		const { voiceSession } = await liveCall();
		// The error state clears itself after a few seconds, so the sentence is
		// read off the store's history rather than off the settled state.
		const seen: Array<{ status: string; errorMessage: string | null }> = [];
		const unsub = voiceSession.subscribe((s) =>
			seen.push({ status: s.status, errorMessage: s.errorMessage })
		);

		FakeSocket.instances[0].drop(1006);
		// Nothing ever answers: every redial times out and arms the next one.
		await vi.advanceTimersByTimeAsync(29 * 60_000);
		expect(get(voiceSession).status).toBe('reconnecting');
		expect(FakeSocket.instances.length).toBeGreaterThan(10);

		await vi.advanceTimersByTimeAsync(2 * 60_000);
		unsub();
		expect(seen.find((s) => s.status === 'error')?.errorMessage).toBe(
			'The call dropped and could not be rejoined. Start it again.'
		);
		expect(released).toHaveBeenCalled();

		const dials = FakeSocket.instances.length;
		await vi.advanceTimersByTimeAsync(60_000);
		expect(FakeSocket.instances).toHaveLength(dials);
	});
});

/**
 * Barge-in tells the server how much of the reply the owner heard, so the
 * employee's memory of the reply is cut back to exactly that.
 */
describe('voiceSession barge-in', () => {
	it('sends the played position of the reply when the owner speaks over it', async () => {
		const { voiceSession, first } = await liveCall();
		first.emit({ type: 'playback_start' });
		first.audio(1000);
		audioClock = 0.4;
		first.emit({ type: 'transcription_start' });

		expect(first.frames().at(-1)).toEqual({ type: 'interrupt', playedMs: 400 });
		expect(sources.at(-1)?.stopped).toBe(true);
		expect(get(voiceSession).status).toBe('listening');
	});

	it('still barges in on the tail the client is playing after generation ended', async () => {
		const { first } = await liveCall();
		first.emit({ type: 'playback_start' });
		first.audio(2000);
		first.emit({ type: 'playback_end' });
		audioClock = 1.25;
		first.emit({ type: 'transcription_start' });

		expect(first.frames().at(-1)).toEqual({ type: 'interrupt', playedMs: 1250 });
	});

	it('counts only this reply, never an earlier one or the silence between', async () => {
		const { first } = await liveCall();
		first.emit({ type: 'playback_start' });
		first.audio(1000);
		audioClock = 1.0;
		sources.at(-1)?.onended?.();
		first.emit({ type: 'playback_end' });

		// Five quiet seconds, then the next reply.
		audioClock = 6.0;
		first.emit({ type: 'playback_start' });
		first.audio(500);
		audioClock = 6.2;
		first.emit({ type: 'transcription_start' });

		expect(first.frames().at(-1)).toEqual({ type: 'interrupt', playedMs: 200 });
	});

	it('sends nothing when nothing is playing', async () => {
		const { first } = await liveCall();
		const sent = first.frames().length;
		first.emit({ type: 'transcription_start' });

		expect(first.frames()).toHaveLength(sent);
	});
});

/**
 * The call opens and closes with a sound the owner can hear: one when it goes
 * live, a different one when it ends. The closing sound is never cut off by
 * the call's own cleanup.
 */
describe('voiceSession chimes', () => {
	/** The call's sounds played so far, in order, once the decoding has landed. */
	async function chimes() {
		await vi.advanceTimersByTimeAsync(0);
		return sources.map((s) => s.buffer?.sound).filter((n) => n !== undefined);
	}

	it('sounds once when the call goes live, and not again for the replies', async () => {
		const { first } = await liveCall();
		first.emit({ type: 'playback_start' });
		first.audio(500);
		expect(await chimes()).toEqual(['connect']);
	});

	it('plays the closing sound when the owner ends the call, and lets the audio go only after it', async () => {
		const { voiceSession } = await liveCall();
		await chimes();
		voiceSession.stop();

		expect(await chimes()).toEqual(['connect', 'disconnect']);
		expect(get(voiceSession).status).toBe('idle');
		// The microphone and the socket went at once; the speaker waits for
		// the goodbye to finish.
		expect(contexts[0].closed).toBe(false);
		await vi.advanceTimersByTimeAsync(500);
		expect(contexts[0].closed).toBe(false);
		sources.at(-1)?.onended?.();
		expect(contexts[0].closed).toBe(true);
	});

	it('never keeps the audio more than a second for the closing sound', async () => {
		const { voiceSession } = await liveCall();
		await chimes();
		voiceSession.stop();
		await vi.advanceTimersByTimeAsync(999);
		expect(contexts[0].closed).toBe(false);
		await vi.advanceTimersByTimeAsync(1);
		expect(contexts[0].closed).toBe(true);
	});

	it('lets the audio go at once when the sounds could not be loaded', async () => {
		vi.stubGlobal('fetch', async () => {
			throw new Error('offline');
		});
		const { voiceSession } = await liveCall();
		voiceSession.stop();
		expect(await chimes()).toEqual([]);
		expect(contexts[0].closed).toBe(true);
	});

	it('plays the closing sound when the call fails', async () => {
		const { first } = await liveCall();
		await chimes();
		first.emit({ type: 'Error', message: 'The employee could not answer.' });
		expect(await chimes()).toEqual(['connect', 'disconnect']);
	});

	it('keeps a redial inside the quiet window silent', async () => {
		const { voiceSession } = await liveCall();
		FakeSocket.instances[0].drop(1006);
		await vi.advanceTimersByTimeAsync(500);
		const second = FakeSocket.instances[1];
		second.open();
		second.emit({ type: 'session_initialized' });
		await vi.advanceTimersByTimeAsync(10_000);
		expect(get(voiceSession).status).toBe('listening');
		expect(await chimes()).toEqual(['connect']);
	});

	it('sounds once when a lost line is announced, and once more when it is back', async () => {
		const { voiceSession } = await liveCall();
		FakeSocket.instances[0].drop(1006);
		// Every redial times out until the quiet window has passed.
		await vi.advanceTimersByTimeAsync(4_000);
		expect(get(voiceSession).status).toBe('reconnecting');
		expect(await chimes()).toEqual(['connect', 'disconnect']);

		const next = FakeSocket.instances.at(-1)!;
		next.open();
		next.emit({ type: 'session_initialized' });
		expect(get(voiceSession).status).toBe('listening');
		expect(await chimes()).toEqual(['connect', 'disconnect', 'connect']);
	});

	it('does not say goodbye twice for a line already announced as lost', async () => {
		const { voiceSession } = await liveCall();
		FakeSocket.instances[0].drop(1006);
		await vi.advanceTimersByTimeAsync(4_000);
		expect(get(voiceSession).status).toBe('reconnecting');
		voiceSession.stop();
		expect(await chimes()).toEqual(['connect', 'disconnect']);
	});
});
