import { describe, expect, it } from 'vitest';
import { shareEntries } from './shareMenu';

describe('shareEntries', () => {
	const entry = { label: 'Make it an app', say: 'Make this design a Nebo app.' };

	it("lists the app's entries in a chat the owner can send in", () => {
		expect(shareEntries({ shareMenu: [entry] }, true)).toEqual([entry]);
	});

	it('lists nothing when the app declares none, or from a bot without the field', () => {
		expect(shareEntries({ shareMenu: [] }, true)).toEqual([]);
		expect(shareEntries({}, true)).toEqual([]);
		expect(shareEntries(null, true)).toEqual([]);
	});

	it('lists nothing in a chat the owner cannot send in', () => {
		expect(shareEntries({ shareMenu: [entry] }, false)).toEqual([]);
	});

	it('skips an entry with nothing to show or say', () => {
		expect(shareEntries({ shareMenu: [{ label: ' ', say: 'x' }, entry, { label: 'Export', say: '' }] }, true)).toEqual([entry]);
	});
});
