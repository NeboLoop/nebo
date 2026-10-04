/**
 * `window.share_menu`: the header's Share button, an app's own ways to share
 * or export its work. Picking an entry sends its `say` into the app's chat
 * as the owner's words, the way Publish sends its starter.
 */

import type { AppWindow } from '$lib/api/neboComponents';

export type ShareEntry = AppWindow['shareMenu'][number];

/** The entries the header's Share button lists for this chat: none for an
 *  employee that is not an app, or a chat the owner cannot send in. */
export function shareEntries(appWindow: Partial<AppWindow> | null | undefined, canSend: boolean): ShareEntry[] {
	if (!canSend) return [];
	return (appWindow?.shareMenu ?? []).filter((e) => !!e?.label?.trim() && !!e?.say?.trim());
}
