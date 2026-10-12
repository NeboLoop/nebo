// Helpers an employee started from this conversation, as the server's
// subagent_start / subagent_progress / subagent_complete events describe
// them. A helper can outlive the turn that started it (it runs in the
// background), so this list is kept apart from the turn's own activity line.

import { isCallName, type CallName } from './callLabel';

export interface HelperLine {
	taskId: string;
	description: string;
	/** The call it is making now, by its structured name. */
	call?: CallName;
	/** Background work's own progress line ("Searching 4 angles"). */
	activity: string;
}

export type HelperEventType = 'subagent_start' | 'subagent_progress' | 'subagent_complete';

interface HelperEventData {
	task_id?: unknown;
	description?: unknown;
	current_operation?: unknown;
	call?: unknown;
}

/** The helper list after one event. Unknown helpers' progress is ignored. */
export function applyHelperEvent(
	list: HelperLine[],
	type: HelperEventType,
	data: HelperEventData
): HelperLine[] {
	const taskId = typeof data.task_id === 'string' ? data.task_id : '';
	if (!taskId) return list;
	const rest = list.filter((h) => h.taskId !== taskId);
	const known = list.find((h) => h.taskId === taskId);
	switch (type) {
		case 'subagent_start':
			return [
				...rest,
				{
					taskId,
					description: typeof data.description === 'string' ? data.description : known?.description ?? '',
					activity: known?.activity ?? '',
					...(known?.call ? { call: known.call } : {}),
				},
			];
		case 'subagent_progress': {
			if (!known) return list;
			// A step is a call; background work says its own line.
			const next: HelperLine = isCallName(data.call)
				? { taskId, description: known.description, activity: '', call: data.call }
				: typeof data.current_operation === 'string'
					? { taskId, description: known.description, activity: data.current_operation }
					: known;
			return list.map((h) => (h.taskId === taskId ? next : h));
		}
		case 'subagent_complete':
			return rest;
	}
}
