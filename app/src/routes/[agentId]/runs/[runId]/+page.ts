// A run's URL stays linkable — it opens the run modal over the workspace.
// Redirect in load, not in a component effect: the old approach mounted a
// page inside the legacy runs layout and the effect never ran.
import { redirect } from '@sveltejs/kit';

export function load({ params }: { params: { agentId: string; runId: string } }) {
	redirect(307, `/${params.agentId}/threads?run=${encodeURIComponent(params.runId)}`);
}
