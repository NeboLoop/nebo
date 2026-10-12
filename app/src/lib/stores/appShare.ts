import { writable } from 'svelte/store';
import { opensHere } from '$lib/websocket/origin';

/**
 * A file an app's page handed the owner to share (`nebo.share`): the app put
 * it into Work, and its chat shows the one Share dialog on it as soon as it
 * is open. The chat clears it when it opens the dialog.
 */
export interface ShareRequest {
	agentId: string;
	/** The chat the app was open on; empty when none. */
	chatId: string;
	/** The file's place in Work (`/api/v1/files/…`). */
	artifact: string;
	title: string;
}

export const shareRequest = writable<ShareRequest | null>(null);

/**
 * The share an `app_share_requested` event asks THIS client to show, or null.
 * The Share dialog belongs to the device whose page asked (`client_id`, the
 * `?client=` the page was opened with): a share made in the app on the phone
 * never opens here, and one no client claims opens nowhere. The file is in
 * Work either way.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function shareRequestOf(data: any): ShareRequest | null {
	if (!data?.agentId || !data?.artifact || !opensHere(data, 'nowhere')) return null;
	return { agentId: data.agentId, chatId: data.chatId ?? '', artifact: data.artifact, title: data.title ?? '' };
}

/** Where the request is answered: the chat the app was open on, else the app's chats. */
export function shareRequestPath(req: ShareRequest): string {
	const app = `/${encodeURIComponent(req.agentId)}/threads`;
	return req.chatId ? `${app}/${encodeURIComponent(req.chatId)}` : app;
}

/** Whether the chat `threadId` of app `agentId` answers the request. */
export function answersShareRequest(req: ShareRequest | null, agentId: string, threadId: string): boolean {
	if (!req || !agentId || req.agentId !== agentId || !req.artifact) return false;
	return !req.chatId || req.chatId === threadId;
}
