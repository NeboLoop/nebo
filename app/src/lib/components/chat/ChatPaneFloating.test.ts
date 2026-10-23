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
const pane = readFileSync(new URL('./ChatPane.svelte', import.meta.url), 'utf8');

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
	{ id: 'u1', type: 'user' as const, content: 'Fix the total', time: '' },
	{ id: 'a1', type: 'assistant' as const, content: 'Fixed it.', time: '' }
];

const rule = (name: string) => css.slice(css.indexOf(`${name} {`), css.indexOf('}', css.indexOf(`${name} {`)));

describe('ChatPane', () => {
	it('floats the composer over the conversation, its running line the last row of the scroller', () => {
		const out = render(ChatPane, { props: { messages, agentId: 'dev', sessionId: 'agent:dev:web', isLoading: true, onsend: () => {} } }).body;
		const scroller = out.indexOf('chat-scroller');
		const status = out.indexOf('class="bg-status');
		const floating = out.indexOf('class="chat-floating');
		const composer = out.indexOf('class="composer-dock px-6 py-3 shrink-0"', floating);
		expect(scroller).toBeGreaterThan(-1);
		expect(status, 'the running line is in the conversation').toBeGreaterThan(out.indexOf('Fixed it.'));
		expect(status, 'before the floating area').toBeLessThan(floating);
		expect(composer, 'the composer floats').toBeGreaterThan(floating);
		expect(out, 'no inline style: the height reaches app.css from the script').not.toMatch(/class="chat-stage"[^>]*style=/);
		expect(pane, 'through the one documented vars action').toContain(`use:vars={{ '--chat-float': floatHeight }}`);
	});

	it('stands the composer card and its menus over the scroll button, and the running tasks over the card', () => {
		const z = (name: string) => Number(/z-(\d+)/.exec(rule(name))?.[1]);
		expect(z('.composer-card')).toBeGreaterThan(z('.chat-scroll-button'));
		expect(z('.bg-status-popover')).toBeGreaterThan(z('.composer-card'));
		expect(rule('.chat-scroller'), 'what is scrolled into view lands above the composer').toContain('scroll-padding-bottom: calc(var(--chat-fade) + var(--chat-float');
		expect(pane, 'a file dropped on the chat shows its cover over the card').toContain('absolute inset-0 z-40 bg-primary/5');
	});

	it('sizes the running tasks to the room above the composer, its line in view', () => {
		expect(rule('.chat-scroll-frame')).toContain('container-type: size');
		expect(rule('.chat-scroll-frame'), 'sized by the column, never its content').toContain('flex-1');
		expect(rule('.bg-status-popover')).toContain('max-height: min(60vh, calc(100cqh - var(--chat-float, 0px) - var(--chat-fade) - 18px - 1.75rem))');
	});

	it('lets the wheel and clicks through the gutters beside the composer', () => {
		expect(rule('.chat-floating')).toContain('pointer-events-none');
		expect(rule('.composer-dock')).toContain('pointer-events-none');
		expect(css).toContain('.chat-floating > * > *,\n.composer-dock > * {\n\t@apply pointer-events-auto;');
	});

	it('leaves the floating height and its fade at the end of the scroller; the fade never takes a click', () => {
		expect(rule('.chat-scroller')).toContain('var(--chat-float');
		expect(rule('.chat-scroller'), 'the fade is room too: the last row is never under it').toContain('var(--chat-fade)');
		expect(rule('.chat-stage'), 'one fade height, in px, read by ChatPane too').toContain('--chat-fade: 16px');
		expect(pane).toContain("getPropertyValue('--chat-fade')");
		expect(rule('.chat-scroll-button')).toContain('var(--chat-float');
		expect(rule('.chat-floating::before')).toContain('pointer-events-none');
		expect(rule('.chat-floating::before'), 'a small, subtle fade').toContain('height: var(--chat-fade)');
	});

	it('glasses the input card alone, nearly opaque, and never the band around it', () => {
		const floating = rule('.chat-floating').replace(/\/\*[\s\S]*?\*\//g, '');
		expect(floating).not.toMatch(/bg-|background|backdrop-filter|z-/);
		const start = css.indexOf('@supports ((backdrop-filter');
		const glass = css.slice(start, css.indexOf('}\n}', start));
		expect(glass).toContain('.composer-card');
		expect(glass).toContain('94%, transparent');
		expect(glass).toContain('backdrop-filter: blur(8px)');
		expect(glass, "the glass is a layer, so the card never contains its menus' fixed covers").toContain('.composer-card::before');
		expect(glass.slice(0, glass.indexOf('.composer-card::before'))).not.toContain('blur(8px)');
	});

	it("opens the composer's full-window sheets over the header's menus and the Work pane", () => {
		const voice = readFileSync(new URL('./VoiceModeOverlay.svelte', import.meta.url), 'utf8');
		expect(voice).toContain('fixed inset-0 z-[70]');
	});
});
