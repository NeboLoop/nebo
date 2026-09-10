import { describe, it, expect } from 'vitest';
import { get } from 'svelte/store';
import { notifications, approvalStatuses, setApprovalStatus, unreadCount, approvalRef, type Notification } from './notifications';

const row = (id: string, read: boolean, type: Notification['type'] = 'system'): Notification => ({
	id, type, title: id, message: '', time: 'now', createdAt: 1, read,
});

describe('unreadCount', () => {
	it('counts unread rows and approvals still pending, however many times they were read', () => {
		notifications.set([
			row('learn:a', true),        // read, but the decision is still open
			row('learn:b', true),        // read, already approved
			row('wf-fail:1', false, 'error'),
			row('wf-fail:2', true, 'error'),
		]);
		approvalStatuses.set({ 'learn:a': 'pending', 'learn:b': 'approved' });
		expect(get(unreadCount)).toBe(2);
		// Deciding from the inbox settles it for the badge too.
		setApprovalStatus('learn:a', 'approved');
		expect(get(unreadCount)).toBe(1);
	});

	it('recognizes the three approval id shapes and nothing else', () => {
		expect(approvalRef('wf-approval:run-1')).toEqual({ kind: 'workflow', id: 'run-1' });
		expect(approvalRef('learn:p-1')).toEqual({ kind: 'learning', id: 'p-1' });
		expect(approvalRef('artifact-update:skill:art-1:1.2.0')).toEqual({ kind: 'update', id: 'art-1' });
		expect(approvalRef('wf-fail:run-1')).toBeNull();
	});
});
