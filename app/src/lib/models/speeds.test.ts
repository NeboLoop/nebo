import { describe, expect, it, vi } from 'vitest';

vi.mock('$lib/api/nebo', () => ({
	listPacks: async () => ({
		default: 'pack/j/medium',
		packs: [
			{ id: 'nebo-ai', name: 'Nebo AI', builtIn: true, fallback: false, levels: {} },
			{ id: 'j', name: 'Janus', builtIn: false, fallback: true, levels: {} },
			{ id: 'w', name: 'Work keys', builtIn: false, fallback: true, levels: {} }
		]
	}),
	listPackChoices: async () => ({ chat: [{ value: 'openai/gpt', provider: 'OpenAI', connection: '', modelId: 'gpt', local: false }], decision: [] })
}));

const { CLOSE_AFTER_MS, selectThenClose, choiceLabel, levelSubtitle, loadIntelligence, packSubtitle, runsOnLine, modelName, packName, resolve, shownLevel, valueFor } = await import('./speeds');
type PackRow = import('./speeds').PackRow;

const packs: PackRow[] = [
	{ id: 'nebo-ai', name: 'Nebo AI', builtIn: true },
	{ id: 'j', name: 'Janus', builtIn: false },
	{ id: 'w', name: 'Work keys', builtIn: false }
];
const t = (k: string) => `<${k}>`;

describe('resolve', () => {
	it('nothing chosen is Nebo AI on Auto', () => {
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
		expect(valueFor('nebo-ai', 'high', 'pack/nebo-ai/high', packs)).toBe('');
	});
});

describe('names', () => {
	it('an owner\'s pack reads exactly as he named it', () => {
		expect(packName(packs[1], t)).toBe('Janus');
		expect(packName(packs[0], t)).toBe('<modelPick.neboAi>');
		expect(packName(packs[2], t)).toBe('Work keys');
		expect(packName({ id: 'x', name: ' my pack ', builtIn: false }, t)).toBe(' my pack ');
	});

	it('the chip: the pack and the level; no level is Auto', () => {
		expect(shownLevel(resolve('', packs))).toBe('auto');
		expect(choiceLabel(resolve('', packs), packs, t)).toBe('<modelPick.neboAi> · <settingsPacks.level.auto>');
		for (const v of ['pack/nebo-ai', 'pack/nebo-ai/auto']) {
			expect(shownLevel(resolve(v, packs))).toBe('auto');
		}
		expect(choiceLabel(resolve('pack/w', packs), packs, t)).toBe('Work keys · <settingsPacks.level.auto>');
		expect(choiceLabel(resolve('pack/nebo-ai/medium', packs), packs, t)).toBe('<modelPick.neboAi> · <settingsPacks.level.medium>');
		expect(choiceLabel(resolve('pack/nebo-ai/high', packs), packs, t)).toBe('<modelPick.neboAi> · <settingsPacks.level.high>');
		expect(choiceLabel(resolve('pack/w/max', packs), packs, t)).toBe('Work keys · <settingsPacks.level.max>');
		expect(choiceLabel(resolve('pack/j/low', packs), packs, t)).toBe('Janus · <settingsPacks.level.low>');
		expect(choiceLabel(resolve('anthropic/claude-x', packs), packs, t)).toBe('<modelPick.custom>');
	});

	it('any stored model string reads without its provider or gateway', () => {
		expect(modelName('pack/nebo-ai/high', packs, t)).toBe('<modelPick.neboAi> · <settingsPacks.level.high>');
		expect(modelName('linked/1f2e/codex-3', packs, t)).toBe('codex-3');
		expect(modelName('pack/j/high', packs, t)).toBe('Janus · <settingsPacks.level.high>');
		expect(modelName('anthropic@x/claude-y', packs, t)).toBe('claude-y');
	});
});

