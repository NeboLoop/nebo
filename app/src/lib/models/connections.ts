/**
 * Pure helpers for Settings → Providers and Settings → Intelligence Packs
 * (Developer mode): reading model strings, naming what a pack level runs on,
 * and filtering / grouping a provider's browsable catalog. No API calls and
 * no copy here — the pages own both.
 */
import type { CatalogModel, Pack, PackChoice } from '$lib/api/neboComponents';
import { EFFORTS, type Effort } from './speeds';

/** The connection kinds Add provider offers, in the order it shows them. */
export const CONNECTION_KINDS = [
	'anthropic',
	'openai',
	'google',
	'openrouter',
	'openai_compatible',
	'systemone_compatible'
] as const;
export type ConnectionKind = (typeof CONNECTION_KINDS)[number];

/** Kinds whose URL the owner types (every other kind has a fixed URL). */
export function needsBaseUrl(kind: string): boolean {
	return kind === 'openai_compatible' || kind === 'systemone_compatible';
}

/** Connections that are not "Your providers": Nebo AI and auto-detected Ollama. */
export function isOwnConnection(provider: string): boolean {
	return provider !== 'neboai' && provider !== 'janus' && provider !== 'ollama';
}

/** A URL without its scheme or trailing slash, as a card shows it. */
export function shortUrl(url: string | undefined | null): string {
	if (!url) return '';
	return url.replace(/^[a-z]+:\/\//i, '').replace(/\/+$/, '');
}

/**
 * A model string's parts: `<kind>@<profileId>/<modelId>` names one
 * connection, `<kind>/<modelId>` the active connection of that kind. The model
 * id keeps any slashes of its own (`openrouter@x/anthropic/claude-sonnet-5`).
 */
export function parseModelString(value: string): { kind: string; profileId: string; modelId: string } {
	const slash = value.indexOf('/');
	if (slash < 0) return { kind: '', profileId: '', modelId: value };
	const head = value.slice(0, slash);
	const modelId = value.slice(slash + 1);
	const at = head.indexOf('@');
	return at < 0
		? { kind: head, profileId: '', modelId }
		: { kind: head.slice(0, at), profileId: head.slice(at + 1), modelId };
}

/** "Anthropic (Company key)", or just the provider when the connection adds nothing. */
export function choiceProviderLabel(choice: Pick<PackChoice, 'provider' | 'connection'>): string {
	const conn = (choice.connection ?? '').trim();
	return conn && conn !== choice.provider ? `${choice.provider} (${conn})` : choice.provider;
}

/** What a pack slot runs on: the provider/connection line and the model id beneath it. */
export type RunsOn = { label: string; modelId: string; local: boolean };

/**
 * Name the model a slot points at. Uses the pack choices (every active model
 * of every connection) first; a model no longer offered still shows its kind
 * and id, so a stale slot is visible rather than blank.
 */
export function runsOn(value: string | undefined, choices: PackChoice[]): RunsOn | null {
	if (!value) return null;
	const hit = choices.find((c) => c.value === value);
	if (hit) return { label: choiceProviderLabel(hit), modelId: hit.modelId, local: hit.local };
	const { kind, modelId } = parseModelString(value);
	const sameKind = choices.find((c) => parseModelString(c.value).kind === kind);
	return { label: sameKind?.provider ?? kind, modelId, local: sameKind?.local ?? false };
}

/**
 * A pack's level rows. Five identical levels collapse to one "every level"
 * row (`level: 'every'`); otherwise one row per Effort level, with `auto`
 * first when the pack has it (Nebo AI).
 */
export function levelRows(pack: Pack): { level: Effort | 'auto' | 'every'; value: string | undefined }[] {
	const values = EFFORTS.map((e) => pack.levels[e]);
	if (!pack.levels.auto && values[0] && values.every((v) => v === values[0])) {
		return [{ level: 'every', value: values[0] }];
	}
	const rows: { level: Effort | 'auto' | 'every'; value: string | undefined }[] = [];
	if (pack.levels.auto) rows.push({ level: 'auto', value: pack.levels.auto });
	for (const e of EFFORTS) rows.push({ level: e, value: pack.levels[e] });
	return rows;
}

/** Whether the bot's default (`pack/<id>` or `pack/<id>/<level>`) is this pack. */
export function isDefaultPack(defaultValue: string | undefined, packId: string): boolean {
	if (!defaultValue) return false;
	return defaultValue === `pack/${packId}` || defaultValue.startsWith(`pack/${packId}/`);
}

/** Whether any model a pack names runs on this computer (it can't route through Janus). */
export function packUsesLocal(pack: Pack, choices: PackChoice[]): boolean {
	return Object.values(pack.levels).some((v) => !!v && !!choices.find((c) => c.value === v)?.local);
}

// ── Browse models ───────────────────────────────────────────────────

export type CatalogFilters = {
	vision: boolean;
	tools: boolean;
	thinking: boolean;
	/** Minimum context window in tokens; 0 = any. */
	minContext: number;
	sort: 'newest' | 'price';
};

/** Keep the models with every ability asked for and at least the context asked for, sorted. */
export function filterCatalog(models: CatalogModel[], f: CatalogFilters): CatalogModel[] {
	const need = [f.vision && 'vision', f.tools && 'tools', f.thinking && 'thinking'].filter(Boolean) as string[];
	const kept = models.filter(
		(m) =>
			need.every((c) => (m.capabilities ?? []).includes(c)) &&
			(f.minContext <= 0 || (m.contextWindow ?? 0) >= f.minContext)
	);
	const price = (m: CatalogModel) => (m.pricing ? m.pricing.input + m.pricing.output : Number.POSITIVE_INFINITY);
	return kept.sort((a, b) =>
		f.sort === 'price' ? price(a) - price(b) : (b.created ?? 0) - (a.created ?? 0)
	);
}

/** The vendor part of a catalog id (`anthropic/claude-sonnet-5` → `anthropic`). */
export function vendorOf(modelId: string): string {
	const slash = modelId.indexOf('/');
	return slash < 0 ? '' : modelId.slice(0, slash);
}

export type FamilyGroup = { family: string; vendors: string[]; models: CatalogModel[] };

/**
 * Group catalog models by family, keeping the order the models arrive in
 * (already sorted): a family sits where its first model sits. A model with no
 * family is its own group under its display name.
 */
export function groupByFamily(models: CatalogModel[]): FamilyGroup[] {
	const groups = new Map<string, FamilyGroup>();
	for (const m of models) {
		const family = (m.family ?? '').trim() || m.displayName || m.modelId;
		let g = groups.get(family);
		if (!g) {
			g = { family, vendors: [], models: [] };
			groups.set(family, g);
		}
		g.models.push(m);
		const v = vendorOf(m.modelId);
		if (v && !g.vendors.includes(v)) g.vendors.push(v);
	}
	return [...groups.values()];
}

/** A context window as the cards show it: 1M, 200k, 131k. */
export function shortContext(tokens: number | undefined): string {
	if (!tokens) return '';
	if (tokens >= 1_000_000) return `${+(tokens / 1_000_000).toFixed(1)}M`;
	if (tokens >= 1000) return `${Math.round(tokens / 1000)}k`;
	return String(tokens);
}

/** Digits typed into a context-window field ("131,072" → 131072); undefined when empty. */
export function parseContext(text: string): number | undefined {
	const n = Number(text.replace(/[^\d]/g, ''));
	return Number.isFinite(n) && n > 0 ? n : undefined;
}
