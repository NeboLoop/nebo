import { describe, expect, it } from 'vitest';
import { computerActions, watchesComputer } from './teach';

describe('teach-a-task on a bot with its own screen', () => {
	it('records locally by default: the monitor teaches, no computer view', () => {
		expect(computerActions(true, false)).toEqual({ teach: true, computer: false });
	});

	it('keeps the virtual computer behind Developer mode', () => {
		expect(computerActions(true, true)).toEqual({ teach: true, computer: true });
	});

	it('does not open the computer view for a local recording', () => {
		expect(watchesComputer('local')).toBe(false);
	});
});

describe('teach-a-task on a cloud bot', () => {
	it('keeps the computer, whatever Developer mode says', () => {
		expect(computerActions(false, false)).toEqual({ teach: false, computer: true });
		expect(computerActions(false, true)).toEqual({ teach: false, computer: true });
	});

	it('watches the recording on the computer', () => {
		expect(watchesComputer('computer')).toBe(true);
	});
});

describe('before the bot has answered', () => {
	it('behaves as it always did', () => {
		expect(computerActions(null, false)).toEqual({ teach: false, computer: true });
		expect(watchesComputer(undefined)).toBe(true);
	});
});
