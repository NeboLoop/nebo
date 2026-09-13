/**
 * The change text a layer update carries, parsed for display.
 *
 * ONE parser, because there is ONE change text: whatever `/api/v1/layers`
 * returns in `pending[].diff` is what the employees read in their update run,
 * so the owner and the employees must be looking at the same words. Two
 * notations reach here and both land on the same model:
 *
 *   1. A unified diff (`diff --git` / `@@` / `+` / `-`) — one section per file,
 *      hunk headers kept as meta lines so the boundaries stay visible.
 *   2. The pack loader's section notation (`## <folder>` / `### <entry>
 *      (added|changed|removed)` followed by the entry's body) — one section per
 *      entry. An added entry's body is new text, so it reads as added; a
 *      removed entry's body is gone, so it reads as removed; a changed entry
 *      arrives as its new body only, which is context, not an addition — the
 *      loader does not say what left, and this must not invent it.
 *
 * Anything else comes back as a single untitled section of context lines,
 * which is the honest rendering of text we cannot read more closely.
 */

/** What happened to the thing a section names. */
export type ChangeKind = 'added' | 'changed' | 'removed';

/** How one line of a section reads. */
export type LineKind = 'add' | 'del' | 'context' | 'meta';

export interface ChangeLine {
	kind: LineKind;
	/** The line without its notation marker, so it copies out clean. */
	text: string;
}

export interface ChangeSection {
	/** The file or entry this section is about; '' when the text names none. */
	title: string;
	/** The typed folder the entry sits in, when the notation says. */
	group?: string;
	kind?: ChangeKind;
	lines: ChangeLine[];
}

export interface ChangeSet {
	sections: ChangeSection[];
	/** True when the text was a unified diff. */
	unified: boolean;
	/** True when there was nothing to read. */
	empty: boolean;
}

const EMPTY: ChangeSet = { sections: [], unified: false, empty: true };

/** A unified diff announces itself in its first few lines. */
function looksUnified(lines: string[]): boolean {
	return lines.some(
		(l) => l.startsWith('@@ ') || l.startsWith('diff --git ') || l.startsWith('--- ') || l.startsWith('+++ ')
	);
}

