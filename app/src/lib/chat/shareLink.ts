import { neboAIShareLink, neboAISetShareLink, neboAITurnOffShareLink } from '$lib/api/nebo';
import type { FileShare } from '$lib/api/neboComponents';

/**
 * Sharing a Work-panel file by link — the one way to share one. The bot
 * uploads the file through the one upload path and the hub keeps the link
 * (https://neboai.com/s/<token>): who can open it, until when, and turning
 * it off. The phone's share screen calls the same three routes.
 */

export type ShareAccess = FileShare['access'];

/** How long a link lasts: never, or a number of days from now. `keep` holds
 *  the date an existing link already has. */
export type ShareExpiry = 'never' | 'keep' | '1' | '7' | '30';

/** The expiresAt a choice sends: '' for never, the link's own for keep. */
export function expiresAtFor(choice: ShareExpiry, current: string, now: Date = new Date()): string {
	if (choice === 'never') return '';
	if (choice === 'keep') return current;
	const at = new Date(now.getTime() + Number(choice) * 24 * 60 * 60 * 1000);
	at.setMilliseconds(0);
	return at.toISOString().replace('.000Z', 'Z');
}

// The query goes through genUrl, which does not encode: a file name with
// "&" or "#" would otherwise cut the reference short.
const q = (artifact: string) => encodeURIComponent(artifact);

/** The file's live link, or null. */
export async function loadShareLink(artifact: string): Promise<FileShare | null> {
	return (await neboAIShareLink(q(artifact))).share ?? null;
}

/** Give the file a link with these settings (creates one when it has none).
 *  An empty password keeps a password link's current one. */
export async function saveShareLink(
	artifact: string,
	access: ShareAccess,
	password: string,
	expiresAt: string
): Promise<FileShare | null> {
	const body: Record<string, unknown> = { artifact, access, expiresAt };
	if (access === 'password' && password) body.password = password;
	return (await neboAISetShareLink(body)).share ?? null;
}

/** Turn the file's link off, for good. */
export async function turnOffShareLink(artifact: string): Promise<void> {
	await neboAITurnOffShareLink(q(artifact));
}
