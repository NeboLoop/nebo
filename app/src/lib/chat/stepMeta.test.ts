import { describe, expect, it } from 'vitest';
import { stepMeta } from './stepMeta';

describe('stepMeta', () => {
	it('shows what a web search looked for, from its list of queries', () => {
		expect(stepMeta({ queries: ['vat rate uk', 'vat rate uk 2026'] })).toEqual({
			text: 'vat rate uk · vat rate uk 2026',
		});
	});

	it('shows a single query and a page address', () => {
		expect(stepMeta({ query: 'march invoices' })).toEqual({ text: 'march invoices' });
		expect(stepMeta({ url: 'https://example.com/a' })).toEqual({
			text: 'https://example.com/a',
			href: 'https://example.com/a',
		});
	});

	it('says nothing when the call has none of those', () => {
		expect(stepMeta({ queries: ['  '] })).toBeNull();
		expect(stepMeta(undefined)).toBeNull();
	});
});
