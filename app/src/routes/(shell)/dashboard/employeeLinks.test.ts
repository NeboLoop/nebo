import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

// Workforce → Employees: in the card grid AND the list, an employee's name and
// avatar are real links to /{id}, which lands where the sidebar's first click
// does (the /[agentId] route's load decides: latest conversation, matters
// list, or a new chat). The page loads its data on mount, so the markup is
// checked at the source.
const src = readFileSync(fileURLToPath(new URL('./+page.svelte', import.meta.url)), 'utf8');

function block(start: string, end: string): string {
	const from = src.indexOf(start);
	const to = src.indexOf(end, from + start.length);
	expect(from).toBeGreaterThan(-1);
	expect(to).toBeGreaterThan(from);
	return src.slice(from, to);
}

const LIST_START = '{:else}\n            <div class="rounded-2xl border border-base-content/15';

describe('Workforce employee links', () => {
	const link = 'href={withBase(`/${e.id}`)}';
	for (const [view, start, end] of [
		['grid', "{:else if view === 'grid'}", LIST_START],
		['list', LIST_START, '<!-- How much work happened']
	] as const) {
		it(`opens the employee from its name and avatar in the ${view} view`, () => {
			const markup = block(start, end);
			const anchors = markup.split('<a ').slice(1).filter((a) => a.includes(link));
			expect(anchors.some((a) => a.includes('<AgentAvatar'))).toBe(true);
			expect(anchors.some((a) => a.includes('>{e.name}</a>'))).toBe(true);
			// The existing actions stay.
			expect(markup).toContain('openChat(e)');
			expect(markup).toContain('openRuns(e.id)');
		});
	}
});
