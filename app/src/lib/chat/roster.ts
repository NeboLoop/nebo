import type { AgentListEntry } from '$lib/api/neboComponents';
import { agentColorName } from '$lib/tokens';
import type { AgentInfo } from '$lib/chat/controller.svelte';

/**
 * The ONE mapper from a roster row to the mention-roster shape ChatPane
 * renders (@mention chips). Every surface that feeds listAgents() into a chat
 * pane goes through here so color/isApp handling can't drift.
 */
export function toMentionAgent(a: AgentListEntry): AgentInfo {
	return {
		id: a.id,
		name: a.name,
		role: a.description || '',
		initial: a.name.charAt(0).toUpperCase(),
		status: a.isEnabled ? 'online' : 'paused',
		color: agentColorName(a.id, a.color),
		isApp: a.isApp ?? false
	};
}
