// The questions and approvals waiting in the chat on screen take the
// composer's place, one at a time; install and connect cards and open
// questions stay in the conversation.
import { describe, it, expect, vi, beforeAll } from 'vitest';
import { render } from 'svelte/server';
import { addMessages, init } from 'svelte-i18n';
import en from '$lib/i18n/locales/en.json';

vi.mock('$lib/api/nebo', () => ({ authLoginAccount: vi.fn(), listPlugins: vi.fn(async () => ({ plugins: [] })), submitCode: vi.fn() }));
vi.mock('$lib/websocket/client', () => ({ getWebSocketClient: () => ({ on: () => () => {} }) }));
vi.mock('$lib/stores/permissionAsks', () => ({ answerAsk: vi.fn() }));
vi.mock('$lib/stores/approvals', () => ({ answerApproval: vi.fn() }));

import { composerAsksOf, shownComposerAsks } from './composerAsks';
import ComposerAsk from '$lib/components/chat/ComposerAsk.svelte';

beforeAll(() => {
	addMessages('en', en);
	init({ fallbackLocale: 'en', initialLocale: 'en' });
});

const pick = { type: 'ask', requestId: 'q1', prompt: 'Which vendor?', widgets: [{ type: 'options' as const, options: [{ label: 'Acme', description: 'Cheaper, slower' }, 'Globex'] }] };
const install = { type: 'ask', requestId: 'q2', prompt: 'Install Shopify?', widgets: [{ type: 'install_plugin' as const, name: 'Shopify', code: 'PLUG-1' }] };
const connect = { type: 'ask', requestId: 'q3', prompt: 'Sign in', widgets: [{ type: 'connect_account' as const, plugin: 'gmail', agentId: 'a' }] };
const open = { type: 'ask', requestId: 'q4', prompt: "What's the client's name?", widgets: [{ type: 'options' as const }] };
const permission = { id: 'p1', status: 'open', employee: 'Bookkeeper', what: 'Delete the folder', reason: 'It cannot be undone.', allowAlways: false, thisOnce: true, details: [], chatId: 'c' } as never;
const approval = { requestId: 'a1', sessionId: 's', agent: 'Chief', actionType: 'goal', actionDetail: 'Agree on this goal?' };

describe('asks in the composer', () => {
	it('only questions and approvals take the composer: never an install, a sign-in or an open question', () => {
		const asks = composerAsksOf([pick, install, connect, open, { type: 'user' }], [permission], [approval], true);
		expect(asks.map((a) => a.id)).toEqual(['q1', 'p1', 'a1']);
		// An answered question, a decided approval: gone.
		expect(composerAsksOf([{ ...pick, response: 'Acme' }], [], [{ ...approval, decision: 'once' }], true)).toEqual([]);
		// A question waits only while its run goes; an approval until answered.
		expect(composerAsksOf([pick], [], [approval], false).map((a) => a.id)).toEqual(['a1']);
	});

	it('closed with the X they stay waiting; a new one comes back up', () => {
		const asks = composerAsksOf([pick], [permission], [], true);
		expect(shownComposerAsks(asks, new Set(['q1', 'p1']))).toEqual([]);
		expect(shownComposerAsks(asks, new Set(['q1']))).toHaveLength(2);
	});

	it('shows the first in the composer place, its label and description, "1 of 3" and an X', () => {
		const asks = composerAsksOf([pick], [permission], [approval], true);
		const html = render(ComposerAsk, { props: { asks, ondismiss: () => {} } }).body;
		expect(html).toContain('data-testid="composer-ask"');
		expect(html).toContain('1 of 3');
		expect(html).toMatch(/aria-label="Close"/);
		expect(html).toMatch(/<span class="ask-option-label">Acme[\s\S]*?<span class="ask-option-description">Cheaper, slower<\/span>/);
		expect(html).not.toContain('Delete the folder');
		// One alone: no count.
		const one = render(ComposerAsk, { props: { asks: asks.slice(1, 2), ondismiss: () => {} } }).body;
		expect(one).not.toContain(' of ');
		expect(one).toContain('Delete the folder');
	});

	it('the chat puts the waiting ask in the composer place and a chip once it is closed', async () => {
		const { readFileSync } = await import('node:fs');
		const pane = readFileSync('src/lib/components/chat/ChatPane.svelte', 'utf8');
		expect(pane).toMatch(/\{#if shownAsks\.length > 0\}\s*<ComposerAsk[\s\S]*?\{:else if composerAsks\.length > 0\}[\s\S]*?ask-waiting-chip[\s\S]*?<div class:hidden=\{shownAsks\.length > 0\}>\s*<ChatComposer/);
		expect(en.chat.askWaiting).toContain('question waiting');
	});
});
