import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
	XK,
	createDesktopClipboard,
	fitsComputerClipboard,
	isPasteShortcut,
	macCopyShortcut,
	pasteIntoComputer,
	type ClipNotice
} from './desktopClipboard';
import desktopView from '$lib/components/chat/DesktopView.svelte?raw';

type Sent = ['cut', string] | ['key', number, string | null, boolean | undefined] | ['focus'];

function fakeRfb() {
	const sent: Sent[] = [];
	return {
		sent,
		rfb: {
			clipboardPasteFrom: (text: string) => sent.push(['cut', text]),
			sendKey: (keysym: number, code: string | null, down?: boolean) => sent.push(['key', keysym, code, down]),
			focus: () => sent.push(['focus'])
		}
	};
}

function key(over: Partial<KeyboardEvent>) {
	const e = {
		key: '',
		code: '',
		ctrlKey: false,
		metaKey: false,
		altKey: false,
		shiftKey: false,
		preventDefault: vi.fn(),
		stopPropagation: vi.fn(),
		...over
	};
	return e as unknown as KeyboardEvent & { preventDefault: ReturnType<typeof vi.fn>; stopPropagation: ReturnType<typeof vi.fn> };
}

function pasteEvent(text: string | null) {
	return {
		clipboardData: text === null ? null : { getData: (t: string) => (t === 'text/plain' ? text : '') },
		preventDefault: vi.fn(),
		stopPropagation: vi.fn()
	} as unknown as ClipboardEvent & { preventDefault: ReturnType<typeof vi.fn> };
}

const CTRL_V = [
	['key', XK.Control_L, 'ControlLeft', true],
	['key', XK.v, 'KeyV', true],
	['key', XK.v, 'KeyV', false],
	['key', XK.Control_L, 'ControlLeft', false]
];

const CMD_UP = [
	['key', XK.Alt_L, 'MetaLeft', false],
	['key', XK.Super_L, 'MetaRight', false]
];

describe('which keys are a paste or a copy', () => {
	it('Ctrl+V and Cmd+V paste, also with Shift, and Shift+Insert', () => {
		expect(isPasteShortcut(key({ key: 'v', code: 'KeyV', ctrlKey: true }))).toBe(true);
		expect(isPasteShortcut(key({ key: 'v', code: 'KeyV', metaKey: true }))).toBe(true);
		expect(isPasteShortcut(key({ key: 'V', code: 'KeyV', ctrlKey: true, shiftKey: true }))).toBe(true);
		expect(isPasteShortcut(key({ key: 'Insert', code: 'Insert', shiftKey: true }))).toBe(true);
	});

	it('a plain v, Alt combinations and other shortcuts are left to the computer', () => {
		expect(isPasteShortcut(key({ key: 'v', code: 'KeyV' }))).toBe(false);
		expect(isPasteShortcut(key({ key: 'v', code: 'KeyV', ctrlKey: true, altKey: true }))).toBe(false);
		expect(isPasteShortcut(key({ key: 'c', code: 'KeyC', ctrlKey: true }))).toBe(false);
		expect(isPasteShortcut(key({ key: 'Insert', code: 'Insert', ctrlKey: true }))).toBe(false);
	});

	it('follows the layout, and the V position on a non-Latin one', () => {
		// Dvorak: the key in V's place types "k"; V is elsewhere.
		expect(isPasteShortcut(key({ key: 'k', code: 'KeyV', ctrlKey: true }))).toBe(false);
		expect(isPasteShortcut(key({ key: 'v', code: 'Period', ctrlKey: true }))).toBe(true);
		// Russian: V's place types "м".
		expect(isPasteShortcut(key({ key: 'м', code: 'KeyV', ctrlKey: true }))).toBe(true);
	});

	it('on a Mac, Cmd+C and Cmd+X become the computer copy; elsewhere Ctrl+C passes through', () => {
		expect(macCopyShortcut(key({ key: 'c', code: 'KeyC', metaKey: true }), true)).toBe('c');
		expect(macCopyShortcut(key({ key: 'x', code: 'KeyX', metaKey: true }), true)).toBe('x');
		expect(macCopyShortcut(key({ key: 'c', code: 'KeyC', ctrlKey: true }), true)).toBe(null);
		expect(macCopyShortcut(key({ key: 'c', code: 'KeyC', metaKey: true }), false)).toBe(null);
	});
});

