import { describe, expect, it } from 'vitest';
import { offersComputer, teachFailure, watchesComputer, SCREEN_RECORDING_SETTINGS, TEACH_WARN_LEFT_S, teachEndingSoon, teachAtCap, teachStopForm } from './teach';
import chatPane from '$lib/components/chat/ChatPane.svelte?raw';
import desktopView from '$lib/components/chat/DesktopView.svelte?raw';

// The header's teach icons, as rendered: every element marked data-teach-icon.
const header = chatPane.slice(chatPane.indexOf('<div class="ms-auto max-lg:hidden'), chatPane.indexOf('<!-- Narrow widths'));

describe('teach a task: one icon, one action', () => {
	it('renders exactly one monitor icon in the header, labelled Teach a task', () => {
		const teachBranch = header.slice(header.indexOf('{#if teachChoice'), header.indexOf('{#if flowsPane}'));
		// Two branches of ONE icon (with or without the Developer choice):
		// each renders the monitor once.
		const branches = teachBranch.split('{:else}');
		expect(branches).toHaveLength(2);
		for (const b of branches) {
			expect(b.match(/computerIcon/g)).toHaveLength(1);
			expect(b).toContain("$t('chatInput.teachTask')");
		}
		expect(header.match(/computerIcon/g)).toHaveLength(2);
		expect(header).not.toContain('chat.botComputer');
		expect(chatPane).not.toContain('virtualComputerIcon');
	});

	it('shares the one action with the composer', () => {
		expect(chatPane).toContain('onteach={teach}');
		expect(header).toContain('computerIcon)}');
		expect(header).toMatch(/headerIcon\([^)]*\$t\('chatInput\.teachTask'\), teach, computerIcon\)/);
	});

	it('lets the bot decide where: the one action names no screen', () => {
		expect(chatPane).toMatch(/function teach\(\) \{\s*if \(teachActive\) void stopTeach\(\);\s*else void startTeach\(\);/);
	});
});

describe('where the recording is watched', () => {
	it('a Mac/Windows/Linux desktop host records its own screen: no computer view', () => {
		expect(watchesComputer('local')).toBe(false);
	});

	it('a cloud or headless bot records on its computer: the computer view opens', () => {
		expect(watchesComputer('computer')).toBe(true);
	});

	it('offers the virtual computer on a host bot only in Developer mode', () => {
		expect(offersComputer(true, true)).toBe(true);
		expect(offersComputer(true, false)).toBe(false);
		expect(offersComputer(null, true)).toBe(false);
	});
});

describe('opening the computer is plain view mode, never a recording', () => {
	it('a cloud bot can always open its computer without teaching', () => {
		expect(offersComputer(false, false)).toBe(true);
		expect(offersComputer(false, true)).toBe(true);
	});

	it('the open choice only shows the computer: it starts no recording', () => {
		const menu = header.slice(header.indexOf('{#if teachChoice'), header.indexOf('{:else}'));
		const open = menu.slice(menu.indexOf('<li>', menu.indexOf('teach();')));
		expect(open).toContain('computerFull = true;');
		expect(open).not.toMatch(/teach\(|startTeach|teachStart/);
		expect(open).toContain("$t('chat.openComputer')");
	});

	it('the computer view records only from its own Record task control', () => {
		const view = chatPane.slice(chatPane.indexOf('<DesktopView'), chatPane.indexOf('/>', chatPane.indexOf('<DesktopView')));
		expect(view).toContain("onrecord={() => (teachActive ? stopTeach() : startTeach('computer'))}");
		expect(view).toContain('recording={teachActive}');
		expect(desktopView).not.toMatch(/teachStart|startTeach/);
	});
});

describe('a start that fails is never silent', () => {
	it('names missing Screen Recording permission, with the settings pane to fix it', () => {
		const e = Object.assign(new Error('Nebo needs Screen Recording permission'), {
			response: { status: 400, data: { reason: 'screen_recording_permission' } }
		});
		expect(teachFailure(e)).toEqual({ message: 'Nebo needs Screen Recording permission', needsScreenPermission: true });
		expect(SCREEN_RECORDING_SETTINGS).toContain('Privacy_ScreenCapture');
		expect(chatPane).toContain('href={SCREEN_RECORDING_SETTINGS}');
	});

	it('shows any other failure as it is', () => {
		expect(teachFailure(new Error('screen capture failed'))).toEqual({ message: 'screen capture failed', needsScreenPermission: false });
	});

	it('shows a starting state while the bot gets ready', () => {
		expect(chatPane).toContain('{#if teachActive || teachStarting || teachError || teachNotice}');
		expect(chatPane).toContain("$t('chat.teachStarting')");
	});
});

describe('a recording has a cap', () => {
	it('warns in its last two minutes and stops at the cap', () => {
		expect(TEACH_WARN_LEFT_S).toBe(120);
		expect(teachEndingSoon(479, 600)).toBe(false);
		expect(teachEndingSoon(480, 600)).toBe(true);
		expect(teachAtCap(599, 600)).toBe(false);
		expect(teachAtCap(600, 600)).toBe(true);
		expect(teachEndingSoon(10_000, 0) || teachAtCap(10_000, 0)).toBe(false);
	});

	it('the app stops at the cap like a normal stop, and says the cap was reached', () => {
		expect(chatPane).toContain('if (teachAtCap(teachSeconds, teachCap)) void stopTeach(true);');
		expect(chatPane).toContain("$t('chat.teachEndingSoon'");
		expect(chatPane).toContain("$t('chat.teachCapReached'");
	});
});

describe('the owner narrates while teaching', () => {
	it('the stop carries the narration after its fields', () => {
		const narration = new File(['abc'], 'recording-20261010-101500.m4a', { type: 'audio/mp4' });
		const form = teachStopForm({ agentId: 'a1', sessionKey: 'agent:a1:web', sessionId: 's1', narration, narrationOffsetMs: 812.4 });
		expect([...form.keys()]).toEqual(['agentId', 'sessionKey', 'sessionId', 'narrationOffsetMs', 'narration']);
		expect(form.get('narrationOffsetMs')).toBe('812');
		expect((form.get('narration') as File).name).toBe('recording-20261010-101500.m4a');
		const silent = teachStopForm({ agentId: 'a1', sessionKey: '', sessionId: 's1' });
		expect(silent.has('narration')).toBe(false);
	});

	it('the microphone records only while the demonstration does, with its cap', () => {
		const start = chatPane.slice(chatPane.indexOf('async function startTeach'), chatPane.indexOf('async function stopTeach'));
		expect(start).toContain('startNarration(recordingStartedAt)');
		expect(chatPane).toContain('capMs: teachCap > 0 ? teachCap * 1000 : undefined');
		const stop = chatPane.slice(chatPane.indexOf('async function stopTeach'), chatPane.indexOf('function fmtTeach'));
		expect(stop.indexOf('finishNarration()')).toBeLessThan(stop.indexOf('postTeachStop('));
		expect(chatPane.match(/createClipRecorder\(/g)).toHaveLength(1);
	});

	it('the record bar shows the microphone, its level and a mute toggle', () => {
		const bar = chatPane.slice(chatPane.indexOf('<!-- Teach-a-task record bar -->'), chatPane.indexOf('<!-- Composer'));
		expect(bar).toContain('data-teach-mic');
		expect(bar).toContain('narrator?.mute(');
		expect(bar).toContain('narratorState.level');
		expect(bar).toContain("$t('chat.teachMute')");
		expect(bar).toContain("$t('chat.teachNoMic')");
	});
});
