<script lang="ts">
	import { onMount, onDestroy, tick } from 'svelte';
	import { Bot, Loader2, Wifi, WifiOff, ArrowDown } from 'lucide-svelte';
	import { getWebSocketClient, type ConnectionStatus } from '$lib/websocket/client';
	import { logger } from '$lib/monitoring/logger';
	import Markdown from '$lib/components/ui/Markdown.svelte';
	import ApprovalModal from '$lib/components/ui/ApprovalModal.svelte';
	import { generateUUID } from '$lib/utils';
	import { MessageGroup, ToolOutputSidebar, ReadingIndicator, ChatInput } from '$lib/components/chat';

	interface Props {
		sessionKey: string;
		systemPrompt?: string;
		title?: string;
		subtitle?: string;
		suggestions?: string[];
	}

	let {
		sessionKey,
		systemPrompt = '',
		title = 'Chat',
		subtitle = '',
		suggestions = []
	}: Props = $props();

	const log = logger.child({ component: 'DevChat-' + sessionKey });

	// Approval modal
	interface ApprovalRequest {
		requestId: string;
		tool: string;
		input: Record<string, unknown>;
	}

	interface Message {
		id: string;
		role: 'user' | 'assistant' | 'system';
		content: string;
		timestamp: Date;
		toolCalls?: ToolCall[];
		streaming?: boolean;
		thinking?: string;
		contentBlocks?: ContentBlock[];
	}

	interface ToolCall {
		id?: string;
		name: string;
		input: string;
		output?: string;
		status?: 'running' | 'complete' | 'error';
	}

	interface ContentBlock {
		type: 'text' | 'tool';
		text?: string;
		toolCallIndex?: number;
	}

	// Group consecutive messages by role
	interface MessageGroupType {
		role: 'user' | 'assistant';
		messages: Message[];
	}

	// State
	let messages = $state<Message[]>([]);
	let inputValue = $state('');
	let isLoading = $state(false);
	let wsConnected = $state(false);
	let messagesContainer: HTMLDivElement;
	let currentStreamingMessage = $state<Message | null>(null);
	let showScrollButton = $state(false);
	let autoScrollEnabled = $state(true);

	// Tool output sidebar
	let sidebarTool = $state<ToolCall | null>(null);

	// Message queue
	interface QueuedMessage {
		id: string;
		content: string;
	}
	let messageQueue = $state<QueuedMessage[]>([]);
	let chatInputRef: { focus: () => void } | undefined;

	// Safety: auto-reset isLoading after 5 minutes
	let loadingTimeoutId: ReturnType<typeof setTimeout> | null = null;
	let cancelTimeoutId: ReturnType<typeof setTimeout> | null = null;
	const LOADING_TIMEOUT_MS = 5 * 60 * 1000;

	$effect(() => {
		if (isLoading) {
			if (loadingTimeoutId) clearTimeout(loadingTimeoutId);
			loadingTimeoutId = setTimeout(() => {
				if (isLoading) {
					log.warn('Loading timeout - force resetting state');
					if (currentStreamingMessage) {
						currentStreamingMessage.streaming = false;
						currentStreamingMessage.content += '\n\n*[Timed out]*';
						messages = [...messages.slice(0, -1), { ...currentStreamingMessage }];
						currentStreamingMessage = null;
					}
					isLoading = false;
					messageQueue = [];
				}
			}, LOADING_TIMEOUT_MS);
		} else {
			if (loadingTimeoutId) {
				clearTimeout(loadingTimeoutId);
				loadingTimeoutId = null;
			}
		}
	});

	// Approval request queue — multiple lanes can request approval concurrently
	let approvalQueue = $state<ApprovalRequest[]>([]);
	const pendingApproval = $derived(approvalQueue.length > 0 ? approvalQueue[0] : null);

	let unsubscribers: (() => void)[] = [];

	// Message grouping
	const groupedMessages = $derived.by((): MessageGroupType[] => {
		const groups: MessageGroupType[] = [];
		let currentGroup: MessageGroupType | null = null;

		for (const msg of messages) {
			if (msg.role === 'system') {
				currentGroup = null;
				continue;
			}
			const role = msg.role as 'user' | 'assistant';
			if (!currentGroup || currentGroup.role !== role) {
				currentGroup = { role, messages: [] };
				groups.push(currentGroup);
			}
			currentGroup.messages.push(msg);
		}

		return groups;
	});

	onMount(() => {
		const client = getWebSocketClient();

		unsubscribers.push(
			client.onStatus((status: ConnectionStatus) => {
				wsConnected = status === 'connected';
			})
		);

		// WebSocket event listeners
		unsubscribers.push(
			client.on('chat_stream', handleChatStream),
			client.on('chat_complete', handleChatComplete),
			client.on('chat_response', handleChatResponse),
			client.on('tool_start', handleToolStart),
			client.on('tool_result', handleToolResult),
			client.on('thinking', handleThinking),
			client.on('error', handleError),
			client.on('approval_request', handleApprovalRequest),
			client.on('stream_status', handleStreamStatus),
			client.on('chat_cancelled', handleChatCancelled)
		);

		// Check for active stream on this session
		if (client.isConnected()) {
			client.send('check_stream', { session_id: sessionKey });
		} else {
			const unsub = client.onStatus((status: ConnectionStatus) => {
				if (status === 'connected') {
					unsub();
					client.send('check_stream', { session_id: sessionKey });
				}
			});
		}
	});

	onDestroy(() => {
		unsubscribers.forEach((unsub) => unsub());
		if (loadingTimeoutId) clearTimeout(loadingTimeoutId);
		if (cancelTimeoutId) clearTimeout(cancelTimeoutId);
	});

	// --- WebSocket Handlers ---

	function handleChatStream(data: Record<string, unknown>) {
		if (data?.session_id !== sessionKey) return;

		const chunk = (data?.content as string) || '';

		if (currentStreamingMessage) {
			if (currentStreamingMessage.toolCalls?.length) {
				const hasRunning = currentStreamingMessage.toolCalls.some(tc => tc.status === 'running');
				if (hasRunning) {
					currentStreamingMessage.toolCalls = currentStreamingMessage.toolCalls.map(tc =>
						tc.status === 'running' ? { ...tc, status: 'complete' as const } : tc
					);
				}
			}
			currentStreamingMessage.content += chunk;
			if (!currentStreamingMessage.contentBlocks) {
				currentStreamingMessage.contentBlocks = [];
			}
			const blocks = currentStreamingMessage.contentBlocks;
			if (blocks.length === 0 || blocks[blocks.length - 1].type !== 'text') {
				blocks.push({ type: 'text', text: chunk });
			} else {
				blocks[blocks.length - 1] = { ...blocks[blocks.length - 1], text: (blocks[blocks.length - 1].text || '') + chunk };
			}
			currentStreamingMessage.contentBlocks = [...blocks];
			messages = [...messages.slice(0, -1), { ...currentStreamingMessage }];
		} else {
			currentStreamingMessage = {
				id: generateUUID(),
				role: 'assistant',
				content: chunk,
				timestamp: new Date(),
				streaming: true,
				contentBlocks: [{ type: 'text', text: chunk }]
			};
			messages = [...messages, currentStreamingMessage];
		}
	}

	function processQueue() {
		if (messageQueue.length > 0) {
			const next = messageQueue[0];
			messageQueue = messageQueue.slice(1);

			const userMessage: Message = {
				id: next.id,
				role: 'user',
				content: next.content,
				timestamp: new Date()
			};
			messages = [...messages, userMessage];
			sendToAgent(next.content);
		}
	}

	function cancelQueuedMessage(queuedId: string) {
		messageQueue = messageQueue.filter(q => q.id !== queuedId);
	}

	function handleChatComplete(data: Record<string, unknown>) {
		if (data?.session_id !== sessionKey) return;

		if (cancelTimeoutId) {
			clearTimeout(cancelTimeoutId);
			cancelTimeoutId = null;
		}

		if (currentStreamingMessage) {
			currentStreamingMessage.streaming = false;
			if (currentStreamingMessage.toolCalls?.length) {
				currentStreamingMessage.toolCalls = currentStreamingMessage.toolCalls.map(tc =>
					tc.status === 'running' ? { ...tc, status: 'complete' as const } : tc
				);
			}
			const finalMsg = { ...currentStreamingMessage };
			messages = [...messages.slice(0, -1), finalMsg];
			currentStreamingMessage = null;
		}
		isLoading = false;
		processQueue();
	}

	function handleChatResponse(data: Record<string, unknown>) {
		if (data?.session_id !== sessionKey) return;
		const assistantMessage: Message = {
			id: generateUUID(),
			role: 'assistant',
			content: (data?.content as string) || '',
			timestamp: new Date(),
			toolCalls: data?.tool_calls as ToolCall[]
		};
		messages = [...messages, assistantMessage];
		isLoading = false;
	}

	function handleToolStart(data: Record<string, unknown>) {
		if (data?.session_id !== sessionKey) return;

		const toolName = data?.tool as string;
		const toolID = (data?.tool_id as string) || '';
		const toolInput = (data?.input as string) || '';
		const newToolCall: ToolCall = { id: toolID, name: toolName, input: toolInput, status: 'running' };

		if (currentStreamingMessage) {
			if (!currentStreamingMessage.toolCalls) {
				currentStreamingMessage.toolCalls = [];
			}
			const toolIndex = currentStreamingMessage.toolCalls.length;
			currentStreamingMessage.toolCalls = [...currentStreamingMessage.toolCalls, newToolCall];
			if (!currentStreamingMessage.contentBlocks) {
				currentStreamingMessage.contentBlocks = [];
			}
			currentStreamingMessage.contentBlocks = [...currentStreamingMessage.contentBlocks, { type: 'tool' as const, toolCallIndex: toolIndex }];
			messages = [...messages.slice(0, -1), { ...currentStreamingMessage }];
		} else {
			currentStreamingMessage = {
				id: generateUUID(),
				role: 'assistant',
				content: '',
				timestamp: new Date(),
				streaming: true,
				toolCalls: [newToolCall],
				contentBlocks: [{ type: 'tool', toolCallIndex: 0 }]
			};
			messages = [...messages, currentStreamingMessage];
		}
	}

	function handleToolResult(data: Record<string, unknown>) {
		if (data?.session_id !== sessionKey) return;

		const result = (data?.result as string) || '';
		const toolID = (data?.tool_id as string) || '';

		const findAndUpdateTool = (toolCalls: ToolCall[]): ToolCall[] | null => {
			const updated = [...toolCalls];
			if (toolID) {
				const idx = updated.findIndex(tc => tc.id === toolID);
				if (idx >= 0) {
					updated[idx] = { ...updated[idx], output: result, status: 'complete' };
					return updated;
				}
			}
			const runningIdx = updated.findIndex(tc => tc.status === 'running');
			if (runningIdx >= 0) {
				updated[runningIdx] = { ...updated[runningIdx], output: result, status: 'complete' };
				return updated;
			}
			return null;
		};

		if (currentStreamingMessage?.toolCalls?.length) {
			const updatedToolCalls = findAndUpdateTool(currentStreamingMessage.toolCalls);
			if (updatedToolCalls) {
				currentStreamingMessage = { ...currentStreamingMessage, toolCalls: updatedToolCalls };
				messages = [...messages.slice(0, -1), currentStreamingMessage];
				return;
			}
		}

		for (let i = messages.length - 1; i >= Math.max(0, messages.length - 5); i--) {
			const msg = messages[i];
			if (msg.role === 'assistant' && msg.toolCalls?.length) {
				const updatedToolCalls = findAndUpdateTool(msg.toolCalls);
				if (updatedToolCalls) {
					messages = [...messages.slice(0, i), { ...msg, toolCalls: updatedToolCalls }, ...messages.slice(i + 1)];
					return;
				}
			}
		}
	}

	function handleThinking(data: Record<string, unknown>) {
		if (data?.session_id !== sessionKey) return;
		const thinkingContent = (data?.content as string) || '';

		if (currentStreamingMessage) {
			currentStreamingMessage.thinking = (currentStreamingMessage.thinking || '') + thinkingContent;
			messages = [...messages.slice(0, -1), { ...currentStreamingMessage }];
		} else {
			currentStreamingMessage = {
				id: generateUUID(),
				role: 'assistant',
				content: '',
				timestamp: new Date(),
				streaming: true,
				thinking: thinkingContent
			};
			messages = [...messages, currentStreamingMessage];
		}
	}

	function handleError(data: Record<string, unknown>) {
		if (data?.session_id !== sessionKey) return;

		if (currentStreamingMessage?.toolCalls?.length) {
			currentStreamingMessage.toolCalls = currentStreamingMessage.toolCalls.map(tc =>
				tc.status === 'running' ? { ...tc, status: 'error' as const } : tc
			);
			messages = [...messages.slice(0, -1), { ...currentStreamingMessage }];
		}

		const errorMessage: Message = {
			id: generateUUID(),
			role: 'assistant',
			content: `Error: ${data?.error || 'Unknown error'}`,
			timestamp: new Date()
		};
		messages = [...messages, errorMessage];
		isLoading = false;
		currentStreamingMessage = null;
		processQueue();
	}

	function handleApprovalRequest(data: Record<string, unknown>) {
		const requestId = data?.request_id as string;
		const tool = data?.tool as string;
		const input = data?.input as Record<string, unknown>;

		if (requestId && tool) {
			approvalQueue = [...approvalQueue, { requestId, tool, input: input || {} }];
		}
	}

	function handleStreamStatus(data: Record<string, unknown>) {
		const active = data?.active as boolean;
		const content = (data?.content as string) || '';

		if (!active || data?.session_id !== sessionKey) return;

		isLoading = true;
		currentStreamingMessage = {
			id: generateUUID(),
			role: 'assistant',
			content: content,
			timestamp: new Date(),
			streaming: true
		};
		messages = [...messages, currentStreamingMessage];
	}

	function handleChatCancelled(data: Record<string, unknown>) {
		if (data?.session_id !== sessionKey) return;

		if (cancelTimeoutId) {
			clearTimeout(cancelTimeoutId);
			cancelTimeoutId = null;
		}

		if (currentStreamingMessage) {
			currentStreamingMessage.streaming = false;
			if (currentStreamingMessage.content) {
				currentStreamingMessage.content += '\n\n*[Generation cancelled]*';
			} else {
				currentStreamingMessage.content = '*[Generation cancelled]*';
			}
			if (currentStreamingMessage.toolCalls?.length) {
				currentStreamingMessage.toolCalls = currentStreamingMessage.toolCalls.map(tc =>
					tc.status === 'running' ? { ...tc, status: 'complete' as const } : tc
				);
			}
			const finalMsg = { ...currentStreamingMessage };
			messages = [...messages.slice(0, -1), finalMsg];
			currentStreamingMessage = null;
		}
		isLoading = false;
		processQueue();
	}

	// --- Approval Handlers ---

	function resolveApproval(requestId: string) {
		approvalQueue = approvalQueue.filter((r) => r.requestId !== requestId);
	}

	function handleApprove(requestId: string) {
		const client = getWebSocketClient();
		client.send('approval_response', { request_id: requestId, approved: true });
		resolveApproval(requestId);
	}

	function handleApproveAlways(requestId: string) {
		const client = getWebSocketClient();
		client.send('approval_response', { request_id: requestId, approved: true, always: true });
		resolveApproval(requestId);
	}

	function handleDeny(requestId: string) {
		const client = getWebSocketClient();
		client.send('approval_response', { request_id: requestId, approved: false });
		resolveApproval(requestId);
	}

	// --- Send / Cancel ---

	function sendToAgent(prompt: string) {
		isLoading = true;
		const client = getWebSocketClient();
		if (client.isConnected()) {
			client.send('chat', {
				session_id: sessionKey,
				prompt: prompt,
				system: systemPrompt || undefined
			});
		} else {
			log.warn('WebSocket not connected, cannot send');
			isLoading = false;
		}
	}

	function sendMessage() {
		if (!inputValue.trim()) return;
		const prompt = inputValue.trim();
		inputValue = '';

		if (isLoading) {
			messageQueue = [...messageQueue, { id: generateUUID(), content: prompt }];
			return;
		}

		const userMessage: Message = {
			id: generateUUID(),
			role: 'user',
			content: prompt,
			timestamp: new Date()
		};
		messages = [...messages, userMessage];
		autoScrollEnabled = true;
		showScrollButton = false;
		sendToAgent(prompt);
	}

	function cancelMessage() {
		const client = getWebSocketClient();
		client.send('cancel', { session_id: sessionKey });

		if (cancelTimeoutId) clearTimeout(cancelTimeoutId);
		cancelTimeoutId = setTimeout(() => {
			cancelTimeoutId = null;
			if (isLoading) {
				if (currentStreamingMessage) {
					currentStreamingMessage.streaming = false;
					currentStreamingMessage.content += '\n\n*[Generation cancelled]*';
					if (currentStreamingMessage.toolCalls?.length) {
						currentStreamingMessage.toolCalls = currentStreamingMessage.toolCalls.map(tc =>
							tc.status === 'running' ? { ...tc, status: 'complete' as const } : tc
						);
					}
					messages = [...messages.slice(0, -1), { ...currentStreamingMessage }];
					currentStreamingMessage = null;
				}
				isLoading = false;
				processQueue();
			}
		}, 2000);
	}

	function selectSuggestion(text: string) {
		inputValue = text;
		sendMessage();
	}

	function resetChat() {
		messages = [];
		currentStreamingMessage = null;
		inputValue = '';
	}

	// --- Scroll Management ---

	$effect(() => {
		const messageCount = messages.length;
		const streamingContent = currentStreamingMessage?.content;
		const isStreaming = !!streamingContent;

		if (messagesContainer && (messageCount > 0 || streamingContent) && autoScrollEnabled) {
			tick().then(() => {
				requestAnimationFrame(() => {
					if (messagesContainer && autoScrollEnabled) {
						messagesContainer.scrollTo({
							top: messagesContainer.scrollHeight,
							behavior: isStreaming ? 'instant' : 'smooth'
						});
					}
				});
			});
		}
	});

	function handleScroll() {
		if (messagesContainer) {
			const { scrollTop, scrollHeight, clientHeight } = messagesContainer;
			const distanceFromBottom = scrollHeight - scrollTop - clientHeight;
			const wasNearBottom = !showScrollButton;
			showScrollButton = distanceFromBottom > 100;

			if (wasNearBottom && showScrollButton) {
				autoScrollEnabled = false;
			} else if (!wasNearBottom && !showScrollButton) {
				autoScrollEnabled = true;
			}
		}
	}

	function scrollToBottom() {
		if (messagesContainer) {
			messagesContainer.scrollTo({ top: messagesContainer.scrollHeight, behavior: 'smooth' });
			showScrollButton = false;
			autoScrollEnabled = true;
		}
	}

	function openToolSidebar(tool: ToolCall) {
		sidebarTool = tool;
	}

	function closeToolSidebar() {
		sidebarTool = null;
	}
