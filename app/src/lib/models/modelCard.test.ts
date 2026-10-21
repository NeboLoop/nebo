import { describe, expect, it } from 'vitest';
import type { ModelCard, PackChoice } from '$lib/api/neboComponents';
import { bandLabel, cardBands, cardFit, cardPriceLine, cardSpeed, cardTier, shortTokens, usd } from './modelCard';
import { catalogErrorKey, choiceGroups, choiceRowLabel } from './connections';

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
	pricing: { version: '2026-10-19.9', currency: 'USD', inputPer1m: '3', outputPer1m: '15', requestFee: '0.00015', providerBills: false, bands: [] }
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

	it('shows the exact decimal the price book holds, never rounded', () => {
		expect(usd('0.125')).toBe('$0.125');
		expect(usd('3.125')).toBe('$3.125');
		expect(usd('0.075')).toBe('$0.075');
		expect(usd('1.1')).toBe('$1.10');
		expect(usd('12.5')).toBe('$12.50');
		expect(usd('3.7500')).toBe('$3.75');
		expect(usd('15.000000')).toBe('$15');
		expect(usd('0.0000375')).toBe('$0.0000375');
		expect(usd('n/a')).toBe('n/a');
	});

	it('shows every price band of a model priced by input size, with where it applies', () => {
		const tiered: ModelCard = {
			...sonnet,
			pricing: {
				...sonnet.pricing!,
				inputPer1m: '2.5',
				outputPer1m: '15',
				bands: [
					{ upToContext: 272_000, inputPer1m: '2.5', outputPer1m: '15' },
					{ inputPer1m: '5', outputPer1m: '22.5' }
				]
			}
		};
		expect(cardBands(tiered)).toHaveLength(2);
		expect(bandLabel(cardBands(tiered), 0, t)).toBe('modelCard.bandUpTo(272K)');
		expect(bandLabel(cardBands(tiered), 1, t)).toBe('modelCard.bandAbove(272K)');
		expect(cardPriceLine(tiered, t)).toBe(
			'modelCard.bandUpTo(272K): modelCard.per1m($2.50,$15); modelCard.bandAbove(272K): modelCard.per1m($5,$22.50)'
		);
		// A flat price (no bands sent by an older Janus) reads as one line, unlabeled.
		expect(cardBands(sonnet)).toHaveLength(1);
		expect(bandLabel(cardBands(sonnet), 0, t)).toBe('');
	});

	it('names only speeds it knows, and the fit as "Fits"', () => {
		expect(cardSpeed(sonnet)).toBe('fast');
		expect(cardSpeed({ ...sonnet, speed: 'ludicrous' })).toBe('');
		expect(cardFit(sonnet, t)).toBe('modelCard.fits(settingsPacks.level.medium)');
		expect(cardFit({ ...sonnet, tier: '' }, t)).toBe('');
	});

	it("words a provider list that could not be read", () => {
		expect(catalogErrorKey('key')).toBe('settingsProviders.browse.errorKey');
		expect(catalogErrorKey('rate_limited')).toBe('settingsProviders.browse.errorBusy');
		expect(catalogErrorKey('unreachable')).toBe('settingsProviders.browse.errorUnreachable');
	});

	it("shows Nebo AI's retail per 1M tokens, and the request fee alone on the owner's own key", () => {
		expect(cardPriceLine(sonnet, t)).toBe('modelCard.per1m($3,$15)');
		const own: ModelCard = { ...sonnet, pricing: { version: '', currency: 'USD', requestFee: '0.00015', providerBills: true, bands: [] } };
		expect(cardPriceLine(own, t)).toBe('modelCard.ownKeyPrice($0.00015)');
		const direct: ModelCard = { ...sonnet, pricing: { version: '', currency: 'USD', providerBills: true, bands: [] } };
		expect(cardPriceLine(direct, t)).toBe('modelCard.providerBills');
		expect(cardPriceLine(undefined, t)).toBe('');
	});

	it('names the level a model fits, and nothing for an unknown one', () => {
		expect(cardTier(sonnet, t)).toBe('settingsPacks.level.medium');
		expect(cardTier({ ...sonnet, tier: '' }, t)).toBe('');
	});
});

describe('pack choices with cards', () => {
	it('reads as provider, model, the level it fits and price', () => {
		expect(choiceRowLabel(choice('janus/claude-sonnet-5', 'Nebo AI', sonnet, 'Claude Sonnet 5'), t)).toBe(
			'Nebo AI · Claude Sonnet 5 · modelCard.fits(settingsPacks.level.medium) · modelCard.per1m($3,$15)'
		);
		// Inside a group that names the provider and family, the row does not repeat them.
		expect(choiceRowLabel(choice('janus/claude-sonnet-5', 'Nebo AI', sonnet, 'Claude Sonnet 5'), t, true)).toBe(
			'Claude Sonnet 5 · modelCard.fits(settingsPacks.level.medium) · modelCard.per1m($3,$15)'
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

describe('the same words as the mobile app', () => {
	it('names token counts the way the phone does', () => {
		expect(shortTokens(8_192)).toBe('8,192');
		expect(shortTokens(128_000)).toBe('128K');
		expect(shortTokens(272_000)).toBe('272K');
		expect(shortTokens(1_000_000)).toBe('1M');
		expect(shortTokens(1_050_000)).toBe('1.05M');
		expect(shortTokens(999)).toBe('999');
	});

	it('names the level a model fits as "Fits"', () => {
		expect(cardFit(sonnet, t)).toBe('modelCard.fits(settingsPacks.level.medium)');
	});
});
