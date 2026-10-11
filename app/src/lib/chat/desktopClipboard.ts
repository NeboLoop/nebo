// Copy and paste between the owner's computer and the bot's computer.
//
// The clipboard rides RFB (noVNC <-> x11vnc): ClientCutText carries the
// owner's text to the computer, ServerCutText brings the computer's copy
// back. The text is often a password, so it is only ever held for the
// moment it takes to hand it over: never logged, never stored, and the
// owner's clipboard is read only on their own paste (Ctrl/Cmd+V in the
// view, or the Paste button).
//
// Paste: the shortcut is kept from noVNC (which would send a bare Ctrl+V
// and paste the computer's stale clipboard), a hidden textarea takes focus
// so the browser's own paste lands there, and its `paste` event hands over
// the text with no permission prompt (Chrome, Edge, Safari, the desktop
// webview). Only if no paste event comes does it fall back to
// navigator.clipboard.readText(). The text is set as the computer's
// clipboard, then Ctrl+V is pressed there so it lands in the focused field.

/** The slice of noVNC's RFB the clipboard uses. */
export interface ClipboardRfb {
	clipboardPasteFrom(text: string): void;
	sendKey(keysym: number, code: string | null, down?: boolean): void;
	focus(options?: FocusOptions): void;
}

/** X keysyms sent to the computer. */
export const XK = {
	Control_L: 0xffe3,
	Alt_L: 0xffe9,
	Super_L: 0xffeb,
	c: 0x63,
	v: 0x76,
	x: 0x78
} as const;

export type ClipNotice = 'copied' | 'pasteBlocked' | 'clipboardEmpty' | 'pasteLossy';

type KeyLike = Pick<KeyboardEvent, 'key' | 'code' | 'ctrlKey' | 'metaKey' | 'altKey' | 'shiftKey'>;

/** The letter the key types: its layout key, or its physical position on a
 *  non-Latin layout (where Ctrl+V is still the key in V's place). */
function isLetter(e: KeyLike, letter: string): boolean {
	const key = (e.key ?? '').toLowerCase();
	if (key === letter) return true;
	return e.code === `Key${letter.toUpperCase()}` && !/^[a-z]$/.test(key);
}

/** Ctrl+V, Cmd+V (also with Shift), or Shift+Insert. */
export function isPasteShortcut(e: KeyLike): boolean {
	if (e.altKey) return false;
	if ((e.ctrlKey || e.metaKey) && isLetter(e, 'v')) return true;
	return e.shiftKey && !e.ctrlKey && !e.metaKey && e.key === 'Insert';
}

/** On a Mac, Cmd+C / Cmd+X: the computer is Linux, where copy is Ctrl+C,
 *  so these are sent as Ctrl. Elsewhere Ctrl+C already reaches it as is. */
export function macCopyShortcut(e: KeyLike, mac: boolean): 'c' | 'x' | null {
	if (!mac || !e.metaKey || e.ctrlKey || e.altKey) return null;
	if (isLetter(e, 'c')) return 'c';
	if (isLetter(e, 'x')) return 'x';
	return null;
}

/** x11vnc's clipboard is Latin-1: other characters arrive as "?". */
/** `text` as the computer's clipboard holds it. */
function asComputerHolds(text: string): string {
	return Array.from(text, (ch) => ((ch.codePointAt(0) ?? 0) > 0xff ? '?' : ch)).join('');
}

/** A copy on the computer reaches the owner's clipboard by itself only
 *  this soon after the owner's own key or click in the view. Anything else
 *  (the employee using the clipboard while the owner watches) waits on the
 *  Copy button, so it never overwrites the owner's clipboard unasked. */
export const OWN_COPY_WINDOW_MS = 5_000;
/** How long a paste may come back from the computer as its "copy". */
const ECHO_WINDOW_MS = 10_000;

export function fitsComputerClipboard(text: string): boolean {
	return /^[\u0000-\u00ff]*$/.test(text);
}

/** Which modifiers the owner is holding, which noVNC has already pressed
 *  on the computer. */
export interface Held {
	ctrl: boolean;
	meta: boolean;
}

const NONE: Held = { ctrl: false, meta: false };

/** Press Ctrl+<key> on the computer. A held Cmd is released there first
 *  (noVNC sends a Mac's left Cmd as Alt, the right one as Super); a Ctrl the
 *  owner is still holding is left down for noVNC to release with their key. */
export function pressCtrl(rfb: ClipboardRfb, keysym: number, code: string, held: Held = NONE) {
	if (held.meta) {
		rfb.sendKey(XK.Alt_L, 'MetaLeft', false);
		rfb.sendKey(XK.Super_L, 'MetaRight', false);
	}
	rfb.sendKey(XK.Control_L, 'ControlLeft', true);
	rfb.sendKey(keysym, code, true);
	rfb.sendKey(keysym, code, false);
	if (!held.ctrl) rfb.sendKey(XK.Control_L, 'ControlLeft', false);
}

/** Set the computer's clipboard to `text` and paste it into the focused
 *  field. */
export function pasteIntoComputer(rfb: ClipboardRfb, text: string, held: Held = NONE): 'sent' | 'empty' | 'lossy' {
	if (!text) return 'empty';
	rfb.clipboardPasteFrom(text);
	pressCtrl(rfb, XK.v, 'KeyV', held);
	return fitsComputerClipboard(text) ? 'sent' : 'lossy';
}