describe('pasting into the computer', () => {
	it('sets the computer clipboard, then presses Ctrl+V there', () => {
		const { rfb, sent } = fakeRfb();
		expect(pasteIntoComputer(rfb, 'S3cr3t!pa$$')).toBe('sent');
		expect(sent).toEqual([['cut', 'S3cr3t!pa$$'], ...CTRL_V]);
	});

	it('releases a held Cmd first, and leaves a held Ctrl for the owner to release', () => {
		const mac = fakeRfb();
		pasteIntoComputer(mac.rfb, 'pw', { ctrl: false, meta: true });
		expect(mac.sent).toEqual([['cut', 'pw'], ...CMD_UP, ...CTRL_V]);

		const win = fakeRfb();
		pasteIntoComputer(win.rfb, 'pw', { ctrl: true, meta: false });
		expect(win.sent).toEqual([['cut', 'pw'], ...CTRL_V.slice(0, 3)]);
	});

	it('sends nothing for an empty clipboard and flags text the computer cannot take', () => {
		const { rfb, sent } = fakeRfb();
		expect(pasteIntoComputer(rfb, '')).toBe('empty');
		expect(sent).toEqual([]);
		expect(fitsComputerClipboard('pässwörd!')).toBe(true);
		expect(fitsComputerClipboard('密码')).toBe(false);
		expect(pasteIntoComputer(rfb, 'pw密')).toBe('lossy');
	});
});

