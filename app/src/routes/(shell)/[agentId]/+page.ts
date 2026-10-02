import { redirect } from '@sveltejs/kit';
import { withBase } from '$lib/nav';
import { employeeLanding } from '$lib/chat/openEmployee';

// /{id} is where a sidebar row lands. The click itself only navigates (a
// real link, so the URL changes at once and the route preloads on hover);
// deciding where the employee opens happens HERE, in the route's load, in
// one round trip (`employeeLanding`): an isolated employee its list of
// matters, everyone else — apps included — their latest conversation (or
// the new-chat page when they have none). Before this, the row's click
// handler awaited two requests before calling goto, and a busy backend made
// every click stall.
export const ssr = false;

export async function load({ params }) {
	const api = await import('$lib/api/nebo');
	redirect(307, withBase(await employeeLanding(api, params.agentId)));
}
