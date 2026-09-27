import { describe, it, expect } from 'vitest';
import { offerMatchingRename } from './botName';

describe('offerMatchingRename', () => {
	it('offers the other rename when the two names matched before', () => {
		expect(offerMatchingRename('Nanna', 'Nanna', 'Ada')).toBe(true);
		expect(offerMatchingRename(' Nanna ', 'Nanna', 'Ada')).toBe(true);
	});

	it('leaves names the owner already chose apart alone', () => {
		expect(offerMatchingRename('Nanna', 'Miller Dental', 'Ada')).toBe(false);
	});

	it('offers nothing when nothing changed or a name is blank', () => {
		expect(offerMatchingRename('Nanna', 'Nanna', 'Nanna')).toBe(false);
		expect(offerMatchingRename('Nanna', 'Nanna', '  ')).toBe(false);
		expect(offerMatchingRename('', '', 'Ada')).toBe(false);
	});
});
