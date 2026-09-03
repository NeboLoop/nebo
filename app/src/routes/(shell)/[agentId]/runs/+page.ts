// The runs list lives in a modal over the workspace now.
import { redirect } from '@sveltejs/kit';
// Under the management tunnel the app lives under /t/<botID>/; a bare
// redirect escapes onto the hub's own site.
import { withBase } from '$lib/nav';

export function load({ params }: { params: { agentId: string } }) {
	redirect(307, withBase(`/${params.agentId}/threads?runs=1`));
}
