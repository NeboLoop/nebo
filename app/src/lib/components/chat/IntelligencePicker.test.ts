// The Intelligence popover: one checkmark per card in every state, the five
// levels and the packs by their names, and never an internal gateway name on
// screen — even for a pack the owner named after it.
import { describe, it, expect, beforeAll } from 'vitest';
import { render } from 'svelte/server';
import { addMessages, init } from 'svelte-i18n';
import en from '$lib/i18n/locales/en.json';
import IntelligencePicker from './IntelligencePicker.svelte';
import { resolve, type PackRow } from '$lib/models/speeds';

beforeAll(() => {
	addMessages('en', en);
	init({ fallbackLocale: 'en', initialLocale: 'en' });
});

const packs: PackRow[] = [
	{ id: 'nebo-ai', name: 'Nebo AI', builtIn: true },
	{ id: 'j', name: 'Janus', builtIn: false, levels: { instant: 'janus/nebo-1-flash', high: 'janus/nebo-1-pro' } },
	{ id: 'w', name: 'Work keys', builtIn: false, levels: { medium: 'openai@p1/gpt-6.1' }, levelEffort: { medium: 'high' } }
];
const choices = [{ value: 'openai@p1/gpt-6.1', provider: 'OpenAI', connection: 'Company key', modelId: 'gpt-6.1', local: false }];

const html = (value: string, view: 'level' | 'packs') =>
	render(IntelligencePicker, {
		props: { packs, choices, choice: resolve(value, packs), view, onpick: () => {}, onclose: () => {} }
	}).body;

const checks = (out: string) => out.split('data-testid="intel-check"').length - 1;
/** The text a reader sees: tags and attribute values gone. */
const text = (out: string) => out.replace(/<[^>]*>/g, ' ').replace(/\s+/g, ' ');

// Every state a conversation can be in: '' (Auto), each level, a deleted
// pack, a raw model, the owner's packs.
const STATES = [
	'',
	'pack/nebo-ai',
	'pack/nebo-ai/auto',
	...['instant', 'low', 'medium', 'high', 'max'].map((l) => `pack/nebo-ai/${l}`),
	'pack/gone/high',
	'anthropic/claude-x',
	'linked/a0b2/codex-3',
	'pack/j/medium',
	'pack/j',
	'pack/w/max'
];

describe('IntelligencePicker', () => {
	for (const value of STATES) {
		it(`shows exactly one checkmark per card for "${value || 'Auto'}"`, () => {
			expect(checks(html(value, 'level'))).toBe(1);
			expect(checks(html(value, 'packs'))).toBe(1);
		});
	}

	it('nothing set (or a pack with no level) is Auto', () => {
		for (const v of ['', 'pack/nebo-ai', 'pack/nebo-ai/auto', 'pack/w']) {
			expect(html(v, 'level')).toMatch(/data-level="auto"[^>]*aria-pressed="true"/);
			expect(html(v, 'level')).not.toMatch(/data-level="medium"[^>]*aria-pressed="true"/);
		}
		expect(html('pack/nebo-ai/high', 'level')).toMatch(/data-level="high"[^>]*aria-pressed="true"/);
		expect(html('pack/j/medium', 'packs')).toMatch(/data-pack="j"[^>]*aria-pressed="true"/);
	});

	it('lists Auto first (Recommended), then the five levels with the cost on Max, and the footnote', () => {
		const out = text(html('', 'level'));
		for (const l of ['Auto', 'Instant', 'Low', 'Medium', 'High', 'Max']) expect(out).toContain(l);
		expect(out.indexOf('Auto')).toBeLessThan(out.indexOf('Instant'));
		expect(out).toMatch(/Auto Recommended Nebo picks the level for each task/);
		expect(out).not.toMatch(/Medium Recommended/);
		expect(out).not.toMatch(/credit/i);
		expect(out).toContain(en.modelPick.recommended);
		expect(out).toContain(en.modelPick.mostBalance);
		expect(out).toContain(en.modelPick.levelHint);
		expect(out).toContain(en.modelPick.pack);
		for (const legacy of ['Fast', 'Balanced', 'Deep', 'Default']) expect(out).not.toMatch(new RegExp(`\\b${legacy}\\b`));
	});

	it('lists packs by name, the built-in as Nebo AI, with the link to build one', () => {
		const out = text(html('pack/w/high', 'packs'));
		expect(out).toContain('Nebo AI');
		expect(out).toContain('Work keys');
		expect(out).toContain('Janus');
		expect(out).toContain('OpenAI');
		expect(out).toContain(en.modelPick.createPack);
		expect(html('', 'packs')).toContain('href="/settings/intelligence-packs"');
	});

	it('the built-in pack never shows "Janus" or a model id, in any state', () => {
		for (const value of STATES) {
			if (/^pack\/(j|w)\b/.test(value)) continue;
			const out = text(html(value, 'level'));
			expect(out).not.toMatch(/janus/i);
			expect(out).not.toMatch(/nebo-1|claude-x|codex-3|a0b2/);
		}
	});

	it('an owner\'s pack shows what he set: its name and each level\'s provider and model', () => {
		const out = text(html('pack/j/high', 'level'));
		expect(out).toContain('Janus');
		expect(out).toContain('Nebo AI · nebo-1-pro');
		expect(out).toContain('Nebo AI · nebo-1-flash');
		expect(out).toContain(`${en.modelPick.levelDesc.medium} · ${en.modelPick.fromNeboAi}`);
		expect(out).toContain(en.modelPick.autoCustom);
		expect(text(html('pack/w', 'level'))).toContain('OpenAI · gpt-6.1 · high effort');
		expect(text(html('pack/w', 'level'))).not.toContain('p1');
	});
});
