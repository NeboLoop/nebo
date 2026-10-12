import type { ChatPermissionMode, Mode } from '$lib/api/neboComponents';

/**
 * The four modes one conversation can run in, in the Permission sheet's
 * order: the id the bot takes and its i18n keys (`chatMode.*`).
 */
export const CHAT_MODES: { id: Mode; name: string; desc: string; pill?: 'recommended' | 'care' }[] = [
	{ id: 'automatic', name: 'chatMode.automatic', desc: 'chatMode.automaticDesc', pill: 'recommended' },
	{ id: 'ask', name: 'chatMode.ask', desc: 'chatMode.askDesc' },
	{ id: 'plan', name: 'chatMode.plan', desc: 'chatMode.planDesc' },
	{ id: 'full_access', name: 'chatMode.fullAccess', desc: 'chatMode.fullAccessDesc', pill: 'care' }
];

/** A mode's short name key, as the + menu's Permission row and the tag by
 *  the model chip show it. */
export function chatModeName(mode: string): string {
	return (CHAT_MODES.find((m) => m.id === mode) ?? CHAT_MODES[0]).name;
}

/** One conversation's mode as the composer holds it: the one picked for
 *  it (`''` = none, the employee's own) and the employee's own (`''` until
 *  read). */
export type ConversationMode = { mode: string; employeeMode: string };

/** The bot's answer (`GET`/`PUT /chats/{id}/permission-mode`) or its
 *  `chat_mode` event, as the composer holds it. */
export function fromBot(view: Partial<ChatPermissionMode> | { mode?: string | null; employeeMode?: string }): ConversationMode {
	return { mode: view.mode ?? '', employeeMode: view.employeeMode ?? 'automatic' };
}

/** The mode the conversation's turns run in. */
export function effective(m: ConversationMode): string {
	return m.mode || m.employeeMode;
}

/** The conversation runs in a mode other than the employee's own: the tag
 *  by the model chip shows it. */
export function differs(m: ConversationMode): boolean {
	return !!m.mode && m.mode !== m.employeeMode;
}

/** What a pick in the sheet stores: the employee's own mode is no pick (the
 *  conversation follows the employee again). */
export function picked(m: ConversationMode, choice: string): ConversationMode {
	return { mode: choice === m.employeeMode ? '' : choice, employeeMode: m.employeeMode };
}

/**
 * A mode picked on the New chat page, before its conversation exists, by
 * employee. The page saves it on the conversation it makes before its first
 * message is sent (`takePending`), so the first turn already runs in it.
 */
const pending = new Map<string, string>();

export function setPending(agentId: string, mode: string): void {
	if (mode) pending.set(agentId, mode);
	else pending.delete(agentId);
}

export function pendingFor(agentId: string): string {
	return pending.get(agentId) ?? '';
}

export function takePending(agentId: string): string {
	const mode = pendingFor(agentId);
	pending.delete(agentId);
	return mode;
}
