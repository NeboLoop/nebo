// Finds English text written straight into a Svelte template instead of going
// through the i18n layer: text between tags and the user-visible attributes
// (placeholder, title, aria-label, alt, label). Expressions (`{...}`), script
// and style blocks, comments and <code>/<pre> contents are skipped.

export interface RawText {
	line: number;
	text: string;
	kind: 'text' | 'attr';
}

const VISIBLE_ATTRS = new Set(['placeholder', 'title', 'aria-label', 'alt', 'label']);
// Text inside these elements is never UI copy.
const SKIP_ELEMENTS = new Set(['script', 'style', 'code', 'pre', 'kbd', 'svg']);

/** A run of two or more letters (any script counts as "words" only when Latin). */
const WORDISH = /[A-Za-z]{2,}/;
/** HTML entities (&times; &middot; …) are glyphs, not words. */
const ENTITY = /&[a-zA-Z]+;|&#\d+;/g;

function lineAt(src: string, index: number): number {
	let line = 1;
	for (let i = 0; i < index && i < src.length; i++) if (src.charCodeAt(i) === 10) line++;
	return line;
}

/** Skips a `{...}` expression starting at `i` (src[i] === '{'); returns the index after it. */
function skipExpression(src: string, i: number): number {
	let depth = 0;
	let quote: string | null = null;
	for (; i < src.length; i++) {
		const c = src[i];
		if (quote) {
			if (c === '\\') i++;
			else if (c === quote) quote = null;
			continue;
		}
		if (c === '"' || c === "'" || c === '`') quote = c;
		else if (c === '{') depth++;
		else if (c === '}') {
			depth--;
			if (depth === 0) return i + 1;
		}
	}
	return src.length;
}

/** Blanks <script> and <style> blocks, keeping their newlines so line numbers hold. */
function blankBlocks(src: string): string {
	return src.replace(/<(script|style)\b[\s\S]*?<\/\1>/g, (m) => m.replace(/[^\n]/g, ' '));
}

export function findRawText(source: string): RawText[] {
	const src = blankBlocks(source);
	const out: RawText[] = [];
	let i = 0;
	let text = '';
	let textStart = 0;
	const skipStack: string[] = [];

	const flushText = () => {
		const s = text.replace(ENTITY, ' ').replace(/\s+/g, ' ').trim();
		if (skipStack.length === 0 && WORDISH.test(s)) {
			out.push({ line: lineAt(src, textStart), text: s, kind: 'text' });
		}
		text = '';
	};

	while (i < src.length) {
		if (src.startsWith('<!--', i)) {
			flushText();
			const end = src.indexOf('-->', i);
			i = end < 0 ? src.length : end + 3;
			continue;
		}
		const c = src[i];
		if (c === '{') {
			// An expression splits text but its contents are not copy.
			text += ' ';
			i = skipExpression(src, i);
			continue;
		}
		if (c === '<' && /[A-Za-z/!]/.test(src[i + 1] ?? '')) {
			flushText();
			// Parse the tag.
			let j = i + 1;
			const closing = src[j] === '/';
			if (closing) j++;
			const nameMatch = /^[A-Za-z][\w:.-]*/.exec(src.slice(j));
			const name = nameMatch ? nameMatch[0] : '';
			j += name.length;
			const lower = name.toLowerCase();
			// Attributes.
			let selfClosing = false;
			while (j < src.length && src[j] !== '>') {
				if (src[j] === '{') {
					j = skipExpression(src, j);
					continue;
				}
				if (src[j] === '/' && src[j + 1] === '>') {
					selfClosing = true;
					j++;
					continue;
				}
				const attr = /^([A-Za-z_:][\w:.-]*)\s*=\s*(["'])/.exec(src.slice(j));
				if (attr) {
					const attrName = attr[1];
					const q = attr[2];
					const valStart = j + attr[0].length;
					let k = valStart;
					let value = '';
					while (k < src.length && src[k] !== q) {
						if (src[k] === '{') {
							value += ' ';
							k = skipExpression(src, k);
							continue;
						}
						value += src[k];
						k++;
					}
					if (
						!closing &&
						skipStack.length === 0 &&
						VISIBLE_ATTRS.has(attrName.toLowerCase()) &&
						WORDISH.test(value.replace(ENTITY, ' '))
					) {
						out.push({ line: lineAt(src, j), text: value.trim(), kind: 'attr' });
					}
					j = k + 1;
					continue;
				}
				j++;
			}
			if (SKIP_ELEMENTS.has(lower)) {
				if (closing) {
					const at = skipStack.lastIndexOf(lower);
					if (at >= 0) skipStack.splice(at, 1);
				} else if (!selfClosing) {
					skipStack.push(lower);
				}
			}
			i = j + 1;
			textStart = i;
			continue;
		}
		if (text === '') textStart = i;
		text += c;
		i++;
	}
	flushText();
	return out;
}
