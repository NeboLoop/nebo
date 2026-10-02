import { describe, expect, it } from 'vitest';
import { employeeLanding, type LandingApi } from './openEmployee';

const later = <T>(ms: number, v: T) => new Promise<T>((r) => setTimeout(() => r(v), ms));

function api(agent: { memoryMode?: string; isApp?: boolean } | null, chats: { id: string }[] | null, chatsMs = 0): LandingApi {
	return {
		getAgent: async () => (agent ? { ...agent, agent: { isApp: agent.isApp } } : null),
		listAgentChats: () => later(chatsMs, chats ? { chats } : null)
	};
}

describe('employeeLanding: where the first click on an employee lands', () => {
	it('waits for a slow chat list and opens the latest conversation, never the new-chat page', async () => {
		const to = await employeeLanding(api({ memoryMode: 'single' }, [{ id: 'c2' }, { id: 'c1' }], 80), 'e1');
		expect(to).toBe('/e1/threads/c2');
	});

	it('opens an app employee on its latest conversation (Flip-Flap, 2026-10-02)', async () => {
		const web = 'agent:ae6bc1be:web';
		const to = await employeeLanding(api({ isApp: true }, [{ id: web }, { id: 'old-thread' }], 50), 'ae6bc1be');
		expect(to).toBe(`/ae6bc1be/threads/${web}`);
		expect(to).not.toBe('/ae6bc1be/threads');
	});

	it('shows the new-chat page only when the employee truly has no conversation', async () => {
		expect(await employeeLanding(api({}, [], 30), 'e1')).toBe('/e1/threads');
	});

	it('opens an isolated employee on its list of matters', async () => {
		expect(await employeeLanding(api({ memoryMode: 'separate' }, [{ id: 'c1' }]), 'e1')).toBe('/e1/threads?list=e1');
	});
});
