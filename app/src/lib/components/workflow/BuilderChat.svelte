<script lang="ts">
	import ChatPane from '$lib/components/chat/ChatPane.svelte';
	import { ARCHITECT_INTRO_MESSAGE } from '$lib/tokens.js';

	let {
		agentId = '',
		workflows = {},
		selectedWorkflowName = null,
		onaction,
	}: {
		agentId: string;
		workflows: Record<string, any>;
		selectedWorkflowName: string | null;
		onaction?: (action: string, payload: unknown) => void;
	} = $props();

	let messages = $state<any[]>([{ ...ARCHITECT_INTRO_MESSAGE }]);
	let isLoading = $state(false);

	function handleSend(text: string) {
		if (!text.trim()) return;

		// Add user message
		messages = [...messages, { type: 'user', content: text }];
		isLoading = true;

		const lowerText = text.toLowerCase();
		const workflowNames = Object.keys(workflows);
		const targetWf = selectedWorkflowName || workflowNames[0] || null;

		// Simulate AI response with delay
		setTimeout(() => {
			// Thinking block
			messages = [...messages, { type: 'thinking', content: 'Analyzing workflow structure and determining the best approach...', duration: '1.2s' }];

			setTimeout(() => {
				if (lowerText.includes('add') || lowerText.includes('create') || lowerText.includes('new')) {
					// Extract a label from the message
					const label = extractLabel(text);
					const intent = extractIntent(text);

					// Tool group
					messages = [...messages, {
						type: 'tool-group',
						tools: [{
							type: 'tool',
							name: 'modify_workflow',
							status: 'success',
							duration: '0.3s',
							request: { action: 'add_activity', workflow: targetWf, label, intent },
							response: JSON.stringify({ success: true, nodeId: label.toLowerCase().replace(/\s+/g, '-') }),
						}],
					}];

					// Assistant response
					messages = [...messages, {
						type: 'assistant',
						content: `Done! I've added a **${label}** activity to the **${targetWf}** workflow.\n\nThe new node has been placed at the end of the chain. You can click on it in the canvas to configure its steps and skills.`,
					}];

					// Actually add the node
					onaction?.('add-activity', {
						workflowName: targetWf,
						label,
						intent,
						skills: [],
						steps: [],
					});
				} else if (lowerText.includes('delete') || lowerText.includes('remove')) {
					messages = [...messages, {
						type: 'assistant',
						content: 'To delete a node, **right-click** on it in the canvas and select **Delete**, or select the node and press the **Delete** key.\n\nI can also help you restructure the workflow — just describe what you want to change.',
					}];
				} else if (lowerText.includes('connect') || lowerText.includes('chain') || lowerText.includes('link')) {
					messages = [...messages, {
						type: 'assistant',
						content: 'To chain workflows together, set an **Emit** event on the source workflow and configure the target workflow\'s trigger as an **Event** type matching that event name.\n\nFor example, if Workflow A emits `brief.delivered`, set Workflow B\'s trigger to listen for `brief.delivered`. The canvas will show a dashed line connecting them.',
					}];
				} else if (lowerText.includes('trigger') || lowerText.includes('schedule') || lowerText.includes('when')) {
					messages = [...messages, {
						type: 'assistant',
						content: 'You can configure triggers by clicking the **trigger node** (the first node) in the canvas. The config panel on the right lets you choose between:\n\n- **Schedule** — run at specific times\n- **Heartbeat** — run on an interval\n- **Event** — react to system events\n- **Manual** — triggered on demand',
					}];
				} else {
					messages = [...messages, {
						type: 'assistant',
						content: 'I can help you with:\n\n- **Adding steps** — "Add an email notification step"\n- **Changing triggers** — "Change trigger to every 30 minutes"\n- **Connecting workflows** — "Chain the morning brief to content calendar"\n- **Configuring nodes** — Click any node to edit in the right panel\n\nWhat would you like to do?',
					}];
				}

				isLoading = false;
			}, 600);
		}, 400);
	}

	function extractLabel(text: string): string {
		// Try to extract a meaningful label from the user's message
		const patterns = [
			/add (?:a |an )?(.+?)(?:\s+step|\s+node|\s+activity)?$/i,
			/create (?:a |an )?(.+?)(?:\s+step|\s+node|\s+activity)?$/i,
			/new (.+?)(?:\s+step|\s+node|\s+activity)?$/i,
		];
		for (const p of patterns) {
			const match = text.match(p);
			if (match && match[1]) {
				const label = match[1].trim().replace(/["']/g, '');
				if (label.length > 2 && label.length < 40) {
					return label.split(' ').map(w => w.charAt(0).toUpperCase() + w.slice(1)).join(' ');
				}
			}
		}
		return 'New Activity';
	}

	function extractIntent(text: string): string {
		const words = text.replace(/^(add|create|new|please|can you)\s*/gi, '').trim();
		return words.charAt(0).toUpperCase() + words.slice(1);
	}
</script>

<ChatPane
	{messages}
	agentName="Architect"
	agentId="__architect__"
	placeholder="Describe a workflow change..."
	emptyIcon="A"
	emptyTitle="Architect"
	emptyDesc="I can help you build and modify workflows. Describe what you want and I'll make it happen."
	onsend={(text) => handleSend(text)}
	{isLoading}
/>