describe('the view clipboard', () => {
	let notices: ClipNotice[];
	let offers: (string | null)[];
	let sink: { value: string; focus: ReturnType<typeof vi.fn> };

	beforeEach(() => {
		vi.useFakeTimers();
		notices = [];
		offers = [];
		sink = { value: 'stale', focus: vi.fn() };
	});
	afterEach(() => vi.useRealTimers());

	function make(clipboard: Partial<Clipboard> | null, connected = true, mac = false) {
		const f = fakeRfb();
		const c = createDesktopClipboard({
			rfb: () => (connected ? f.rfb : null),
			sink: () => sink as unknown as HTMLTextAreaElement,
			notice: (k) => notices.push(k),
			offerCopy: (t) => offers.push(t),
			mac,
			clipboard: clipboard as Clipboard | null
		});
		return { c, sent: f.sent };
	}

	it('Ctrl+V: keeps the key from noVNC, lets the browser paste into the sink, and hands that text over without reading the clipboard', () => {
		const readText = vi.fn();
		const { c, sent } = make({ readText });
		const e = key({ key: 'v', code: 'KeyV', ctrlKey: true });
		c.onkeydown(e);
		expect(e.stopPropagation).toHaveBeenCalled();
		expect(e.preventDefault).not.toHaveBeenCalled();
		expect(sink.focus).toHaveBeenCalled();
		expect(sink.value).toBe('');

		const p = pasteEvent('hunter2');
		c.onpaste(p);
		expect(p.preventDefault).toHaveBeenCalled();
		expect(sent).toEqual([['cut', 'hunter2'], ...CTRL_V.slice(0, 3), ['focus']]);
		vi.runAllTimers();
		expect(readText).not.toHaveBeenCalled();
		expect(sink.value).toBe('');
	});

	it('falls back to reading the clipboard when the browser sends no paste event', async () => {
		const { c, sent } = make({ readText: vi.fn().mockResolvedValue('pw') });
		c.onkeydown(key({ key: 'v', code: 'KeyV', metaKey: true }));
		await vi.runAllTimersAsync();
		expect(sent[0]).toEqual(['cut', 'pw']);
		expect(sent.slice(1, 3)).toEqual(CMD_UP);
	});

	it('says so when the clipboard cannot be read, and sends nothing', async () => {
		const { c, sent } = make({ readText: vi.fn().mockRejectedValue(new Error('denied')) });
		await c.pasteButton();
		expect(notices).toEqual(['pasteBlocked']);
		expect(sent).toEqual([['focus']]);
	});

	it('the Paste button reads within its click and pastes', async () => {
		const { c, sent } = make({ readText: vi.fn().mockResolvedValue('user@example.com') });
		await c.pasteButton();
		expect(sent).toEqual([['cut', 'user@example.com'], ...CTRL_V, ['focus']]);
		expect(notices).toEqual([]);
	});

	it('an empty or non-Latin paste says so', () => {
		const { c } = make(null);
		c.onpaste(pasteEvent(''));
		c.onpaste(pasteEvent('пароль'));
		expect(notices).toEqual(['clipboardEmpty', 'pasteLossy']);
	});

	it('does nothing while the computer is not connected', async () => {
		const readText = vi.fn();
		const { c, sent } = make({ readText }, false);
		const e = key({ key: 'v', code: 'KeyV', ctrlKey: true });
		c.onkeydown(e);
		const p = pasteEvent('pw');
		c.onpaste(p);
		await c.pasteButton();
		expect(e.stopPropagation).not.toHaveBeenCalled();
		expect(p.preventDefault).not.toHaveBeenCalled();
		expect(readText).not.toHaveBeenCalled();
		expect(sent).toEqual([]);
	});

	it('other keys go to the computer untouched', () => {
		const { c, sent } = make(null);
		const e = key({ key: 'c', code: 'KeyC', ctrlKey: true });
		c.onkeydown(e);
		expect(e.stopPropagation).not.toHaveBeenCalled();
		expect(sink.focus).not.toHaveBeenCalled();
		expect(sent).toEqual([]);
	});

	it('Cmd+C on a Mac copies on the computer with Ctrl+C', () => {
		const { c, sent } = make(null, true, true);
		const e = key({ key: 'c', code: 'KeyC', metaKey: true });
		c.onkeydown(e);
		expect(e.preventDefault).toHaveBeenCalled();
		expect(e.stopPropagation).toHaveBeenCalled();
		expect(sent).toEqual([
			...CMD_UP,
			['key', XK.Control_L, 'ControlLeft', true],
			['key', XK.c, 'KeyC', true],
			['key', XK.c, 'KeyC', false],
			['key', XK.Control_L, 'ControlLeft', false]
		]);
	});

	it("the owner's copy on the computer goes straight to their clipboard", async () => {
		const writeText = vi.fn().mockResolvedValue(undefined);
		const { c } = make({ writeText });
		c.onkeydown(key({ key: 'c', code: 'KeyC', ctrlKey: true }));
		vi.advanceTimersByTime(1200); // x11vnc polls about once a second
		await c.fromComputer('copied in the computer');
		expect(writeText).toHaveBeenCalledWith('copied in the computer');
		expect(notices).toEqual(['copied']);
		expect(offers).toEqual([null]);
	});

	it('when the browser wants a click, it offers a Copy button that writes on click', async () => {
		const writeText = vi.fn().mockRejectedValueOnce(new Error('NotAllowedError')).mockResolvedValue(undefined);
		const { c } = make({ writeText });
		c.onpointerdown();
		await c.fromComputer('secret');
		expect(offers).toEqual(['secret']);
		expect(notices).toEqual([]);
		await c.copyOffered('secret');
		expect(writeText).toHaveBeenLastCalledWith('secret');
		expect(offers).toEqual(['secret', null]);
		expect(notices).toEqual(['copied']);
	});

	it('a copy the owner did not make (the employee working) waits on the button', async () => {
		const writeText = vi.fn().mockResolvedValue(undefined);
		const { c } = make({ writeText });
		await c.fromComputer('from the employee');
		c.onpointerdown();
		vi.advanceTimersByTime(6000);
		await c.fromComputer('later');
		expect(writeText).not.toHaveBeenCalled();
		expect(offers).toEqual(['from the employee', 'later']);
	});

	it('a paste coming back from the computer is not a copy', async () => {
		const writeText = vi.fn().mockResolvedValue(undefined);
		const { c } = make({ writeText });
		c.onpaste(pasteEvent('pw密'));
		await c.fromComputer('pw?');
		expect(writeText).not.toHaveBeenCalled();
		expect(offers).toEqual([]);
		// Only once: the same text copied again later is a real copy.
		c.onpointerdown();
		await c.fromComputer('pw?');
		expect(writeText).toHaveBeenCalledWith('pw?');
	});

	it('ignores an empty copy from the computer', async () => {
		const writeText = vi.fn();
		const { c } = make({ writeText });
		await c.fromComputer('');
		expect(writeText).not.toHaveBeenCalled();
	});
});

describe('clipboard text never leaves the view', () => {
	it('the view and its clipboard never log, store or persist it', async () => {
		const src = (await import('./desktopClipboard.ts?raw')).default as string;
		for (const code of [src, desktopView]) {
			expect(code).not.toMatch(/console\.(log|info|debug|warn|error)/);
			expect(code).not.toMatch(/localStorage|sessionStorage|indexedDB/);
		}
		expect(desktopView).not.toMatch(/from '\$lib\/stores\/(?!ownScreen)/);
	});

	it('the view wires the clipboard to noVNC and its own paste', () => {
		expect(desktopView).toContain("addEventListener('clipboard'");
		expect(desktopView).toContain('onkeydowncapture');
		expect(desktopView).toContain('onpaste');
		expect(desktopView).toContain('desktop-clipboard-sink');
	});
});
