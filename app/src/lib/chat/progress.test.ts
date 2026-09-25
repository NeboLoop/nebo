import { describe, expect, it } from 'vitest';
import { isThinking } from './progress';

describe('isThinking', () => {
	it('is true while the conversation’s run has no call running', () => {
		const runs = [{ sessionKey: 'agent:ops:web', currentTool: '', activity: '' }];
		expect(isThinking(runs, 'agent:ops:web')).toBe(true);
	});

	it('is false while a call runs: the call’s own label is the status', () => {
		const runs = [{ sessionKey: 'agent:ops:web', currentTool: 'read', activity: 'Reading notes.md' }];
		expect(isThinking(runs, 'agent:ops:web')).toBe(false);
	});

	it('reads only this conversation’s run', () => {
		const runs = [{ sessionKey: 'agent:other:web', currentTool: '' }];
		expect(isThinking(runs, 'agent:ops:web')).toBe(false);
		expect(isThinking(undefined, 'agent:ops:web')).toBe(false);
		expect(isThinking(runs, '')).toBe(false);
	});
});
