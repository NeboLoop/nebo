// A run's URL stays linkable — it opens the run modal over the workspace.
// Redirect in load, not in a component effect: the old approach mounted a
// page inside the legacy runs layout and the effect never ran.
import { redirect } from '@sveltejs/kit';
// Under the management tunnel the app lives under /t/<botID>/; a bare
// redirect escapes onto the hub's own site.
import { withBase } from '$lib/nav';

export function load({ params }: { params: { agentId: string; runId: string } }) {
	redirect(307, withBase(`/${params.agentId}/threads?run=${encodeURIComponent(params.runId)}`));
}
