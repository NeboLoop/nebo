/**
 * Pure helpers for a model's card (what it is, what it is good for, its
 * price) as the pickers show it: Nebo AI's retail prices per 1M tokens, or on
 * the owner's own key the request fee alone. No API calls; copy comes from
 * the caller's translator.
 */
import type { ModelCard } from '$lib/api/neboComponents';

/** The translator the page passes in (svelte-i18n's `$t`). */
export type Translate = (key: string, opts?: { values?: Record<string, string> }) => string;

/** A price as the cards show it: "$3", "$0.30", "$0.00015". */
export function usd(amount: string | undefined): string {
	if (!amount) return '';
	const n = Number(amount);
	if (!Number.isFinite(n)) return '';
	if (n === 0) return '$0';
	if (n >= 1) return `$${+n.toFixed(2)}`;
	if (n >= 0.01) return `$${n.toFixed(2)}`;
	return `$${+n.toPrecision(2)}`;
}

/**
 * A model's price in one line: Nebo AI's retail rates per 1M tokens in and
 * out, or, on the owner's own key, the request fee with his provider billing
 * the model. '' when nothing is known.
 */
export function cardPriceLine(card: ModelCard | undefined, t: Translate): string {
	const p = card?.pricing;
	if (!p) return '';
	if (p.providerBills) {
		return p.requestFee
			? t('modelCard.ownKeyPrice', { values: { fee: usd(p.requestFee) } })
			: t('modelCard.providerBills');
	}
	if (!p.inputPer1m || !p.outputPer1m) return '';
	return t('modelCard.per1m', { values: { input: usd(p.inputPer1m), output: usd(p.outputPer1m) } });
}

/** The Intelligence level a model fits, in the owner's words; '' unknown. */
export function cardTier(card: ModelCard | undefined, t: Translate): string {
	const tier = card?.tier ?? '';
	return ['instant', 'low', 'medium', 'high', 'max'].includes(tier) ? t(`settingsPacks.level.${tier}`) : '';
}