export interface DesktopClipboardOptions {
	/** The live connection, or null while there is none. */
	rfb: () => ClipboardRfb | null;
	/** The hidden textarea the browser pastes into. */
	sink: () => HTMLTextAreaElement | undefined;
	notice: (kind: ClipNotice) => void;
	/** The computer copied text the browser would not let us write without
	 *  a click: offer it on a button (null withdraws the offer). */
	offerCopy: (text: string | null) => void;
	mac: boolean;
	clipboard?: Pick<Clipboard, 'readText' | 'writeText'> | null;
	/** How long a paste shortcut waits for the browser's paste event. */
	pasteWaitMs?: number;
}

export function createDesktopClipboard(o: DesktopClipboardOptions) {
	const clip = () => (o.clipboard === undefined ? globalThis.navigator?.clipboard ?? null : o.clipboard);
	let waiting: { held: Held; timer: ReturnType<typeof setTimeout> } | null = null;
	let ownInputAt = -Infinity;
	/** The last paste, briefly: the computer hands it back once as a copy
	 *  (its clipboard changed owner), which must not count as one. */
	let echo: { text: string; until: number } | null = null;

	function deliver(text: string | null, held: Held) {
		const rfb = o.rfb();
		if (!rfb) return;
		if (text === null) {
			o.notice('pasteBlocked');
		} else {
			const r = pasteIntoComputer(rfb, text, held);
			if (r !== 'empty') echo = { text: asComputerHolds(text), until: Date.now() + ECHO_WINDOW_MS };
			if (r === 'empty') o.notice('clipboardEmpty');
			else if (r === 'lossy') o.notice('pasteLossy');
		}
		rfb.focus({ preventScroll: true });
	}

	async function readLocal(): Promise<string | null> {
		const c = clip();
		if (!c?.readText) return null;
		try {
			return await c.readText();
		} catch {
			return null;
		}
	}

	return {
		/** Capture-phase keydown on the view, ahead of noVNC's own. */
		onkeydown(e: KeyboardEvent) {
			if (!o.rfb()) return;
			ownInputAt = Date.now();
			const copy = macCopyShortcut(e, o.mac);
			if (copy) {
				e.preventDefault();
				e.stopPropagation();
				const rfb = o.rfb()!;
				pressCtrl(rfb, copy === 'c' ? XK.c : XK.x, copy === 'c' ? 'KeyC' : 'KeyX', { ctrl: false, meta: true });
				return;
			}
			if (!isPasteShortcut(e)) return;
			// Not preventDefault: the browser's paste is what we are after,
			// into the sink rather than noVNC's canvas.
			e.stopPropagation();
			const held = { ctrl: e.ctrlKey, meta: e.metaKey };
			const sink = o.sink();
			if (sink) {
				sink.value = '';
				sink.focus({ preventScroll: true });
			}
			if (waiting) clearTimeout(waiting.timer);
			waiting = {
				held,
				timer: setTimeout(async () => {
					if (!waiting) return;
					waiting = null;
					deliver(await readLocal(), held);
				}, o.pasteWaitMs ?? 400)
			};
		},

		/** A paste into the view: from the shortcut above, or the browser's
		 *  Edit > Paste while the view has focus. */
		onpaste(e: ClipboardEvent) {
			if (!o.rfb()) return;
			e.preventDefault();
			e.stopPropagation();
			const held = waiting?.held ?? NONE;
			if (waiting) {
				clearTimeout(waiting.timer);
				waiting = null;
			}
			const sink = o.sink();
			if (sink) sink.value = '';
			deliver(e.clipboardData?.getData('text/plain') ?? '', held);
		},

		/** The owner's click in the view (a right-click Copy, say). */
		onpointerdown() {
			ownInputAt = Date.now();
		},

		/** The Paste button: read within its click. */
		async pasteButton() {
			if (!o.rfb()) return;
			deliver(await readLocal(), NONE);
		},

		/** noVNC's `clipboard` event: the computer copied `text`. */
		async fromComputer(text: string) {
			if (!text) return;
			const now = Date.now();
			if (echo && now < echo.until && text === echo.text) {
				echo = null;
				return;
			}
			echo = null;
			if (now - ownInputAt > OWN_COPY_WINDOW_MS) {
				o.offerCopy(text);
				return;
			}
			const c = clip();
			try {
				if (!c?.writeText) throw new Error('no clipboard');
				await c.writeText(text);
				o.offerCopy(null);
				o.notice('copied');
			} catch {
				o.offerCopy(text);
			}
		},

		/** The "Copy to my computer" button, with the offered text. */
		async copyOffered(text: string) {
			const c = clip();
			try {
				if (!c?.writeText) throw new Error('no clipboard');
				await c.writeText(text);
				o.notice('copied');
			} catch {
				// Leave the offer up: the owner can try again.
				return;
			}
			o.offerCopy(null);
		},

		dispose() {
			if (waiting) clearTimeout(waiting.timer);
			waiting = null;
			echo = null;
		}
	};
}

/** Whether the owner's keyboard is a Mac's (Cmd for copy and paste). */
export function isMacPlatform(): boolean {
	const nav = globalThis.navigator as (Navigator & { userAgentData?: { platform?: string } }) | undefined;
	const p = nav?.userAgentData?.platform || nav?.platform || '';
	return /mac|iphone|ipad|ipod/i.test(p);
}
