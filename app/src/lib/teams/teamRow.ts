// What a team's roster row says about its work at a glance: the newest line
// in any of its threads (its own conversation and its projects') and how
// many of its projects are going ("2 live"), or "done" once none is. Seeded
// by GET /teams, then kept by what the server already sends: `team_message`
// and `project_message` for the line, `project_updated` for the projects.
// Pure, so the shell's handlers stay one line each and every rule is tested.

import type { RosterTeam, TeamMessage } from '$lib/api/neboComponents';
import { stripAttachmentNotes } from '$lib/types/attachment';

/** The newest post in a team's threads: who said it and what. */
export type TeamLatest = { from: string; mine: boolean; text: string };

/** One team's row state: its projects by id with their state, and its
 *  newest post. */
export type TeamRow = { projects: Record<string, string>; latest: TeamLatest | null };

/** Every team's row state, by team id. */
export type TeamRows = Record<string, TeamRow>;

const EMPTY: TeamRow = { projects: {}, latest: null };

const latestOf = (m: Pick<TeamMessage, 'from' | 'role' | 'content'>): TeamLatest => ({
	from: m.from,
	mine: m.role === 'user',
	text: m.content,
});

/** The rows as the team list (GET /teams) has them. */
export function seedTeamRows(teams: Pick<RosterTeam, 'id' | 'projects' | 'latest'>[]): TeamRows {
	const rows: TeamRows = {};
	for (const t of teams) {
		rows[t.id] = {
			projects: Object.fromEntries((t.projects ?? []).map((p) => [p.id, p.state])),
			latest: t.latest ? latestOf(t.latest) : null,
		};
	}
	return rows;
}

/** A project was created or its state moved (`project_updated`). */
export function projectChanged(
	rows: TeamRows,
	project: { id?: unknown; teamId?: unknown; state?: unknown } | null | undefined
): TeamRows {
	const { id, teamId, state } = project ?? {};
	if (typeof id !== 'string' || typeof teamId !== 'string' || typeof state !== 'string' || !id || !teamId) return rows;
	const row = rows[teamId] ?? EMPTY;
	if (row.projects[id] === state) return rows;
	return { ...rows, [teamId]: { ...row, projects: { ...row.projects, [id]: state } } };
}

/** A post landed in one of the team's threads (`team_message`,
 *  `project_message`). A clear's divider is not a post. */
export function postArrived(
	rows: TeamRows,
	data: { teamId?: unknown; text?: unknown; senderName?: unknown; from?: unknown; role?: unknown; cleared?: unknown } | null | undefined
): TeamRows {
	const teamId = data?.teamId;
	const text = data?.text;
	if (typeof teamId !== 'string' || !teamId || typeof text !== 'string' || !text || data?.cleared) return rows;
	const from = typeof data?.senderName === 'string' && data.senderName ? data.senderName : typeof data?.from === 'string' ? data.from : '';
	const row = rows[teamId] ?? EMPTY;
	return { ...rows, [teamId]: { ...row, latest: latestOf({ from, role: data?.role === 'user' ? 'user' : 'assistant', content: text }) } };
}

/** How many of the team's projects are going (live or paused). */
export function liveCount(row: TeamRow | undefined): number {
	return Object.values(row?.projects ?? {}).filter((s) => s !== 'done').length;
}

/** The row's chip: "N live" while projects are going, "done" once every
 *  project it has is done, nothing before its first project. */
export function rowChip(row: TeamRow | undefined): { live: number } | 'done' | null {
	const live = liveCount(row);
	if (live > 0) return { live };
	return Object.keys(row?.projects ?? {}).length > 0 ? 'done' : null;
}

/** The latest line as the row shows it: "Sender · first line", mention
 *  tokens as names and the attachment notes off. `you` names the owner;
 *  `nameOf` names a mentioned employee by id. */
export function latestLine(latest: TeamLatest | null, you: string, nameOf: (id: string) => string): string {
	if (!latest) return '';
	const words = stripAttachmentNotes(latest.text)
		.replace(/<@([A-Za-z0-9._-]+)>/g, (_, id: string) => `@${nameOf(id)}`)
		.split('\n')
		.map((l) => l.replace(/^\s*(?:[#>*-]+\s*)+/, '').replace(/[*_`]{2,}/g, '').trim())
		.find(Boolean);
	if (!words) return '';
	const who = latest.mine ? you : latest.from;
	return who ? `${who} · ${words}` : words;
}
