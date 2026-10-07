import { describe, expect, it } from 'vitest';
import { modelLabel, type ModelOption } from './speeds';

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
