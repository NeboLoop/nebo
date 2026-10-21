// Code work in the conversation is one line the size of a tool row, however
// many reviewers and rounds it had; its sheet holds every round as it goes,
// each reviewer and each of the project's checks; the status line above the
// composer carries the work still being checked beside the running tasks.
import { describe, it, expect, beforeAll, vi } from 'vitest';
import { render } from 'svelte/server';
import { addMessages, init } from 'svelte-i18n';
import en from '$lib/i18n/locales/en.json';
import type { VerificationView } from '$lib/api/neboComponents';
// Node's own localStorage stub has no methods; per-install state is memory here.
vi.mock('$lib/storage', () => {
	const m = new Map<string, string>();
	return {
		storage: {
			get: (k: string) => m.get(k) ?? null,
			set: (k: string, v: string) => void m.set(k, v),
			remove: (k: string) => void m.delete(k)
		}
	};
});

// The running tasks' transcript pulls in the remote desktop viewer; it
// touches `window` on import.
vi.mock('@novnc/novnc', () => ({ default: class {} }));

import VerifiedBadge from './VerifiedBadge.svelte';
import VerificationHistory from './VerificationHistory.svelte';
import VerificationQuestion from './VerificationQuestion.svelte';
import BackgroundStatus from '$lib/components/chat/BackgroundStatus.svelte';

beforeAll(() => {
	addMessages('en', en);
	init({ fallbackLocale: 'en', initialLocale: 'en' });
});

function view(state = 'verified', extra: Partial<VerificationView> = {}): VerificationView {
	return {
		id: 'v-1',
		sessionKey: 'agent:dev:web',
		messageId: 'm-1',
		createdAt: 100,
		finishedAt: 250,
		open: [],
		changed: [{ path: 'total.py', status: 'modified', added: 4, removed: 1 }],
		badge: {
			state,
			phase: state === 'checking' ? 'reviewers' : 'done',
			round: 2,
			project: 'shop',
			reviewers: 3,
			lenses: ['correctness', 'security'],
			rounds: 2,
			checks: ['Rust tests'],
			checksPassed: true,
			openBlockers: 0,
			openMajors: 0,
			openMinors: 0,
			deferred: 0,
			landed: 'abc1234567890',
			place: 'landed',
		},
		history: [
			{
				round: 1,
				kind: 'review',
				lenses: ['correctness', 'security'],
				findings: [{ id: 'r1-correctness-1', lens: 'correctness', severity: 'major', title: 'Off by one', evidence: 'total.py:12' }],
				verifiedOk: [],
				fixed: [],
				files: [{ path: 'total.py', status: 'modified', added: 4, removed: 1 }],
				reviewers: [
					{ lens: 'correctness', state: 'found', findings: 1 },
					{ lens: 'security', state: 'running', findings: 0 },
				],
				checkRuns: [
					{ label: 'Rust build', state: 'passed', durationMs: 4200, tail: '' },
					{ label: 'Rust tests', state: 'failed', durationMs: 9000, tail: 'assertion failed: total == 3' },
				],
				startedAt: 100,
				finishedAt: 160,
			},
			{
				round: 2,
				kind: 'review',
				lenses: ['correctness'],
				findings: [],
				verifiedOk: [],
				fixed: ['r1-correctness-1'],
				files: [{ path: 'total.py', status: 'modified', added: 1, removed: 1 }],
				reviewers: [{ lens: 'correctness', state: 'clean', findings: 0 }],
				checkRuns: [{ label: 'Rust tests', state: 'running', durationMs: 0, tail: '' }],
				startedAt: 200,
			},
		],
		...extra,
	} as unknown as VerificationView;
}

describe('VerifiedBadge', () => {
	it('is exactly one line, however many reviewers and rounds it had', () => {
		const out = render(VerifiedBadge, { props: { verification: view() } }).body;
		expect(out.match(/class="verify-line /g)?.length).toBe(1);
		expect(out).toContain('Verified');
		expect(out).not.toContain('Off by one');
		expect(out).not.toContain('Round');
		expect(out).not.toContain('$');
	});

	it('says how many issues are left when it was not verified', () => {
		const out = render(VerifiedBadge, {
			props: { verification: view('not_verified', { badge: { ...view().badge, state: 'not_verified', openMajors: 2 } }) },
		}).body;
		expect(out).toContain('2 issues left');
	});
});

describe('VerificationHistory', () => {
	it('shows each round with its reviewers, its checks and what changed since the round before', () => {
		const out = render(VerificationHistory, { props: { verification: view('checking') } }).body;
		expect(out).toContain('Round 1');
		expect(out).toContain('Round 2');
		expect(out).toContain('Changed since the last round');
		expect(out).toContain('found 1 issue');
		expect(out).toContain('reading now');
		expect(out).toContain('no issues');
		expect(out).toContain('Off by one');
		expect(out).toContain('total.py:12');
		expect(out).toContain('passed');
		expect(out).toContain('4s');
		expect(out).toContain('assertion failed: total == 3');
		expect(out).toContain('running now');
		expect(out).not.toContain('cost');
	});
});

describe('BackgroundStatus', () => {
	it('carries code work being checked on its one line, beside the running tasks', () => {
		const out = render(BackgroundStatus, {
			props: { sessionKey: 'agent:dev:web', checking: [view('checking', { messageId: undefined })] },
		}).body;
		expect(out.match(/class="bg-status-row"/g)?.length).toBe(1);
		expect(out).toContain('Checking · round 2');
		expect(out).toContain('verify-line-status');
	});

	it('shows nothing when nothing runs and nothing is checked', () => {
		const out = render(BackgroundStatus, { props: { sessionKey: 'agent:dev:web' } }).body;
		expect(out).not.toContain('bg-status-row');
	});
});

describe('VerificationQuestion', () => {
	it("asks the loop's question in its sheet, with each choice, Skip, and his own words", () => {
		const out = render(VerificationQuestion, {
			props: { ask: { question: 'Pin saves only to local store', options: ['Fix it', 'Leave it as it is'] }, onpick: () => {}, onclose: () => {} },
		}).body;
		expect(out).toContain('Pin saves only to local store');
		expect(out).toContain('data-answer="Fix it"');
		expect(out).toContain('data-answer="Leave it as it is"');
		expect(out).toContain('data-answer="__skip__"');
		expect(out).toContain('Tell it what to do');
	});
});
