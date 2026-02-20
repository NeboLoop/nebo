/**
 * Single configured Marked instance for the entire app.
 * Extensions are applied once at module load — not per component instance.
 */
import { Marked } from 'marked';
import { embedExtension } from './markdown-embeds';

const parser = new Marked();

parser.setOptions({
	breaks: true,
	gfm: true
});

parser.use(embedExtension());

// Open external links in system browser instead of navigating the webview
parser.use({
	renderer: {
		link(token) {
			const href = token.href;
			const text = this.parser.parseInline(token.tokens || []);
			const title = token.title ? ` title="${token.title}"` : '';
			if (href.startsWith('http://') || href.startsWith('https://')) {
				return `<a href="${href}"${title} target="_blank" rel="noopener noreferrer">${text}</a>`;
			}
			return `<a href="${href}"${title}>${text}</a>`;
		}
	}
});

export function parseMarkdown(content: string): string {
	return parser.parse(content || '') as string;
}
