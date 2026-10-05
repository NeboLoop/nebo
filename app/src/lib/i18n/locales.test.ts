// Every locale carries every English key, and every message is a valid ICU
// message whose placeholders are the English ones: a missing key falls back to
// English on screen, and a misspelt {name} or a broken plural fails at render.
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { parse, TYPE, type MessageFormatElement } from '@formatjs/icu-messageformat-parser';
import { LANGUAGES } from './languages';

const DIR = join(__dirname, 'locales');

type Tree = { [key: string]: string | Tree };

function flatten(tree: Tree, prefix = '', out: Record<string, string> = {}): Record<string, string> {
	for (const [k, v] of Object.entries(tree)) {
		const key = prefix ? `${prefix}.${k}` : k;
		if (typeof v === 'string') out[key] = v;
		else flatten(v, key, out);
	}
	return out;
}

function load(code: string): Record<string, string> {
	return flatten(JSON.parse(readFileSync(join(DIR, `${code}.json`), 'utf8')));
}

/** The placeholders a message uses: arguments, plurals, selects and tags. */
function placeholders(elements: MessageFormatElement[], out = new Set<string>()): Set<string> {
	for (const el of elements) {
		switch (el.type) {
			case TYPE.argument:
			case TYPE.number:
			case TYPE.date:
			case TYPE.time:
				out.add(el.value);
				break;
			case TYPE.plural:
			case TYPE.select:
				out.add(el.value);
				for (const opt of Object.values(el.options)) placeholders(opt.value, out);
				break;
			case TYPE.tag:
				out.add(`<${el.value}>`);
				placeholders(el.children, out);
				break;
		}
	}
	return out;
}

/** Plurals and selects keep the `other` branch every locale needs. */
function missingOther(elements: MessageFormatElement[]): boolean {
	return elements.some((el) => {
		if (el.type === TYPE.plural || el.type === TYPE.select) {
			if (!('other' in el.options)) return true;
			return Object.values(el.options).some((o) => missingOther(o.value));
		}
		if (el.type === TYPE.tag) return missingOther(el.children);
		return false;
	});
}

const en = load('en');
const files = readdirSync(DIR)
	.filter((f) => f.endsWith('.json'))
	.map((f) => f.replace(/\.json$/, ''))
	.sort();

it('the language list and the locale files are the same 25 languages', () => {
	expect(files).toEqual(LANGUAGES.map((l) => l.code).sort());
	expect(LANGUAGES).toHaveLength(25);
});

describe.each(files)('locale %s', (code) => {
	const messages = load(code);

	it('has every English key and no others', () => {
		const missing = Object.keys(en).filter((k) => !(k in messages));
		const extra = Object.keys(messages).filter((k) => !(k in en));
		expect({ missing, extra }).toEqual({ missing: [], extra: [] });
	});

	it('sends people to the phone page at its current address', () => {
		for (const key of ['agentSettings.phoneAttachTitle', 'agentSettings.phoneAttachDesc', 'agentSettings.phoneNoFreeNumbers']) {
			expect(messages[key], key).toBeTruthy();
		}
		const stale = Object.entries(messages).filter(([, v]) => v.includes('neboai.com/manage/phone'));
		expect(stale.map(([k]) => k)).toEqual([]);
	});

	it('has valid messages with the English placeholders', () => {
		const problems: string[] = [];
		for (const [key, source] of Object.entries(en)) {
			const text = messages[key];
			if (typeof text !== 'string') continue;
			let want: Set<string>;
			try {
				want = placeholders(parse(source, { ignoreTag: false }));
			} catch {
				problems.push(`${key}: the English message is not valid ICU`);
				continue;
			}
			try {
				const ast = parse(text, { ignoreTag: false });
				const got = placeholders(ast);
				const lost = [...want].filter((p) => !got.has(p));
				const added = [...got].filter((p) => !want.has(p));
				if (lost.length || added.length) {
					problems.push(`${key}: placeholders ${[...got].join(',') || '(none)'} ≠ ${[...want].join(',') || '(none)'}`);
				}
				if (missingOther(ast)) problems.push(`${key}: a plural or select has no "other"`);
			} catch (e) {
				problems.push(`${key}: not valid ICU (${(e as Error).message})`);
			}
		}
		expect(problems).toEqual([]);
	});
});
