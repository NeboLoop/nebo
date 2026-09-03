import { describe, it, expect } from 'vitest';
import { finishUserTranscript } from './voiceTranscript';

describe('finishUserTranscript', () => {
	it('replaces the open entry with the corrected finish and appends after an agent turn', () => {
		const first = finishUserTranscript([], 'what would you add', false);
		expect(first).toEqual([{ speaker: 'user', text: 'what would you add' }]);
		const corrected = finishUserTranscript(first, 'what would you add to it?', true);
		expect(corrected).toEqual([{ speaker: 'user', text: 'what would you add to it?' }]);
		const afterAgent = finishUserTranscript([...corrected, { speaker: 'agent', text: 'Two things.' }], 'thanks', false);
		expect(afterAgent.map((e) => e.speaker)).toEqual(['user', 'agent', 'user']);
		expect(finishUserTranscript(corrected, '', true)).toBe(corrected);
	});
});
