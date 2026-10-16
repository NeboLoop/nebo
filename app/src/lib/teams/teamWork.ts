// Who is working in a team's conversation, or in one of its projects',
// right now: each member at work in its seat for that thread
// (`agent:<member>:coworker:<thread>`) and each helper a member started
// there. Loaded once from GET /teams/{id}/working (GET /projects/{id}/working
// for a project), then kept by the events the work already sends — never
// polled. `thread` is the thread's key (`teamKey`, `projectKey`).
import type { TeamWorkEntry } from '$lib/api/neboComponents';
import { endsRun } from '$lib/chat/runEnd';
import { isCallName, type CallName } from '$lib/chat/callLabel';
import { projectKey, teamKey } from '$lib/chat/sessionKey';

export type WorkEntry = TeamWorkEntry;

/** A member's seat for the thread: the session its work there runs in. */
export function seatKey(agentId: string, thread: string): string {
	return `agent:${agentId}:coworker:${thread}`;
}

/** The member whose seat for `thread` the session key is, or '' when it is
 *  not one. */
export function seatMember(key: unknown, thread: string): string {
	if (typeof key !== 'string') return '';
	const suffix = `:coworker:${thread}`;
	if (!key.startsWith('agent:') || !key.endsWith(suffix)) return '';
	return key.slice('agent:'.length, key.length - suffix.length);
}

/** One entry's identity: a member by its seat, a helper by its task. */
export const workKey = (e: Pick<WorkEntry, 'kind' | 'sessionKey' | 'taskId'>) =>
	e.kind === 'helper' ? `helper:${e.taskId}` : `member:${e.sessionKey}`;

/** What the team's Stop sends for one entry: that member, or that helper. */
export function stopBody(e: WorkEntry): { agentId: string; taskId?: string } {
	return e.kind === 'helper' ? { agentId: e.agentId, taskId: e.taskId } : { agentId: e.agentId };
}

interface EventData {
	session_id?: unknown;
	call?: unknown;
	task_id?: unknown;
	description?: unknown;
	current_operation?: unknown;
	teamId?: unknown;
	projectId?: unknown;
	agentId?: unknown;
	state?: unknown;
	stop_reason?: unknown;
	runs?: { sessionKey?: string; call?: unknown }[];
}

const text = (v: unknown) => (typeof v === 'string' ? v : '');
const callOf = (v: unknown): CallName | null => (isCallName(v) ? v : null);

/** The thread a `team_activity` or `project_activity` event is about. */
function activityThread(type: string, data: EventData): string {
	const team = text(data.teamId);
	if (!team) return '';
	if (type === 'team_activity') return teamKey(team);
	const project = text(data.projectId);
	return project ? projectKey(team, project) : '';
}

/** The list after one event, for the thread `thread`. `nameOf` names a
 *  member by its agent id. Events for other conversations change nothing. */
export function applyWorkEvent(
	list: WorkEntry[],
	type: string,
	data: EventData,
	thread: string,
	nameOf: (agentId: string) => string
): WorkEntry[] {
	// `base` with the member at work, the call it makes updated when given.
	const member = (base: WorkEntry[], agentId: string, call?: CallName | null): WorkEntry[] => {
		const sessionKey = seatKey(agentId, thread);
		const known = base.find((e) => e.kind === 'member' && e.sessionKey === sessionKey);
		const entry: WorkEntry = known
			? { ...known, call: call ?? known.call }
			: {
					kind: 'member',
					agentId,
					member: nameOf(agentId),
					title: nameOf(agentId),
					taskId: '',
					call: call ?? null,
					activity: '',
					sessionKey,
					chatId: '',
				};
		return known ? base.map((e) => (e === known ? entry : e)) : [...base, entry];
	};
	const without = (keep: (e: WorkEntry) => boolean) => list.filter(keep);

	switch (type) {
		case 'tool_start':
		case 'thinking':
		case 'chat_created': {
			const agentId = seatMember(data.session_id, thread);
			if (!agentId) return list;
			return member(list, agentId, type === 'tool_start' ? callOf(data.call) : undefined);
		}
		case 'chat_complete':
		case 'chat_error':
		case 'chat_cancelled': {
			const agentId = seatMember(data.session_id, thread);
			// A queued message's own completion: the member is still at work.
			if (!agentId || !endsRun(data)) return list;
			return without((e) => !(e.kind === 'member' && e.agentId === agentId));
		}
		case 'team_activity':
		case 'project_activity': {
			if (activityThread(type, data) !== thread) return list;
			const agentId = text(data.agentId);
			if (!agentId) return list;
			return data.state === 'started'
				? member(list, agentId)
				: without((e) => !(e.kind === 'member' && e.agentId === agentId));
		}
		case 'subagent_start':
		case 'subagent_progress': {
			const agentId = seatMember(data.session_id, thread);
			const taskId = text(data.task_id);
			if (!agentId || !taskId) return list;
			const known = list.find((e) => e.kind === 'helper' && e.taskId === taskId);
			// A step is a call; background work says its own line.
			const call = type === 'subagent_progress' ? callOf(data.call) : null;
			const activity = type === 'subagent_progress' ? text(data.current_operation) : '';
			if (known) {
				const moved = call ? { call, activity: '' } : activity ? { call: null, activity } : {};
				return list.map((e) => (e === known ? { ...e, ...moved } : e));
			}
			return [
				...list,
				{
					kind: 'helper',
					agentId,
					member: nameOf(agentId),
					title: text(data.description) || nameOf(agentId),
					taskId,
					call,
					activity,
					sessionKey: `subagent:${seatKey(agentId, thread)}:${taskId}`,
					chatId: '',
				},
			];
		}
		case 'subagent_complete': {
			const taskId = text(data.task_id);
			return taskId ? without((e) => !(e.kind === 'helper' && e.taskId === taskId)) : list;
		}
		case 'agent_progress': {
			// The periodic snapshot of every running turn: a member's seat
			// that is no longer in it has stopped; one that is gets its
			// current call. Helpers are not turns; their own events keep
			// them.
			const runs = Array.isArray(data.runs) ? data.runs : [];
			const live = new Map<string, CallName | null>();
			for (const r of runs) {
				const agentId = seatMember(r.sessionKey, thread);
				if (agentId) live.set(agentId, callOf(r.call));
			}
			let next = list.filter((e) => e.kind === 'helper' || live.has(e.agentId));
			for (const [agentId, call] of live) next = member(next, agentId, call ?? undefined);
			return next;
		}
		default:
			return list;
	}
}

/** The events the list is kept by. */
export const WORK_EVENTS = [
	'tool_start',
	'thinking',
	'chat_created',
	'chat_complete',
	'chat_error',
	'chat_cancelled',
	'team_activity',
	'project_activity',
	'subagent_start',
	'subagent_progress',
	'subagent_complete',
	'agent_progress',
] as const;
