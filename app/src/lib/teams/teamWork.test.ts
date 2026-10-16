import { describe, expect, it } from 'vitest';
import { applyWorkEvent, seatKey, seatMember, stopBody, type WorkEntry } from './teamWork';
import { projectKey, teamKey } from '$lib/chat/sessionKey';

const TEAM = 't-1';
const THREAD = teamKey(TEAM);
const names: Record<string, string> = { ann: 'Ann', bo: 'Bo' };
const nameOf = (id: string) => names[id] ?? id;
const apply = (list: WorkEntry[], type: string, data: Record<string, unknown>) =>
	applyWorkEvent(list, type, data, THREAD, nameOf);

// The strip reads the list: collapsed it shows the count, expanded one line
// per member and helper with what each is doing now, and it hides when the
// list is empty.
describe('team work list', () => {
	it('knows a seat of this team and no other conversation', () => {
		expect(seatMember(seatKey('ann', THREAD), THREAD)).toBe('ann');
		expect(seatMember('agent:ann:web', THREAD)).toBe('');
		expect(seatMember(seatKey('ann', teamKey('other')), THREAD)).toBe('');
	});

	it('lists members and their helpers with their current action, then empties when the work ends', () => {
		let list: WorkEntry[] = [];
		list = apply(list, 'tool_start', { session_id: seatKey('ann', THREAD), call: { kind: 'search', params: { query: 'agent platforms' } } });
		list = apply(list, 'thinking', { session_id: seatKey('bo', THREAD) });
		list = apply(list, 'tool_start', { session_id: 'agent:ann:web', call: { kind: 'read', params: { path: 'mail.txt' } } });
		list = apply(list, 'subagent_start', { session_id: seatKey('ann', THREAD), task_id: 'h-1', description: 'research rival pricing' });
		list = apply(list, 'subagent_progress', { session_id: seatKey('ann', THREAD), task_id: 'h-1', call: { kind: 'fetch', params: { url: 'acme.com/pricing' } } });
		expect(list.length).toBe(3);
		expect(list.map((e) => [e.kind, e.title, e.call?.kind ?? ''])).toEqual([
			['member', 'Ann', 'search'],
			['member', 'Bo', ''],
			['helper', 'research rival pricing', 'fetch'],
		]);
		list = apply(list, 'subagent_progress', { session_id: seatKey('ann', THREAD), task_id: 'h-1', current_operation: 'Searching 4 angles' });
		expect([list[2].call, list[2].activity]).toEqual([null, 'Searching 4 angles']);
		expect(list[2].member).toBe('Ann');

		list = apply(list, 'chat_complete', { session_id: seatKey('ann', THREAD) });
		expect(list.map((e) => e.title)).toEqual(['Bo', 'research rival pricing']);
		list = apply(list, 'subagent_complete', { session_id: seatKey('ann', THREAD), task_id: 'h-1' });
		list = apply(list, 'team_activity', { teamId: TEAM, agentId: 'bo', state: 'stopped' });
		expect(list).toEqual([]);
	});

	it('takes the running snapshot as the truth for members and leaves helpers to their own events', () => {
		let list: WorkEntry[] = [];
		list = apply(list, 'team_activity', { teamId: TEAM, agentId: 'ann', state: 'started' });
		list = apply(list, 'subagent_start', { session_id: seatKey('ann', THREAD), task_id: 'h-2', description: 'count the till' });
		list = apply(list, 'agent_progress', { runs: [{ sessionKey: seatKey('bo', THREAD), call: { kind: 'read', params: { path: 'ad.md' } } }, { sessionKey: 'agent:ann:web', call: { kind: 'read', params: { path: 'x' } } }] });
		expect(list.map((e) => [e.kind, e.title, e.call?.params?.path ?? ''])).toEqual([
			['helper', 'count the till', ''],
			['member', 'Bo', 'ad.md'],
		]);
	});

	it('stops only the row it is asked for', () => {
		let list: WorkEntry[] = [];
		list = apply(list, 'thinking', { session_id: seatKey('bo', THREAD) });
		list = apply(list, 'subagent_start', { session_id: seatKey('ann', THREAD), task_id: 'h-3', description: 'read the reviews' });
		expect(stopBody(list[0])).toEqual({ agentId: 'bo' });
		expect(stopBody(list[1])).toEqual({ agentId: 'ann', taskId: 'h-3' });
	});

	// A project's thread is its own conversation: its seats and its
	// `project_activity` count there, the team's own never do, and the
	// reverse.
	it("keeps a project's work apart from its team's", () => {
		const PROJECT = projectKey(TEAM, 'p-1');
		const inProject = (list: WorkEntry[], type: string, data: Record<string, unknown>) =>
			applyWorkEvent(list, type, data, PROJECT, nameOf);
		expect(seatMember(seatKey('ann', PROJECT), THREAD)).toBe('');
		let project: WorkEntry[] = [];
		project = inProject(project, 'project_activity', { teamId: TEAM, projectId: 'p-1', agentId: 'ann', state: 'started' });
		project = inProject(project, 'team_activity', { teamId: TEAM, agentId: 'bo', state: 'started' });
		project = inProject(project, 'project_activity', { teamId: TEAM, projectId: 'p-2', agentId: 'bo', state: 'started' });
		project = inProject(project, 'thinking', { session_id: seatKey('bo', THREAD) });
		expect(project.map((e) => [e.title, e.sessionKey])).toEqual([['Ann', seatKey('ann', PROJECT)]]);
		expect(apply([], 'project_activity', { teamId: TEAM, projectId: 'p-1', agentId: 'ann', state: 'started' })).toEqual([]);
		project = inProject(project, 'chat_complete', { session_id: seatKey('ann', PROJECT) });
		expect(project).toEqual([]);
	});

	// A message sent to a member while it works is taken into its running
	// turn; that message's own stream ends with the queued stop at once. The
	// member is still at work until its run ends.
	it("keeps a member working through a queued message's completion until its run ends", () => {
		let list: WorkEntry[] = [];
		list = apply(list, 'tool_start', { session_id: seatKey('ann', THREAD), call: { kind: 'read', params: { path: 'order.csv' } } });
		list = apply(list, 'chat_complete', { session_id: seatKey('ann', THREAD), stop_reason: 'queued_into_running_turn', stop_notice: '' });
		expect(list.map((e) => [e.title, e.call?.params?.path])).toEqual([['Ann', 'order.csv']]);
		list = apply(list, 'chat_complete', { session_id: seatKey('ann', THREAD) });
		expect(list).toEqual([]);
	});
});
