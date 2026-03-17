export interface AppItem {
	id: string;
	slug: string;
	code: string;
	name: string;
	author: string;
	authorVerified: boolean;
	description: string;
	rating: number;
	ratingCount: number;
	installs: number;
	category: string;
	free: boolean;
	builtForNebo: boolean;
	installed: boolean;
	iconBg: string;
	iconEmoji: string;
	type: 'skill' | 'workflow' | 'role';
	price: string;
	priceCents: number;
}

/** Returns the correct URL path for any marketplace item based on its type. */
export function itemHref(item: AppItem): string {
	switch (item.type) {
		case 'workflow': return `/marketplace/roles/${item.slug}`;
		case 'role': return `/marketplace/roles/${item.slug}`;
		default: return `/marketplace/skills/${item.slug}`;
	}
}

export const gradients = [
	'bg-gradient-to-br from-violet-500 to-indigo-600',
	'bg-gradient-to-br from-sky-400 to-blue-500',
	'bg-gradient-to-br from-emerald-400 to-teal-500',
	'bg-gradient-to-br from-amber-400 to-orange-500',
	'bg-gradient-to-br from-rose-400 to-pink-500',
	'bg-gradient-to-br from-cyan-400 to-sky-500',
	'bg-gradient-to-br from-fuchsia-400 to-purple-500',
	'bg-gradient-to-br from-lime-400 to-green-500',
	'bg-gradient-to-br from-teal-400 to-emerald-500',
	'bg-gradient-to-br from-blue-400 to-indigo-500',
	'bg-gradient-to-br from-yellow-400 to-amber-500',
	'bg-gradient-to-br from-purple-400 to-violet-500'
];

/** Format cents as a price string: 0/null -> "Get", >0 -> "$X.XX/mo" */
function formatPrice(cents: number | null | undefined): string {
	if (!cents || cents <= 0) return 'Get';
	const dollars = cents / 100;
	return `$${dollars % 1 === 0 ? dollars.toFixed(0) : dollars.toFixed(2)}/mo`;
}

export function toAppItem(raw: any, i: number): AppItem {
	const priceCents = raw.minPriceCents ?? raw.min_price_cents ?? 0;
	return {
		id: raw.id || String(i),
		slug: raw.slug || raw.id || String(i),
		code: raw.code || '',
		name: raw.name || 'Untitled',
		author: raw.authorName || (typeof raw.author === 'string' ? raw.author : raw.author?.name || raw.author?.email || ''),
		authorVerified: raw.authorVerified || false,
		description: raw.description || raw.shortDescription || '',
		rating: raw.rating || 0,
		ratingCount: raw.ratingCount || raw.reviewCount || 0,
		installs: raw.installCount || raw.installs || 0,
		category: raw.category || '',
		free: !priceCents || priceCents <= 0,
		builtForNebo: raw.builtForNebo || raw.isOfficial || false,
		installed: raw.installed || false,
		iconBg: gradients[i % gradients.length],
		iconEmoji: raw.iconEmoji || raw.icon || '\u{1F4E6}',
		type: raw.type || 'skill',
		price: formatPrice(priceCents),
		priceCents: priceCents || 0
	};
}
