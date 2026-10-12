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

/** The file's link (null when it has none), and whether the file changed
 *  since the version a link that keeps its version opens. */
export interface ShareState {
	share: FileShare | null;
	outdated: boolean;
}

const stateOf = (r: { share?: FileShare; outdated?: boolean }): ShareState => ({
	share: r.share ?? null,
	outdated: !!r.outdated
});

/** The file's link, if it has one. */
export async function loadShareLink(artifact: string): Promise<ShareState> {
	return stateOf(await neboAIShareLink(q(artifact)));
}

/** Give the file a link with these settings (creates one when it has none).
 *  An empty password keeps a password link's current one. A live link
 *  follows the file; otherwise it keeps its version, and `newVersion` puts
 *  the file as it is now behind it. `address` publishes it as a site at
 *  <address>.nebo.page ('' stops; undefined keeps). */
export async function saveShareLink(
	artifact: string,
	access: ShareAccess,
	password: string,
	expiresAt: string,
	live: boolean,
	newVersion = false,
	address?: string
): Promise<ShareState> {
	const body: Record<string, unknown> = { artifact, access, expiresAt, live };
	if (access === 'password' && password) body.password = password;
	if (newVersion) body.newVersion = true;
	if (address !== undefined) body.address = address;
	return stateOf(await neboAISetShareLink(body));
}

/** A site address made from a file's title: "Grandview Party.html" →
 *  "grandview-party". The hub has the last word on what it accepts. */
export function siteAddressFor(title: string): string {
	return title
		.replace(/\.[a-z0-9]+$/i, '')
		.normalize('NFKD')
		.replace(/[\u0300-\u036f]/g, '')
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, '-')
		.replace(/^-+|-+$/g, '')
		.slice(0, 40)
		.replace(/-+$/, '');
}

/** Turn the file's link off, for good. */
export async function turnOffShareLink(artifact: string): Promise<void> {
	await neboAITurnOffShareLink(q(artifact));
}
