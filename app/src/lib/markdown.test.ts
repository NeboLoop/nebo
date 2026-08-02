import { describe, expect, it } from 'vitest';
import { parseMarkdown } from './markdown';

describe('parseMarkdown', () => {
	// A link in a reply points off the app. Same-tab navigation throws the
	// session away — worst on a tunneled bot, where the app IS the tab.
	it('opens explicit markdown links in a new tab', () => {
		const html = parseMarkdown('The article is live at [the post](https://example.com/post/)');
		expect(html).toContain('target="_blank"');
		expect(html).toContain('rel="noopener noreferrer"');
		expect(html).toContain('href="https://example.com/post/"');
	});

	// The reported case: the model wrote a bare URL, gfm autolinked it.
	it('opens autolinked bare URLs in a new tab', () => {
		const html = parseMarkdown('now live at https://wordpress.examples.neboai.com/i-hired-my-first/');
		expect(html).toContain('target="_blank"');
		expect(html).toContain('rel="noopener noreferrer"');
	});

	// marked itself does no url sanitization, so this guard is ours. Markdown
	// here is model-written and steerable by any page an agent reads.
	it('renders dangerous hrefs as text, not links', () => {
		const html = parseMarkdown('[click me](javascript:alert(1))');
		expect(html).not.toContain('javascript:');
		expect(html).not.toContain('<a ');
		expect(html).toContain('click me');
	});

	it('is not fooled by control characters in the scheme', () => {
		for (const href of ['java\tscript:alert(1)', ' javascript:alert(1)', 'JaVaScRiPt:alert(1)']) {
			const html = parseMarkdown(`[x](${href})`);
			expect(html, href).not.toContain('<a ');
		}
	});

	it('leaves relative links and fragments alone', () => {
		expect(parseMarkdown('[settings](/settings/account)')).toContain('href="/settings/account"');
		expect(parseMarkdown('[top](#top)')).toContain('href="#top"');
	});

	it('keeps gfm and breaks enabled', () => {
		expect(parseMarkdown('one\ntwo')).toContain('<br>');
		expect(parseMarkdown('~~gone~~')).toContain('<del>');
	});
});
