import { describe, expect, it } from 'vitest';
import type { EnrichedChat } from '$lib/types/agentPage';
import { conversationLists, teammateLabel } from './teammates';

const en: Record<string, string> = {
	'sidebar.withTeammate': 'With {name}',
	'sidebar.teamThread': '{name} team'
};
const t = (key: string, opts: { values: Record<string, string> }) =>
	(en[key] ?? key).replace('{name}', opts.values.name);

function chat(id: string, kind: EnrichedChat['kind'], sessionName: string, title: string, withName: string | null = null): EnrichedChat {
	return {
		id,
		name: title,
		title,
		kind,
		with: withName,
		preview: '',
		updatedAt: 'just now',
		messages: 1,
		createdAt: 0,
		updatedAtEpoch: 0,
		sessionName
	};
}

// The list GET /agents/{id}/chats returns for the owner's coding employee:
// his own conversation, Top Coder's message to it, and its team seat.
const listed = {
	chats: [chat('mine', 'owner', 'agent:cc:web', 'Fix the login bug')],
	teammates: [
		chat('peer', 'colleague', 'agent:cc:coworker:top-coder', 'From Top Coder', 'Top Coder'),
		chat('seat', 'team', 'agent:cc:coworker:team:dev', 'Team: Development', 'Development')
	]
};

describe('an employee conversation list', () => {
	it('shows only the owner conversations by default', () => {
		const { own } = conversationLists(listed);
		expect(own.map((c) => c.id)).toEqual(['mine']);
		expect(own.some((c) => c.title === 'From Top Coder')).toBe(false);
	});

	it('keeps colleague and team threads for the teammates section, named by who they are with', () => {
		const { teammates } = conversationLists(listed);
		expect(teammates.map((c) => teammateLabel(c, t))).toEqual(['With Top Coder', 'Development team']);
	});

	it('never says team twice, and falls back to the title when no name is on record', () => {
		expect(teammateLabel(chat('s', 'team', 'agent:cc:coworker:team:x', 'Team: Growth Team', 'Growth Team'), t)).toBe('Growth Team');
		expect(teammateLabel(chat('p', 'colleague', 'agent:cc:coworker:case-1', 'Top Coder'), t)).toBe('With Top Coder');
	});

	it('is empty, not broken, before the list arrives', () => {
		expect(conversationLists(null)).toEqual({ own: [], teammates: [] });
	});
});
