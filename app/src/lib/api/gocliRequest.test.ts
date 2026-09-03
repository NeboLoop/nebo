import { describe, it, expect, vi, afterEach } from 'vitest';
import { fetchBounded, READ_TIMEOUT_MS, RETRY_DELAY_MS } from './gocliRequest';

// A fetch that never answers until its signal aborts, then one that answers.
function hangingThenOk() {
	let calls = 0;
	const impl = ((_url: string, init?: RequestInit) => {
		calls++;
		if (calls === 1) {
			return new Promise<Response>((_, reject) => {
				init?.signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')));
			});
		}
		return Promise.resolve(new Response('{"ok":true}', { status: 200 }));
	}) as typeof fetch;
	return { impl, calls: () => calls };
}

describe('fetchBounded', () => {
	afterEach(() => vi.useRealTimers());

	it('retries a hung read once after the timeout', async () => {
		vi.useFakeTimers();
		const f = hangingThenOk();
		const p = fetchBounded('http://x/api/v1/agents', { method: 'GET' }, f.impl);
		await vi.advanceTimersByTimeAsync(READ_TIMEOUT_MS + RETRY_DELAY_MS);
		const { text } = await p;
		expect(text).toBe('{"ok":true}');
		expect(f.calls()).toBe(2);
	});

	it('never retries a write', async () => {
		vi.useFakeTimers();
		const f = hangingThenOk();
		const p = fetchBounded('http://x/api/v1/agents', { method: 'POST' }, f.impl).catch((e) => e);
		await vi.advanceTimersByTimeAsync(120_000 + RETRY_DELAY_MS);
		expect((await p).name).toBe('AbortError');
		expect(f.calls()).toBe(1);
	});
});
