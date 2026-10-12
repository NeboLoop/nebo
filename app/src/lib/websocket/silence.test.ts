/**
 * A socket that went silent is dead. Through the tunnel the far side can go
 * and leave this side open, hearing nothing: the conversation froze until a
 * refresh. The bot keeps a heartbeat (a `ping` every 20 s); a socket that has
 * heard it and then hears nothing for SILENCE_MS is let go and dialled again,
 * which reloads the open conversation. A bot that never pings is never taken
 * for dead on silence alone.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

vi.mock('$lib/api/base', () => ({ backendWsBase: () => 'ws://bot.test' }));
vi.mock('$lib/storage', () => ({ storage: { get: () => null } }));
vi.mock('$lib/api/gocliRequest', () => ({ sendClientEvent: () => {} }));

class FakeSocket {
	static OPEN = 1;
	static CONNECTING = 0;
	static all: FakeSocket[] = [];
	readyState = 0;
	sent: string[] = [];
	closed = false;
	onopen: (() => void) | null = null;
	onclose: ((e: { code: number; reason: string; wasClean: boolean }) => void) | null = null;
	onerror: ((e: unknown) => void) | null = null;
	onmessage: ((e: { data: string }) => void) | null = null;
	constructor(public url: string) {
		FakeSocket.all.push(this);
	}
	send(s: string) {
		this.sent.push(s);
	}
	close() {
		this.closed = true;
	}
	open() {
		this.readyState = 1;
		this.onopen?.();
	}
	frame(msg: Record<string, unknown>) {
		this.onmessage?.({ data: JSON.stringify(msg) });
	}
}

describe('the silence watchdog', () => {
	beforeEach(() => {
		vi.useFakeTimers();
		FakeSocket.all = [];
		vi.stubGlobal('WebSocket', FakeSocket);
		vi.resetModules();
	});
	afterEach(() => {
		vi.unstubAllGlobals();
		vi.useRealTimers();
	});

	async function connected() {
		const { getWebSocketClient, SILENCE_MS } = await import('./client');
		const ws = getWebSocketClient();
		const statuses: string[] = [];
		ws.onStatus((s) => statuses.push(s));
		ws.connect();
		const sock = FakeSocket.all[0];
		sock.open();
		sock.frame({ type: 'auth_ok' });
		expect(ws.getStatus()).toBe('connected');
		return { ws, sock, statuses, SILENCE_MS };
	}

	it('drops a socket that heard the heartbeat and then went quiet, and dials again', async () => {
		const { ws, sock, SILENCE_MS } = await connected();
		sock.frame({ type: 'ping' });
		expect(sock.sent.some((s) => JSON.parse(s).type === 'pong')).toBe(true);

		// Beats keep it alive.
		vi.advanceTimersByTime(SILENCE_MS - 1000);
		sock.frame({ type: 'ping' });
		vi.advanceTimersByTime(SILENCE_MS - 1000);
		expect(ws.getStatus()).toBe('connected');
		expect(sock.closed).toBe(false);

		// Then nothing: the line is dead.
		vi.advanceTimersByTime(2000);
		expect(sock.closed).toBe(true);
		expect(ws.getStatus()).toBe('disconnected');
		expect(ws.getDisruptionCount()).toBe(1);

		// It dials again on its own.
		vi.advanceTimersByTime(2000);
		expect(FakeSocket.all.length).toBe(2);
	});

	it('never drops a quiet socket to a bot that keeps no heartbeat', async () => {
		const { ws, sock, SILENCE_MS } = await connected();
		vi.advanceTimersByTime(SILENCE_MS * 5);
		expect(sock.closed).toBe(false);
		expect(ws.getStatus()).toBe('connected');
		expect(FakeSocket.all.length).toBe(1);
	});

	it('a dial now (Try again) replaces the waiting redial, never adds a second', async () => {
		vi.stubGlobal('document', { visibilityState: 'visible', addEventListener: () => {} });
		const { ws, sock, SILENCE_MS } = await connected();
		// The line goes dead; the redial waits its 2 s.
		sock.frame({ type: 'ping' });
		vi.advanceTimersByTime(SILENCE_MS + 1000);
		expect(ws.getStatus()).toBe('disconnected');
		expect(FakeSocket.all.length).toBe(1);

		// Try again before the redial is due: one new socket, now.
		ws.connect();
		expect(FakeSocket.all.length).toBe(2);

		// Every dial fails at once. One chain of redials follows, backing off
		// 2 s, 4 s, 8 s, 16 s — never a second chain beside it.
		const dialTimes: number[] = [];
		for (let ms = 0; ms <= 40_000; ms += 100) {
			for (const s of FakeSocket.all.slice(1)) {
				if (s.closed) continue;
				s.closed = true;
				dialTimes.push(ms);
				s.onclose?.({ code: 1006, reason: '', wasClean: false });
			}
			vi.advanceTimersByTime(100);
		}
		expect(dialTimes).toEqual([0, 2000, 6000, 14000, 30000]);
	});

	it('stops watching when the owner closes the socket', async () => {
		const { ws, sock, SILENCE_MS } = await connected();
		sock.frame({ type: 'ping' });
		ws.disconnect();
		vi.advanceTimersByTime(SILENCE_MS * 3);
		expect(FakeSocket.all.length).toBe(1);
		expect(ws.getDisruptionCount()).toBe(0);
	});
});
