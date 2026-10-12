import { describe, expect, it } from 'vitest';
import type { CatalogModel, Pack, PackChoice } from '$lib/api/neboComponents';
import {
	filterCatalog,
	groupByFamily,
	isDefaultPack,
	levelRows,
	parseContext,
	parseModelString,
	runsOn,
	shortContext,
	shortUrl
} from './connections';

const choices: PackChoice[] = [
	{ value: 'anthropic@p1/claude-sonnet-5', provider: 'Anthropic', connection: 'Company key', modelId: 'claude-sonnet-5', local: false },
	{ value: 'janus/jev-latest', provider: 'Nebo AI', connection: '', modelId: 'jev-latest', local: false },
	{ value: 'ollama/llama3.2:3b', provider: 'Ollama', connection: 'Ollama', modelId: 'llama3.2:3b', local: true }
];

function pack(levels: Pack['levels']): Pack {
	return { id: 'a', name: 'A', levels, fallback: true, builtIn: false, routeThroughJanus: false, lanes: {}, levelEffort: {} };
}

function model(p: Partial<CatalogModel> & { modelId: string }): CatalogModel {
	return { displayName: p.modelId, family: '', kind: 'chat', capabilities: [], added: false, ...p };
}

describe('parseModelString', () => {
	it('reads a connection-scoped string and keeps slashes in the model id', () => {
		expect(parseModelString('openrouter@x/anthropic/claude-sonnet-5')).toEqual({
			kind: 'openrouter',
			profileId: 'x',
			modelId: 'anthropic/claude-sonnet-5'
		});
		expect(parseModelString('janus/nebo-1-pro')).toEqual({ kind: 'janus', profileId: '', modelId: 'nebo-1-pro' });
	});
});

describe('runsOn', () => {
	it('names the provider with its connection, and drops a connection that repeats the provider', () => {
		expect(runsOn('anthropic@p1/claude-sonnet-5', choices)).toEqual({
			label: 'Anthropic (Company key)',
			modelId: 'claude-sonnet-5',
			local: false
		});
		expect(runsOn('ollama/llama3.2:3b', choices)).toEqual({ label: 'Ollama', modelId: 'llama3.2:3b', local: true });
		expect(runsOn('janus/jev-latest', choices)?.label).toBe('Nebo AI');
	});

	it('still shows a model that is no longer offered, and nothing for an empty slot', () => {
		expect(runsOn('anthropic@gone/claude-opus-5', choices)).toEqual({
			label: 'Anthropic',
			modelId: 'claude-opus-5',
			local: false
		});
		expect(runsOn(undefined, choices)).toBeNull();
	});
});

describe('levelRows', () => {
	it('collapses five identical levels into one row', () => {
		const v = 'ollama/llama3.2:3b';
		expect(levelRows(pack({ instant: v, low: v, medium: v, high: v, max: v }))).toEqual([{ level: 'every', value: v }]);
	});

	it('lists Auto first, then every level, set or not', () => {
		const rows = levelRows(pack({ auto: 'janus/auto', medium: 'm' }));
		expect(rows.map((r) => r.level)).toEqual(['auto', 'instant', 'low', 'medium', 'high', 'max']);
		expect(rows[3].value).toBe('m');
	});
});

it('isDefaultPack matches the pack and its levels, not a pack whose id starts the same', () => {
	expect(isDefaultPack('pack/a', 'a')).toBe(true);
	expect(isDefaultPack('pack/a/high', 'a')).toBe(true);
	expect(isDefaultPack('pack/ab', 'a')).toBe(false);
	expect(isDefaultPack('', 'a')).toBe(false);
});

describe('the catalog', () => {
	const models = [
		model({ modelId: 'anthropic/claude-sonnet-5', family: 'Claude Sonnet', capabilities: ['vision', 'tools'], contextWindow: 1_000_000, created: 3, pricing: { input: 3, output: 15 } }),
		model({ modelId: 'meta/llama-4', family: 'Llama', capabilities: ['tools'], contextWindow: 128_000, created: 2, pricing: { input: 0.2, output: 0.6 } }),
		model({ modelId: 'anthropic/claude-sonnet-4-6', family: 'Claude Sonnet', capabilities: ['vision', 'tools'], contextWindow: 1_000_000, created: 1 })
	];

	it('filters by ability and context and sorts newest first or cheapest first', () => {
		const base = { vision: false, tools: false, thinking: false, minContext: 0, sort: 'newest' as const };
		expect(filterCatalog(models, base).map((m) => m.created)).toEqual([3, 2, 1]);
		expect(filterCatalog(models, { ...base, vision: true }).map((m) => m.created)).toEqual([3, 1]);
		expect(filterCatalog(models, { ...base, minContext: 200_000 }).length).toBe(2);
		expect(filterCatalog(models, { ...base, sort: 'price' }).map((m) => m.created)).toEqual([2, 3, 1]);
	});

	it('groups by family in arrival order, with the vendors seen', () => {
		const groups = groupByFamily(models);
		expect(groups.map((g) => g.family)).toEqual(['Claude Sonnet', 'Llama']);
		expect(groups[0].models.length).toBe(2);
		expect(groups[0].vendors).toEqual(['anthropic']);
		expect(groupByFamily([model({ modelId: 'x/solo' })])[0].family).toBe('x/solo');
	});
});

it('formats and parses context windows and URLs', () => {
	expect(shortContext(1_000_000)).toBe('1M');
	expect(shortContext(131_072)).toBe('131k');
	expect(shortContext(undefined)).toBe('');
	expect(parseContext('131,072')).toBe(131072);
	expect(parseContext('')).toBeUndefined();
	expect(shortUrl('https://api.groq.com/openai/v1/')).toBe('api.groq.com/openai/v1');
});
