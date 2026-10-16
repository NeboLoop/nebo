import { describe, expect, it } from 'vitest';
import type { Project } from '$lib/api/neboComponents';
import { orderProjects, projectUpdated, splitProjects } from './projects';

const project = (id: string, state: string, createdAt: number, closedAt?: number): Project => ({
	id,
	teamId: 'launch',
	leadAgentId: 'pm',
	title: id,
	ask: '',
	askerSessionKey: '',
	askerAgentId: '',
	threadSessionKey: `team:launch:project:${id}`,
	state,
	limitSteps: 60,
	limitSpendCents: 500,
	createdAt,
	closedAt,
});

// The team page lists projects live first, then "Done · N" folded.
describe('team projects', () => {
	it('lists live and paused first, newest first, then the done ones', () => {
		const list = orderProjects([
			project('old-done', 'done', 1, 5),
			project('ph', 'live', 10),
			project('recent-done', 'done', 2, 20),
			project('preview', 'paused', 12),
		]);
		expect(list.map((p) => p.id)).toEqual(['preview', 'ph', 'recent-done', 'old-done']);
		const { going, done } = splitProjects(list);
		expect(going.map((p) => p.id)).toEqual(['preview', 'ph']);
		expect(done.map((p) => p.id)).toEqual(['recent-done', 'old-done']);
	});

	it('keeps the list current: a new project, one done, one reopened', () => {
		let list = [project('ph', 'live', 10)];
		list = projectUpdated(list, 'launch', project('preview', 'live', 12));
		expect(list.map((p) => p.id)).toEqual(['preview', 'ph']);
		list = projectUpdated(list, 'launch', project('preview', 'done', 12, 30));
		expect(list.map((p) => [p.id, p.state])).toEqual([['ph', 'live'], ['preview', 'done']]);
		list = projectUpdated(list, 'launch', project('preview', 'live', 12));
		expect(list.map((p) => p.id)).toEqual(['preview', 'ph']);
		// Another team's project is not this page's.
		expect(projectUpdated(list, 'launch', { ...project('x', 'live', 40), teamId: 'other' })).toBe(list);
		expect(projectUpdated(list, 'launch', null)).toBe(list);
	});
});
