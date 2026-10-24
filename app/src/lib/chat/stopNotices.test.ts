/**
 * A stop Nebo words itself (it went in circles, reached its spending limit,
 * waited too long with nothing happening) is in the owner's language, like
 * every card he sees: the bot's `chat_error` carries the line's key and
 * values beside its English, which is the fallback.
 */
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { addMessages, init, locale } from 'svelte-i18n';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import en from '$lib/i18n/locales/en.json';
import de from '$lib/i18n/locales/de.json';

const handlers = new Map<string, Array<(data: any) => void>>();

vi.mock('$lib/websocket/client', () => ({
	getWebSocketClient: () => ({
		send: () => {},
		on: (type: string, fn: (data: any) => void) => {
			handlers.set(type, [...(handlers.get(type) ?? []), fn]);
			return () => {};
		},
		onStatus: () => () => {},
		getDisruptionCount: () => 0
	})
}));
vi.mock('$lib/api/gocliRequest', () => ({ sendClientEvent: () => {} }));
vi.mock('$lib/marketplace/installCodes', () => ({ sendInstallCode: () => false }));

import { createChatController } from './controller.svelte';

const KEY = 'agent:assistant:thread:c1';
const dir = join(__dirname, '../i18n/locales');

function server(type: string, data: Record<string, unknown>) {
	for (const fn of handlers.get(type) ?? []) fn(data);
}

beforeAll(() => {
	addMessages('en', en);
	addMessages('de', de);
	init({ fallbackLocale: 'en', initialLocale: 'de' });
});

describe('stop notices in the owner language', () => {
	beforeEach(() => handlers.clear());

	it("a keyed stop is worded by its key and values; the bot's English is the fallback", () => {
		locale.set('de');
		const chat = createChatController({ agentId: 'assistant', sessionKey: KEY });
		chat.send('Hello');
		server('chat_error', {
			session_id: KEY,
			agentId: 'assistant',
			error: 'Stopped: this run reached its spending limit ($0.50 of $0.50).',
			stop_reason: 'spend_cap_reached',
			notice_key: 'chat.stop.spendLimit',
			notice_values: { spent: '$0.50', cap: '$0.50' }
		});
		const stop = chat.messages.find((m) => m.type === 'runError') as { content: string } | undefined;
		expect(stop?.content).toBe('Gestoppt: Dieser Lauf hat sein Ausgabenlimit erreicht ($0.50 von $0.50).');
		chat.destroy();

		// A key this app doesn't hold (a newer bot's) shows the English.
		const other = createChatController({ agentId: 'assistant', sessionKey: KEY });
		other.send('Hello');
		server('chat_error', { session_id: KEY, agentId: 'assistant', error: 'Stopped: something new.', stop_reason: 'x', notice_key: 'chat.stop.somethingNew' });
		expect((other.messages.find((m) => m.type === 'runError') as { content: string }).content).toBe('Stopped: something new.');
		other.destroy();
		locale.set('en');
	});

	it('every desktop locale words every stop, with the same placeholders', () => {
		const names = (m: string) => [...m.matchAll(/\{(\w+)\}/g)].map((x) => x[1]).sort();
		const keys = Object.keys(en.chat.stop);
		expect(keys).toEqual(['stepLimit', 'spendLimit', 'appHalted', 'appHaltedBecause', 'stuck', 'loop', 'contextStalled', 'stalled']);
		for (const f of readdirSync(dir).filter((f) => f.endsWith('.json'))) {
			const stop = JSON.parse(readFileSync(join(dir, f), 'utf8')).chat?.stop ?? {};
			for (const k of keys) {
				expect(typeof stop[k], `${f} chat.stop.${k}`).toBe('string');
				expect(names(stop[k]), `${f} chat.stop.${k}`).toEqual(names((en.chat.stop as Record<string, string>)[k]));
			}
		}
	});
});
