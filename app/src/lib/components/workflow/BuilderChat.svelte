<script lang="ts">
	import ChatPane from '$lib/components/chat/ChatPane.svelte';
	import { createChatController } from '$lib/chat/controller.svelte';
	import { ARCHITECT_INTRO_MESSAGE } from '$lib/tokens.js';
	import { onDestroy } from 'svelte';
	import type { WorkflowConfig } from '$lib/types/agentPage';

	let {
		agentId = '',
		workflows = {},
		selectedWorkflowName = null,
		selectedActivityId = null,
		onaction,
	}: {
		agentId: string;
		workflows: Record<string, WorkflowConfig>;
		selectedWorkflowName: string | null;
		selectedActivityId: string | null;
		onaction?: (action: string, payload: unknown) => void;
	} = $props();

	let chat = $state<ReturnType<typeof createChatController> | null>(null);
	let sessionKey = $state<string | null>(null);
	let initLoading = $state(false);
	let initDone = $state(false);

	$effect(() => {
		// Auto-init on mount (only once)
		if (!initDone && agentId) {
			initChat();
		}
	});

	async function initChat() {
		if (initLoading) return;
		initLoading = true;
		try {
			const api = await import('$lib/api/nebo');
			const resp = await api.startWorkflowChat(agentId, {
				workflows: $state.snapshot(workflows),
				selectedWorkflow: selectedWorkflowName || '',
				selectedActivity: selectedActivityId || '',
			}) as { sessionKey: string; agentId: string };

			if (resp.sessionKey) {
				sessionKey = resp.sessionKey;
				chat = createChatController({
					agentId: resp.agentId || agentId,
					sessionKey: resp.sessionKey,
					channel: 'help:workflow',
				});
				// Load the seeded messages (system context + greeting)
				try {
					const msgs = await api.getSessionMessages(resp.sessionKey) as {
						messages?: { id: string; role: string; content: string; html?: string }[];
					};
					if (msgs?.messages?.length) {
						chat.setMessages(
							msgs.messages
								.filter((m) => m.role === 'user' || m.role === 'assistant')
								.map((m) => ({
									id: m.id,
									type: m.role as 'user' | 'assistant',
									content: m.content,
									html: m.html || undefined,
								}))
						);
					}
				} catch {
					/* first visit — greeting already in controller */
				}
			}
		} catch {
			/* silent — fall back to intro message */
		} finally {
			initLoading = false;
			initDone = true;
		}
	}

	onDestroy(() => {
		if (chat) {
			chat.destroy();
			chat = null;
		}
	});
</script>

<ChatPane
	messages={chat?.messages ?? [{ ...ARCHITECT_INTRO_MESSAGE }]}
	agentName="Architect"
	{agentId}
	sessionId={sessionKey ?? ''}
	placeholder="Describe a workflow change..."
	emptyIcon="A"
	emptyTitle="Architect"
	emptyDesc="I can help you build and modify workflows. Describe what you want and I'll make it happen."
	onsend={(text) => chat?.send(text)}
	onstop={() => chat?.stop()}
	isLoading={chat?.isLoading ?? initLoading}
/>
