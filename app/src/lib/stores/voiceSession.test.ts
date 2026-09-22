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
	onmessage: ((ev: { data: string }) => void) | null = null;

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
}

const released = vi.fn();

async function startCall(chatId?: string) {
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
	vi.stubGlobal('AudioContext', class {
		destination = {};
		createBuffer() {
			return { copyToChannel: () => {} };
		}
		createBufferSource() {
			return { buffer: null, connect: () => {}, start: () => {}, stop: () => {}, onended: null };
		}
		close() {}
	});
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
