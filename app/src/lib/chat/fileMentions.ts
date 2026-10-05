/**
 * A file an employee's reply names by its path — "Done — it's in
 * `/data/files/BUG/bug-report.md`." — becomes a link that opens the file in
 * the Work panel, the way the reply's document card does.
 *
 * What counts is decided here and nowhere else on the web; the phone has the
 * same rule (nebo-mobile `lib/api/file_mentions.dart`), and the two test
 * tables (`fileMentions.test.ts`, `file_mentions_test.dart`) are kept row for
 * row. The rule:
 *
 * - an absolute path (`/…`), a home path (`~/…`), or a `file://` link to an
 *   absolute path (percent-decoded);
 * - inside a `files` folder — every bot keeps what it makes under
 *   `<its data folder>/files` (`/data/files/…` on a cloud bot,
 *   `…/Nebo/files/…` on a computer), so a path with no `/files/` segment
 *   cannot be one of its files and stays text;
 * - ending in a file name with an extension (`report.md`, not `BUG/` or
 *   `.env`), so folders and code snippets don't light up.
 *
 * Lighting up is only a guess about the text. The bot decides what opens:
 * `GET /api/v1/work/locate` finds the path inside its files root, refuses
 * anything outside it (`..` and symlinks included), and says when a path is
 * a folder or is gone.
 */

const FILES_SEGMENT = /\/files\//;
const FILE_NAME = /^[^/.][^/]*\.[A-Za-z0-9]{1,10}$/;

/** The path a mention names (absolute or `~/…`), or null when it is not a
 *  file in a bot's files. `raw` is the whole of an inline code span or a
 *  bare token from the text. */
export function fileMention(raw: string): string | null {
	let path = raw.trim();
	if (/[\n`<>]/.test(path)) return null;
	if (path.startsWith('file://')) {
		try {
			path = decodeURIComponent(path.slice('file://'.length));
		} catch {
			return null;
		}
		if (!path.startsWith('/')) return null;
	}
	if (!path.startsWith('/') && !path.startsWith('~/')) return null;
	if (!FILES_SEGMENT.test(path)) return null;
	const name = path.slice(path.lastIndexOf('/') + 1);
	return FILE_NAME.test(name) ? path : null;
}

/** A bare path in running text: no spaces, and the sentence's own closing
 *  punctuation (or the end of an emphasis) is not part of it. Optionally led by the character before it
 *  (a space or an opening bracket/quote), which the tokenizer keeps as text. */
const BARE = /^([\s(["']?)((?:file:\/\/|~)?\/[^\s<>()[\]"'`]+)/;
const TRAILING = /[.,;:!?*_]+$/;

/** Split a bare path off the start of `src`: the character before it, the
 *  path as written, and the path it names — or null. */
export function bareFileMention(src: string): { lead: string; text: string; path: string } | null {
	const m = BARE.exec(src);
	if (!m) return null;
	const text = m[2].replace(TRAILING, '');
	const path = fileMention(text);
	return path ? { lead: m[1], text, path } : null;
}

/** Where in `src` a bare path may begin: just after a space or an opening
 *  bracket/quote — never inside a word or a web address. */
export function bareFileMentionStart(src: string): number | undefined {
	const at = /[\s(["'](?=(?:file:\/\/|~)?\/)/.exec(src);
	return at ? at.index : undefined;
}

/** What tapping a linked path comes to: the file to open in the viewer, a
 *  folder (nothing opens), a file that is gone, or a failure with its
 *  HTTP status (0 when the bot could not be reached). */
export type FileMentionTarget =
	| { kind: 'file'; url: string; filename: string }
	| { kind: 'folder' }
	| { kind: 'gone' }
	| { kind: 'failed'; status: number };

/** Ask the bot (`locate` — the generated `locateWorkFile`) where the file a
 *  linked path names is. */
export async function locateFileMention(
	path: string,
	locate: (path: string) => Promise<{ url: string; filename: string; directory: boolean }>
): Promise<FileMentionTarget> {
	try {
		const found = await locate(path);
		return found.directory ? { kind: 'folder' } : { kind: 'file', url: found.url, filename: found.filename };
	} catch (err) {
		const status = (err as { response?: { status?: number } })?.response?.status ?? 0;
		return status === 404 ? { kind: 'gone' } : { kind: 'failed', status };
	}
}
