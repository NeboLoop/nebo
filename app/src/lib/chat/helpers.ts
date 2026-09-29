// Helpers an employee started from this conversation, as the server's
// subagent_start / subagent_progress / subagent_complete events describe
// them. A helper can outlive the turn that started it (it runs in the
// background), so this list is kept apart from the turn's own activity line.

export interface HelperLine {
	taskId: string;
	description: string;
	/** What it is doing now ("reading notes.md"); empty until its first step. */
	activity: string;
}

export type HelperEventType = 'subagent_start' | 'subagent_progress' | 'subagent_complete';

interface HelperEventData {
	task_id?: unknown;
	description?: unknown;
	current_operation?: unknown;
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
				},
			];
		case 'subagent_progress': {
			if (!known) return list;
			const activity = typeof data.current_operation === 'string' ? data.current_operation : known.activity;
			return list.map((h) => (h.taskId === taskId ? { ...h, activity } : h));
		}
		case 'subagent_complete':
			return rest;
	}
}
