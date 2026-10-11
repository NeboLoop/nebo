import { describe, expect, it } from 'vitest';
import { CHAT_MODES, chatModeName, differs, effective, fromBot, pendingFor, picked, setPending, takePending } from './chatMode';

describe("a conversation's permission mode", () => {
	it('lists the four modes, Automatic recommended and Full access with care', () => {
		expect(CHAT_MODES.map((m) => m.id)).toEqual(['automatic', 'ask', 'plan', 'full_access']);
		expect(CHAT_MODES.find((m) => m.pill === 'recommended')?.id).toBe('automatic');
		expect(CHAT_MODES.find((m) => m.pill === 'care')?.id).toBe('full_access');
		expect(chatModeName('plan')).toBe('chatMode.plan');
	});

	it("runs in the mode picked for it, else the employee's own, and tags only a difference", () => {
		const none = fromBot({ chatId: 'c1', mode: null, employeeMode: 'ask' } as never);
		expect(effective(none)).toBe('ask');
		expect(differs(none)).toBe(false);
		const plan = fromBot({ mode: 'plan', employeeMode: 'ask' });
		expect(effective(plan)).toBe('plan');
		expect(differs(plan)).toBe(true);
	});

	it("hands the conversation back to the employee when its own mode is picked", () => {
		const m = { mode: 'full_access', employeeMode: 'automatic' };
		expect(picked(m, 'automatic')).toEqual({ mode: '', employeeMode: 'automatic' });
		expect(picked(m, 'plan')).toEqual({ mode: 'plan', employeeMode: 'automatic' });
	});

	it('keeps a pick made before the conversation exists until the page takes it', () => {
		setPending('emp', 'plan');
		expect(pendingFor('emp')).toBe('plan');
		expect(takePending('emp')).toBe('plan');
		expect(takePending('emp')).toBe('');
		setPending('emp', 'ask');
		setPending('emp', '');
		expect(pendingFor('emp')).toBe('');
	});
});
