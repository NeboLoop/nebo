// The dependencies an employee's install is still bringing in, per employee.
// The install modal's Done never waits on them: they land in the background
// and the employee's roster row shows how far along they are. The server's
// `dep_*` events name the employee a cascade sets up (`agentId`), so every
// client's row follows the same cascade, whichever client asked for it.

import { writable } from 'svelte/store';
import { onWsEvent } from '$lib/websocket/subscribe';

export type SetupProgress = { settled: number; total: number };

/** Employee id → its cascade's progress, while one is running. */
export const employeeSetup = writable<Record<string, SetupProgress>>({});

type DepEvent = { agentId?: string | null; total?: number };

export const SETUP_EVENTS = ['dep_cascade_start', 'dep_installed', 'dep_failed', 'dep_cascade_complete'] as const;
export type SetupEvent = (typeof SETUP_EVENTS)[number];

/** The progress after one `dep_*` event. Events of a cascade that sets up no
 *  employee change nothing. */
export function applySetupEvent(
	state: Record<string, SetupProgress>,
	kind: SetupEvent,
	data: DepEvent | null | undefined
): Record<string, SetupProgress> {
	const id = data?.agentId;
	if (!id) return state;
	switch (kind) {
		case 'dep_cascade_start':
			return { ...state, [id]: { settled: 0, total: data?.total ?? 0 } };
		case 'dep_installed':
		case 'dep_failed': {
			const was = state[id];
			if (!was) return state;
			// A dependency's own dependencies count as they land.
			const settled = was.settled + 1;
			return { ...state, [id]: { settled, total: Math.max(was.total, settled) } };
		}
		case 'dep_cascade_complete': {
			if (!(id in state)) return state;
			const { [id]: _done, ...rest } = state;
			return rest;
		}
	}
}

/** Follow every cascade's progress into `employeeSetup`. Call once, from the
 *  shell's setup (the subscriptions end with it). */
export function trackEmployeeSetup(): void {
	for (const kind of SETUP_EVENTS) {
		onWsEvent<DepEvent>(kind, (data) => employeeSetup.update((s) => applySetupEvent(s, kind, data)));
	}
}
