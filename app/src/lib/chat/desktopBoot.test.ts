import { describe, expect, it, vi } from 'vitest';
import { afterDisconnect, onFirstFrame, RETRIES_AFTER_READY, RETRIES_BEFORE_READY } from './desktopBoot';
import desktopView from '$lib/components/chat/DesktopView.svelte?raw';

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

	it('after the desktop was up, redials as before', () => {
		expect(afterDisconnect(true, 1, false)).toEqual({ status: 'reconnecting', redial: true });
		expect(afterDisconnect(true, RETRIES_AFTER_READY, false).redial).toBe(false);
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
		expect(desktopView).toContain('BOOT_DEADLINE_MS');
	});

	it('the first frame, not the handshake, opens the view', () => {
		expect(desktopView).toContain('onFirstFrame(r,');
		expect(desktopView).not.toContain("addEventListener('connect'");
	});
});
