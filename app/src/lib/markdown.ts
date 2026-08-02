// The one markdown pipeline. Every surface that renders model-written or
// catalog-written text parses through here, so options and link behavior can't
// drift between them.
//
// `marked` is a module singleton, so its config is process-global. It used to be
// set by whichever component happened to load first (ChatPane called
// setOptions), which meant the inbox and marketplace inherited chat's options by
// accident. Owning it in one module makes that explicit.
import { marked, Renderer } from 'marked';

/**
 * marked does NO url sanitization — it dropped `sanitize` years ago and renders
 * whatever href the source contains, `javascript:` included. Markdown here is
 * model-written and can be steered by any page an agent reads, so an injected
 * `[click me](javascript:…)` would otherwise render as a live link that runs in
 * the app's own origin (on a tunneled bot, the bot's origin) when clicked.
 *
 * Allow the schemes a link in a reply legitimately uses; anything scheme-like
 * that is not on the list renders as plain text instead of an anchor. Relative
 * hrefs and fragments have no scheme and stay linkable.
 */
const SAFE_SCHEMES = ['http', 'https', 'mailto'];

function isSafeHref(href: string): boolean {
	// Strip control characters and whitespace first — `java\tscript:` and
	// leading-newline tricks are how this check normally gets bypassed.
	const cleaned = href.replace(/[\u0000-\u0020]/g, '').toLowerCase();
	const scheme = /^([a-z][a-z0-9+.-]*):/.exec(cleaned);
	return !scheme || SAFE_SCHEMES.includes(scheme[1]);
}

marked.use({
	gfm: true,
	// Chat text is written with single newlines and means them.
	breaks: true,
	renderer: {
		link(token) {
			if (!isSafeHref(token.href)) {
				// Keep the words, drop the link.
				return this.parser.parseInline(token.tokens);
			}
			// Delegate the anchor itself to marked so escaping stays its job —
			// this only adds the target.
			const html = Renderer.prototype.link.call(this, token);
			if (typeof html !== 'string') return html;
			// A link in a reply points somewhere else. Same-tab navigation throws
			// the session away — worst on a tunneled bot, where the app IS the
			// tab — and under Tauri target=_blank is what hands the URL to the
			// system browser instead of replacing the app window.
			// noopener/noreferrer: the opened page must not reach back via
			// window.opener.
			return html.replace('<a ', '<a target="_blank" rel="noopener noreferrer" ');
		},
	},
});

/** Render markdown to HTML with the app's shared configuration. */
export function parseMarkdown(src: string): string {
	if (!src) return '';
	return marked.parse(src, { async: false }) as string;
}
