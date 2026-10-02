/** The bot's `chat_user_message`: the owner's message, sent into a
 *  conversation from anywhere (this window's composer, another window, the
 *  phone, an app console's "Send to"). */
export interface UserMessageEvent {
	session_id?: string;
	id?: string;
	content?: string;
	createdAt?: number;
	client_id?: string | null;
}

/**
 * The row an open conversation adds for `data`, or null when it adds none:
 * the event is about another conversation, or this page typed it (its
 * composer already shows it). Every other view shows it as it is sent, so
 * the work that follows never streams under a message the owner cannot see.
 */
export function userMessageRow(
	data: UserMessageEvent | null | undefined,
	activeSessionKey: string | null | undefined,
	ownClientId: string
): { id: string; content: string; createdAt: number } | null {
	if (!data || !activeSessionKey || data.session_id !== activeSessionKey) return null;
	if (data.client_id && data.client_id === ownClientId) return null;
	const content = (data.content ?? '').trim();
	if (!content) return null;
	return { id: data.id || `msg-${Date.now()}`, content: data.content as string, createdAt: data.createdAt || Date.now() };
}
