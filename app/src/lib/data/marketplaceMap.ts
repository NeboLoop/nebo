import { listStoreMarketplaceMap } from '$lib/api/nebo';

/**
 * The curated marketplace reorganization map — the same single-source
 * Employees / Tools / Collections presentation layer the NeboAI website uses,
 * proxied (and cached) by the local server at /store/marketplace-map.
 * Keyed by artifact Code; joined against the catalog client-side.
 */
export type MapEntry = { d: 'E' | 'T' | 'C'; dept?: string; role?: string; tc?: string };

export interface MarketplaceMap {
	departments: string[];
	toolCategories: string[];
	entries: Record<string, MapEntry>;
	responsibilities: Record<string, string[]>;
}

let cached: Promise<MarketplaceMap | null> | null = null;

/** Fetch the map once per session (server side already caches for 10 min). */
export function loadMarketplaceMap(): Promise<MarketplaceMap | null> {
	cached ??= (listStoreMarketplaceMap() as Promise<unknown>)
		.then((raw) => {
			const m = raw as Partial<MarketplaceMap> | null;
			if (!m || typeof m !== 'object' || !m.entries) return null;
			return {
				departments: m.departments ?? [],
				toolCategories: m.toolCategories ?? [],
				entries: m.entries,
				responsibilities: m.responsibilities ?? {}
			};
		})
		.catch(() => null);
	return cached;
}

/** Website-compatible slugs — round-trip exactly against the map's names. */
export function mapSlugify(name: string): string {
	return name
		.toLowerCase()
		.replace(/&/g, ' ')
		.replace(/[^a-z0-9]+/g, '-')
		.replace(/^-+|-+$/g, '');
}

export function deptFromSlug(map: MarketplaceMap, slug: string): string {
	return map.departments.find((d) => mapSlugify(d) === slug) ?? '';
}

export function toolCatFromSlug(map: MarketplaceMap, slug: string): string {
	return map.toolCategories.find((c) => mapSlugify(c) === slug) ?? '';
}
