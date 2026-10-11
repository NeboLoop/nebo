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

const { CLOSE_AFTER_MS, selectThenClose, choiceLabel, isInternalName, loadIntelligence, modelName, packName, resolve, shownLevel, valueFor } = await import('./speeds');
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

describe('selectThenClose', () => {
	const run = (current: string, pack: string, level: import('./speeds').Effort | null) => {
		vi.useFakeTimers();
		const onpick = vi.fn();
		const onclose = vi.fn();
		selectThenClose(resolve(current, packs), pack, level, onpick, onclose);
		const pickedAt = { picked: onpick.mock.calls.length, closedAtOnce: onclose.mock.calls.length };
		vi.advanceTimersByTime(CLOSE_AFTER_MS);
		vi.useRealTimers();
		return { onpick, onclose, ...pickedAt };
	};

	it('a level: picked at once, its checkmark shows, then the picker closes', () => {
		const r = run('pack/nebo-ai/low', 'nebo-ai', 'high');
		expect(r.onpick).toHaveBeenCalledWith('nebo-ai', 'high');
		expect(r.picked).toBe(1);
		expect(r.closedAtOnce).toBe(0);
		expect(r.onclose).toHaveBeenCalledTimes(1);
	});

	it('a pack: picked keeping the level, then the whole picker closes', () => {
		const r = run('pack/nebo-ai/high', 'w', 'high');
		expect(r.onpick).toHaveBeenCalledWith('w', 'high');
		expect(r.onclose).toHaveBeenCalledTimes(1);
	});

	it('the choice already in use closes without a write', () => {
		for (const [current, pack, level] of [
			['', 'nebo-ai', 'medium'],
			['pack/j/medium', 'j', 'medium'],
			['pack/w', 'w', null],
			['nebo-1-pro', 'nebo-ai', 'high']
		] as const) {
			const r = run(current, pack, level);
			expect(r.onpick).not.toHaveBeenCalled();
			expect(r.closedAtOnce).toBe(1);
		}
	});
});