/** `a/vocabulary/terms.md` → `vocabulary/terms.md`; `/dev/null` stays. */
function stripPrefix(p: string): string {
	const cut = p.replace(/^[ab]\//, '');
	return cut.split('\t')[0];
}

/** The typed folder and the file, for a path inside a pack. */
function splitPath(path: string): { group?: string; title: string } {
	const parts = path.split('/').filter(Boolean);
	if (parts.length < 2) return { title: path };
	return { group: parts.slice(0, -1).join('/'), title: parts[parts.length - 1] };
}

/**
 * `fallback` is what the pending entry itself says happened — `added` for a
 * pack that has just arrived whole, `removed` for one that is going. The
 * section notation does not repeat it, so it seeds the sections that carry no
 * marker of their own: a whole new pack reads as added, a retirement as
 * removed. A unified diff says everything itself and ignores it.
 */
export function parseChangeText(text: string, fallback?: ChangeKind): ChangeSet {
	if (!text || !text.trim()) return EMPTY;
	const lines = text.replace(/\r\n?/g, '\n').split('\n');
	return looksUnified(lines) ? parseUnified(lines) : parseSections(lines, fallback);
}

function parseUnified(lines: string[]): ChangeSet {
	const sections: ChangeSection[] = [];
	let current: ChangeSection | null = null;
	// The pack loader writes one `--- a/x` / `+++ b/x` pair per changed file and
	// no `diff --git` line, so a `---` is normally the start of the next file.
	// The exception is the `---` that follows a `diff --git`, which belongs to
	// the section that line already opened.
	let inHeader = false;

	const open = (path: string): ChangeSection => {
		const { group, title } = splitPath(path);
		const section: ChangeSection = { title, group, lines: [] };
		sections.push(section);
		return section;
	};

	for (const line of lines) {
		if (line.startsWith('diff --git ')) {
			// `diff --git a/x b/x` — the b side is the name it has now.
			const parts = line.slice('diff --git '.length).split(' ');
			current = open(stripPrefix(parts[parts.length - 1] || ''));
			current.kind = 'changed';
			inHeader = true;
			continue;
		}
		if (line.startsWith('--- ')) {
			const path = stripPrefix(line.slice(4));
			const added = path === '/dev/null';
			if (!current || !inHeader) current = open(added ? '' : path);
			else if (!current.title && !added) {
				const { group, title } = splitPath(path);
				current.title = title;
				current.group = group;
			}
			current.kind = added ? 'added' : (current.kind ?? 'changed');
			inHeader = true;
			continue;
		}
		if (line.startsWith('+++ ')) {
			const path = stripPrefix(line.slice(4));
			if (!current) current = open('');
			if (path === '/dev/null') current.kind = 'removed';
			else if (!current.title) {
				const { group, title } = splitPath(path);
				current.title = title;
				current.group = group;
			}
			continue;
		}
		if (line.startsWith('new file mode')) {
			if (current) current.kind = 'added';
			continue;
		}
		if (line.startsWith('deleted file mode')) {
			if (current) current.kind = 'removed';
			continue;
		}
		if (/^(index |old mode|similarity |rename |copy |Binary files )/.test(line)) continue;

		if (!current) current = open('');
		inHeader = false;
		if (line.startsWith('@@')) current.lines.push({ kind: 'meta', text: line });
		else if (line.startsWith('+')) current.lines.push({ kind: 'add', text: line.slice(1) });
		else if (line.startsWith('-')) current.lines.push({ kind: 'del', text: line.slice(1) });
		else if (line.startsWith('\\')) current.lines.push({ kind: 'meta', text: line.slice(1).trim() });
		else current.lines.push({ kind: 'context', text: line.startsWith(' ') ? line.slice(1) : line });
	}

	return finish(sections, true);
}

const HEADING = /^(#{1,3})\s+(.*?)(?:\s+\((added|changed|removed)\))?\s*$/;

function parseSections(lines: string[], fallback?: ChangeKind): ChangeSet {
	const sections: ChangeSection[] = [];
	let group: string | undefined;
	let current: ChangeSection | null = null;

	for (const line of lines) {
		const heading = HEADING.exec(line);
		if (heading) {
			const [, hashes, title, kind] = heading;
			// `# Name (company layer, version 2)` is the pack itself, so it ends
			// any folder the headings above it opened.
			if (hashes === '#') {
				group = undefined;
				current = { title, kind: (kind as ChangeKind | undefined) ?? fallback, lines: [] };
				sections.push(current);
				continue;
			}
			// A bare `## folder` names the typed folder the entries below sit
			// in; it is a label, not a change of its own.
			if (hashes === '##' && !kind) {
				group = title;
				current = null;
				continue;
			}
			current = {
				title,
				group: hashes === '###' ? group : undefined,
				kind: (kind as ChangeKind | undefined) ?? fallback ?? 'changed',
				lines: []
			};
			sections.push(current);
			continue;
		}
		if (!current) {
			if (!line.trim()) continue;
			current = { title: '', kind: fallback, lines: [] };
			sections.push(current);
		}
		const kind: LineKind =
			current.kind === 'added' ? 'add' : current.kind === 'removed' ? 'del' : 'context';
		current.lines.push({ kind, text: line });
	}

	return finish(sections, false);
}

/** Drop the blank run at the end of every section, and empty sections. */
function finish(sections: ChangeSection[], unified: boolean): ChangeSet {
	for (const s of sections) {
		while (s.lines.length && !s.lines[s.lines.length - 1].text.trim()) s.lines.pop();
	}
	const kept = sections.filter((s) => s.lines.length > 0 || s.title);
	return { sections: kept, unified, empty: kept.length === 0 };
}

/** How many lines the change adds and removes, for the summary line. */
export function countChanges(set: ChangeSet): { added: number; removed: number } {
	let added = 0;
	let removed = 0;
	for (const s of set.sections) {
		for (const l of s.lines) {
			if (l.kind === 'add') added++;
			else if (l.kind === 'del') removed++;
		}
	}
	return { added, removed };
}
