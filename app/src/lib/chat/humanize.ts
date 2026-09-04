/**
 * Owner-facing tool names — mirrors `crates/tools/src/humanize.rs` so a
 * reloaded thread reads the same as the live stream ("Read a file", not
 * "os" / "file: read").
 */

const ACTIVITY: Record<string, string> = {
	bash: 'running a command',
	grep: 'searching files',
	glob: 'finding files',
	read: 'reading a file',
	write: 'writing a file',
	edit: 'editing a file',
	web: 'searching the web',
	browser: 'reading a page',
	bot: 'thinking it through',
	desktop: 'using the desktop',
	event: 'checking the schedule',
	loop: 'sending a message',
	os: 'checking the workspace',
};

const OUTCOME: Record<string, string> = {
	bash: 'Ran a command',
	grep: 'Searched files',
	glob: 'Found files',
	read: 'Read a file',
	write: 'Wrote a file',
	edit: 'Edited a file',
	web: 'Searched the web',
	browser: 'Read a page',
	bot: 'Thought it through',
	desktop: 'Used the desktop',
	event: 'Checked the schedule',
	loop: 'Sent a message',
	os: 'Checked the workspace',
};

const STRAP_VERB: Record<string, [string, string]> = {
	create: ['creating', 'Created'],
	add: ['creating', 'Created'],
	insert: ['creating', 'Created'],
	read: ['reading', 'Read'],
	get: ['reading', 'Read'],
	view: ['reading', 'Read'],
	fetch: ['reading', 'Read'],
	list: ['listing', 'Listed'],
	ls: ['listing', 'Listed'],
	search: ['searching', 'Searched'],
	find: ['searching', 'Searched'],
	query: ['searching', 'Searched'],
	glob: ['searching', 'Searched'],
	grep: ['searching', 'Searched'],
	update: ['updating', 'Updated'],
	edit: ['updating', 'Updated'],
	set: ['updating', 'Updated'],
	patch: ['updating', 'Updated'],
	rename: ['updating', 'Updated'],
	move: ['updating', 'Updated'],
	delete: ['deleting', 'Deleted'],
	remove: ['deleting', 'Deleted'],
	clear: ['deleting', 'Deleted'],
	send: ['sending', 'Sent'],
	post: ['sending', 'Sent'],
	reply: ['sending', 'Sent'],
	dm: ['sending', 'Sent'],
	run: ['running', 'Ran'],
	exec: ['running', 'Ran'],
	execute: ['running', 'Ran'],
	shell: ['running', 'Ran'],
	write: ['writing', 'Wrote'],
	save: ['writing', 'Wrote'],
	download: ['downloading', 'Downloaded'],
	upload: ['uploading', 'Uploaded'],
	open: ['opening', 'Opened'],
	launch: ['opening', 'Opened'],
	start: ['opening', 'Opened'],
	stop: ['stopping', 'Stopped'],
	close: ['stopping', 'Stopped'],
	kill: ['stopping', 'Stopped'],
	check: ['checking', 'Checked'],
	status: ['checking', 'Checked'],
	verify: ['checking', 'Checked'],
	notify: ['notifying', 'Notified'],
	alert: ['notifying', 'Notified'],
};

function serviceName(slug: string): string {
	return slug
		.split(/[-_]/)
		.filter(Boolean)
		.map((w) => w.charAt(0).toUpperCase() + w.slice(1))
		.join(' ');
}

function rawName(toolName: string): [string, string] {
	const n = toolName.replace(/_/g, ' ');
	return [`using ${n}`, `Used ${n}`];
}

/** Returns (activity gerund, past-tense outcome) for a tool call. */
export function humanizeToolCall(
	toolName: string,
	input: Record<string, unknown> = {},
): { label: string; outcome: string } {
	if (toolName.startsWith('mcp__')) {
		const rest = toolName.slice(5);
		const sep = rest.indexOf('__');
		if (sep >= 0) {
			const slug = rest.slice(0, sep);
			const tool = rest.slice(sep + 2).replace(/_/g, ' ');
			return { label: `using ${slug} (${tool})`, outcome: `Used ${slug}: ${tool}` };
		}
	}

	const resource = typeof input.resource === 'string' ? input.resource : undefined;
	const action = typeof input.action === 'string' ? input.action : undefined;

	if (toolName === 'web') {
		let site = 'a page';
		const url = typeof input.url === 'string' ? input.url : undefined;
		if (url) {
			try {
				const host = new URL(url).hostname.replace(/^www\./, '');
				if (host) site = host;
			} catch {
				/* keep default */
			}
		}
		switch (action) {
			case 'search':
			case undefined:
				return { label: 'searching the web', outcome: 'Searched the web' };
			case 'fetch':
				return { label: `reading ${site}`, outcome: `Read ${site}` };
			case 'navigate':
				return { label: `opening ${site}`, outcome: `Opened ${site}` };
			case 'read_page':
				return { label: 'reading the page', outcome: 'Read the page' };
			default: {
				const a = (action ?? '').replace(/_/g, ' ');
				return { label: `${a} (web)`, outcome: `Web: ${a}` };
			}
		}
	}

	if (toolName === 'plugin') {
		if (action === 'discover') {
			return { label: 'browsing the marketplace', outcome: 'Browsed the marketplace' };
		}
		if (action === 'list') {
			return { label: 'checking available tools', outcome: 'Checked available tools' };
		}
		if (resource) {
			const svc = serviceName(resource);
			return { label: `using ${svc}`, outcome: `Used ${svc}` };
		}
	}

	if (resource && action) {
		const noun = resource.replace(/_/g, ' ');
		const verb = STRAP_VERB[action];
		if (verb) {
			return { label: `${verb[0]} ${noun}`, outcome: `${verb[1]} ${noun}` };
		}
		return {
			label: `running ${action} on ${noun}`,
			outcome: `Ran ${action} on ${noun}`,
		};
	}

	const a = ACTIVITY[toolName];
	const o = OUTCOME[toolName];
	if (a && o) return { label: a, outcome: o };
	const [rl, ro] = rawName(toolName);
	return { label: rl, outcome: ro };
}
