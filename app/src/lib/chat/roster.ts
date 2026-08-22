import type { Agent } from '$lib/api/neboComponents';
import type { AgentInfo } from '$lib/chat/controller.svelte';

/**
 * The ONE mapper from a generated Agent to the mention-roster shape ChatPane
 * renders (@mention chips). Every surface that feeds listAgents() into a chat
 * pane goes through here so color/isApp handling can't drift.
 */
export function toMentionAgent(a: Agent): AgentInfo {
	return {
		id: a.id,
		name: a.name,
		role: a.description || '',
		initial: a.name.charAt(0).toUpperCase(),
		status: a.isEnabled ? 'online' : 'paused',
		color: a.color || 'teal',
		isApp: a.isApp ?? false
	};
}
