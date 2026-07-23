import { listStoreProducts, listStoreFeatured, listStoreCategories } from '$lib/api/nebo';
import { type AppItem, toAppItem } from '$lib/types/marketplace';

/**
 * Session-cached marketplace catalog — ONE fetch pathway for products, featured,
 * and categories, shared by the marketplace layout and every view. Before this,
 * each visit refired the whole fan-out (all product pages + featured +
 * categories, with the layout fetching categories again) — ~8 round-trips that
 * each traverse the tunnel on mobile. Now the first visit fetches once and tab
 * switches are instant. The local server additionally caches the upstream proxy.
 */
export interface StoreCatalog {
	items: AppItem[];
	featured: AppItem[];
	categoryOrder: string[];
	categories: { name: string; count: number }[];
}

const PAGE_SIZE = 100;
let cached: Promise<StoreCatalog> | null = null;

async function fetchCatalog(): Promise<StoreCatalog> {
	const first = (await listStoreProducts(undefined, undefined, 1, PAGE_SIZE).catch(() => ({
		products: [],
		total: 0
	}))) as { products?: unknown[]; total?: number };
	const total = Number(first?.total ?? (first?.products?.length ?? 0));
	const pages = Math.max(1, Math.ceil(total / PAGE_SIZE));
	const [rest, featuredRes, catsRes] = await Promise.all([
		Promise.all(
			Array.from({ length: pages - 1 }, (_, i) =>
				listStoreProducts(undefined, undefined, i + 2, PAGE_SIZE).catch(() => ({ products: [] }))
			)
		),
		listStoreFeatured().catch(() => ({ products: [] })),
		listStoreCategories().catch(() => ({ categories: [] }))
	]);
	const seen = new Set<string>();
	const items: AppItem[] = [];
	for (const res of [first, ...rest]) {
		for (const r of ((res as { products?: unknown[] })?.products as Record<string, unknown>[]) || []) {
			const id = String(r?.id ?? '');
			if (!id || seen.has(id)) continue;
			seen.add(id);
			items.push(toAppItem(r, items.length));
		}
	}
	const featured = (((featuredRes as { products?: unknown[] })?.products as Record<string, unknown>[]) || []).map(
		(r, i) => toAppItem(r, i)
	);
	const cats = (((catsRes as { categories?: unknown[] })?.categories as Record<string, unknown>[]) || []).map(
		(c) => ({ name: String(c.name || ''), count: Number(c.count ?? 0) })
	);
	return { items, featured, categoryOrder: cats.map((c) => c.name), categories: cats };
}

export function loadStoreCatalog(): Promise<StoreCatalog> {
	cached ??= fetchCatalog().catch((e) => {
		cached = null; // don't cache failures
		throw e;
	});
	return cached;
}
