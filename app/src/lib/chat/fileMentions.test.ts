import { describe, expect, it } from 'vitest';
import { fileMention, locateFileMention } from './fileMentions';
import { parseMarkdown } from '$lib/markdown';
import chatPane from '$lib/components/chat/ChatPane.svelte?raw';

// The rule, as a table. The phone (nebo-mobile test/file_mentions_test.dart)
// keeps the same rows: a row that changes here without changing there is a
// reply that links on one surface and not the other.
const rows: Array<{ raw: string; path: string | null }> = [
	{ raw: '/data/files/BUG/bug-report-quickbooks-docnumber.md', path: '/data/files/BUG/bug-report-quickbooks-docnumber.md' },
	{ raw: '~/Library/Application Support/Nebo/files/report.pdf', path: '~/Library/Application Support/Nebo/files/report.pdf' },
	{ raw: '/Users/al/.local/share/nebo/files/q3/summary.final.xlsx', path: '/Users/al/.local/share/nebo/files/q3/summary.final.xlsx' },
	{ raw: 'file:///data/files/My%20Report.docx', path: '/data/files/My Report.docx' },
	{ raw: '  /data/files/notes.txt  ', path: '/data/files/notes.txt' },
	// The owner's workspace on a computer: ~/NeboAI, on every platform.
	{ raw: '~/NeboAI/Media/outputs/demo/final.mp4', path: '~/NeboAI/Media/outputs/demo/final.mp4' },
	{ raw: '/Users/al/NeboAI/report.md', path: '/Users/al/NeboAI/report.md' },
	{ raw: 'C:\\Users\\al\\NeboAI\\Q3 plan.docx', path: 'C:\\Users\\al\\NeboAI\\Q3 plan.docx' },
	{ raw: 'C:\\Windows\\win.ini', path: null },
	// A folder, with or without its slash: no file name, no extension.
	{ raw: '/data/files/BUG/', path: null },
	{ raw: '/data/files/BUG', path: null },
	// A dot file is not a file name with an extension.
	{ raw: '/data/files/.env', path: null },
	// Not in a files folder: not one of the bot's files.
	{ raw: '/etc/hosts.conf', path: null },
	{ raw: '/usr/local/bin/run.sh', path: null },
	// Relative paths, web addresses, other schemes, code.
	{ raw: 'files/report.md', path: null },
	{ raw: 'https://example.com/files/report.md', path: null },
	{ raw: 'file://server/files/report.md', path: null },
	{ raw: 'npm run build', path: null },
	{ raw: 'const x = a.b', path: null },
];

describe('fileMention', () => {
	for (const { raw, path } of rows) {
		it(`${JSON.stringify(raw)} → ${path === null ? 'text' : 'link'}`, () => {
			expect(fileMention(raw)).toBe(path);
		});
	}
});

describe('parseMarkdown with filePaths', () => {
	const linked = (html: string) => [...html.matchAll(/data-file-path="([^"]*)"/g)].map((m) => m[1]);

	it('links a path in an inline code span, keeping the code', () => {
		const html = parseMarkdown("Done — it's in `/data/files/BUG/bug-report-quickbooks-docnumber.md`.", { filePaths: true });
		expect(linked(html)).toEqual(['/data/files/BUG/bug-report-quickbooks-docnumber.md']);
		expect(html).toContain('class="link link-primary"><code>/data/files/BUG/bug-report-quickbooks-docnumber.md</code></a>');
	});

	it('links a bare path in text and leaves the full stop outside', () => {
		const html = parseMarkdown('Saved to /data/files/q3_summary.csv. Open it any time.', { filePaths: true });
		expect(linked(html)).toEqual(['/data/files/q3_summary.csv']);
		expect(html).toContain('/data/files/q3_summary.csv</a>.');
	});

	it('links a home path and a file:// link', () => {
		const html = parseMarkdown('See ~/Nebo/files/a.md and (file:///data/files/b%20c.md)', { filePaths: true });
		expect(linked(html)).toEqual(['~/Nebo/files/a.md', '/data/files/b c.md']);
	});

	it('never touches a fenced code block', () => {
		const html = parseMarkdown('```\ncat /data/files/BUG/report.md\n```\n\n```\n/data/files/BUG/report.md\n```', { filePaths: true });
		expect(linked(html)).toEqual([]);
	});

	it('leaves folders, paths outside files, words and web addresses alone', () => {
		const html = parseMarkdown(
			'Folder `/data/files/BUG/`, system `/etc/hosts.conf`, and/or/files/x.md, https://example.com/files/report.md',
			{ filePaths: true }
		);
		expect(linked(html)).toEqual([]);
	});

	it('escapes what it writes into the page', () => {
		const html = parseMarkdown('`/data/files/a"b<i>.md`', { filePaths: true });
		expect(linked(html)).toEqual([]);
		const quoted = parseMarkdown('`/data/files/a&"b.md`', { filePaths: true });
		expect(quoted).toContain('data-file-path="/data/files/a&amp;&quot;b.md"');
	});

	it('is off unless asked for', () => {
		expect(linked(parseMarkdown('`/data/files/BUG/report.md`'))).toEqual([]);
	});
});

describe('tapping a linked path', () => {
	const httpError = (status: number) => Object.assign(new Error(`HTTP ${status}`), { response: { status } });

	it('opens the file the bot found, by its files URL', async () => {
		const asked: string[] = [];
		const target = await locateFileMention('/data/files/BUG/report.md', async (path) => {
			asked.push(path);
			return { url: '/api/v1/files/BUG/report.md', filename: 'report.md', directory: false };
		});
		expect(asked).toEqual(['/data/files/BUG/report.md']);
		expect(target).toEqual({ kind: 'file', url: '/api/v1/files/BUG/report.md', filename: 'report.md' });
	});

	it('opens nothing for a folder', async () => {
		const target = await locateFileMention('/data/files/site.d', async () => ({ url: '', filename: 'site.d', directory: true }));
		expect(target).toEqual({ kind: 'folder' });
	});

	it('says a missing (or refused) file is gone', async () => {
		expect(await locateFileMention('/data/files/x.md', async () => { throw httpError(404); })).toEqual({ kind: 'gone' });
	});

	it('reports any other failure with its status', async () => {
		expect(await locateFileMention('/data/files/x.md', async () => { throw httpError(500); })).toEqual({ kind: 'failed', status: 500 });
		expect(await locateFileMention('/data/files/x.md', async () => { throw new Error('offline'); })).toEqual({ kind: 'failed', status: 0 });
	});

	// The chat's half, as written: the reply's prose asks for linked paths,
	// a click on one goes to openFilePath before the image lightbox can take
	// its href, and a found file opens through openArtifact — the Work
	// panel's one door, the same one a document card uses.
	it('routes a click on a linked path to the Work panel', () => {
		expect(chatPane).toContain('renderMarkdown(block.text, { filePaths: true })');
		const click = chatPane.slice(chatPane.indexOf('function handleWorkMentionClick'), chatPane.indexOf('// Preview ↔ Code toggle'));
		expect(click.indexOf("closest?.('[data-file-path]')")).toBeGreaterThan(-1);
		expect(click.indexOf("closest?.('[data-file-path]')")).toBeLessThan(click.indexOf("closest?.('a')"));
		const open = chatPane.slice(chatPane.indexOf('async function openFilePath'), chatPane.indexOf('function openArtifact('));
		expect(open).toContain('locateFileMention(path, locateWorkFile)');
		expect(open).toContain("$t('chat.fileGone')");
		expect(open).toContain('openArtifact(id)');
	});
});
