// What a tool row says after its label: the page's address (as a link), or
// what it searched for. `search_web` takes a list of queries; the other
// search tools take one `query`.

export interface StepMeta {
	text: string;
	href?: string;
}

export function stepMeta(request: Record<string, unknown> | undefined): StepMeta | null {
	const r = request ?? {};
	const str = (k: string) => (typeof r[k] === 'string' ? (r[k] as string) : '');
	const url = str('url');
	if (url) return { text: url, href: url };
	const queries = Array.isArray(r.queries)
		? r.queries.filter((q): q is string => typeof q === 'string' && q.trim() !== '')
		: [];
	if (queries.length) return { text: queries.join(' · ') };
	const query = str('query') || str('q');
	if (query) return { text: query };
	return null;
}
