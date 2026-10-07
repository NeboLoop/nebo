/**
 * The speeds an employee can work at — ONE source, the sellable list Janus
 * publishes (GET /v1/models, synced into the catalog and served by
 * /api/v1/models). Settings → General → MODEL and the composer chip both read
 * this; neither keeps a list of its own. With Developer mode on, the
 * intelligence packs follow the speeds (a feature under test: never shown to
 * an owner without it).
 */
import { get } from 'svelte/store';
import * as api from '$lib/api/nebo';
import { devMode, loadDevMode } from '$lib/stores/devmode';

export type ModelOption = { value: string; label: string; description: string };

type CatalogModel = {
	id: string;
	displayName: string;
	description?: string | null;
	isActive: boolean;
};

/** The catalog id of the default speed. Its option value is '' — "no choice". */
const DEFAULT_ID = 'nebo-1';

/**
 * The options, Default first, then the ladder as Janus orders it, then (in
 * Developer mode) the intelligence packs. Empty when the catalog has not
 * synced — callers render nothing rather than a guess.
 */
export async function loadModelOptions(): Promise<ModelOption[]> {
	try {
		const [res, packs] = await Promise.all([
			api.listModels() as Promise<{ models?: Record<string, CatalogModel[]> }>,
			packOptions()
		]);
		const janus = (res.models?.['janus'] ?? []).filter(
			(m) => m.isActive && (m.id === DEFAULT_ID || m.description)
		);
		janus.sort((a, b) => (a.id === DEFAULT_ID ? -1 : b.id === DEFAULT_ID ? 1 : 0));
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

/**
 * Developer mode only: each pack as `pack/<id>` (Nebo AI's Auto, else the
 * pack's default level) and once per Effort level (`pack/<id>/<level>`).
 * Developer surfaces are English.
 */
export async function packOptions(): Promise<ModelOption[]> {
	await loadDevMode();
	if (!get(devMode)) return [];
	try {
		const res = await api.listPacks();
		return (res.packs ?? []).flatMap((p) => [
			{
				value: `pack/${p.id}`,
				label: p.levels.auto ? `${p.name} · Auto` : p.name,
				description: p.levels.auto ? 'Nebo AI picks the level' : 'Its default level (Medium)'
			},
			...EFFORTS.map((e) => ({
				value: `pack/${p.id}/${e}`,
				label: `${p.name} · ${EFFORT_LABELS[e]}`,
				description: p.levels[e] ?? 'The nearest level that is set'
			}))
		]);
	} catch {
		return [];
	}
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
