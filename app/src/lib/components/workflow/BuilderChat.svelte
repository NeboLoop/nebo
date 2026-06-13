<script lang="ts">
	import ChatPane from '$lib/components/chat/ChatPane.svelte';
	import { createChatController } from '$lib/chat/controller.svelte';
	import { ARCHITECT_INTRO_MESSAGE } from '$lib/tokens.js';
	import { extractOpsBlock, type WorkflowOp } from '$lib/utils/workflowOps';
	import { onDestroy } from 'svelte';
	import type { WorkflowConfig } from '$lib/types/agentPage';

	let {
		agentId = '',
		workflows = {},
		selectedWorkflowName = null,
		selectedActivityId = null,
		onops,
	}: {
		agentId: string;
		workflows: Record<string, WorkflowConfig>;
		selectedWorkflowName: string | null;
		selectedActivityId: string | null;
		/** Apply Architect edit ops to the builder draft; returns the summary. */
		onops?: (ops: WorkflowOp[]) => {
			applied: string[];
			skipped: { op: string; reason: string }[];
		};
	} = $props();

	let chat = $state<ReturnType<typeof createChatController> | null>(null);
	let sessionKey = $state<string | null>(null);
	let initLoading = $state(false);
	let initDone = $state(false);
	/** Draft state the Architect's system context was last synced against. */
	let lastSyncedDraft = $state('');

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
			const snapshot = $state.snapshot(workflows);
			const resp = await api.startWorkflowChat(agentId, {
				workflows: snapshot,
				selectedWorkflow: selectedWorkflowName || '',
				selectedActivity: selectedActivityId || '',
			}) as { sessionKey: string; agentId: string };
			lastSyncedDraft = JSON.stringify(snapshot);

			if (resp.sessionKey) {
				sessionKey = resp.sessionKey;
				chat = createChatController({
					agentId: resp.agentId || agentId,
					sessionKey: resp.sessionKey,
					channel: 'help:workflow',
					onResponseComplete: handleArchitectReply,
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

	/**
	 * Send a message, resyncing the Architect's system context first when the
	 * draft changed since the last sync (canvas edits, applied ops, undo) —
	 * the seeded snapshot must never go stale or the model edits ghosts.
	 */
	async function sendWithSync(text: string) {
		if (!chat) return;
		const draft = JSON.stringify($state.snapshot(workflows));
		if (draft !== lastSyncedDraft && sessionKey) {
			try {
				const api = await import('$lib/api/nebo');
				await api.startWorkflowChat(agentId, {
					workflows: $state.snapshot(workflows),
					selectedWorkflow: selectedWorkflowName || '',
					selectedActivity: selectedActivityId || '',
					refresh: true,
				});
				lastSyncedDraft = draft;
			} catch {
				/* stale context is better than a lost message — send anyway */
			}
		}
		chat.send(text);
	}

	/**
	 * On reply completion: extract a ```workflow-ops block, apply it to the
	 * builder draft (one undo snapshot), and rewrite the rendered message —
	 * the raw ops JSON is transport, not conversation.
	 */
	function handleArchitectReply(content: string) {
		if (!chat || !onops) return;
		const extracted = extractOpsBlock(content);
		if (!extracted) return;

		const { applied, skipped } = onops(extracted.ops);

		let summary = '';
		if (applied.length > 0) {
			summary += `\n\n**Applied to draft:**\n${applied.map((a) => `- ${a}`).join('\n')}`;
		}
		if (skipped.length > 0) {
			summary += `\n\n**Skipped:**\n${skipped.map((s) => `- ${s.op}: ${s.reason}`).join('\n')}`;
		}
		if (applied.length > 0) {
			summary += '\n\n_Review on the canvas — undo reverts the whole change; Save persists it._';
		}

		const msgs = [...chat.messages];
		for (let i = msgs.length - 1; i >= 0; i--) {
			const msg = msgs[i];
			if (msg.type === 'assistant' && extractOpsBlock(msg.content)) {
				msgs[i] = {
					...msg,
					content: (extracted.cleaned || 'Done.') + summary,
					html: undefined,
				};
				chat.setMessages(msgs);
				break;
			}
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
	allowAttachments={false}
	emptyIcon="A"
	emptyTitle="Architect"
	emptyDesc="I can help you build and modify workflows. Describe what you want and I'll make it happen."
	onsend={(text) => sendWithSync(text)}
	onstop={() => chat?.stop()}
	isLoading={chat?.isLoading ?? initLoading}
/>
