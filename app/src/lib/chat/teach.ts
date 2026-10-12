// Teach-a-task: ONE action, behind the chat header's one monitor icon and
// the composer's Teach a task entry alike.
//
// Where it records is the bot's to say, from its real platform: a bot on a
// Mac, Windows or Linux desktop host records the owner's own screen, right
// where they work, with no window and no virtual machine; a cloud bot or a
// headless server has no screen and records on its computer, watched in the
// full-window view. The app never guesses: it asks the bot to start, and
// the bot's answer (`where`) says which it did.
//
// A recording runs at most the bot's cap (teach/start's `capSeconds`): the
// owner is warned TEACH_WARN_LEFT_S before it, and at it the recording stops
// and is learned from like any other. While it records, the owner's own
// microphone records his narration (the clip recorder, the same cap), and
// the stop carries it to the bot.

import { backendBase } from '$lib/api/base';
import { storage } from '$lib/storage';
import type { TeachStopResponse } from '$lib/api/neboComponents';

/** How long before the cap the owner is warned. */
export const TEACH_WARN_LEFT_S = 120;

/** Whether the recording is in its last TEACH_WARN_LEFT_S. */
export function teachEndingSoon(seconds: number, capSeconds: number): boolean {
	return capSeconds > 0 && seconds >= capSeconds - TEACH_WARN_LEFT_S;
}

/** Whether the recording has reached its cap. */
export function teachAtCap(seconds: number, capSeconds: number): boolean {
	return capSeconds > 0 && seconds >= capSeconds;
}

export interface TeachStop {
	agentId: string;
	sessionKey: string;
	/** The recording, from teach/start. */
	sessionId: string;
	/** The owner's narration, when he gave one. */
	narration?: File | null;
	/** How far into the recording the microphone started. */
	narrationOffsetMs?: number;
}

/** The stop's multipart body: the fields first, the narration last. */
export function teachStopForm(stop: TeachStop): FormData {
	const form = new FormData();
	form.append('agentId', stop.agentId);
	form.append('sessionKey', stop.sessionKey);
	form.append('sessionId', stop.sessionId);
	if (stop.narration) {
		form.append('narrationOffsetMs', String(Math.max(0, Math.round(stop.narrationOffsetMs ?? 0))));
		form.append('narration', stop.narration, stop.narration.name);
	}
	return form;
}

/** POST /api/v1/desktop/teach/stop. Multipart, so it is sent here rather
 *  than through the generated client, which speaks JSON only; the response
 *  type is the generated one. */
export async function postTeachStop(stop: TeachStop): Promise<TeachStopResponse> {
	const headers: Record<string, string> = {};
	const token = storage.get('nebo_token');
	if (token) headers['Authorization'] = `Bearer ${token}`;
	// No Content-Type: the browser sets the multipart boundary itself.
	const response = await fetch(`${backendBase()}/api/v1/desktop/teach/stop`, {
		method: 'POST',
		credentials: 'include',
		headers,
		body: teachStopForm(stop)
	});
	const text = await response.text();
	let parsed: unknown = {};
	try {
		parsed = text ? JSON.parse(text) : {};
	} catch {
		parsed = { error: text };
	}
	if (!response.ok) {
		const body = parsed as { error?: string; message?: string };
		throw new Error(body.error || body.message || `HTTP ${response.status}`);
	}
	return parsed as TeachStopResponse;
}

/** Whether a recording the bot started (teach/start's `where`) is watched in
 *  the full-window computer view. Only a recording of the host's own screen
 *  is not. */
export function watchesComputer(where: unknown): boolean {
	return where !== 'local';
}

/** Whether the one icon offers a choice (Teach a task, or just open the
 *  computer) instead of starting a recording at once. A cloud or headless
 *  bot always does: opening its computer to look at it must never record.
 *  A host bot does in Developer mode, where its virtual computer is still
 *  reachable. A host bot otherwise, or one not yet known, gets the one
 *  action. */
export function offersComputer(ownScreen: boolean | null, devMode: boolean): boolean {
	return ownScreen === false || (ownScreen === true && devMode);
}

/** The settings pane that grants Screen Recording on macOS. */
export const SCREEN_RECORDING_SETTINGS =
	'x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture';

/** What a failed start means for the owner: the message, and whether it is
 *  the missing Screen Recording permission (which gets the settings button). */
export function teachFailure(e: unknown): { message: string; needsScreenPermission: boolean } {
	const data = (e as { response?: { data?: { reason?: unknown } } })?.response?.data;
	return {
		message: e instanceof Error ? e.message : String(e),
		needsScreenPermission: data?.reason === 'screen_recording_permission'
	};
}
