// The one markdown pipeline. Every surface that renders model-written or
// catalog-written text parses through here, so options and link behavior can't
// drift between them.
//
// `marked` is a module singleton, so its config is process-global. It used to be
// set by whichever component happened to load first (ChatPane called
// setOptions), which meant the inbox and marketplace inherited chat's options by
// accident. Owning it in one module makes that explicit.
import { marked, Renderer } from 'marked';
import type { TokenizerAndRendererExtension, Tokens } from 'marked';
import katex from 'katex';

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

/*
 * Mathematics. The model is told (server prompt, `channel_guidance`) that
 * `$…$` is inline math and `$$…$$` a displayed equation, and that both
 * surfaces render them — this is the web's half; the phone renders the same
 * delimiters with flutter_math (nebo-mobile `lib/theme/chat_math.dart`).
 *
 * The rule below IS the phone's rule, so a reply reads the same in both
 * places. marked-katex-extension cannot express it — even with
 * `nonStandard: false` it allows a `$` inside the span and requires nothing
 * of the characters next to the delimiters, so `Between $5 and $10, pick
 * $x$.` rendered a red KaTeX error across the prose and `Spaces $ x $ here.`
 * became math. Hence our own tokenizers, with KaTeX still the renderer.
 *
 * A span is inline math when: the opening `$` is not followed by a space,
 * the closing `$` is not preceded by a space, the span holds no other `$`
 * and no newline, and the closing `$` is not followed by a digit. `$$…$$`
 * is a displayed equation, on one line or on its own lines. Inline code and
 * fenced code are untouched: both tokenizers are anchored, so marked's code
 * rules win at the backtick and a fence is consumed whole before either
 * one is offered the text.
 */

// The phone writes the "not preceded by a space" half as a lookbehind. Here
// it is unrolled into "the last character of the span is not a space", which
// is the same language and needs no lookbehind support from the webview.
const INLINE_MATH =
	/^(?:\$\$([\s\S]+?)\$\$|\$(?!\s)((?:\\\$|[^$\n])*?[^\s$])\$(?![0-9]))/;

// `$$` alone on a line opens a displayed equation; `$$` alone on a later
// line closes it.
const BLOCK_MATH = /^[ \t]*\$\$[ \t]*\r?\n([\s\S]*?)(?:\r?\n)?[ \t]*\$\$[ \t]*(?:\r?\n|$)/;

function escapeHtml(text: string): string {
	return text
		.replace(/&/g, '&amp;')
		.replace(/</g, '&lt;')
		.replace(/>/g, '&gt;')
		.replace(/"/g, '&quot;');
}

/**
 * Malformed TeX is still the model's words. KaTeX's own `throwOnError: false`
 * paints the source red mid-sentence, which reads as a fault in the app; the
 * phone shows the source in its code style instead, so the web does too.
 */
function renderMath(tex: string, display: boolean): string {
	try {
		return katex.renderToString(tex, { displayMode: display, throwOnError: true });
	} catch {
		const fence = display ? '$$' : '$';
		return `<code>${escapeHtml(fence + tex + fence)}</code>`;
	}
}

const mathBlock: TokenizerAndRendererExtension = {
	name: 'mathBlock',
	level: 'block',
	start(src: string) {
		const at = /(^|\n)[ \t]*\$\$/.exec(src);
		return at ? at.index : undefined;
	},
	tokenizer(src: string) {
		const match = BLOCK_MATH.exec(src);
		if (!match) return undefined;
		return { type: 'mathBlock', raw: match[0], text: match[1].trim() };
	},
	renderer(token: Tokens.Generic) {
		return renderMath(String(token.text), true);
	},
};

const mathInline: TokenizerAndRendererExtension = {
	name: 'mathInline',
	level: 'inline',
	start(src: string) {
		const at = src.indexOf('$');
		return at < 0 ? undefined : at;
	},
	tokenizer(src: string) {
		const match = INLINE_MATH.exec(src);
		if (!match) return undefined;
		const display = match[1] !== undefined;
		return {
			type: 'mathInline',
			raw: match[0],
			text: (display ? match[1] : match[2]).trim(),
			display,
		};
	},
	renderer(token: Tokens.Generic) {
		return renderMath(String(token.text), Boolean(token.display));
	},
};

marked.use({ extensions: [mathBlock, mathInline] });

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
