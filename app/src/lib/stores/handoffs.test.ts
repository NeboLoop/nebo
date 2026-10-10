import { describe, it, expect, vi } from 'vitest';

vi.mock('$lib/websocket/client', () => ({
	getWebSocketClient: () => ({ on: () => () => {} })
}));

import { isLive, receiverHref, statusLine } from './handoffs';
import type { HandoffView } from '$lib/api/neboComponents';

const base: HandoffView = {
	id: 'h1',
	kind: 'message',
	fromAgentId: 'lead',
	fromName: 'Office Lead',
	toAgentId: 'bk',
	toName: 'Bookkeeper',
	teamId: '',
	senderSession: 'agent:lead:web',
	senderLink: '/lead/threads/agent%3Alead%3Aweb',
	receiverSession: 'agent:bk:coworker:lead',
	receiverLink: '/bk/threads/agent%3Abk%3Acoworker%3Alead',
	ask: 'Pull last month’s invoices',
	status: 'running',
	result: '',
	error: '',
	createdAt: 1
};

const tr = (key: string, opts?: { values: Record<string, string> }) =>
	opts ? `${key}(${Object.values(opts.values).join(',')})` : key;

describe('hand-offs', () => {
	it('is live while queued or running', () => {
		expect(isLive(base)).toBe(true);
		expect(isLive({ ...base, status: 'queued' })).toBe(true);
		expect(isLive({ ...base, status: 'failed' })).toBe(false);
	});

	it('opens a message in the coworker transcript over the current page', () => {
		const href = receiverHref(base, new URL('http://x/lead/threads/t1?run=r'));
		const url = new URL(href, 'http://x');
		expect(url.pathname).toBe('/lead/threads/t1');
		expect(url.searchParams.get('cw')).toBe('agent:bk:coworker:lead');
		expect(url.searchParams.get('cwf')).toBe('Office Lead');
		expect(url.searchParams.get('run')).toBe('r');
	});

	it('opens an assignment at its case', () => {
		const h = { ...base, kind: 'assignment', receiverLink: '/bk/cases?case=c1' };
		expect(receiverHref(h, new URL('http://x/lead'))).toBe('/bk/cases?case=c1');
	});

	it('says what came back, and why it failed', () => {
		expect(statusLine(base, tr)).toBe('handoff.status.running');
		expect(statusLine({ ...base, status: 'done', result: '\n12 invoices found.\nMore detail' }, tr)).toBe('handoff.doneWith(12 invoices found.)');
		expect(statusLine({ ...base, status: 'done' }, tr)).toBe('handoff.status.done');
		expect(statusLine({ ...base, status: 'failed', error: 'Could not connect.' }, tr)).toBe('handoff.failedWith(Could not connect.)');
		expect(statusLine({ ...base, status: 'stopped' }, tr)).toBe('handoff.status.stopped');
	});
});
