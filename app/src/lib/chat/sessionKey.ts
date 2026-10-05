/**
 * Canonical chat session-key templates — the ONE place that knows the
 * `agent:<id>:thread:<chatId>` and `agent:<id>:app[:<ctx>]` formats.
 * Build and parse keys here; never hand-assemble the templates inline.
 */

/** Session key for one of an agent's conversations:
 *  `agent:<agentId>:thread:<threadId>`. A legacy conversation's id IS its
 *  session key (`agent:<agentId>:web`, the one the app console's Send to and
 *  the phone also write in): it is used as it is, never wrapped into
 *  `agent:<id>:thread:agent:<id>:web` — a second session over the same
 *  chat, whose live events the other doors never saw. */
export function threadKey(agentId: string, threadId: string): string {
	if (threadId.startsWith(`agent:${agentId}:`)) return threadId;
	return `agent:${agentId}:thread:${threadId}`;
}

/** Session key for an app-embedded chat: `agent:<agentId>:app[:<ctx>]`. */
export function appKey(agentId: string, ctx?: string): string {
	return `agent:${agentId}:app${ctx ? ':' + ctx : ''}`;
}

/** Session key of a team's own thread: `team:<teamId>`. */
export function teamKey(teamId: string): string {
	return `team:${teamId}`;
}

/** Whether `key` is a member's seat in the team `teamId`
 *  (`agent:<member>:coworker:team:<teamId>`): where the member works for it. */
export function isTeamSeat(key: string, teamId: string): boolean {
	return key.startsWith('agent:') && key.endsWith(`:coworker:${teamKey(teamId)}`);
}

/** The thread id embedded in a thread session key, or '' when `key` isn't one. */
export function threadIdFromKey(key: string): string {
	return key.split(':thread:')[1] ?? '';
}

/** Whether `text` is a raw session key (`agent:<id>:…`) rather than words. */
export function isSessionKey(text: string): boolean {
	return /^agent:[^:\s]+:\S*$/.test(text.trim());
}

/** What a conversation is called on screen: its title, or — while it has
 *  none, or only the session key a legacy chat was stored under — the
 *  employee's name. A raw `agent:<id>:web` is never shown. */
export function conversationTitle(
	chat: { title?: string | null; name?: string | null } | null | undefined,
	fallback: string,
): string {
	for (const t of [chat?.title, chat?.name]) {
		const s = (t ?? '').trim();
		if (s && !isSessionKey(s)) return s;
	}
	return fallback;
}
