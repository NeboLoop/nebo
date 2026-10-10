/**
 * The speeds an employee can work at — ONE source, the sellable list Janus
 * publishes (GET /v1/models, synced into the catalog and served by
 * /api/v1/models). Settings → General → MODEL and the composer chip both read
 * this; neither keeps a list of its own. With Developer mode on, the
 * Intelligence Packs section follows the speeds (a feature under test: never
 * shown to an owner without it).
 */
import { get } from 'svelte/store';
import * as api from '$lib/api/nebo';
import { devMode, loadDevMode } from '$lib/stores/devmode';

/**
 * One row of a model picker. `section: 'packs'` rows are the Developer-mode
 * "Intelligence Packs" section; `packId` marks a whole-pack row (one the
 * owner can make the bot's default), `isDefault` the bot's default pack.
 */
export type ModelOption = {
	value: string;
	label: string;
	description: string;
	section?: 'packs';
	packId?: string;
	isDefault?: boolean;
};

type CatalogModel = {
	id: string;
	displayName: string;
	description?: string | null;
	isActive: boolean;
	/** The speed's place on Janus's ladder (nebo-1 0, Fast 1, Balanced 2, Deep 3). */
	rank?: number | null;
};

/** The catalog id of the default speed. Its option value is '' — "no choice". */
const DEFAULT_ID = 'nebo-1';

/**
 * The options: Default first, then the speeds by their rank on the ladder
 * (never by name), then, in Developer mode, the Intelligence Packs section.
 * `current` is the value in use: a pack choice made in Developer mode stays
 * listed (as its one row) after Developer mode is turned off. Empty when the
 * catalog has not synced — callers render nothing rather than a guess.
 */
export async function loadModelOptions(current = ''): Promise<ModelOption[]> {
	try {
		const [res, packs] = await Promise.all([
			api.listModels() as Promise<{ models?: Record<string, CatalogModel[]> }>,
			packOptions(current)
		]);
		const janus = (res.models?.['janus'] ?? []).filter(
			(m) => m.isActive && (m.id === DEFAULT_ID || m.description)
		);
		const rank = (m: CatalogModel) =>
			m.id === DEFAULT_ID ? -1 : (m.rank ?? Number.MAX_SAFE_INTEGER);
		janus.sort((a, b) => rank(a) - rank(b));
		const speeds = janus.map((m) => ({
			value: m.id === DEFAULT_ID ? '' : `janus/${m.id}`,
			label: m.displayName,
			description: m.description ?? ''
		}));
		return speeds.length ? [...speeds, ...packs] : [];
	} catch {
		return [];
	}
}

/** The Effort ladder, lowest first (`types::packs::Effort`). */
export const EFFORTS = ['instant', 'low', 'medium', 'high', 'max'] as const;
export type Effort = (typeof EFFORTS)[number];
export const EFFORT_LABELS: Record<Effort, string> = {
	instant: 'Instant',
	low: 'Low',
	medium: 'Medium',
	high: 'High',
	max: 'Max'
};

/** What each level is for, in plain words. */
const EFFORT_DESCRIPTIONS: Record<Effort, string> = {
	instant: 'Quickest replies',
	low: 'Quick, with a little thinking',
	medium: 'Everyday work',
	high: 'Harder problems',
	max: 'The most thinking'
};

/** The built-in Nebo AI pack: its levels are the speeds, so only its row is listed. */
const NEBO_AI_ID = 'nebo-ai';

/**
 * The Intelligence Packs section (Developer mode only): Nebo AI as one row
 * (back to the standard speeds), then each of the owner's own packs as its
 * row (`pack/<id>`, its default level) and once per level
 * (`pack/<id>/<level>`). Never a model id: a level reads as what it is for.
 * With Developer mode off, only `current` is kept, when it is a pack choice.
 * Developer surfaces are English.
 */
export async function packOptions(current = ''): Promise<ModelOption[]> {
	await loadDevMode();
	const dev = get(devMode);
	if (!dev && !current.startsWith('pack/')) return [];
	try {
		const res = await api.listPacks();
		// The bot's default: a pack ref, or a model (Nebo AI's own speeds).
		const def = res.default ?? '';
		const all = (res.packs ?? []).flatMap((p): ModelOption[] => {
			const row: ModelOption = {
				value: `pack/${p.id}`,
				label: p.name,
				description: p.builtIn ? 'The standard speeds' : 'Your pack at its default level',
				section: 'packs',
				packId: p.id,
				isDefault: def.startsWith('pack/')
					? def === `pack/${p.id}` || def.startsWith(`pack/${p.id}/`)
					: p.builtIn
			};
			return [
				row,
				...EFFORTS.map((e) => ({
					value: `pack/${p.id}/${e}`,
					label: `${p.name} · ${EFFORT_LABELS[e]}`,
					description: p.levels[e]
						? EFFORT_DESCRIPTIONS[e]
						: `${EFFORT_DESCRIPTIONS[e]} · uses the nearest level that is set`,
					section: 'packs' as const
				}))
			];
		});
		return all.filter((o) =>
			o.value === current || `${o.value}/auto` === current || (dev && (o.packId !== undefined || !o.value.startsWith(`pack/${NEBO_AI_ID}/`)))
		);
	} catch {
		return [];
	}
}

/** Make a pack the bot's default (Developer mode, the picker's pack rows). */
export async function makeDefaultPack(packId: string): Promise<void> {
	await api.setDefaultPack({ value: `pack/${packId}` });
}

/**
 * The name to show for a stored value. Matches on the bare model id so a value
 * written as `nebo-1-pro` and one written as `janus/nebo-1-pro` land on the
 * same row. '' (or no match) is the default speed — the first option.
 */
export function modelLabel(value: string, options: ModelOption[]): string {
	if (options.length === 0) return '';
	const bare = (v: string) => v.split('/').pop() ?? v;
	if (!value) return options[0]?.label ?? '';
	// A pack's levels share their last part ("high"): only an exact match names one.
	const exact = options.find((o) => o.value === value);
	if (exact || value.startsWith('pack/')) return exact?.label ?? options[0]?.label ?? '';
	return (
		options.find((o) => !o.value.startsWith('pack/') && bare(o.value) === bare(value))?.label ??
		options[0]?.label ??
		''
	);
}
