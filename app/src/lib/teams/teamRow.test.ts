import { describe, expect, it } from 'vitest';
import { latestLine, liveCount, postArrived, projectChanged, rowChip, seedTeamRows } from './teamRow';

const msg = (from: string, role: string, content: string) => ({
	id: 'm',
	from,
	fromAgentId: '',
	role,
	content,
	attachments: [],
	createdAt: 0,
});

// The roster row reads these: the latest line under the team's name, and
// "N live" (or "done") beside it.
describe('team row', () => {
	it('seeds from the team list: projects by state and the newest post', () => {
		const rows = seedTeamRows([
			{ id: 'launch', projects: [{ id: 'p1', state: 'live' }, { id: 'p2', state: 'paused' }, { id: 'p3', state: 'done' }], latest: msg('Script Writer', 'assistant', 'hero script done, cutdown next') },
			{ id: 'month', projects: [{ id: 'p4', state: 'done' }], latest: undefined },
			{ id: 'new', projects: [], latest: undefined },
		]);
		expect(liveCount(rows.launch)).toBe(2);
		expect(rowChip(rows.launch)).toEqual({ live: 2 });
		expect(rowChip(rows.month)).toBe('done');
		expect(rowChip(rows.new)).toBe(null);
		expect(rowChip(rows.unknown)).toBe(null);
		expect(latestLine(rows.launch.latest, 'You', (id) => id)).toBe('Script Writer · hero script done, cutdown next');
		expect(latestLine(rows.month.latest, 'You', (id) => id)).toBe('');
	});

	it('counts a project live when it opens and drops it when it is done; writing reopens it', () => {
		let rows = seedTeamRows([{ id: 'launch', projects: [], latest: undefined }]);
		rows = projectChanged(rows, { id: 'p1', teamId: 'launch', state: 'live' });
		rows = projectChanged(rows, { id: 'p2', teamId: 'launch', state: 'live' });
		expect(rowChip(rows.launch)).toEqual({ live: 2 });
		rows = projectChanged(rows, { id: 'p1', teamId: 'launch', state: 'done' });
		rows = projectChanged(rows, { id: 'p1', teamId: 'launch', state: 'done' });
		expect(rowChip(rows.launch)).toEqual({ live: 1 });
		rows = projectChanged(rows, { id: 'p2', teamId: 'launch', state: 'done' });
		expect(rowChip(rows.launch)).toBe('done');
		rows = projectChanged(rows, { id: 'p1', teamId: 'launch', state: 'live' });
		expect(rowChip(rows.launch)).toEqual({ live: 1 });
		// A team the list did not have yet (created since) gets its row.
		rows = projectChanged(rows, { id: 'p9', teamId: 'fresh', state: 'live' });
		expect(rowChip(rows.fresh)).toEqual({ live: 1 });
		// Something that is not a project changes nothing.
		expect(projectChanged(rows, null)).toBe(rows);
		expect(projectChanged(rows, { id: 'p1', state: 'live' })).toBe(rows);
	});

	it("takes the newest post from the team's own thread and its projects'", () => {
		let rows = seedTeamRows([{ id: 'launch', projects: [], latest: msg('Owner', 'user', 'kick off') }]);
		expect(latestLine(rows.launch.latest, 'You', (id) => id)).toBe('You · kick off');
		rows = postArrived(rows, { teamId: 'launch', projectId: 'p1', senderName: 'Product Marketer', role: 'assistant', text: 'Script Writer: hero 30s\nDone when: both scripts saved' } as never);
		expect(latestLine(rows.launch.latest, 'You', (id) => id)).toBe('Product Marketer · Script Writer: hero 30s');
		// A clear's divider is not a post.
		expect(postArrived(rows, { teamId: 'launch', text: 'Cleared here.', cleared: true })).toBe(rows);
		expect(postArrived(rows, { text: 'no team' })).toBe(rows);
	});

	it('says the line in plain words: names for mentions, no markdown marks, no attachment notes', () => {
		const names: Record<string, string> = { ve: 'Video Editor' };
		const line = latestLine(
			{ from: 'Script Writer', mine: false, text: '\n## **Both scripts** saved. <@ve> you are up' },
			'You',
			(id) => names[id] ?? id
		);
		expect(line).toBe('Script Writer · Both scripts saved. @Video Editor you are up');
	});
});
