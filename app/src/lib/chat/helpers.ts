// Helpers an employee started from this conversation, as the server's
// subagent_start / subagent_progress / subagent_complete events describe
// them. A helper can outlive the turn that started it (it runs in the
// background), so this list is kept apart from the turn's own activity line.

import { isCallName, type CallName } from './callLabel';

export interface HelperLine {
	taskId: string;
	/** Its title among its siblings, as the server gives it on each start
	 * and step (the background list's). */
	title: string;
	/** The call it is making now, by its structured name. */
	call?: CallName;
	/** Background work's own progress line ("Searching 4 angles"). */
	activity: string;
}

export type HelperEventType = 'subagent_start' | 'subagent_progress' | 'subagent_complete' | 'subagent_titles';

interface HelperEventData {
	task_id?: unknown;
	title?: unknown;
	/** `subagent_titles`: the siblings retitled as one started or ended. */
	titles?: unknown;
	current_operation?: unknown;
	call?: unknown;
}

/** The titles a `subagent_titles` event gives, by task id: every running
 * sibling's, retitled at once as one of them started or ended. */
export function titlesOf(data: { titles?: unknown }): Map<string, string> {
	const out = new Map<string, string>();
	for (const t of Array.isArray(data.titles) ? data.titles : []) {
		if (typeof t?.task_id === 'string' && typeof t?.title === 'string' && t.title) out.set(t.task_id, t.title);
	}
	return out;
}

/** The helper list after one event. Unknown helpers' progress is ignored. */
export function applyHelperEvent(
	list: HelperLine[],
	type: HelperEventType,
	data: HelperEventData
): HelperLine[] {
	if (type === 'subagent_titles') {
		const titles = titlesOf(data);
		return list.map((h) => (titles.has(h.taskId) ? { ...h, title: titles.get(h.taskId)! } : h));
	}
	const taskId = typeof data.task_id === 'string' ? data.task_id : '';
	if (!taskId) return list;
	const rest = list.filter((h) => h.taskId !== taskId);
	const known = list.find((h) => h.taskId === taskId);
	// Every start, step and end carries its title as it stands.
	const title = typeof data.title === 'string' && data.title ? data.title : known?.title ?? '';
	switch (type) {
		case 'subagent_start':
			return [
				...rest,
				{
					taskId,
					title,
					activity: known?.activity ?? '',
					...(known?.call ? { call: known.call } : {}),
				},
			];
		case 'subagent_progress': {
			if (!known) return list;
			// A step is a call; background work says its own line.
			const next: HelperLine = isCallName(data.call)
				? { taskId, title, activity: '', call: data.call }
				: typeof data.current_operation === 'string'
					? { taskId, title, activity: data.current_operation }
					: { ...known, title };
			return list.map((h) => (h.taskId === taskId ? next : h));
		}
		case 'subagent_complete':
			return rest;
	}
}
