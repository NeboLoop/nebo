// No English written straight into a template: every word a person reads goes
// through $t, so all 25 languages get it. Brand names, code and example values
// are not copy; they are listed below, each for a reason.
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { describe, expect, it } from 'vitest';
import { findRawText } from './rawText';

const SRC = join(__dirname, '..', '..');

/** Never translated: the product's own names. */
const BRAND = /\b(NeboAI|Nebo AI|Nebo)\b/g;

/** Text that is not copy: code, file and key names, example values, units. */
const ALLOWED = new Set([
	'null', // PrettyJson renders JSON's own literal
	'bash', // a code block's language tag
	'curl',
	'Python', // code-sample tabs
	'agent.json',
	'finance.ap.invoice_mailbox', // example key paths in a placeholder
	'invoice_mailbox',
	'America/Denver', // an IANA time zone, as typed
	'NEBO-XXXX-XXXX', // the shape of an install code
	'support@neboai.com',
	'neboai.com/app/manage/phone',
	'AM',
	'PM',
	'(Esc)' // a key name beside a translated label
]);

function svelteFiles(dir: string, out: string[] = []): string[] {
	for (const name of readdirSync(dir)) {
		const path = join(dir, name);
		if (statSync(path).isDirectory()) svelteFiles(path, out);
		else if (name.endsWith('.svelte')) out.push(path);
	}
	return out;
}

function isCopy(text: string): boolean {
	if (ALLOWED.has(text)) return false;
	return /[A-Za-z]{2,}/.test(text.replace(BRAND, ''));
}

describe('templates', () => {
	it('the scanner finds text and visible attributes, and skips expressions, code and scripts', () => {
		const found = findRawText(
			`<script>const a = '<b>Hidden</b>';</script>
<p>Hello there</p>
<input placeholder="Type here" class="x" />
<p>{$t('a.b')} &times;</p>
<code>npm run build</code>
<button title={$t('c.d')}>{label}</button>`
		);
		expect(found.map((f) => f.text)).toEqual(['Hello there', 'Type here']);
	});

	it('carry no raw English', () => {
		const hits: string[] = [];
		for (const file of svelteFiles(SRC)) {
			for (const hit of findRawText(readFileSync(file, 'utf8'))) {
				if (isCopy(hit.text)) hits.push(`${relative(SRC, file)}:${hit.line} ${hit.kind} "${hit.text}"`);
			}
		}
		expect(hits).toEqual([]);
	});
});
