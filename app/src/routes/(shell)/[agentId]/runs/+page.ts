// The runs list lives in a modal over the workspace now.
import { redirect } from '@sveltejs/kit';

export function load({ params }: { params: { agentId: string } }) {
	redirect(307, `/${params.agentId}/threads?runs=1`);
}
