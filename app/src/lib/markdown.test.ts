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

	// Math. The web and the phone (nebo-mobile lib/theme/chat_math.dart) must
	// read the same reply the same way, so the eight forms that decide the rule
	// are a table here and a table there. A row that changes on one side without
	// changing on the other is the drift this pair exists to prevent.
	const eightForms: Array<{ form: string; src: string; expect: 'inline' | 'display' | 'text' }> = [
		{ form: 'inline math', src: 'The area is $\\pi r^2$ here.', expect: 'inline' },
		{ form: 'displayed, one line', src: 'Energy: $$E = mc^2$$ as written.', expect: 'display' },
		{ form: 'displayed, own lines', src: 'Energy:\n\n$$\nE = mc^2\n$$\n\nas written.', expect: 'display' },
		// The span may hold no second `$`, which is what keeps a pair of prices
		// out of KaTeX: `$5 and $` is not a candidate at all.
		{ form: 'two prices', src: 'Plans run $5 and $10 a month.', expect: 'text' },
		{ form: 'space after the opener', src: 'Spaces $ x$ here.', expect: 'text' },
		{ form: 'space before the closer', src: 'Spaces $x $ here.', expect: 'text' },
		{ form: 'a digit after the closer', src: 'Was $x$5 before.', expect: 'text' },
		{ form: 'a newline inside the span', src: 'Costs $5\nand $10 later.', expect: 'text' },
	];

	for (const { form, src, expect: want } of eightForms) {
		it(`${form} renders as ${want}`, () => {
			const html = parseMarkdown(src);
			if (want === 'text') {
				expect(html, form).not.toContain('katex');
			} else if (want === 'display') {
				expect(html, form).toContain('katex-display');
			} else {
				expect(html, form).toContain('class="katex"');
				expect(html, form).not.toContain('katex-display');
			}
		});
	}

	// The case the review named: with marked-katex-extension's own inline rule
	// this sentence rendered a red KaTeX error across the prose, because `$5
	// and $10, pick $` was taken as one span. Two prices stay prices and the
	// one variable is the only math in the line.
	it('leaves two prices alone and renders the one variable', () => {
		const html = parseMarkdown('Between $5 and $10, pick $x$.');
		expect(html).toContain('$5 and $10');
		expect(html).toContain('class="katex"');
		expect(html).not.toContain('katex-error');
		expect(html.match(/class="katex"/g)).toHaveLength(1);
	});

	// A dollar sign inside code is a dollar sign. Both tokenizers are anchored,
	// so marked's code rules take the backtick first.
	it('leaves inline code and fenced code untouched', () => {
		expect(parseMarkdown('Run `echo $x$` first.')).not.toContain('katex');
		expect(parseMarkdown('```\n$$\nE = mc^2\n$$\n```')).not.toContain('katex');
	});

	it('shows malformed TeX as its source rather than throwing', () => {
		let html = '';
		expect(() => (html = parseMarkdown('Bad: $\\frac{1$ here.'))).not.toThrow();
		expect(html).toContain('$\\frac{1$');
		expect(html).not.toContain('katex-error');
	});

	it('keeps gfm and breaks enabled', () => {
		expect(parseMarkdown('one\ntwo')).toContain('<br>');
		expect(parseMarkdown('~~gone~~')).toContain('<del>');
	});
});
