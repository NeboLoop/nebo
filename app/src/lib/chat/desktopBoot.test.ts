import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
	afterDisconnect,
	BOOT_DEADLINE_MS,
	BOOT_REDIAL_MS,
	desktopSession,
	lastFrameOf,
	onFirstFrame,
	RECONNECT_DEADLINE_MS,
	redialDelay,
	REDIAL_FIRST_MS,
	REDIAL_MAX_MS,
	RETRIES_AFTER_READY,
	RETRIES_BEFORE_READY,
	type DesktopStatus
} from './desktopBoot';
import desktopView from '$lib/components/chat/DesktopView.svelte?raw';
import en from '$lib/i18n/locales/en.json';

function fakeRfb() {
	const flips: number[] = [];
	const rfb = Object.assign(new EventTarget(), {
		_display: {
			flip(n: number) {
				flips.push(n);
				return n;
			}
		}
	});
	return { rfb, flips };
}

describe('the computer view is ready only at its first frame', () => {
	it('opens the gate on the first drawn frame, once', () => {
		const { rfb, flips } = fakeRfb();
		const ready = vi.fn();
		onFirstFrame(rfb, ready);
		rfb.dispatchEvent(new Event('connect'));
		expect(ready).not.toHaveBeenCalled();
		expect(rfb._display.flip(1)).toBe(1);
		rfb._display.flip(2);
		expect(ready).toHaveBeenCalledTimes(1);
		expect(flips).toEqual([1, 2]);
	});

	it('falls back to the connect event when noVNC hides its display', () => {
		const rfb = new EventTarget();
		const ready = vi.fn();
		onFirstFrame(rfb, ready);
		rfb.dispatchEvent(new Event('connect'));
		expect(ready).toHaveBeenCalledTimes(1);
	});
});

describe('a dropped connection during boot', () => {
	it('redials quietly and still says the computer is starting', () => {
		for (let a = 1; a < RETRIES_BEFORE_READY; a++) {
			expect(afterDisconnect(false, a, false)).toEqual({ status: 'starting', redial: true });
		}
	});

	it('then says it could not start, with Try again', () => {
		expect(afterDisconnect(false, RETRIES_BEFORE_READY, false)).toEqual({ status: 'failed', redial: false });
		expect(afterDisconnect(false, RETRIES_BEFORE_READY, null)).toEqual({ status: 'failed', redial: false });
	});

	it('on a host bot with no virtual computer, says there is none', () => {
		expect(afterDisconnect(false, RETRIES_BEFORE_READY, true)).toEqual({ status: 'unavailable', redial: false });
	});

});

describe('a dropped connection after the desktop was up', () => {
	it('redials silently: the view stays ready', () => {
		for (let a = 1; a < RETRIES_AFTER_READY; a++) {
			expect(afterDisconnect(true, a, false)).toEqual({ status: 'ready', redial: true });
		}
	});

	it('out of redials, says it could not start, with Try again', () => {
		expect(afterDisconnect(true, RETRIES_AFTER_READY, false)).toEqual({ status: 'failed', redial: false });
		expect(afterDisconnect(true, RETRIES_AFTER_READY, true)).toEqual({ status: 'failed', redial: false });
	});

	it('redials quickly first, then backs off to a cap', () => {
		expect(redialDelay(true, 1)).toBe(REDIAL_FIRST_MS);
		expect(redialDelay(true, 2)).toBe(REDIAL_FIRST_MS * 2);
		expect(redialDelay(true, 20)).toBe(REDIAL_MAX_MS);
		expect(redialDelay(false, 1)).toBe(BOOT_REDIAL_MS);
	});
});

/** A stand-in for noVNC: an EventTarget with a display that flips and a
 *  screen element that holds its last frame. */
class FakeRfb extends EventTarget {
	_display = { flip: () => undefined };
	_screen = { nodeType: 1, id: 0 };
	disconnected = false;
	disconnect() {
		this.disconnected = true;
	}
	/** noVNC drew a complete frame. */
	frame() {
		this._display.flip();
	}
	/** The transport dropped. */
	drop() {
		this.dispatchEvent(new Event('disconnect'));
	}
}

function harness(ownScreen: boolean | null = false) {
	const conns: FakeRfb[] = [];
	const statuses: DesktopStatus[] = [];
	const kept: FakeRfb[] = [];
	let frameKept: FakeRfb | null = null;
	const session = desktopSession<FakeRfb>({
		dial: () => {
			const c = new FakeRfb();
			c._screen.id = conns.length;
			conns.push(c);
			return c;
		},
		ownScreen: () => ownScreen,
		onStatus: (s) => statuses.push(s),
		keepFrame: (c) => {
			kept.push(c);
			frameKept = c;
		},
		dropFrame: () => {
			frameKept = null;
		}
	});
	return {
		session,
		conns,
		statuses,
		kept,
		last: () => conns[conns.length - 1],
		frameKept: () => frameKept,
		status: () => statuses[statuses.length - 1]
	};
}

