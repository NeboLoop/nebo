import { describe, expect, it } from 'vitest';
import { offersVirtualComputer, teachFailure, watchesComputer, SCREEN_RECORDING_SETTINGS } from './teach';
import chatPane from '$lib/components/chat/ChatPane.svelte?raw';

// The header's teach icons, as rendered: every element marked data-teach-icon.
const header = chatPane.slice(chatPane.indexOf('<div class="ml-auto max-lg:hidden'), chatPane.indexOf('<!-- Narrow widths'));

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

	it('offers the virtual computer only on a host bot in Developer mode', () => {
		expect(offersVirtualComputer(true, true)).toBe(true);
		expect(offersVirtualComputer(true, false)).toBe(false);
		expect(offersVirtualComputer(false, true)).toBe(false);
		expect(offersVirtualComputer(null, true)).toBe(false);
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
		expect(chatPane).toContain('{#if teachActive || teachStarting || teachError}');
		expect(chatPane).toContain("$t('chat.teachStarting')");
	});
});
