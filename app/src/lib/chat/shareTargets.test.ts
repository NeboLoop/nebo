import { describe, it, expect } from 'vitest';
import type { ShareMember } from '$lib/api/neboComponents';
import { orderShareMembers } from './shareTargets';

function bot(botName: string, isOnline: boolean, loopName = 'Studio', loopId = 'l1'): ShareMember {
	return { botId: botName.toLowerCase().replace(/\s+/g, '-'), botName, loopId, loopName, isOnline };
}

const names = (list: ShareMember[]) => list.map((m) => m.botName);

describe('orderShareMembers', () => {
	it('puts the reachable bots first, then the unreachable, each group by name', () => {
		const ordered = orderShareMembers([
			bot('Zane', true),
			bot('Ada', false),
			bot('Marlow', true),
			bot('Bex', false)
		]);
		expect(names(ordered)).toEqual(['Marlow', 'Zane', 'Ada', 'Bex']);
	});

	it('sorts names case-insensitively rather than uppercase-first', () => {
		const ordered = orderShareMembers([bot('zeta', true), bot('Alpha', true), bot('beta', true)]);
		expect(names(ordered)).toEqual(['Alpha', 'beta', 'zeta']);
	});

	it('orders bots from different loops into one list, reachability first', () => {
		const ordered = orderShareMembers([
			bot('Quill', false, 'Shared with me', 'l2'),
			bot('Ada', true, 'Studio', 'l1'),
			bot('Nori', false, 'Studio', 'l1'),
			bot('Pike', true, 'Shared with me', 'l2')
		]);
		expect(names(ordered)).toEqual(['Ada', 'Pike', 'Nori', 'Quill']);
	});

	it('keeps the selected bot in the list, wherever the order puts it', () => {
		const members = [bot('Ada', false), bot('Marlow', true), bot('Zane', true)];
		const selected = 'ada';
		const ordered = orderShareMembers(members);
		expect(names(ordered)).toEqual(['Marlow', 'Zane', 'Ada']);
		expect(ordered.filter((m) => m.botId === selected)).toHaveLength(1);
		expect(ordered.findIndex((m) => m.botId === selected)).toBe(2);
	});

	it('moves a bot above the others the moment it comes online', () => {
		const before = [bot('Ada', true), bot('Marlow', true), bot('Zane', false)];
		expect(names(orderShareMembers(before))).toEqual(['Ada', 'Marlow', 'Zane']);

		const after = before.map((m) => (m.botName === 'Zane' ? { ...m, isOnline: true } : m));
		expect(names(orderShareMembers(after))).toEqual(['Ada', 'Marlow', 'Zane']);

		// A name that sorts early proves the move, not just the group flip.
		const nowOnline = [bot('Ada', true), bot('Marlow', true), bot('Bex', false)];
		expect(names(orderShareMembers(nowOnline))).toEqual(['Ada', 'Marlow', 'Bex']);
		const flipped = nowOnline.map((m) => (m.botName === 'Bex' ? { ...m, isOnline: true } : m));
		expect(names(orderShareMembers(flipped))).toEqual(['Ada', 'Bex', 'Marlow']);
	});

	it('breaks a name tie on loop id so the keyed list is stable', () => {
		const twice = [bot('Ada', true, 'Shared with me', 'l2'), bot('Ada', true, 'Studio', 'l1')];
		expect(orderShareMembers(twice).map((m) => m.loopId)).toEqual(['l1', 'l2']);
	});

	it('leaves the caller list untouched', () => {
		const members = [bot('Zane', false), bot('Ada', true)];
		orderShareMembers(members);
		expect(names(members)).toEqual(['Zane', 'Ada']);
	});

	it('handles an empty list', () => {
		expect(orderShareMembers([])).toEqual([]);
	});
});
