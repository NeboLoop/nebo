import type { ShareMember } from '$lib/api/neboComponents';

/**
 * The ONE order for a list of bots: the ones you can reach first, then the
 * ones you cannot, and inside each group by name, case-insensitive.
 *
 * Reachability is the dot the row already shows — `isOnline` — not a second
 * signal invented for the sort, so a bot that comes online rises the moment
 * its dot turns green and nothing else has to agree. A bot in two loops is
 * listed once per loop (that is the row's identity), so loop id breaks a name
 * tie to keep the keyed list stable instead of letting two equal names swap
 * places on every re-render.
 *
 * Returns a new array: the caller's list is left as the server sent it.
 */
export function orderShareMembers(members: ShareMember[]): ShareMember[] {
	return [...members].sort((a, b) => {
		if (a.isOnline !== b.isOnline) return a.isOnline ? -1 : 1;
		const byName = a.botName.localeCompare(b.botName, undefined, { sensitivity: 'base' });
		if (byName !== 0) return byName;
		return a.loopId.localeCompare(b.loopId);
	});
}
