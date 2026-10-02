import { describe, expect, it } from 'vitest';
import { applyWorkEvent, seatKey, seatMember, stopBody, type WorkEntry } from './teamWork';

const TEAM = 't-1';
const names: Record<string, string> = { ann: 'Ann', bo: 'Bo' };
const nameOf = (id: string) => names[id] ?? id;
const apply = (list: WorkEntry[], type: string, data: Record<string, unknown>) =>
	applyWorkEvent(list, type, data, TEAM, nameOf);

// The strip reads the list: collapsed it shows the count, expanded one line
// per member and helper with what each is doing now, and it hides when the
// list is empty.
describe('team work list', () => {
	it('knows a seat of this team and no other conversation', () => {
		expect(seatMember(seatKey('ann', TEAM), TEAM)).toBe('ann');
		expect(seatMember('agent:ann:web', TEAM)).toBe('');
		expect(seatMember(seatKey('ann', 'other'), TEAM)).toBe('');
	});

	it('lists members and their helpers with their current action, then empties when the work ends', () => {
		let list: WorkEntry[] = [];
		list = apply(list, 'tool_start', { session_id: seatKey('ann', TEAM), label: 'Searching Product Hunt for agent platforms' });
		list = apply(list, 'thinking', { session_id: seatKey('bo', TEAM) });
		list = apply(list, 'tool_start', { session_id: 'agent:ann:web', label: 'Reading her own mail' });
		list = apply(list, 'subagent_start', { session_id: seatKey('ann', TEAM), task_id: 'h-1', description: 'research rival pricing' });
		list = apply(list, 'subagent_progress', { session_id: seatKey('ann', TEAM), task_id: 'h-1', current_operation: 'Reading acme.com/pricing' });
		expect(list.length).toBe(3);
		expect(list.map((e) => [e.kind, e.title, e.activity])).toEqual([
			['member', 'Ann', 'Searching Product Hunt for agent platforms'],
			['member', 'Bo', ''],
			['helper', 'research rival pricing', 'Reading acme.com/pricing'],
		]);
		expect(list[2].member).toBe('Ann');

		list = apply(list, 'chat_complete', { session_id: seatKey('ann', TEAM) });
		expect(list.map((e) => e.title)).toEqual(['Bo', 'research rival pricing']);
		list = apply(list, 'subagent_complete', { session_id: seatKey('ann', TEAM), task_id: 'h-1' });
		list = apply(list, 'team_activity', { teamId: TEAM, agentId: 'bo', state: 'stopped' });
		expect(list).toEqual([]);
	});

	it('takes the running snapshot as the truth for members and leaves helpers to their own events', () => {
		let list: WorkEntry[] = [];
		list = apply(list, 'team_activity', { teamId: TEAM, agentId: 'ann', state: 'started' });
		list = apply(list, 'subagent_start', { session_id: seatKey('ann', TEAM), task_id: 'h-2', description: 'count the till' });
		list = apply(list, 'agent_progress', { runs: [{ sessionKey: seatKey('bo', TEAM), activity: 'Writing the ad' }, { sessionKey: 'agent:ann:web', activity: 'x' }] });
		expect(list.map((e) => [e.kind, e.title, e.activity])).toEqual([
			['helper', 'count the till', ''],
			['member', 'Bo', 'Writing the ad'],
		]);
	});

	it('stops only the row it is asked for', () => {
		let list: WorkEntry[] = [];
		list = apply(list, 'thinking', { session_id: seatKey('bo', TEAM) });
		list = apply(list, 'subagent_start', { session_id: seatKey('ann', TEAM), task_id: 'h-3', description: 'read the reviews' });
		expect(stopBody(list[0])).toEqual({ agentId: 'bo' });
		expect(stopBody(list[1])).toEqual({ agentId: 'ann', taskId: 'h-3' });
	});

	// A message sent to a member while it works is taken into its running
	// turn; that message's own stream ends with the queued stop at once. The
	// member is still at work until its run ends.
	it("keeps a member working through a queued message's completion until its run ends", () => {
		let list: WorkEntry[] = [];
		list = apply(list, 'tool_start', { session_id: seatKey('ann', TEAM), label: 'Pricing the order' });
		list = apply(list, 'chat_complete', { session_id: seatKey('ann', TEAM), stop_reason: 'queued_into_running_turn', stop_notice: '' });
		expect(list.map((e) => [e.title, e.activity])).toEqual([['Ann', 'Pricing the order']]);
		list = apply(list, 'chat_complete', { session_id: seatKey('ann', TEAM) });
		expect(list).toEqual([]);
	});
});
