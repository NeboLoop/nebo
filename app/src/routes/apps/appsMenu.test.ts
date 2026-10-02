import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

// The Apps page's ⋮ menu: Open App and Settings. Every settings section lives
// inside the employee's settings; the old per-section links duplicated them,
// and "Runs" pointed at /{id}/settings/runs, a section that never existed.
const src = readFileSync(fileURLToPath(new URL('./+page.svelte', import.meta.url)), 'utf8');

describe('Apps ⋮ menu', () => {
	it('offers only Open App and Settings', () => {
		const menu = src.slice(src.indexOf('const menuItems = ['), src.indexOf('];', src.indexOf('const menuItems = [')));
		const ids = [...menu.matchAll(/id: '([^']+)'/g)].map((m) => m[1]);
		expect(ids).toEqual(['open', 'settings']);
	});

	it('opens Settings on the general section', () => {
		expect(src).toContain('goto(`/${app.id}/settings/general`)');
		expect(src).not.toContain('/settings/${section}');
	});
});
