// The computer view's ready gate. Opening the bot's computer shows ONE
// loading state ("Starting the computer…") until the desktop is all the way
// up: the bot answers /ws/desktop only once its desktop is running (x11vnc
// accepting, the browser on screen), and the view stays covered until noVNC
// has drawn its first frame. Never a black or half-drawn screen, never a
// connection error during a normal boot.

/** How long a boot may take before the view says it could not start. The
 *  bot's own bounded waits come to well under this. */
export const BOOT_DEADLINE_MS = 60_000;

/** Quiet redials allowed before the first frame (a transport blip during
 *  boot), and after it (the desktop survives transport loss). */
export const RETRIES_BEFORE_READY = 3;
export const RETRIES_AFTER_READY = 10;

export type DesktopStatus = 'starting' | 'ready' | 'reconnecting' | 'failed' | 'unavailable';

/** What a dropped connection means for the view.
 *  - Before the first frame: redial quietly, still "starting".
 *  - After it: redial, "reconnecting".
 *  - Out of redials: a bot with a screen of its own may simply have no
 *    virtual computer (/ws/desktop 404s off Linux) — "unavailable"; anywhere
 *    else the computer could not start — "failed", with Try again. */
export function afterDisconnect(
	everReady: boolean,
	attempts: number,
	ownScreen: boolean | null
): { status: DesktopStatus; redial: boolean } {
	const limit = everReady ? RETRIES_AFTER_READY : RETRIES_BEFORE_READY;
	if (attempts >= limit) {
		return { status: ownScreen === true ? 'unavailable' : 'failed', redial: false };
	}
	return { status: everReady ? 'reconnecting' : 'starting', redial: true };
}

/** Calls `cb` once, when noVNC has put its first complete framebuffer update
 *  on screen. noVNC 1.7 has no public event for it; every finished update
 *  ends in its display's `flip()`, so the first flip is the first frame.
 *  Should that internal ever be missing, the gate falls back to `connect`
 *  rather than staying shut. */
export function onFirstFrame(rfb: EventTarget, cb: () => void): void {
	let fired = false;
	const fire = () => {
		if (fired) return;
		fired = true;
		cb();
	};
	const display = (rfb as unknown as { _display?: { flip?: (...a: unknown[]) => unknown } })._display;
	if (!display || typeof display.flip !== 'function') {
		rfb.addEventListener('connect', fire);
		return;
	}
	const flip = display.flip;
	display.flip = function (this: unknown, ...args: unknown[]) {
		const out = flip.apply(this, args);
		fire();
		return out;
	};
}
