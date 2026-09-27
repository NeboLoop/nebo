import { writable } from 'svelte/store';

// The bot's name, as NeboAI holds it. The name is the owner's: the web app
// and the phone rename it with the owner's own session, and Nebo only reads
// it. "" until loaded, and when the bot is not connected to NeboAI.
export const botName = writable('');
// The bot's page on the NeboAI web app, where the owner renames it.
export const botRenameUrl = writable('');

let loaded: Promise<void> | null = null;

/**
 * Load the bot's name. Later calls reuse the first load unless `fresh` is
 * set (back from renaming it on the web).
 */
export function loadBotName(fresh = false): Promise<void> {
	if (fresh) loaded = null;
	loaded ??= (async () => {
		try {
			const api = await import('$lib/api/nebo');
			const res = await api.neboAIGetBot();
			botName.set(res?.name ?? '');
			botRenameUrl.set(res?.renameUrl ?? '');
		} catch {
			// NeboAI unreachable: the name stays as last known.
			loaded = null;
		}
	})();
	return loaded;
}

/** Open the bot's page on the NeboAI web app, where the owner renames it. */
export function openBotRename(url: string): void {
	if (url) window.open(url, '_blank', 'noopener,noreferrer');
}

/**
 * The bot and its primary employee start with the same name. Right after one
 * of them is renamed, offer to rename the other — only when they matched
 * before, so two names the owner already chose apart are left alone.
 */
export function offerMatchingRename(before: string, other: string, after: string): boolean {
	const b = before.trim();
	const a = after.trim();
	return b !== '' && a !== '' && a !== b && b === other.trim();
}
