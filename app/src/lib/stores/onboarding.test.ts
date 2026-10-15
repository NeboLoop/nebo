// The first-run gate: which of the server's steps shows what once setup is
// complete. intro → the three intro screens; a chat step → the guided chat
// resumes in the assistant's own chat (start is idempotent); done, or an
// older bot that answers 404 → nothing.
import { describe, it, expect, vi, beforeEach } from 'vitest';

const api = vi.hoisted(() => ({
	getOnboarding: vi.fn(),
	startOnboarding: vi.fn()
}));
vi.mock('$lib/api/nebo', () => api);

import { checkFirstRun, firstRunView, startFirstRun } from './onboarding';

const notFound = () => Object.assign(new Error('HTTP 404'), { response: { status: 404, data: {} } });

beforeEach(() => {
	api.getOnboarding.mockReset();
	api.startOnboarding.mockReset();
	api.startOnboarding.mockResolvedValue({ status: { step: 'name' }, threadId: 't1' });
});

describe('firstRunView', () => {
	it('maps every server step', () => {
		expect(firstRunView('intro')).toBe('intro');
		for (const step of ['name', 'focus', 'connect', 'plate', 'day', 'daily', 'hire', 'set']) {
			expect(firstRunView(step)).toBe('chat');
		}
		expect(firstRunView('done')).toBe('done');
		expect(firstRunView(undefined)).toBe('done');
		expect(firstRunView('something-new')).toBe('done');
	});
});

describe('checkFirstRun', () => {
	it('shows the intro screens at intro, without starting anything', async () => {
		api.getOnboarding.mockResolvedValue({ step: 'intro' });
		expect(await checkFirstRun()).toBe('intro');
		expect(api.startOnboarding).not.toHaveBeenCalled();
	});

	it('resumes a chat step once', async () => {
		api.getOnboarding.mockResolvedValue({ step: 'connect' });
		expect(await checkFirstRun()).toBe('chat');
		expect(api.startOnboarding).toHaveBeenCalledTimes(1);
	});

	it('shows nothing when done', async () => {
		api.getOnboarding.mockResolvedValue({ step: 'done' });
		expect(await checkFirstRun()).toBe('done');
		expect(api.startOnboarding).not.toHaveBeenCalled();
	});

	it('treats an older bot (404) as done', async () => {
		api.getOnboarding.mockRejectedValue(notFound());
		expect(await checkFirstRun()).toBe('done');
		expect(api.startOnboarding).not.toHaveBeenCalled();
	});

	it('treats an older bot that answers with the app page (no step) as done', async () => {
		api.getOnboarding.mockResolvedValue({ message: '<!doctype html>' });
		expect(await checkFirstRun()).toBe('done');
		expect(api.startOnboarding).not.toHaveBeenCalled();
	});
});

describe('startFirstRun', () => {
	it('starts the chat and returns the assistant thread to open', async () => {
		expect(await startFirstRun()).toBe('t1');
		expect(api.startOnboarding).toHaveBeenCalledTimes(1);
	});

	it('opens nothing on an older bot (404)', async () => {
		api.startOnboarding.mockRejectedValue(notFound());
		expect(await startFirstRun()).toBeNull();
	});
});
