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

// Open external links in system browser instead of navigating the webview.
// NeboLoop links get a data attribute so the Markdown component can intercept
// clicks and route them through the local API (same as Settings buttons).
parser.use({
	renderer: {
		link(token) {
			const href = token.href;
			const text = this.parser.parseInline(token.tokens || []);
			const title = token.title ? ` title="${token.title}"` : '';
			if (href.startsWith('http://') || href.startsWith('https://')) {
				// NeboLoop links → open via backend API (Wails webview can't
				// open external URLs directly with target="_blank")
				try {
					const url = new URL(href);
					if (url.hostname === 'neboloop.com' || url.hostname.endsWith('.neboloop.com')) {
						return `<a href="#" data-neboloop-path="${url.pathname}" class="link link-primary cursor-pointer">${text}</a>`;
					}
				} catch {
					// invalid URL — fall through to default
				}
				return `<a href="${href}"${title} target="_blank" rel="noopener noreferrer">${text}</a>`;
			}
			return `<a href="${href}"${title}>${text}</a>`;
		}
	}
});

export function parseMarkdown(content: string): string {
	return parser.parse(content || '') as string;
}
