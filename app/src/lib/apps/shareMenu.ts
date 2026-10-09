/**
 * `window.share_menu`: the header's Share button, an app's own ways to share
 * or export its work. Picking an entry sends its `say` into the app's chat
 * as the owner's words, the way Publish sends its starter.
 *
 * An entry with `share: true` asks for a link. Its `say` asks the app for
 * the file, and the one Share dialog opens on the file the answer hands
 * over, on the device it was picked on: the owner chooses there who can open
 * the link. (The app's page may answer through `nebo.share` instead; that
 * opens the same dialog by itself.) The app never makes the link.
 */

import type { AppWindow } from '$lib/api/neboComponents';

export type ShareEntry = AppWindow['shareMenu'][number];

/** The entries the header's Share button lists for this chat: none for an
 *  employee that is not an app, or a chat the owner cannot send in. */
export function shareEntries(appWindow: Partial<AppWindow> | null | undefined, canSend: boolean): ShareEntry[] {
	if (!canSend) return [];
	return (appWindow?.shareMenu ?? []).filter((e) => !!e?.label?.trim() && !!e?.say?.trim());
}

/** A link asked for in a chat: the transcript held `from` messages then. */
export interface LinkAsk {
	chat: string;
	from: number;
}

/** A file the answer handed over, by its Work reference (`/api/v1/files/…`). */
export interface LinkFile {
	url: string;
	title: string;
}

interface FileRef {
	url?: string;
	filename?: string;
	title?: string;
}

interface TranscriptRow {
	type: string;
	attachments?: FileRef[];
	workItems?: FileRef[];
}

const FILES = '/api/v1/files/';

/**
 * The file a link ask's answer handed over: the last file of the replies
 * after the ask (`from` is where its own message went), up to the owner's
 * next message. Only a file in Work can have a link. Null while none came.
 */
export function linkFile(messages: TranscriptRow[], from: number): LinkFile | null {
	let found: LinkFile | null = null;
	let asked = false;
	for (const m of messages.slice(Math.max(0, from))) {
		if (m.type === 'user') {
			if (asked) break;
			asked = true;
			continue;
		}
		if (m.type !== 'assistant') continue;
		for (const f of [...(m.attachments ?? []), ...(m.workItems ?? [])]) {
			const at = f.url?.indexOf(FILES) ?? -1;
			if (at < 0) continue;
			const url = f.url!.slice(at);
			found = { url, title: f.title || f.filename || decodeURIComponent(url.split('/').pop() || 'file') };
		}
	}
	return found;
}
