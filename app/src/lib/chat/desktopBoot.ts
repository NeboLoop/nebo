// The computer view's ready gate. Opening the bot's computer shows ONE
// loading state ("Starting the computer…") until the desktop is all the way
// up: the bot answers /ws/desktop only once its desktop is running (x11vnc
// accepting, the browser on screen), and the view stays covered until noVNC
// has drawn its first frame. Never a black or half-drawn screen, never a
// connection error during a normal boot.
//
// Once the desktop is up, a dropped connection is silent: the last frame
// stays on screen while the view redials, and only when the redials run out
// does it say the computer could not start, with Try again.

/** How long a boot may take before the view says it could not start. The
 *  bot's own bounded waits come to well under this. */
export const BOOT_DEADLINE_MS = 60_000;

/** How long a desktop that was up may stay away before the view gives up. */
export const RECONNECT_DEADLINE_MS = 30_000;

/** Quiet redials allowed before the first frame (a transport blip during
 *  boot), and after it (the desktop survives transport loss). */
export const RETRIES_BEFORE_READY = 3;
export const RETRIES_AFTER_READY = 10;

/** Wait before a redial during boot. */
export const BOOT_REDIAL_MS = 3000;
/** Waits before redialing a desktop that was up: quick first, then capped. */
export const REDIAL_FIRST_MS = 500;
export const REDIAL_MAX_MS = 3000;

export type DesktopStatus = 'starting' | 'ready' | 'failed' | 'unavailable';

/** What a dropped connection means for the view.
 *  - Before the first frame: redial quietly, still "starting".
 *  - After it: redial quietly, still "ready" (the last frame stays up).
 *  - Out of redials before the first frame: a bot with a screen of its own
 *    may simply have no virtual computer (/ws/desktop 404s off Linux) —
 *    "unavailable"; anywhere else the computer could not start — "failed".
 *  - Out of redials after it: "failed", with Try again. */
export function afterDisconnect(
	everReady: boolean,
	attempts: number,
	ownScreen: boolean | null
): { status: DesktopStatus; redial: boolean } {
	if (everReady) {
		return attempts >= RETRIES_AFTER_READY ? { status: 'failed', redial: false } : { status: 'ready', redial: true };
	}
	if (attempts >= RETRIES_BEFORE_READY) {
		return { status: ownScreen === true ? 'unavailable' : 'failed', redial: false };
	}
	return { status: 'starting', redial: true };
}

/** Wait before redial number `attempt` (1-based). */
export function redialDelay(everReady: boolean, attempt: number): number {
	if (!everReady) return BOOT_REDIAL_MS;
	return Math.min(REDIAL_FIRST_MS * 2 ** Math.max(0, attempt - 1), REDIAL_MAX_MS);
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

/** The element holding a dropped connection's last drawn frame. noVNC 1.7
 *  takes its screen out of the page on disconnect but leaves the canvas
 *  painted; putting that element back keeps the frame on screen. Null when
 *  the internal is missing (the view then just shows its ground). */
export function lastFrameOf(rfb: EventTarget): Node | null {
	const screen = (rfb as unknown as { _screen?: unknown })._screen;
	return screen && typeof screen === 'object' && (screen as Node).nodeType === 1 ? (screen as Node) : null;
}

export interface DesktopConnection extends EventTarget {
	disconnect(): void;
}

export interface DesktopSessionOptions<C extends DesktopConnection> {
	/** Opens one connection (noVNC into the view), or null when it cannot. */
	dial: () => C | null;
	ownScreen: () => boolean | null;
	onStatus: (status: DesktopStatus) => void;
	/** A connection that was showing the desktop dropped: keep its frame up. */
	keepFrame?: (dropped: C) => void;
	/** A live frame is back, or the view boots afresh: let the kept one go. */
	dropFrame?: () => void;
}

export interface DesktopSession<C extends DesktopConnection> {
	/** (Re)start from the loading state: on open and on Try again. */
	boot(): void;
	/** The connection in view, if any. */
	current(): C | null;
	/** The view is gone: no more redials, timers cleared, connection closed. */
	close(): void;
}

/** One computer view's connection: the boot gate, quiet bounded redials, and
 *  the deadlines that turn a desktop that never comes (back) into "failed". */
export function desktopSession<C extends DesktopConnection>(o: DesktopSessionOptions<C>): DesktopSession<C> {
	let conn: C | null = null;
	let closed = false;
	let status: DesktopStatus = 'starting';
	let attempts = 0;
	let everReady = false;
	/** The connection in view has drawn a frame. */
	let live = false;
	let deadline: ReturnType<typeof setTimeout> | null = null;
	let redialTimer: ReturnType<typeof setTimeout> | null = null;
	/** Bumped by every (re)boot and give-up, so a stale redial is dropped. */
	let generation = 0;

	const set = (s: DesktopStatus) => {
		status = s;
		o.onStatus(s);
	};

	function clearDeadline() {
		if (deadline) {
			clearTimeout(deadline);
			deadline = null;
		}
	}

	function clearTimers() {
		clearDeadline();
		if (redialTimer) {
			clearTimeout(redialTimer);
			redialTimer = null;
		}
	}

	function hangUp() {
		const c = conn;
		conn = null;
		c?.disconnect();
	}

	function giveUp(s: DesktopStatus) {
		clearTimers();
		generation += 1;
		hangUp();
		set(s);
	}

	function startDeadline(ms: number) {
		clearDeadline();
		deadline = setTimeout(() => {
			deadline = null;
			if (closed || live) return;
			giveUp('failed');
		}, ms);
	}

	function dial() {
		if (closed) return;
		attempts += 1;
		const c = o.dial();
		if (!c) return;
		conn = c;
		onFirstFrame(c, () => {
			if (conn !== c || closed) return;
			clearDeadline();
			live = true;
			everReady = true;
			attempts = 0;
			o.dropFrame?.();
			set('ready');
		});
		c.addEventListener('disconnect', () => {
			// A connection we already let go (closed, timed out, replaced)
			// says nothing about the one in view.
			if (conn !== c || closed) return;
			conn = null;
			if (live) {
				live = false;
				o.keepFrame?.(c);
				startDeadline(RECONNECT_DEADLINE_MS);
			}
			const next = afterDisconnect(everReady, attempts, o.ownScreen());
			if (!next.redial) {
				giveUp(next.status);
				return;
			}
			if (next.status !== status) set(next.status);
			// The desktop survives transport loss server-side (x11vnc -forever,
			// tunnel redial) — keep trying quietly instead of dying on a blip.
			const gen = generation;
			redialTimer = setTimeout(() => {
				redialTimer = null;
				if (!closed && gen === generation && conn === null) dial();
			}, redialDelay(everReady, attempts + 1));
		});
	}

	return {
		boot() {
			clearTimers();
			generation += 1;
			hangUp();
			attempts = 0;
			everReady = false;
			live = false;
			o.dropFrame?.();
			set('starting');
			startDeadline(BOOT_DEADLINE_MS);
			dial();
		},
		current: () => conn,
		close() {
			closed = true;
			clearTimers();
			hangUp();
		}
	};
}
