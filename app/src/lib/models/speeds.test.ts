import { beforeEach, describe, expect, it, vi } from 'vitest';
import { writable } from 'svelte/store';

const dev = writable(false);
vi.mock('$lib/stores/devmode', () => ({ devMode: dev, loadDevMode: async () => {} }));
vi.mock('$lib/api/nebo', () => ({
	// The catalog as the DB once returned it: by name.
	listModels: async () => ({
		models: {
			janus: [
				{ id: 'nebo-1-medium', displayName: 'Balanced', description: 'Everyday', isActive: true, rank: 2 },
				{ id: 'nebo-1-pro', displayName: 'Deep', description: 'Hard work', isActive: true, rank: 3 },
				{ id: 'nebo-1', displayName: 'Default', description: 'Recommended', isActive: true, rank: 0 },
				{ id: 'nebo-1-flash', displayName: 'Fast', description: 'Quick', isActive: true, rank: 1 }
			]
		}
	}),
	listPacks: async () => ({
		default: 'pack/j/medium',
		packs: [
			{ id: 'nebo-ai', name: 'Nebo AI', builtIn: true, fallback: false, levels: { auto: 'janus/nebo-1', instant: 'janus/nebo-1-flash', medium: 'janus/nebo-1-medium' } },
			{ id: 'j', name: 'Janus', builtIn: false, fallback: true, levels: { medium: 'janus/x-model', high: 'openai@p1/gpt' } }
		]
	}),
	setDefaultPack: async () => ({})
}));

const { loadModelOptions, modelLabel } = await import('./speeds');
type ModelOption = import('./speeds').ModelOption;

const options: ModelOption[] = [
	{ value: '', label: 'Default', description: '' },
	{ value: 'janus/nebo-1-pro', label: 'Deep', description: '' },
	{ value: 'pack/a/high', label: 'Mine · High', description: '' },
	{ value: 'pack/b/high', label: 'Work · High', description: '' }
];

describe('modelLabel', () => {
	it('names a speed by its bare id, written either way', () => {
		expect(modelLabel('janus/nebo-1-pro', options)).toBe('Deep');
		expect(modelLabel('nebo-1-pro', options)).toBe('Deep');
		expect(modelLabel('', options)).toBe('Default');
	});

	it('names a pack level only by an exact match: two packs share "high"', () => {
		expect(modelLabel('pack/b/high', options)).toBe('Work · High');
		expect(modelLabel('pack/a/high', options)).toBe('Mine · High');
		expect(modelLabel('pack/gone/high', options)).toBe('Default');
	});
});

describe('loadModelOptions', () => {
	beforeEach(() => dev.set(false));

	it('lists Default, then the speeds by rank, never by name', async () => {
		const opts = await loadModelOptions();
		expect(opts.map((o) => o.label)).toEqual(['Default', 'Fast', 'Balanced', 'Deep']);
	});

	it('lists no pack without Developer mode, but keeps the one in use', async () => {
		const opts = await loadModelOptions('pack/j/high');
		expect(opts.map((o) => o.label)).toEqual(['Default', 'Fast', 'Balanced', 'Deep', 'Janus · High']);
		expect(modelLabel('pack/j/high', opts)).toBe('Janus · High');
	});

	it('with Developer mode: Nebo AI as one row, own packs by level, never a model id', async () => {
		dev.set(true);
		const opts = await loadModelOptions();
		const packs = opts.filter((o) => o.section === 'packs');
		expect(packs.map((o) => o.label)).toEqual([
			'Nebo AI',
			'Janus',
			'Janus · Instant',
			'Janus · Low',
			'Janus · Medium',
			'Janus · High',
			'Janus · Max'
		]);
		expect(packs.find((o) => o.packId === 'j')?.isDefault).toBe(true);
		expect(packs.find((o) => o.packId === 'nebo-ai')?.isDefault).toBe(false);
		for (const o of opts) expect(o.description).not.toMatch(/janus\/|@|\//);
	});
});
