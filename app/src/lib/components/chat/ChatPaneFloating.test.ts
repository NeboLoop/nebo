// The conversation fills the column and the composer floats over its end:
// the running line is the conversation's last row, inside the scroller after
// the newest message, and the scroller leaves the floating area's height at
// its end (app.css, `--chat-float`).
import { describe, it, expect, beforeAll, vi } from 'vitest';
import { render } from 'svelte/server';
import { addMessages, init } from 'svelte-i18n';
import en from '$lib/i18n/locales/en.json';
import { readFileSync } from 'node:fs';

const css = readFileSync(new URL('../../../app.css', import.meta.url), 'utf8');

// Node's own localStorage stub has no methods; per-install state is memory here.
vi.mock('$lib/storage', () => {
	const m = new Map<string, string>();
	return {
		storage: {
			get: (k: string) => m.get(k) ?? null,
			set: (k: string, v: string) => void m.set(k, v),
			remove: (k: string) => void m.delete(k)
		}
	};
});
// The remote desktop viewer touches `window` on import.
vi.mock('@novnc/novnc', () => ({ default: class {} }));

import ChatPane from './ChatPane.svelte';

beforeAll(() => {
	addMessages('en', en);
	init({ fallbackLocale: 'en', initialLocale: 'en' });
});

const messages = [
	{ id: 'u1', type: 'user', content: 'Fix the total', time: '' },
	{ id: 'a1', type: 'assistant', content: 'Fixed it.', time: '' }
];

describe('ChatPane', () => {
	it('floats the composer over the conversation, its running line the last row of the scroller', () => {
		const out = render(ChatPane, { props: { messages, agentId: 'dev', sessionId: 'agent:dev:web', isLoading: true, onsend: () => {} } }).body;
		const scroller = out.indexOf('chat-scroller');
		const status = out.indexOf('class="bg-status');
		const floating = out.indexOf('class="chat-floating');
		const composer = out.indexOf('class="px-6 py-3 shrink-0"', floating);
		expect(scroller).toBeGreaterThan(-1);
		expect(status, 'the running line is in the conversation').toBeGreaterThan(out.indexOf('Fixed it.'));
		expect(status, 'before the floating area').toBeLessThan(floating);
		expect(composer, 'the composer floats').toBeGreaterThan(floating);
		expect(out).toContain('--chat-float: 0px');
	});

	it('leaves the floating height at the end of the scroller, with a fade that never takes a click', () => {
		const rule = (name: string) => css.slice(css.indexOf(`${name} {`), css.indexOf('}', css.indexOf(`${name} {`)));
		expect(rule('.chat-scroller')).toContain('var(--chat-float');
		expect(rule('.chat-scroll-button')).toContain('var(--chat-float');
		expect(rule('.chat-floating::before')).toContain('pointer-events-none');
		expect(rule('.chat-floating::before')).toContain('to-base-100');
	});
});
