import { describe, expect, it } from 'vitest';
import type { ModelCard, PackChoice } from '$lib/api/neboComponents';
import { cardPriceLine, cardTier, usd } from './modelCard';
import { choiceGroups, choiceRowLabel } from './connections';

/** A translator that shows the key and its values, so a test reads what was asked for. */
const t = (key: string, opts?: { values?: Record<string, string> }) =>
	opts?.values ? `${key}(${Object.values(opts.values).join(',')})` : key;

const sonnet: ModelCard = {
	displayName: 'Claude Sonnet 5',
	family: 'Claude Sonnet',
	maker: 'Anthropic',
	description: 'Everyday work.',
	tier: 'medium',
	reasons: true,
	abilities: { vision: true, tools: true, reasoning: true, audio: false },
	speed: 'fast',
	pricing: { version: '2026-10-19.9', currency: 'USD', inputPer1m: '3', outputPer1m: '15', requestFee: '0.00015', providerBills: false }
};

const choice = (value: string, provider: string, card?: ModelCard, name?: string): PackChoice => ({
	value,
	provider,
	connection: '',
	modelId: value.split('/').pop() ?? value,
	name,
	local: false,
	card
});

describe('model card prices', () => {
	it('reads dollars the way a price list does', () => {
		expect(usd('3')).toBe('$3');
		expect(usd('0.3')).toBe('$0.30');
		expect(usd('3.75')).toBe('$3.75');
		expect(usd('0.00015')).toBe('$0.00015');
		expect(usd(undefined)).toBe('');
	});

	it("shows Nebo AI's retail per 1M tokens, and the request fee alone on the owner's own key", () => {
		expect(cardPriceLine(sonnet, t)).toBe('modelCard.per1m($3,$15)');
		const own: ModelCard = { ...sonnet, pricing: { version: '', currency: 'USD', requestFee: '0.00015', providerBills: true } };
		expect(cardPriceLine(own, t)).toBe('modelCard.ownKeyPrice($0.00015)');
		const direct: ModelCard = { ...sonnet, pricing: { version: '', currency: 'USD', providerBills: true } };
		expect(cardPriceLine(direct, t)).toBe('modelCard.providerBills');
		expect(cardPriceLine(undefined, t)).toBe('');
	});

	it('names the level a model fits, and nothing for an unknown one', () => {
		expect(cardTier(sonnet, t)).toBe('settingsPacks.level.medium');
		expect(cardTier({ ...sonnet, tier: '' }, t)).toBe('');
	});
});

describe('pack choices with cards', () => {
	it('reads as provider, model, level and price', () => {
		expect(choiceRowLabel(choice('janus/claude-sonnet-5', 'Nebo AI', sonnet, 'Claude Sonnet 5'), t)).toBe(
			'Nebo AI · Claude Sonnet 5 · settingsPacks.level.medium · modelCard.per1m($3,$15)'
		);
		expect(choiceRowLabel(choice('janus/nebo-1-pro', 'Nebo AI', undefined, 'High and Max'), t)).toBe('Nebo AI · High and Max');
	});

	it("groups Nebo AI's models by family and everything else by provider, in the order they arrive", () => {
		const groups = choiceGroups([
			choice('janus/nebo-1-flash', 'Nebo AI'),
			choice('janus/claude-sonnet-5', 'Nebo AI', sonnet),
			choice('anthropic@a/claude-opus-5-5', 'Anthropic'),
			choice('janus/claude-sonnet-4-6', 'Nebo AI', { ...sonnet, displayName: 'Claude Sonnet 4.6' })
		]);
		expect(groups.map((g) => [g.label, g.choices.length])).toEqual([
			['Nebo AI', 1],
			['Nebo AI · Claude Sonnet', 2],
			['Anthropic', 1]
		]);
	});
});
