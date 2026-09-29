/**
 * Whose surface an event is. The owner hired from his phone and came back to
 * a desktop full of install dialogs stuck midway (2026-09-27): every client
 * heard the phone's install and opened it. Now the page names itself on the
 * wire and opens a surface only for the work it started.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('$lib/api/base', () => ({ backendWsBase: () => 'ws://bot.test', backendBase: () => 'http://bot.test' }));
vi.mock('$lib/storage', () => ({ storage: { get: () => null, set: () => {}, remove: () => {} } }));
vi.mock('$lib/monitoring', () => ({
	logger: { child: () => ({ info: () => {}, warn: () => {}, error: () => {}, debug: () => {} }) }
}));
vi.mock('$lib/monitoring/logger', () => ({
	logger: { child: () => ({ info: () => {}, warn: () => {}, error: () => {}, debug: () => {} }) }
}));
vi.mock('$lib/stores/notifications', () => ({
	notifications: { subscribe: () => () => {} },
	pushNotification: () => {},
	loadNotifications: () => {},
	settleUpdateNotices: () => {}
}));
vi.mock('$lib/stores/permissionAsks', () => ({
	askRaised: () => {},
	askSettled: () => {},
	loadOpenAsks: async () => {}
}));
vi.mock('$lib/stores/toast', () => ({ addToast: () => 0, removeToast: () => {} }));
vi.mock('$lib/stores/update', () => ({
	onUpdateAvailable: () => {},
	onUpdateProgress: () => {},
	onUpdateReady: () => {},
	onUpdateError: () => {}
}));

/** The bot's end of the socket, driven by hand. */
class FakeSocket {
	static CONNECTING = 0;
	static OPEN = 1;
	static CLOSING = 2;
	static CLOSED = 3;
	static instances: FakeSocket[] = [];
	readyState = 0;
	sent: string[] = [];
	onopen: (() => void) | null = null;
	onerror: (() => void) | null = null;
	onclose: ((ev: { code: number; reason: string; wasClean: boolean }) => void) | null = null;
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
	open() {
		this.readyState = FakeSocket.OPEN;
		this.onopen?.();
	}
	emit(type: string, data: Record<string, unknown>) {
		this.onmessage?.({ data: JSON.stringify({ type, data }) });
	}
	frames(): Array<{ type: string; data?: Record<string, unknown> }> {
		return this.sent.map((s) => JSON.parse(s));
	}
}

const PHONE = 'phone-page-0f1c';

beforeEach(() => {
	vi.resetModules();
	FakeSocket.instances = [];
	vi.stubGlobal('WebSocket', FakeSocket);
	vi.stubGlobal('document', { visibilityState: 'visible', addEventListener: () => {} });
	vi.stubGlobal('fetch', vi.fn(async () => new Response('{}', { status: 200 })));
});

/** A connected socket for this page, as the bot sees it. */
async function connected() {
	const { getWebSocketClient } = await import('./client');
	const ws = getWebSocketClient();
	ws.connect('token');
	const socket = FakeSocket.instances[0];
	socket.open();
	socket.emit('auth_ok', {});
	return { ws, socket };
}

describe('opensHere: the one decision', () => {
	it('an install the phone started never opens here; one this page started does', async () => {
		const { clientId, opensHere } = await import('./origin');
		// code_processing as the server stamps it (codes::handle_code).
		const fromPhone = { code: 'AGNT-AAAA-0001', code_type: 'agent', client_id: PHONE, session_id: 'agent:main:web' };
		const fromHere = { ...fromPhone, client_id: clientId };
		expect(opensHere(fromPhone, 'nowhere')).toBe(false);
		expect(opensHere(fromHere, 'nowhere')).toBe(true);
	});

	it('an install no client asked for (a hire on the account, an employee’s call) opens nowhere', async () => {
		const { opensHere } = await import('./origin');
		expect(opensHere({ client_id: null, session_id: 'install-event-a1' }, 'nowhere')).toBe(false);
		expect(opensHere({ session_id: 'install-event-a1' }, 'nowhere')).toBe(false);
	});

	it('an approval: the phone’s run asks on the phone; a run nobody started asks wherever the owner is', async () => {
		const { clientId, opensHere } = await import('./origin');
		expect(opensHere({ client_id: PHONE }, 'everywhere')).toBe(false);
		expect(opensHere({ client_id: clientId }, 'everywhere')).toBe(true);
		expect(opensHere({ client_id: null }, 'everywhere')).toBe(true);
	});
});

describe('the page names itself on the wire', () => {
	it('in the socket handshake, the same name it matches events against', async () => {
		const { socket } = await connected();
		const { clientId } = await import('./origin');
		const handshake = socket.frames()[0];
		expect(handshake.type).toBe('auth');
		expect(handshake.data?.client_id).toBe(clientId);
	});

	it('on every API request', async () => {
		const { request } = await import('$lib/api/gocliRequest');
		const { clientId, CLIENT_HEADER } = await import('./origin');
		await request({ method: 'post', url: '/api/v1/store/products/a1/install', data: {} });
		const init = (fetch as unknown as ReturnType<typeof vi.fn>).mock.calls[0][1] as RequestInit;
		expect((init.headers as Record<string, string>)[CLIENT_HEADER]).toBe(clientId);
	});
});

describe('a sign-in window opens on the client that asked', () => {
	it('the phone’s sign-in does not open on this desktop; this desktop’s own does', async () => {
		const opened: string[] = [];
		vi.stubGlobal('window', { open: (url: string) => (opened.push(url), {}), location: { pathname: '/' } });
		const { socket } = await connected();
		const { attachWebSocketListeners } = await import('./listeners');
		const { clientId } = await import('./origin');
		attachWebSocketListeners();

		socket.emit('plugin_auth_url', { plugin: 'xero', url: 'https://login.example.com/phone', client_id: PHONE, session_id: '' });
		socket.emit('plugin_auth_url', { plugin: 'xero', url: 'https://login.example.com/mine', client_id: clientId, session_id: '' });
		await new Promise((r) => setTimeout(r, 0));
		expect(opened).toEqual(['https://login.example.com/mine']);
	});
});