describe('an owner\'s pack, as he set it', () => {
	const choices = [
		{ value: 'openai@p1/gpt-6.1', provider: 'OpenAI', connection: 'Company key', modelId: 'gpt-6.1', local: false },
		{ value: 'janus/nebo-1-pro', provider: 'Nebo AI', connection: '', modelId: 'nebo-1-pro', local: false }
	];
	const mine: PackRow = {
		id: 'm',
		name: 'Janus',
		builtIn: false,
		levels: { instant: 'janus/nebo-1-flash', high: 'janus/nebo-1-pro', max: 'openai@p1/gpt-6.1' },
		levelEffort: { max: 'high' }
	};

	it('each level: the provider and the model verbatim, no connection key', () => {
		expect(runsOnLine('openai@p1/gpt-6.1', choices)).toBe('OpenAI · gpt-6.1');
		expect(runsOnLine('janus/nebo-1-pro', choices)).toBe('Janus · nebo-1-pro');
		expect(runsOnLine('anthropic@zz/claude-sonnet-5', [])).toBe('Anthropic · claude-sonnet-5');
		expect(runsOnLine('openrouter@k/anthropic/claude-x', [])).toBe('OpenRouter · anthropic/claude-x');
		expect(runsOnLine('linked/1f2e/codex-3', [])).toBe('linked · codex-3');
	});

	it('the level rows: set, with effort, Auto, and an empty level from Nebo AI', () => {
		expect(levelSubtitle(mine, 'high', choices, t)).toEqual({ text: 'Janus · nebo-1-pro', muted: false });
		expect(levelSubtitle(mine, 'instant', choices, t).text).toBe('Janus · nebo-1-flash');
		expect(levelSubtitle(mine, 'max', choices, t).text).toBe('OpenAI · gpt-6.1 · <settingsPacks.withEffort>');
		expect(levelSubtitle(mine, 'auto', choices, t).text).toBe('<modelPick.autoCustom>');
		expect(levelSubtitle(mine, 'medium', choices, t)).toEqual({ text: '<modelPick.levelDesc.medium> · <modelPick.fromNeboAi>', muted: true });
	});

	it('Nebo AI keeps its own words, never a model id', () => {
		for (const row of ['auto', 'instant', 'low', 'medium', 'high', 'max'] as const) {
			const sub = levelSubtitle(packs[0], row, choices, t);
			expect(sub.text).toBe(`<modelPick.levelDesc.${row}>`);
		}
		expect(packSubtitle(packs[0], choices, t)).toBe('<modelPick.packBuiltIn>');
	});

	it('the pack list names an owner\'s pack by its providers', () => {
		expect(packSubtitle(mine, choices, t)).toBe('Janus, OpenAI');
		expect(packSubtitle({ id: 'e', name: 'Empty', builtIn: false }, choices, t)).toBe('<modelPick.packOwn>');
	});
});

describe('loadIntelligence', () => {
	it('reads the packs and the bot default', async () => {
		const i = await loadIntelligence();
		expect(i?.botDefault).toBe('pack/j/medium');
		expect(i?.packs.map((p) => p.id)).toEqual(['nebo-ai', 'j', 'w']);
		expect(i?.choices.map((c) => c.provider)).toEqual(['OpenAI']);
	});
});

describe('Auto', () => {
	it('picking Auto sends \'\' when that is what is inherited, else the pack itself', () => {
		expect(valueFor('nebo-ai', null, '', packs)).toBe('');
		expect(valueFor('nebo-ai', null, 'pack/j/medium', packs)).toBe('pack/nebo-ai');
		expect(valueFor('w', null, '', packs)).toBe('pack/w');
		expect(valueFor('j', null, 'pack/j', packs)).toBe('');
	});

	it('Medium is a real pick while Auto is in use', () => {
		vi.useFakeTimers();
		const onpick = vi.fn();
		const onclose = vi.fn();
		selectThenClose(resolve('', packs), 'nebo-ai', 'medium', onpick, onclose);
		vi.advanceTimersByTime(CLOSE_AFTER_MS);
		vi.useRealTimers();
		expect(onpick).toHaveBeenCalledWith('nebo-ai', 'medium');
		expect(onclose).toHaveBeenCalledTimes(1);
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
			['', 'nebo-ai', null],
			['pack/j/medium', 'j', 'medium'],
			['pack/w', 'w', null],
			['pack/nebo-ai/high', 'nebo-ai', 'high']
		] as const) {
			const r = run(current, pack, level);
			expect(r.onpick).not.toHaveBeenCalled();
			expect(r.closedAtOnce).toBe(1);
		}
	});
});
