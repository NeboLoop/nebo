import { describe, expect, it } from 'vitest';
import { isOpenQuestion, takesWords } from './ask';

// Live 2026-10-11: an open question came as a card with only "Other…" and
// Skip while the composer sat unused. An open question shows as the
// employee's message and the composer answers it; a card with options keeps
// its buttons; a card done in the app is never answered in words.
describe('how an ask shows', () => {
	it('a question with no options is an open question the composer answers', () => {
		expect(isOpenQuestion([{ type: 'options', multiSelect: false, options: [] }])).toBe(true);
		expect(isOpenQuestion([{ type: 'options' }])).toBe(true);
		expect(takesWords([{ type: 'options', options: [] }])).toBe(true);
	});

	it('a question with options keeps its card, and typed words still answer it', () => {
		const pick = [{ type: 'options' as const, options: ['Invoicing', 'Marketing'] }];
		expect(isOpenQuestion(pick)).toBe(false);
		expect(takesWords(pick)).toBe(true);
	});

	it('an install, a sign-in or a hire is done in the app', () => {
		for (const type of ['install_plugin', 'connect_account', 'hire_employee'] as const) {
			expect(isOpenQuestion([{ type }])).toBe(false);
			expect(takesWords([{ type }])).toBe(false);
		}
	});
});
