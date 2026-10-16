// A team's projects as its page lists them: read once from
// GET /teams/{id}/projects, then kept by `project_updated`. The order is the
// server's: live and paused first, newest first, then the done ones, most
// recently done first.

import type { Project } from '$lib/api/neboComponents';

const isDone = (p: Pick<Project, 'state'>) => p.state === 'done';
const when = (p: Pick<Project, 'createdAt' | 'closedAt'>) => p.closedAt ?? p.createdAt;

/** `list` in the server's order. */
export function orderProjects(list: Project[]): Project[] {
	return [...list].sort(
		(a, b) => Number(isDone(a)) - Number(isDone(b)) || when(b) - when(a) || a.id.localeCompare(b.id)
	);
}

/** The list after `project_updated` for the team `teamId`: the project
 *  added or replaced, in order. Another team's project changes nothing. */
export function projectUpdated(list: Project[], teamId: string, project: Project | null | undefined): Project[] {
	if (!project?.id || project.teamId !== teamId) return list;
	return orderProjects([...list.filter((p) => p.id !== project.id), project]);
}

/** The projects still going and the done ones, for the page's two lists. */
export function splitProjects(list: Project[]): { going: Project[]; done: Project[] } {
	return { going: list.filter((p) => !isDone(p)), done: list.filter(isDone) };
}
