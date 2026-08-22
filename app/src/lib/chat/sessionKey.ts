/**
 * Canonical chat session-key templates — the ONE place that knows the
 * `agent:<id>:thread:<chatId>` and `agent:<id>:app[:<ctx>]` formats.
 * Build and parse keys here; never hand-assemble the templates inline.
 */

/** Session key for an agent thread: `agent:<agentId>:thread:<threadId>`. */
export function threadKey(agentId: string, threadId: string): string {
	return `agent:${agentId}:thread:${threadId}`;
}

/** Session key for an app-embedded chat: `agent:<agentId>:app[:<ctx>]`. */
export function appKey(agentId: string, ctx?: string): string {
	return `agent:${agentId}:app${ctx ? ':' + ctx : ''}`;
}

/** The thread id embedded in a thread session key, or '' when `key` isn't one. */
export function threadIdFromKey(key: string): string {
	return key.split(':thread:')[1] ?? '';
}
