import { describe, expect, it, vi } from 'vitest';

vi.mock('$lib/api/nebo', () => ({
	listPacks: async () => ({
		default: 'pack/j/medium',
		packs: [
			{ id: 'nebo-ai', name: 'Nebo AI', builtIn: true, fallback: false, levels: {} },
			{ id: 'j', name: 'Janus', builtIn: false, fallback: true, levels: {} },
			{ id: 'w', name: 'Work keys', builtIn: false, fallback: true, levels: {} }
		]
	})
}));

const { choiceLabel, isInternalName, loadIntelligence, modelName, packName, resolve, shownLevel, valueFor } = await import('./speeds');
type PackRow = import('./speeds').PackRow;

const packs: PackRow[] = [
	{ id: 'nebo-ai', name: 'Nebo AI', builtIn: true },
	{ id: 'j', name: 'Janus', builtIn: false },
	{ id: 'w', name: 'Work keys', builtIn: false }
];
const t = (k: string) => `<${k}>`;

describe('resolve', () => {
	it('reads the legacy speeds as levels of Nebo AI, written either way', () => {
		expect(resolve('janus/nebo-1-pro', packs)).toEqual({ pack: 'nebo-ai', level: 'high' });
		expect(resolve('nebo-1-pro', packs)).toEqual({ pack: 'nebo-ai', level: 'high' });
		expect(resolve('janus/nebo-1-flash', packs)).toEqual({ pack: 'nebo-ai', level: 'instant' });
		expect(resolve('nebo-1-medium', packs)).toEqual({ pack: 'nebo-ai', level: 'medium' });
		expect(resolve('janus/nebo-1', packs)).toEqual({ pack: 'nebo-ai', level: null });
		expect(resolve('', packs)).toEqual({ pack: 'nebo-ai', level: null });
	});

	it('reads pack references, and a deleted pack as Nebo AI at the same level', () => {
		expect(resolve('pack/w', packs)).toEqual({ pack: 'w', level: null });
		expect(resolve('pack/w/auto', packs)).toEqual({ pack: 'w', level: null });
		expect(resolve('pack/j/high', packs)).toEqual({ pack: 'j', level: 'high' });
		expect(resolve('pack/gone/low', packs)).toEqual({ pack: 'nebo-ai', level: 'low' });
	});

	it('anything else is a custom model', () => {
		for (const v of ['anthropic/claude-x', 'linked/1f2e/codex-3', 'claude-code/opus', 'pack/j/deep', 'openai@p1/gpt']) {
			expect(resolve(v, packs)).toEqual({ custom: true });
		}
	});
});

describe('valueFor', () => {
	it('writes the existing model strings: a pack, or a pack at a level', () => {
		expect(valueFor('nebo-ai', 'high', 'pack/j/medium', packs)).toBe('pack/nebo-ai/high');
		expect(valueFor('w', null, 'pack/j/medium', packs)).toBe('pack/w');
		expect(valueFor('nebo-ai', null, 'pack/j/medium', packs)).toBe('pack/nebo-ai');
	});

	it('clears the override when the pick is what it would inherit', () => {
		expect(valueFor('nebo-ai', null, '', packs)).toBe('');
		expect(valueFor('j', 'medium', 'pack/j/medium', packs)).toBe('');
		expect(valueFor('nebo-ai', 'high', 'janus/nebo-1-pro', packs)).toBe('');
	});
});

describe('names', () => {
	it('never shows an internal pack name', () => {
		expect(isInternalName(' Janus ')).toBe(true);
		expect(isInternalName('janus-test')).toBe(true);
		expect(isInternalName('Work keys')).toBe(false);
		expect(packName(packs[1], t)).toBe('<modelPick.customPack>');
		expect(packName(packs[0], t)).toBe('<modelPick.neboAi>');
		expect(packName(packs[2], t)).toBe('Work keys');
	});

	it('the chip: no level reads as Medium; a pack name only for the owner\'s own packs', () => {
		expect(shownLevel(resolve('', packs))).toBe('medium');
		expect(choiceLabel(resolve('', packs), packs, t)).toBe('<settingsPacks.level.medium>');
		expect(choiceLabel(resolve('nebo-1-pro', packs), packs, t)).toBe('<settingsPacks.level.high>');
		expect(choiceLabel(resolve('pack/w/max', packs), packs, t)).toBe('Work keys · <settingsPacks.level.max>');
		expect(choiceLabel(resolve('pack/j/low', packs), packs, t)).toBe('<modelPick.customPack> · <settingsPacks.level.low>');
		expect(choiceLabel(resolve('anthropic/claude-x', packs), packs, t)).toBe('<modelPick.custom>');
	});

	it('any stored model string reads without its provider or gateway', () => {
		expect(modelName('janus/nebo-1-pro', packs, t)).toBe('<settingsPacks.level.high>');
		expect(modelName('linked/1f2e/codex-3', packs, t)).toBe('codex-3');
		for (const v of ['janus/nebo-1', 'janus/nebo-1-flash', 'pack/j/high', 'anthropic@x/claude-y']) {
			expect(modelName(v, packs, t).toLowerCase()).not.toContain('janus');
		}
	});
});

describe('loadIntelligence', () => {
	it('reads the packs and the bot default', async () => {
		const i = await loadIntelligence();
		expect(i?.botDefault).toBe('pack/j/medium');
		expect(i?.packs.map((p) => p.id)).toEqual(['nebo-ai', 'j', 'w']);
	});
});
