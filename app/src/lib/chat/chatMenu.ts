// The chat row's context menu (Rename / Delete) — the logic the sidebar row
// wires up, kept here so it is testable without mounting the shell.

/** Where the menu opens. A mouse right-click opens it at the cursor; the
 *  keyboard (Shift+F10 or the Menu key) fires `contextmenu` with no pointer
 *  position, so it opens under the row instead. */
export function menuAnchor(e: Pick<MouseEvent, 'clientX' | 'clientY'>, row: Element): { x: number; y: number } {
	if (e.clientX !== 0 || e.clientY !== 0) return { x: e.clientX, y: e.clientY };
	const r = row.getBoundingClientRect();
	return { x: r.left, y: r.bottom };
}

export interface DeleteChatRowDeps<C extends { id: string }> {
	chatId: string;
	/** The conversation open in the page right now ('' when none). */
	openChatId: string;
	chats: C[];
	/** Deletes the chat on the server (the generated client's `deleteChat`). */
	remove: (id: string) => Promise<unknown>;
	/** Receives the list without the deleted chat. */
	apply: (chats: C[]) => void;
	/** Called when the deleted chat was the open one, so the page moves off it. */
	leave: () => void;
}

/** Deletes a chat, drops its row and moves off it when it was open. A failed
 *  delete leaves the list and the page untouched and rethrows. */
export async function deleteChatRow<C extends { id: string }>(d: DeleteChatRowDeps<C>): Promise<void> {
	await d.remove(d.chatId);
	d.apply(d.chats.filter((c) => c.id !== d.chatId));
	if (d.openChatId === d.chatId) d.leave();
}
