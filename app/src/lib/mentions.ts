import { AGENT_COLORS_MAP } from '$lib/tokens.js';

export type MentionAgent = {
	id: string;
	name: string;
	initial?: string;
	color?: string;
	/** Hub identity — the wire may carry <@loopAgentId> instead of the local id. */
	loopAgentId?: string;
};

/**
 * The ONE mention renderer: replaces already-HTML-escaped `<@id>` tokens with
 * styled agent chips. `id` may be a local agent id or a hub loop_agent_id —
 * both resolve to the same chip, so transcripts never show a raw UUID.
 */
export function renderMentionChips(escapedHtml: string, agents: MentionAgent[]): string {
	return escapedHtml.replace(/&lt;@([a-zA-Z0-9._-]+)&gt;/g, (_, id) => {
		const agent = agents.find((a) => a.id === id || a.loopAgentId === id);
		if (!agent)
			return `<span class="inline-flex items-center gap-1 px-1.5 py-0.5 rounded-md text-xs font-medium bg-base-300 text-base-content/70 align-baseline">@unknown</span>`;
		const c = AGENT_COLORS_MAP[agent.color || 'teal'] || AGENT_COLORS_MAP['teal'];
		return `<span class="inline-flex items-center gap-1 px-1.5 py-0.5 rounded-md text-xs font-medium align-baseline ${c.bgClass} ${c.inkClass}"><span class="w-4 h-4 rounded-sm flex items-center justify-center text-xs font-semibold shrink-0">${agent.initial || agent.name.charAt(0).toUpperCase()}</span><span>${agent.name}</span></span>`;
	});
}
