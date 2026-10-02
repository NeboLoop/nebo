// Who is working in a team's conversation right now: each member at work in
// its seat for the team (`agent:<member>:coworker:team:<team>`) and each
// helper a member started there. Loaded once from GET /teams/{id}/working,
// then kept by the events the work already sends — never polled.
import type { TeamWorkEntry } from '$lib/api/neboComponents';
import { endsRun } from '$lib/chat/runEnd';

export type WorkEntry = TeamWorkEntry;

/** A member's seat for the team: the session its team work runs in. */
export function seatKey(agentId: string, teamId: string): string {
	return `agent:${agentId}:coworker:team:${teamId}`;
}

/** The member whose seat for `teamId` the session key is, or '' when it is
 *  not one. */
export function seatMember(key: unknown, teamId: string): string {
	if (typeof key !== 'string') return '';
	const suffix = `:coworker:team:${teamId}`;
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
	label?: unknown;
	task_id?: unknown;
	description?: unknown;
	current_operation?: unknown;
	teamId?: unknown;
	agentId?: unknown;
	state?: unknown;
	stop_reason?: unknown;
	runs?: { sessionKey?: string; activity?: string }[];
}

const text = (v: unknown) => (typeof v === 'string' ? v : '');

/** The list after one event, for the team `teamId`. `nameOf` names a
 *  member by its agent id. Events for other conversations change nothing. */
export function applyWorkEvent(
	list: WorkEntry[],
	type: string,
	data: EventData,
	teamId: string,
	nameOf: (agentId: string) => string
): WorkEntry[] {
	// `base` with the member at work, its activity updated when given.
	const member = (base: WorkEntry[], agentId: string, activity?: string): WorkEntry[] => {
		const sessionKey = seatKey(agentId, teamId);
		const known = base.find((e) => e.kind === 'member' && e.sessionKey === sessionKey);
		const entry: WorkEntry = known
			? { ...known, activity: activity ?? known.activity }
			: {
					kind: 'member',
					agentId,
					member: nameOf(agentId),
					title: nameOf(agentId),
					taskId: '',
					activity: activity ?? '',
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
			const agentId = seatMember(data.session_id, teamId);
			if (!agentId) return list;
			return member(list, agentId, type === 'tool_start' ? text(data.label) : undefined);
		}
		case 'chat_complete':
		case 'chat_error':
		case 'chat_cancelled': {
			const agentId = seatMember(data.session_id, teamId);
			// A queued message's own completion: the member is still at work.
			if (!agentId || !endsRun(data)) return list;
			return without((e) => !(e.kind === 'member' && e.agentId === agentId));
		}
		case 'team_activity': {
			if (data.teamId !== teamId) return list;
			const agentId = text(data.agentId);
			if (!agentId) return list;
			return data.state === 'started'
				? member(list, agentId)
				: without((e) => !(e.kind === 'member' && e.agentId === agentId));
		}
		case 'subagent_start':
		case 'subagent_progress': {
			const agentId = seatMember(data.session_id, teamId);
			const taskId = text(data.task_id);
			if (!agentId || !taskId) return list;
			const known = list.find((e) => e.kind === 'helper' && e.taskId === taskId);
			const activity = type === 'subagent_progress' ? text(data.current_operation) : '';
			if (known) {
				return list.map((e) => (e === known ? { ...e, activity: activity || e.activity } : e));
			}
			return [
				...list,
				{
					kind: 'helper',
					agentId,
					member: nameOf(agentId),
					title: text(data.description) || nameOf(agentId),
					taskId,
					activity,
					sessionKey: `subagent:${seatKey(agentId, teamId)}:${taskId}`,
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
			// current activity. Helpers are not turns; their own events keep
			// them.
			const runs = Array.isArray(data.runs) ? data.runs : [];
			const live = new Map<string, string>();
			for (const r of runs) {
				const agentId = seatMember(r.sessionKey, teamId);
				if (agentId) live.set(agentId, r.activity ?? '');
			}
			let next = list.filter((e) => e.kind === 'helper' || live.has(e.agentId));
			for (const [agentId, activity] of live) next = member(next, agentId, activity || undefined);
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
	'subagent_start',
	'subagent_progress',
	'subagent_complete',
	'agent_progress',
] as const;
