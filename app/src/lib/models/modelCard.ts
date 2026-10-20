/**
 * Pure helpers for a model's card (what it is, what it is good for, its
 * price) as the pickers show it: Nebo AI's retail prices per 1M tokens, or on
 * the owner's own key the request fee alone. No API calls; copy comes from
 * the caller's translator.
 */
import type { CardBand, ModelCard } from '$lib/api/neboComponents';

/** The translator the page passes in (svelte-i18n's `$t`). */
export type Translate = (key: string, opts?: { values?: Record<string, string> }) => string;

/**
 * A price as the cards show it: the exact decimal the price book holds, with
 * trailing zeros trimmed and a lone tenth shown as cents ("$3", "$0.30",
 * "$0.125", "$3.125", "$0.00015"). Never rounded: what the ledger charges is
 * what the card says. Anything that is not a plain decimal is shown as given.
 */
export function usd(amount: string | undefined): string {
	if (!amount) return '';
	const s = amount.trim();
	if (!/^\d+(\.\d+)?$/.test(s)) return s;
	const [whole0, frac0 = ''] = s.split('.');
	const whole = whole0.replace(/^0+(?=\d)/, '');
	const frac = frac0.replace(/0+$/, '');
	if (!frac) return `$${whole}`;
	return frac.length === 1 ? `$${whole}.${frac}0` : `$${whole}.${frac}`;
}

/** A card's price bands, ascending (one for a model priced the same at every size). */
export function cardBands(card: ModelCard | undefined): CardBand[] {
	const p = card?.pricing;
	if (!p || p.providerBills) return [];
	if (p.bands?.length) return p.bands;
	return [{ inputPer1m: p.inputPer1m, outputPer1m: p.outputPer1m, cacheReadPer1m: p.cacheReadPer1m, cacheWritePer1m: p.cacheWritePer1m }];
}

/** Where a band applies: "Up to 272K tokens", "Above 272K tokens"; '' for a flat price. */
export function bandLabel(bands: CardBand[], i: number, t: Translate): string {
	if (bands.length < 2) return '';
	const top = bands[i].upToContext;
	if (top) return t('modelCard.bandUpTo', { values: { tokens: shortTokens(top) } });
	const below = bands[i - 1]?.upToContext;
	return below ? t('modelCard.bandAbove', { values: { tokens: shortTokens(below) } }) : '';
}

/**
 * A token count as a card shows it, the same rule the mobile app uses:
 * "200K", "272K", "1M", "1.05M"; under ten thousand in full ("8,192").
 */
export function shortTokens(n: number): string {
	const trim = (v: number) => v.toFixed(2).replace(/\.?0+$/, '');
	if (n >= 1_000_000) return `${trim(n / 1_000_000)}M`;
	if (n >= 10_000) return `${trim(n / 1_000)}K`;
	const s = String(n);
	return s.length > 3 ? `${s.slice(0, -3)},${s.slice(-3)}` : s;
}

/**
 * A model's price in one line: Nebo AI's retail rates per 1M tokens in and
 * out (each band with where it applies, for a model priced by input size),
 * or, on the owner's own key, the request fee with his provider billing the
 * model. '' when nothing is known.
 */
export function cardPriceLine(card: ModelCard | undefined, t: Translate): string {
	const p = card?.pricing;
	if (!p) return '';
	if (p.providerBills) {
		return p.requestFee
			? t('modelCard.ownKeyPrice', { values: { fee: usd(p.requestFee) } })
			: t('modelCard.providerBills');
	}
	const bands = cardBands(card).filter((b) => b.inputPer1m && b.outputPer1m);
	if (!bands.length) return '';
	return bands
		.map((b, i) => {
			const price = t('modelCard.per1m', { values: { input: usd(b.inputPer1m), output: usd(b.outputPer1m) } });
			const where = bandLabel(bands, i, t);
			return where ? `${where}: ${price}` : price;
		})
		.join('; ');
}

/** The level a model fits, as a row says it ("Fits Medium"); '' unknown. */
export function cardFit(card: ModelCard | undefined, t: Translate): string {
	const level = cardTier(card, t);
	return level ? t('modelCard.fits', { values: { level } }) : '';
}

/** A speed the card can name; '' for one this app does not know. */
export function cardSpeed(card: ModelCard | undefined): string {
	const speed = card?.speed ?? '';
	return ['fastest', 'fast', 'standard', 'slow'].includes(speed) ? speed : '';
}

/** The Intelligence level a model fits, in the owner's words; '' unknown. */
export function cardTier(card: ModelCard | undefined, t: Translate): string {
	const tier = card?.tier ?? '';
	return ['instant', 'low', 'medium', 'high', 'max'].includes(tier) ? t(`settingsPacks.level.${tier}`) : '';
}

