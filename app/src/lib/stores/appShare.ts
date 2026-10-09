import { writable } from 'svelte/store';

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
