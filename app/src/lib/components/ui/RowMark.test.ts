// The sidebar row's mark: working while a run goes, the "New reply" dot
// once a reply waits unread, nothing otherwise, and a name for screen
// readers either way.
import { describe, it, expect, beforeAll } from 'vitest';
import { render } from 'svelte/server';
import { addMessages, init } from 'svelte-i18n';
import en from '$lib/i18n/locales/en.json';
import RowMark from './RowMark.svelte';

beforeAll(() => {
	addMessages('en', en);
	init({ fallbackLocale: 'en', initialLocale: 'en' });
});

const html = (mark: 'working' | 'unread' | null) => render(RowMark, { props: { mark } }).body;

describe('RowMark', () => {
	it('shows the unread dot, named "New reply"', () => {
		const out = html('unread');
		expect(out).toContain('data-testid="row-unread"');
		expect(out).toContain('aria-label="New reply"');
		expect(out).toContain('bg-primary');
		expect(out).not.toContain('row-working');
	});

	it('shows the working pulse, named "Working", stilled under reduced motion by its class', () => {
		const out = html('working');
		expect(out).toContain('data-testid="row-working"');
		expect(out).toContain('aria-label="Working"');
		expect(out).toContain('agent-working-dot');
		expect(out).not.toContain('row-unread');
	});

	it('shows nothing when the conversation is idle and read', () => {
		const out = html(null);
		expect(out).not.toContain('row-unread');
		expect(out).not.toContain('row-working');
	});
});