</script>

<div class="flex flex-col h-full">
	<!-- Header -->
	<header class="border-b border-base-300 bg-base-100/80 backdrop-blur-sm shrink-0">
		<div class="flex items-center justify-between px-4 h-12">
			<div class="flex flex-col justify-center">
				<h2 class="text-sm font-semibold text-base-content leading-tight">{title}</h2>
				{#if subtitle}
					<p class="text-xs text-base-content/50 leading-tight">{subtitle}</p>
				{/if}
			</div>
			<div class="flex items-center gap-2 shrink-0">
				{#if wsConnected}
					<div class="flex items-center gap-1.5 text-xs text-success px-2">
						<span class="w-1.5 h-1.5 rounded-full bg-success"></span>
					</div>
				{:else}
					<div class="flex items-center gap-1.5 text-xs text-warning px-2">
						<span class="w-1.5 h-1.5 rounded-full bg-warning"></span>
					</div>
				{/if}
			</div>
		</div>
	</header>

	<!-- Messages Area -->
	<div class="relative flex-1 min-h-0">
		<div
			bind:this={messagesContainer}
			onscroll={handleScroll}
			class="h-full overflow-y-auto overscroll-contain scroll-pb-4"
		>
			<div class="max-w-4xl mx-auto p-4 space-y-4">
				{#if messages.length === 0}
					<div class="flex flex-col items-center justify-center pt-8 text-center">
						<div class="w-12 h-12 rounded-2xl bg-primary/10 flex items-center justify-center mb-3">
							<Bot class="w-6 h-6 text-primary" />
						</div>
						<h3 class="font-display text-lg font-bold text-base-content mb-1">{title}</h3>
						{#if subtitle}
							<p class="text-sm text-base-content/60 max-w-md mb-6">{subtitle}</p>
						{/if}
						{#if suggestions.length > 0}
							<div class="grid grid-cols-1 gap-2 max-w-md w-full">
								{#each suggestions as suggestion}
									<button
										type="button"
										onclick={() => selectSuggestion(suggestion)}
										class="text-left px-3 py-2 rounded-lg bg-base-200 text-sm text-base-content/70 hover:bg-base-300 hover:text-base-content transition-colors"
										disabled={isLoading}
									>
										{suggestion}
									</button>
								{/each}
							</div>
						{/if}
					</div>
				{:else}
					{#each groupedMessages as group, groupIndex (groupIndex)}
						<MessageGroup
							messages={group.messages}
							role={group.role}
							onViewToolOutput={openToolSidebar}
							isStreaming={group.role === 'assistant' && isLoading && groupIndex === groupedMessages.length - 1}
						/>
					{/each}

					{#if isLoading && !currentStreamingMessage && (groupedMessages.length === 0 || groupedMessages[groupedMessages.length - 1]?.role !== 'assistant')}
						<div class="flex gap-3 mb-4">
							<div class="w-8 h-8 rounded-lg flex-shrink-0 self-end mb-1 grid place-items-center font-semibold text-xs bg-base-300 text-base-content/60">
								A
							</div>
							<div class="flex flex-col gap-0.5 max-w-[min(900px,calc(100%-60px))] items-start">
								<div class="rounded-xl px-3 py-2 bg-base-200 animate-pulse-border">
									<ReadingIndicator />
								</div>
							</div>
						</div>
					{/if}
				{/if}
			</div>
		</div>

		{#if showScrollButton}
			<div class="absolute bottom-4 left-1/2 -translate-x-1/2 z-10">
				<button
					type="button"
					onclick={scrollToBottom}
					class="p-2 rounded-full bg-base-200 border border-base-300 text-base-content/60 hover:bg-base-300 hover:text-base-content transition-all shadow-lg"
					title="Scroll to bottom"
				>
					<ArrowDown class="w-4 h-4" />
				</button>
			</div>
		{/if}
	</div>

	<!-- Chat Input -->
	<ChatInput
		bind:this={chatInputRef}
		bind:value={inputValue}
		{isLoading}
		isRecording={false}
		voiceMode={false}
		queuedMessages={messageQueue}
		onSend={sendMessage}
		onCancel={cancelMessage}
		onCancelQueued={cancelQueuedMessage}
		onNewSession={resetChat}
		onToggleVoice={() => {}}
	/>
</div>

<ApprovalModal
	request={pendingApproval}
	onApprove={handleApprove}
	onApproveAlways={handleApproveAlways}
	onDeny={handleDeny}
/>

<ToolOutputSidebar tool={sidebarTool} onClose={closeToolSidebar} />
