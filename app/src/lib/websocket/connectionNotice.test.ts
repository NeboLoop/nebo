/**
 * What the owner is told about the app's connection to his bot: nothing for
 * a drop the redials fix, and once the connection has stayed down past the
 * quiet window, the plain failure with Try again — never a "Reconnecting".
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import en from '$lib/i18n/locales/en.json';
import type { ConnectionStatus } from './client';
import { watchConnection, QUIET_MS } from './connectionNotice';

/** A socket client driven by hand. */
function fakeSocket() {
	let status: ConnectionStatus = 'connected';
	const handlers = new Set<(s: ConnectionStatus) => void>();
	const sock = {
		dials: 0,
		onStatus(h: (s: ConnectionStatus) => void) {
			handlers.add(h);
			h(status);
			return () => handlers.delete(h);
		},
		getStatus: () => status,
		connect() {
			sock.dials++;
			sock.set('connecting');
		},
		set(s: ConnectionStatus) {
			status = s;
			handlers.forEach((h) => h(s));
		},
		/** The line dropped (onerror, then onclose), as the client reports it. */
		drop() {
			sock.set('error');
			sock.set('disconnected');
		}
	};
	return sock;
}

/** The notice as the owner sees it: every show and hide, in order. */
function fakeView() {
	const view = {
		events: [] as string[],
		up: false,
		tryAgain: null as null | (() => void),
		show(tryAgain: () => void) {
			view.events.push('show');
			view.up = true;
			view.tryAgain = tryAgain;
		},
		hide() {
			view.events.push('hide');
			view.up = false;
		},
		/** The owner presses Try again; the toast takes itself down. */
		press() {
			view.up = false;
			view.tryAgain?.();
		}
	};
	return view;
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe('the connection notice', () => {
	it('says nothing at all about a drop the redials fix inside the quiet window', async () => {
		const sock = fakeSocket();
		const view = fakeView();
		watchConnection(sock, view);

		sock.drop();
		await vi.advanceTimersByTimeAsync(2_000);
		sock.set('connecting');
		sock.drop();
		await vi.advanceTimersByTimeAsync(4_000);
		sock.set('connecting');
		sock.set('connected');

		// Long after the window: still nothing ever shown, nothing taken down.
		await vi.advanceTimersByTimeAsync(QUIET_MS * 2);
		expect(view.events).toEqual([]);
	});

	it('shows the failure once, after the quiet window, and takes it down when back', async () => {
		const sock = fakeSocket();
		const view = fakeView();
		watchConnection(sock, view);

		sock.drop();
		await vi.advanceTimersByTimeAsync(QUIET_MS - 1);
		expect(view.events).toEqual([]);
		await vi.advanceTimersByTimeAsync(1);
		expect(view.events).toEqual(['show']);

		// The redials keep failing behind it: still one notice, never a second.
		for (let i = 0; i < 5; i++) {
			sock.set('connecting');
			sock.drop();
			await vi.advanceTimersByTimeAsync(QUIET_MS);
		}
		expect(view.events).toEqual(['show']);

		sock.set('connected');
		expect(view.events).toEqual(['show', 'hide']);
	});

	it('Try again dials at once, and a failed dial is reported straight away', async () => {
		const sock = fakeSocket();
		const view = fakeView();
		watchConnection(sock, view);
		sock.drop();
		await vi.advanceTimersByTimeAsync(QUIET_MS);
		expect(view.up).toBe(true);

		view.press();
		expect(sock.dials).toBe(1);
		expect(view.up).toBe(false);

		// The owner asked: the answer does not wait out another quiet window.
		sock.drop();
		expect(view.events).toEqual(['show', 'show']);
		expect(view.up).toBe(true);
	});

	it('Try again that connects leaves nothing on screen', async () => {
		const sock = fakeSocket();
		const view = fakeView();
		watchConnection(sock, view);
		sock.drop();
		await vi.advanceTimersByTimeAsync(QUIET_MS);
		view.press();
		sock.set('connected');
		expect(view.up).toBe(false);

		// A later short drop is silent again.
		sock.drop();
		await vi.advanceTimersByTimeAsync(1_000);
		sock.set('connected');
		await vi.advanceTimersByTimeAsync(QUIET_MS);
		expect(view.events).toEqual(['show']);
	});

	it('stopping takes the notice down and arms nothing more', async () => {
		const sock = fakeSocket();
		const view = fakeView();
		const stop = watchConnection(sock, view);
		sock.drop();
		await vi.advanceTimersByTimeAsync(QUIET_MS);
		stop();
		expect(view.events).toEqual(['show', 'hide']);

		sock.set('connected');
		sock.drop();
		await vi.advanceTimersByTimeAsync(QUIET_MS * 2);
		expect(view.events).toEqual(['show', 'hide']);
	});

	it('the failure says what to do, and nothing about reconnecting', () => {
		const line = en.common.couldNotConnect.replace('{name}', 'Danny');
		expect(line).toBe('Could not connect to Danny. Try again.');
		const all = JSON.stringify(en.common) + JSON.stringify(en.voice) + JSON.stringify(en.background);
		expect(all).not.toMatch(/reconnect|rejoining|still trying/i);
	});
});
