import { describe, it, expect, vi } from 'vitest';
import { menuAnchor, deleteChatRow } from './chatMenu';

const row = { getBoundingClientRect: () => ({ left: 12, bottom: 240 }) } as unknown as Element;

describe('menuAnchor', () => {
	it('opens at the cursor on a right-click', () => {
		expect(menuAnchor({ clientX: 180, clientY: 96 }, row)).toEqual({ x: 180, y: 96 });
	});

	it('opens under the row when the keyboard fires the menu (no pointer position)', () => {
		expect(menuAnchor({ clientX: 0, clientY: 0 }, row)).toEqual({ x: 12, y: 240 });
	});
});

describe('deleteChatRow', () => {
	const chats = [
		{ id: 'a', title: 'New Chat' },
		{ id: 'b', title: '2026 Year To Date P&L' },
		{ id: 'c', title: '' }
	];

	it('deletes on the server, drops the row and leaves the open chat', async () => {
		const remove = vi.fn().mockResolvedValue({ success: true });
		const apply = vi.fn();
		const leave = vi.fn();
		await deleteChatRow({ chatId: 'b', openChatId: 'b', chats, remove, apply, leave });
		expect(remove).toHaveBeenCalledWith('b');
		expect(apply).toHaveBeenCalledWith([chats[0], chats[2]]);
		expect(leave).toHaveBeenCalledOnce();
	});

	it('stays on the open chat when another one is deleted, whatever its title', async () => {
		const apply = vi.fn();
		const leave = vi.fn();
		await deleteChatRow({ chatId: 'c', openChatId: 'a', chats, remove: vi.fn().mockResolvedValue({}), apply, leave });
		expect(apply).toHaveBeenCalledWith([chats[0], chats[1]]);
		expect(leave).not.toHaveBeenCalled();
	});

	it('changes nothing when the server refuses the delete', async () => {
		const apply = vi.fn();
		const leave = vi.fn();
		await expect(
			deleteChatRow({ chatId: 'a', openChatId: 'a', chats, remove: vi.fn().mockRejectedValue(new Error('500')), apply, leave })
		).rejects.toThrow('500');
		expect(apply).not.toHaveBeenCalled();
		expect(leave).not.toHaveBeenCalled();
	});
});