describe('the computer view session', () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it('a short drop after the desktop was up retries quietly, keeping the last frame', () => {
		const h = harness();
		h.session.boot();
		expect(h.status()).toBe('starting');
		h.last().frame();
		expect(h.status()).toBe('ready');
		const statusesBefore = h.statuses.length;

		const first = h.last();
		first.drop();
		// Nothing changes on screen: no new status, the dropped frame is kept.
		expect(h.statuses.length).toBe(statusesBefore);
		expect(h.frameKept()).toBe(first);
		expect(h.conns.length).toBe(1);

		vi.advanceTimersByTime(REDIAL_FIRST_MS);
		expect(h.conns.length).toBe(2);
		expect(h.session.current()).toBe(h.last());
		// Still the old frame until the new connection draws.
		expect(h.frameKept()).toBe(first);
		h.last().frame();
		expect(h.frameKept()).toBeNull();
		expect(h.status()).toBe('ready');
		expect(h.statuses.slice(1)).not.toContain('starting');
		expect(h.statuses.slice(1).every((s) => s === 'ready')).toBe(true);
		expect(h.statuses).not.toContain('failed');
	});

	it('never reaches a reconnecting state', () => {
		const h = harness();
		h.session.boot();
		h.last().frame();
		for (let i = 0; i < 3; i++) {
			h.last().drop();
			vi.advanceTimersByTime(REDIAL_MAX_MS);
		}
		h.last().frame();
		expect(new Set(h.statuses)).toEqual(new Set(['starting', 'ready']));
	});

	it('after the redials run out, shows the failure state and stops dialing', () => {
		const h = harness(true);
		h.session.boot();
		h.last().frame();
		let waited = 0;
		for (let redial = 1; redial <= RETRIES_AFTER_READY; redial++) {
			h.last().drop();
			expect(h.status()).toBe('ready');
			vi.advanceTimersByTime(redialDelay(true, redial));
			waited += redialDelay(true, redial);
		}
		// Every redial is used before the deadline would have stepped in.
		expect(waited).toBeLessThan(RECONNECT_DEADLINE_MS);
		expect(h.conns.length).toBe(1 + RETRIES_AFTER_READY);
		h.last().drop();
		// Out of redials: the existing failure state (not "unavailable", the
		// computer was there), and no more dialing.
		expect(h.status()).toBe('failed');
		vi.advanceTimersByTime(RECONNECT_DEADLINE_MS * 2);
		expect(h.conns.length).toBe(1 + RETRIES_AFTER_READY);
		expect(h.statuses.filter((s) => s === 'failed').length).toBe(1);
	});

	it('a redial that never draws gives up at the reconnect deadline', () => {
		const h = harness();
		h.session.boot();
		h.last().frame();
		h.last().drop();
		vi.advanceTimersByTime(REDIAL_FIRST_MS);
		const hanging = h.last();
		expect(h.status()).toBe('ready');
		vi.advanceTimersByTime(RECONNECT_DEADLINE_MS);
		expect(h.status()).toBe('failed');
		expect(hanging.disconnected).toBe(true);
	});

	it('Try again boots afresh from the loading state and lets the kept frame go', () => {
		const h = harness();
		h.session.boot();
		h.last().frame();
		h.last().drop();
		vi.advanceTimersByTime(RECONNECT_DEADLINE_MS);
		expect(h.status()).toBe('failed');
		expect(h.frameKept()).not.toBeNull();

		h.session.boot();
		expect(h.status()).toBe('starting');
		expect(h.frameKept()).toBeNull();
		h.last().frame();
		expect(h.status()).toBe('ready');
	});

	it('a boot that never draws fails at the boot deadline', () => {
		const h = harness();
		h.session.boot();
		vi.advanceTimersByTime(BOOT_DEADLINE_MS);
		expect(h.status()).toBe('failed');
		expect(h.last().disconnected).toBe(true);
	});

	it('a drop during boot redials and still says starting', () => {
		const h = harness();
		h.session.boot();
		h.last().drop();
		expect(h.status()).toBe('starting');
		expect(h.frameKept()).toBeNull();
		vi.advanceTimersByTime(BOOT_REDIAL_MS);
		expect(h.conns.length).toBe(2);
	});

	it('closing stops any pending redial and hangs up', () => {
		const h = harness();
		h.session.boot();
		h.last().frame();
		h.last().drop();
		h.session.close();
		vi.advanceTimersByTime(RECONNECT_DEADLINE_MS * 2);
		expect(h.conns.length).toBe(1);
		expect(h.status()).toBe('ready');
	});

	it('the dropped connection lends its painted screen as the last frame', () => {
		const r = new FakeRfb();
		expect(lastFrameOf(r)).toBe(r._screen);
		expect(lastFrameOf(new EventTarget())).toBeNull();
	});
});

describe('what the view shows while it starts', () => {
	it('one opaque loading state, then the failure copy with Try again', () => {
		const starting = desktopView.slice(desktopView.indexOf("{:else if status === 'starting'}"));
		expect(starting).toMatch(/absolute inset-0 bg-neutral/);
		expect(starting).toContain('loading-spinner');
		expect(starting).toContain("$t('chat.startingComputer')");
		expect(desktopView).toContain("$t('chat.computerStartFailed')");
		expect(desktopView).toContain("$t('chat.tryAgain')");
	});

	it('the session gates the view; nothing listens for the handshake', () => {
		expect(desktopView).toContain('desktopSession<RFB>(');
		expect(desktopView).not.toContain("addEventListener('connect'");
	});

	it('a dropped connection shows no reconnecting copy, only the kept frame', () => {
		expect(desktopView).not.toMatch(/[Rr]econnect/);
		expect(desktopView).toContain('data-desktop-last-frame');
		expect(en.chat).not.toHaveProperty('computerReconnecting');
	});
});
