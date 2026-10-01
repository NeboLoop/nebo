/**
 * Talking to an app while it rebuilds itself, on the desktop.
 *
 * An app opens in its own window (`neboapp://<agentId>/`), and its page is the
 * app's own: the employee rewrites and reloads it. The voice call cannot live
 * there. It lives in the main window, in the ONE `voiceSession` store, as
 * every desktop call does. The app window carries a small voice pill: a
 * borderless child window (`voice-<agentId>`) pinned to the app window's
 * bottom-right corner (the shell keeps it there as the app window moves and
 * closes it with it), loading `/voice-pill/<agentId>`. The pill drives the
 * main window's call over Tauri events, and the main window reports the call
 * back:
 *
 *   pill → main  `app-voice:command`  { action: 'hello' | 'start' | 'end' | 'mute', agentId }
 *   main → all   `app-voice:state`    { status, agentId, muted }
 *
 * A page reload in the app window touches neither the pill nor the call.
 */

import { get } from 'svelte/store';
import { withBase } from '$lib/nav';
import { voiceSession, hasVoiceCloudConsent, type VoiceSessionStatus } from '$lib/stores/voiceSession';

export const VOICE_COMMAND = 'app-voice:command';
export const VOICE_STATE = 'app-voice:state';

export type AppVoiceAction = 'hello' | 'start' | 'end' | 'mute';

export interface AppVoiceCommand {
	action: AppVoiceAction;
	agentId: string;
}

export interface AppVoiceState {
	status: VoiceSessionStatus;
	/** The employee the call is with; null when there is no call. */
	agentId: string | null;
	muted: boolean;
}

/** The pill window's label for an app. */
export function voicePillLabel(agentId: string): string {
	return `voice-${agentId}`;
}

/** The pill's size, logical pixels. */
export const VOICE_PILL_SIZE = { width: 300, height: 48 };

/** The gap between the pill and the app window's corner, logical pixels. */
export const VOICE_PILL_MARGIN = 16;

function isTauri(): boolean {
	// eslint-disable-next-line @typescript-eslint/no-explicit-any
	return typeof window !== 'undefined' && !!(window as any).__TAURI_INTERNALS__;
}

/**
 * The main window's half: answers the pills' commands with the one call and
 * tells every pill what the call is doing. Returns the stop function. Only
 * the main window runs it; a no-op elsewhere.
 */
export async function startAppVoiceBridge(): Promise<() => void> {
	if (!isTauri()) return () => {};
	const { getCurrentWindow } = await import('@tauri-apps/api/window');
	if (getCurrentWindow().label !== 'main') return () => {};
	const { emit, listen } = await import('@tauri-apps/api/event');

	let last = '';
	const report = (force = false) => {
		const s = get(voiceSession);
		const state: AppVoiceState = {
			status: s.status,
			agentId: s.status === 'idle' ? null : s.agentId,
			muted: s.isMuted
		};
		const key = JSON.stringify(state);
		// The store changes with every microphone level; the pills hear only
		// what they show.
		if (!force && key === last) return;
		last = key;
		void emit(VOICE_STATE, state);
	};
	const unsubscribe = voiceSession.subscribe(() => report());

	const unlisten = await listen<AppVoiceCommand>(VOICE_COMMAND, (e) => {
		const cmd = e.payload;
		if (!cmd?.agentId) return;
		const s = get(voiceSession);
		const mine = s.status !== 'idle' && s.agentId === cmd.agentId;
		switch (cmd.action) {
			case 'hello':
				report(true);
				break;
			case 'start':
				// One call at a time; an ended call is put down before the next.
				if (s.status === 'error') voiceSession.stop();
				if (get(voiceSession).status !== 'idle' || !hasVoiceCloudConsent()) {
					report(true);
					break;
				}
				// No thread named: the server joins the employee's live
				// thread or starts one, as a call from an empty chat does.
				void voiceSession.start(cmd.agentId, '');
				break;
			case 'end':
				if (mine) voiceSession.stop();
				break;
			case 'mute':
				if (mine) voiceSession.toggleMute();
				break;
		}
	});

	return () => {
		unsubscribe();
		unlisten();
	};
}

/**
 * Open the app window's voice pill, pinned inside its bottom-right corner,
 * unless it is already open. Called right after the app window is created
 * (or focused); the shell moves it with the app window from then on.
 */
export async function openVoicePill(agentId: string, appLabel: string, appName: string): Promise<void> {
	if (!isTauri()) return;
	const { WebviewWindow } = await import('@tauri-apps/api/webviewWindow');
	const label = voicePillLabel(agentId);
	if (await WebviewWindow.getByLabel(label)) return;
	const app = await WebviewWindow.getByLabel(appLabel);
	if (!app) return;
	const scale = await app.scaleFactor();
	const at = (await app.innerPosition()).toLogical(scale);
	const size = (await app.innerSize()).toLogical(scale);
	const pill = new WebviewWindow(label, {
		url: new URL(
			withBase(`/voice-pill/${encodeURIComponent(agentId)}?name=${encodeURIComponent(appName)}`),
			window.location.origin
		).href,
		parent: appLabel,
		x: at.x + size.width - VOICE_PILL_SIZE.width - VOICE_PILL_MARGIN,
		y: at.y + size.height - VOICE_PILL_SIZE.height - VOICE_PILL_MARGIN,
		width: VOICE_PILL_SIZE.width,
		height: VOICE_PILL_SIZE.height,
		decorations: false,
		resizable: false,
		skipTaskbar: true,
		focus: false,
		shadow: true,
		title: ''
	});
	pill.once('tauri://error', (e) => {
		console.error('[voicePill] window error:', e);
	});
}
