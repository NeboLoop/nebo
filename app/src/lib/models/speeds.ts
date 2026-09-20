/**
 * The speeds an employee can work at — ONE source, the sellable list Janus
 * publishes (GET /v1/models, synced into the catalog and served by
 * /api/v1/models). Settings → General → MODEL and the composer chip both read
 * this; neither keeps a list of its own.
 */
import * as api from '$lib/api/nebo';

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
 * The options, Default first, then the ladder as Janus orders it. Empty when
 * the catalog has not synced — callers render nothing rather than a guess.
 */
export async function loadModelOptions(): Promise<ModelOption[]> {
	try {
		const res = (await api.listModels()) as { models?: Record<string, CatalogModel[]> };
		const janus = (res.models?.['janus'] ?? []).filter(
			(m) => m.isActive && (m.id === DEFAULT_ID || m.description)
		);
		janus.sort((a, b) => (a.id === DEFAULT_ID ? -1 : b.id === DEFAULT_ID ? 1 : 0));
		return janus.map((m) => ({
			value: m.id === DEFAULT_ID ? '' : `janus/${m.id}`,
			label: m.displayName,
			description: m.description ?? ''
		}));
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
	return options.find((o) => bare(o.value) === bare(value))?.label ?? options[0]?.label ?? '';
}
