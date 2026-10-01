/**
 * The main window's half of an app window's voice pill: the pill's commands
 * drive the ONE call, and the main window tells the pills what the call is
 * doing — only when that changes, never on every microphone level.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { writable } from 'svelte/store';

type Handler = (e: { payload: unknown }) => void;
const emitted: Array<{ event: string; payload: any }> = [];
const handlers = new Map<string, Handler>();

vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }));
vi.mock('@tauri-apps/api/event', () => ({
	emit: async (event: string, payload: unknown) => {
		emitted.push({ event, payload });
	},
	listen: async (event: string, h: Handler) => {
		handlers.set(event, h);
		return () => handlers.delete(event);
	}
}));
vi.mock('$lib/nav', () => ({ withBase: (p: string) => p }));

const state = writable({ status: 'idle', agentId: null as string | null, isMuted: false, audioLevel: 0 });
const calls: string[] = [];
let consent = true;
vi.mock('$lib/stores/voiceSession', () => ({
	voiceSession: {
		subscribe: (fn: (v: unknown) => void) => state.subscribe(fn),
		start: async (agentId: string, chatId: string) => {
			calls.push(`start:${agentId}:${chatId}`);
			state.update((s) => ({ ...s, status: 'connecting', agentId }));
		},
		stop: () => {
			calls.push('stop');
			state.update((s) => ({ ...s, status: 'idle' }));
		},
		toggleMute: () => {
			calls.push('mute');
			state.update((s) => ({ ...s, isMuted: !s.isMuted }));
		}
	},
	hasVoiceCloudConsent: () => consent
}));

import { startAppVoiceBridge, VOICE_COMMAND, VOICE_STATE } from './voiceBridge';

const command = (action: string, agentId = 'kart') => handlers.get(VOICE_COMMAND)!({ payload: { action, agentId } });
const states = () => emitted.filter((e) => e.event === VOICE_STATE).map((e) => e.payload);

describe('app voice bridge (main window)', () => {
	beforeEach(() => {
		emitted.length = 0;
		calls.length = 0;
		consent = true;
		handlers.clear();
		state.set({ status: 'idle', agentId: null, isMuted: false, audioLevel: 0 });
		(globalThis as any).window = { __TAURI_INTERNALS__: {} };
	});

	it('starts the call with the app\'s employee, drives it, and reports only real changes', async () => {
		const stop = await startAppVoiceBridge();
		command('start');
		expect(calls).toEqual(['start:kart:']);
		state.update((s) => ({ ...s, status: 'listening' }));
		const before = states().length;
		state.update((s) => ({ ...s, audioLevel: 0.4 }));
		state.update((s) => ({ ...s, audioLevel: 0.7 }));
		expect(states().length).toBe(before);
		command('mute');
		command('end');
		expect(calls).toEqual(['start:kart:', 'mute', 'stop']);
		expect(states().at(-1)).toEqual({ status: 'idle', agentId: null, muted: true });
		stop();
	});

	it('never starts a second call, and leaves another employee\'s call alone', async () => {
		await startAppVoiceBridge();
		state.set({ status: 'listening', agentId: 'nanna', isMuted: false, audioLevel: 0 });
		command('start');
		command('end');
		command('mute');
		expect(calls).toEqual([]);
		command('hello');
		expect(states().at(-1)).toEqual({ status: 'listening', agentId: 'nanna', muted: false });
	});

	it('starts nothing without the cloud consent', async () => {
		consent = false;
		await startAppVoiceBridge();
		command('start');
		expect(calls).toEqual([]);
	});
});
