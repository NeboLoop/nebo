import { redirect } from '@sveltejs/kit';
import { withBase } from '$lib/nav';

// /{id} is where a sidebar row lands. The click itself only navigates (a
// real link, so the URL changes at once and the route preloads on hover);
// deciding where the employee opens happens HERE, in the route's load, in
// one round trip: an app opens its overview, an isolated employee its list
// of matters, everyone else their latest conversation (or the new-chat page
// when they have none). Before this, the row's click handler awaited two
// requests before calling goto, and a busy backend made every click stall.
export const ssr = false;

export async function load({ params }) {
	const api = await import('$lib/api/nebo');
	const id = params.agentId;
	const [detail, chats] = await Promise.all([
		api.getAgent(id).catch(() => null),
		api.listAgentChats(id).catch(() => null)
	]);
	let target = `/${id}/threads`;
	if (detail?.agent?.isApp) {
		target = `/${id}/overview`;
	} else {
		let isolated = false;
		try {
			isolated = JSON.parse(detail?.agent?.frontmatter || '{}')?.memory?.context_isolated === true;
		} catch {
			/* unreadable frontmatter reads as not isolated, the same as the roster */
		}
		const latest = chats?.chats?.[0]?.id;
		if (isolated) target = `/${id}/threads?list=${encodeURIComponent(id)}`;
		else if (latest) target = `/${id}/threads/${latest}`;
	}
	redirect(307, withBase(target));
}
