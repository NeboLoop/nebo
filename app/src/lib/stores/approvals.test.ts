import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

const sent: { type: string; data: unknown }[] = [];
vi.mock('$lib/websocket/client', () => ({
	getWebSocketClient: () => ({ send: (type: string, data: unknown) => sent.push({ type, data }) })
}));

import { approvals, approvalRaised, approvalSettled, answerApproval, chatApprovalsOf } from './approvals';

const raise = (requestId: string, sessionId: string) =>
	approvalRaised({ requestId, sessionId, agent: 'Bookkeeper', actionType: 'shell_command', actionDetail: 'ls' });

describe('an approval is a card in the chat that raised it, never a dialog', () => {
	beforeEach(() => {
		approvals.set([]);
		sent.length = 0;
	});

	it("shows only in its own chat; a schedule's never in a chat", () => {
		raise('r1', 'agent:bk:thread:t1');
		raise('r2', 'agent:bk:cron:morning');
		const list = get(approvals);
		expect(chatApprovalsOf(list, 'agent:bk:thread:t1').map((a) => a.requestId)).toEqual(['r1']);
		expect(chatApprovalsOf(list, 'agent:bk:thread:t2')).toEqual([]);
		expect(chatApprovalsOf(list, '')).toEqual([]);
	});

	it('Allow always answers always and the card keeps its place as a receipt', () => {
		raise('r1', 'agent:bk:thread:t1');
		answerApproval('r1', 'always');
		expect(sent).toEqual([{ type: 'approval_response', data: { request_id: 'r1', approved: true, always: true } }]);
		expect(get(approvals)[0].decision).toBe('always');
	});

	it('No denies; settled elsewhere, a late copy never shows open', () => {
		raise('r1', 'agent:bk:thread:t1');
		answerApproval('r1', 'deny');
		expect(sent[0].data).toEqual({ request_id: 'r1', approved: false, always: false });
		approvalSettled('r9', 'once');
		raise('r9', 'agent:bk:thread:t1');
		expect(get(approvals).find((a) => a.requestId === 'r9')?.decision).toBe('once');
	});
});
