// What a sidebar row says about its conversation at a glance: an employee
// (or a team) is working in it, or it holds a reply the owner has not read.
// The order of the rows never changes; these marks are how he knows where to
// look. Pure state transitions, so the shell's WebSocket handlers stay one
// line each and every rule here is tested.
//
// Read state lives on the bot (`chats.read_message_id`), so the desktop, the
// web console and the phone agree: the roster seeds it (`unreadSessions`,
// `unreadTeams`), `chat_complete` / `team_message` say whether a turn left a
// reply unread, `chat_read` clears it everywhere, and opening a conversation
// marks it read (`PUT /chats/{id}/read`, `PUT /teams/{teamId}/read`).

import { isTeamSeat, teamKey } from './sessionKey';

/** Who is working right now: agent id → session key → what it is doing
 *  ("reading a file", or '' while it thinks). */
export type Working = Record<string, Record<string, string>>;

/** The main employee runs with an empty agentId on the wire; the roster
 *  knows it as 'assistant'. */
export const workerId = (id: unknown): string => (typeof id === 'string' && id ? id : 'assistant');
const sessionOf = (id: unknown): string => (typeof id === 'string' && id ? id : '_');
/** Every team thread's key starts with this. */
const TEAM_THREAD = teamKey('');

/** A run started (or reported progress) in a session. */
export function startWork(w: Working, agentId: unknown, sessionId: unknown, label = ''): Working {
	const id = workerId(agentId);
	const sid = sessionOf(sessionId);
	return { ...w, [id]: { ...(w[id] ?? {}), [sid]: label || w[id]?.[sid] || '' } };
}

/** The run in a session ended (completed, failed, or stopped). */
export function endWork(w: Working, agentId: unknown, sessionId: unknown): Working {
	const id = workerId(agentId);
	const rest = { ...(w[id] ?? {}) };
	delete rest[sessionOf(sessionId)];
	const next = { ...w };
	if (Object.keys(rest).length === 0) delete next[id];
	else next[id] = rest;
	return next;
}

/** The server's periodic snapshot of every live run (`agent_progress`): an
 *  employee no run names any more has stopped, whatever event was missed. */
export function snapshotWork(
	w: Working,
	runs: { entityId?: string; sessionKey?: string; activity?: string }[]
): Working {
	const live = new Set(runs.map((r) => workerId(r.entityId)));
	let next: Working = Object.fromEntries(Object.entries(w).filter(([id]) => live.has(id)));
	for (const r of runs) next = startWork(next, r.entityId, r.sessionKey, r.activity ?? '');
	return next;
}

/** Whether a member works in the team's conversation (its seat for it). */
export function teamWorking(w: Working, teamId: string): boolean {
	return Object.values(w).some((sessions) => Object.keys(sessions).some((key) => isTeamSeat(key, teamId)));
}

/** Replace the employees' unread conversations with the roster's
 *  (`unreadSessions`); the teams' are kept. */
export function seedAgentUnread(unread: Set<string>, sessionKeys: string[]): Set<string> {
	return new Set([...[...unread].filter((k) => k.startsWith(TEAM_THREAD)), ...sessionKeys]);
}

/** Replace the teams' unread threads with the team list's (`unreadTeams`);
 *  the employees' are kept. */
export function seedTeamUnread(unread: Set<string>, teamIds: string[]): Set<string> {
	return new Set([...[...unread].filter((k) => !k.startsWith(TEAM_THREAD)), ...teamIds.map(teamKey)]);
}

/** The conversation the owner is looking at: open in the main pane, with the
 *  window on screen. */
export type Open = { key: string; visible: boolean };

/** A turn ended in `key`, and the bot says whether it left a reply the owner
 *  has not read. Read where he is looking (`markRead`: tell the bot), lit
 *  everywhere else. */
export function replyArrived(
	unread: Set<string>,
	key: string,
	isUnread: boolean,
	open: Open
): { unread: Set<string>; markRead: boolean } {
	if (!key) return { unread, markRead: false };
	const seen = open.visible && open.key === key;
	if (isUnread && !seen) return { unread: unread.has(key) ? unread : new Set([...unread, key]), markRead: false };
	return { unread: conversationRead(unread, key), markRead: isUnread && seen };
}

/** The conversation was read (here, or on another surface: `chat_read`). */
export function conversationRead(unread: Set<string>, key: string): Set<string> {
	if (!unread.has(key)) return unread;
	const next = new Set(unread);
	next.delete(key);
	return next;
}

/** The open conversation holds an unread reply and the owner can see it:
 *  mark it read. */
export function openNeedsRead(unread: Set<string>, open: Open): boolean {
	return open.visible && !!open.key && unread.has(open.key);
}

/** Whether any of the employee's own conversations holds an unread reply. */
export function agentUnread(unread: Set<string>, agentId: string): boolean {
	const prefix = `agent:${agentId}:`;
	for (const k of unread) if (k.startsWith(prefix)) return true;
	return false;
}

/** The one mark a row shows: working while a run is going (a finished run
 *  with a new reply becomes the unread dot), else unread, else none. */
export type RowMark = 'working' | 'unread' | null;
export function rowMark(working: boolean, unread: boolean): RowMark {
	return working ? 'working' : unread ? 'unread' : null;
}
