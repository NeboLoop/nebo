import type { EnrichedChat } from '$lib/types/agentPage';

type Pinnable = Pick<EnrichedChat, 'id' | 'pinned' | 'pinnedAt'>;

/**
 * An employee's conversations as its list shows them: the ones the owner
 * pinned first, most recently pinned first, then the rest in the order the
 * bot gave them (newest first). The list itself keeps the bot's order, so
 * its first row is still the latest conversation.
 */
export function pinnedFirst<T extends Pinnable>(chats: T[]): T[] {
	const pinned = chats.filter((c) => c.pinned).sort((a, b) => (b.pinnedAt ?? 0) - (a.pinnedAt ?? 0));
	return [...pinned, ...chats.filter((c) => !c.pinned)];
}

/** The list with one conversation pinned or unpinned, as the bot says it now
 *  stands (`PUT /chats/{id}/pin`, or `chat_pinned` from another surface). */
export function withPin<T extends Pinnable>(chats: T[], chatId: string, pinned: boolean, pinnedAt: number | null): T[] {
	return chats.map((c) => (c.id === chatId ? { ...c, pinned, pinnedAt: pinned ? pinnedAt : null } : c));
}
