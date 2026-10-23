/**
 * Canonical chat session-key templates — the ONE place that knows the
 * `agent:<id>:thread:<chatId>` format. (An app's own conversation,
 * `agent:<id>:app[:<thread>]`, is the server's: `handlers::apps::app_chat_key`.)
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

/** Session key of one of an employee's cases: `agent:<agentId>:case:<caseId>`
 *  (an assignment is worked in one). */
export function caseKey(agentId: string, caseId: string): string {
	return `agent:${agentId}:case:${caseId}`;
}

/** Session key of a team's own thread: `team:<teamId>`. */
export function teamKey(teamId: string): string {
	return `team:${teamId}`;
}

/** Session key of a project's thread: `team:<teamId>:project:<projectId>`
 *  (the server's `project_thread_key`). */
export function projectKey(teamId: string, projectId: string): string {
	return `${teamKey(teamId)}:project:${projectId}`;
}

/** Whether `key` is a member's seat in the team `teamId` or in one of its
 *  projects (`agent:<member>:coworker:team:<teamId>[:project:<id>]`):
 *  where the member works for it. */
export function isTeamSeat(key: string, teamId: string): boolean {
	if (!key.startsWith('agent:')) return false;
	const team = `:coworker:${teamKey(teamId)}`;
	return key.endsWith(team) || key.includes(`${team}:project:`);
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
