<script lang="ts">
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { onMount, onDestroy, tick } from 'svelte';
	import { browser } from '$app/environment';
	import { Send, Bot, User, Loader2, Mic, MicOff, Wifi, WifiOff, ArrowDown, Copy, Check } from 'lucide-svelte';
	import { getWebSocketClient, type ConnectionStatus } from '$lib/websocket/client';
	import { getChat } from '$lib/api';
	import type { ChatMessage as ApiChatMessage } from '$lib/api';
	import Badge from '$lib/components/ui/Badge.svelte';
	import Markdown from '$lib/components/ui/Markdown.svelte';
	import ApprovalModal from '$lib/components/ui/ApprovalModal.svelte';

	interface ApprovalRequest {
		requestId: string;
		tool: string;
		input: Record<string, unknown>;
	}

	const DRAFT_STORAGE_PREFIX = 'gobot_chat_draft_';

	// eslint-disable-next-line @typescript-eslint/no-explicit-any
	type SpeechRecognitionType = any;

	interface Message {
		id: string;
		role: 'user' | 'assistant' | 'system';
		content: string;
		timestamp: Date;
		toolCalls?: ToolCall[];
		streaming?: boolean;
	}

	interface ToolCall {
		name: string;
		input: string;
		output?: string;
		status?: 'running' | 'complete' | 'error';
	}

	const chatId = $derived($page.params.chatId);

	let chatTitle = $state('Chat');
	let messages = $state<Message[]>([]);
	let inputValue = $state('');
	let isLoading = $state(false);
	let wsConnected = $state(false);
	let messagesContainer: HTMLDivElement;
	let currentStreamingMessage = $state<Message | null>(null);
	let chatLoaded = $state(false);
	let copiedMessageId = $state<string | null>(null);
	let showScrollButton = $state(false);
	let autoScrollEnabled = $state(true);
	let draftInitialized = $state(false);

	// Voice recording state
	let isRecording = $state(false);
	let recognition: SpeechRecognitionType | null = null;

	// Message queue
	let messageQueue = $state<string[]>([]);
	let textareaElement: HTMLTextAreaElement;

	// Approval requests
	let pendingApproval = $state<ApprovalRequest | null>(null);

	let unsubscribers: (() => void)[] = [];

	onMount(async () => {
		const client = getWebSocketClient();

		unsubscribers.push(
			client.onStatus((status: ConnectionStatus) => {
				wsConnected = status === 'connected';
			})
		);

		// WebSocket event listeners - filter by chatId as session_id
		unsubscribers.push(
			client.on('chat_stream', handleChatStream),
			client.on('chat_complete', handleChatComplete),
			client.on('chat_response', handleChatResponse),
			client.on('tool_start', handleToolStart),
			client.on('tool_result', handleToolResult),
			client.on('error', handleError),
			client.on('approval_request', handleApprovalRequest)
		);

		// Load draft from localStorage
		if (browser) {
			const savedDraft = localStorage.getItem(DRAFT_STORAGE_PREFIX + chatId);
			if (savedDraft) {
				inputValue = savedDraft;
			}
			draftInitialized = true;
		}

		// Load existing chat messages
		await loadChat();
	});

	onDestroy(() => {
		unsubscribers.forEach((unsub) => unsub());
	});

	// Save draft to localStorage when input changes
	$effect(() => {
		if (browser && draftInitialized) {
			if (inputValue) {
				localStorage.setItem(DRAFT_STORAGE_PREFIX + chatId, inputValue);
			} else {
				localStorage.removeItem(DRAFT_STORAGE_PREFIX + chatId);
			}
		}
	});

	function clearDraft() {
		if (browser) {
			localStorage.removeItem(DRAFT_STORAGE_PREFIX + chatId);
		}
	}

	async function loadChat() {
		if (!chatId) return;
		try {
			const res = await getChat({}, chatId);
			chatTitle = res.chat.title;
			messages = (res.messages || []).map((m: ApiChatMessage) => ({
				id: m.id,
				role: m.role as 'user' | 'assistant' | 'system',
				content: m.content,
				timestamp: new Date(m.createdAt)
			}));
			chatLoaded = true;
		} catch (err) {
			console.error('Failed to load chat:', err);
			goto('/agent');
		}
	}

	function sendToAgent(prompt: string) {
		isLoading = true;
		const client = getWebSocketClient();

		if (client.isConnected()) {
			client.send('chat', {
				session_id: chatId,
				prompt: prompt
			});
		} else {
			isLoading = false;
		}
	}

	function handleChatStream(data: Record<string, unknown>) {
		console.log('[Chat] Received stream:', data);
		if (data?.session_id !== chatId) {
			console.log('[Chat] Session mismatch:', data?.session_id, 'vs', chatId);
			return;
		}

		const chunk = (data?.content as string) || '';
		console.log('[Chat] Processing chunk:', chunk.length, 'bytes');

		if (currentStreamingMessage) {
			currentStreamingMessage.content += chunk;
			messages = [...messages.slice(0, -1), { ...currentStreamingMessage }];
		} else {
			currentStreamingMessage = {
				id: crypto.randomUUID(),
				role: 'assistant',
				content: chunk,
				timestamp: new Date(),
				streaming: true
			};
			messages = [...messages, currentStreamingMessage];
		}
	}

	function handleChatComplete(data: Record<string, unknown>) {
		if (data?.session_id !== chatId) return;

		if (currentStreamingMessage) {
			currentStreamingMessage.streaming = false;
			messages = [...messages.slice(0, -1), { ...currentStreamingMessage }];
			currentStreamingMessage = null;
		}
		isLoading = false;

		// Server saves the assistant message - no need to save here

		// Process queued messages
		if (messageQueue.length > 0) {
			const nextPrompt = messageQueue[0];
			messageQueue = messageQueue.slice(1);
			handleSendPrompt(nextPrompt);
		}
	}

	function handleChatResponse(data: Record<string, unknown>) {
		if (data?.session_id !== chatId) return;

		const assistantMessage: Message = {
			id: crypto.randomUUID(),
			role: 'assistant',
			content: (data?.content as string) || '',
			timestamp: new Date(),
			toolCalls: data?.tool_calls as ToolCall[]
		};
		messages = [...messages, assistantMessage];
		isLoading = false;
	}

	function handleToolStart(data: Record<string, unknown>) {
		if (data?.session_id !== chatId) return;

		const toolName = data?.tool as string;
		const toolMessage: Message = {
			id: crypto.randomUUID(),
			role: 'system',
			content: `Running tool: ${toolName}`,
			timestamp: new Date(),
			toolCalls: [{ name: toolName, input: (data?.input as string) || '', status: 'running' }]
		};
		messages = [...messages, toolMessage];
	}

	function handleToolResult(data: Record<string, unknown>) {
		if (data?.session_id !== chatId) return;

		const lastToolIdx = messages.findLastIndex((m) => m.role === 'system' && m.toolCalls?.length);
		if (lastToolIdx >= 0) {
			const updated = { ...messages[lastToolIdx] };
			if (updated.toolCalls?.[0]) {
				updated.toolCalls[0].output = (data?.result as string) || '';
				updated.toolCalls[0].status = 'complete';
			}
			messages = [...messages.slice(0, lastToolIdx), updated, ...messages.slice(lastToolIdx + 1)];
		}
	}

	function handleError(data: Record<string, unknown>) {
		if (data?.session_id !== chatId) return;

		const errorMessage: Message = {
			id: crypto.randomUUID(),
			role: 'assistant',
			content: `Error: ${data?.error || 'Unknown error'}`,
			timestamp: new Date()
		};
		messages = [...messages, errorMessage];
		isLoading = false;
		currentStreamingMessage = null;
	}

	function handleApprovalRequest(data: Record<string, unknown>) {
		console.log('[Chat] Received approval request:', data);
		const requestId = data?.request_id as string;
		const tool = data?.tool as string;
		const input = data?.input as Record<string, unknown>;

		if (requestId && tool) {
			pendingApproval = { requestId, tool, input: input || {} };
		}
	}

	function handleApprove(requestId: string) {
		const client = getWebSocketClient();
		client.send('approval_response', {
			request_id: requestId,
			approved: true
		});
		pendingApproval = null;
	}

	function handleDeny(requestId: string) {
		const client = getWebSocketClient();
		client.send('approval_response', {
			request_id: requestId,
			approved: false
		});
		pendingApproval = null;
	}

	function handleSendPrompt(prompt: string) {
		// Server saves user message - just send to agent via WebSocket
		sendToAgent(prompt);
	}

	function sendMessage() {
		if (!inputValue.trim()) return;

		const prompt = inputValue.trim();
		inputValue = '';
		clearDraft();

		// If already loading, queue the message
		if (isLoading) {
			messageQueue = [...messageQueue, prompt];
			const userMessage: Message = {
				id: crypto.randomUUID(),
				role: 'user',
				content: prompt,
				timestamp: new Date()
			};
			messages = [...messages, userMessage];
			return;
		}

		// Show user message immediately
		const userMessage: Message = {
			id: crypto.randomUUID(),
			role: 'user',
			content: prompt,
			timestamp: new Date()
		};
		messages = [...messages, userMessage];

		handleSendPrompt(prompt);
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && !e.shiftKey) {
			e.preventDefault();
			sendMessage();
		}
	}

	// Auto-scroll when new messages arrive or streaming content updates
	$effect(() => {
		// Track both messages array and streaming message content
		const messageCount = messages.length;
		const streamingContent = currentStreamingMessage?.content;

		if (messagesContainer && (messageCount > 0 || streamingContent) && autoScrollEnabled) {
			// Wait for Svelte to update DOM, then wait for browser to paint
			tick().then(() => {
				requestAnimationFrame(() => {
					if (messagesContainer && autoScrollEnabled) {
						messagesContainer.scrollTo({
							top: messagesContainer.scrollHeight,
							behavior: 'smooth'
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
			messagesContainer.scrollTo({
				top: messagesContainer.scrollHeight,
				behavior: 'smooth'
			});
			showScrollButton = false;
			autoScrollEnabled = true;
		}
	}

	async function copyMessage(messageId: string, content: string) {
		try {
			await navigator.clipboard.writeText(content);
			copiedMessageId = messageId;
			setTimeout(() => {
				copiedMessageId = null;
			}, 2000);
		} catch (err) {
			console.error('Failed to copy:', err);
		}
	}

	// Voice recording
	function toggleRecording() {
		if (isRecording) {
			stopRecording();
		} else {
			startRecording();
		}
	}

	function startRecording() {
		// eslint-disable-next-line @typescript-eslint/no-explicit-any
		const SpeechRecognition =
			(window as any).SpeechRecognition || (window as any).webkitSpeechRecognition;
		if (!SpeechRecognition) {
			alert('Speech recognition not supported in this browser. Try Chrome or Edge.');
			return;
		}

		recognition = new SpeechRecognition();
		recognition.continuous = true;
		recognition.interimResults = true;
		recognition.lang = 'en-US';

		let finalTranscript = '';

		// eslint-disable-next-line @typescript-eslint/no-explicit-any
		recognition.onresult = (event: any) => {
			let interimTranscript = '';
			for (let i = event.resultIndex; i < event.results.length; i++) {
				const transcript = event.results[i][0].transcript;
				if (event.results[i].isFinal) {
					finalTranscript += transcript + ' ';
				} else {
					interimTranscript += transcript;
				}
			}
			inputValue = finalTranscript + interimTranscript;
		};

		// eslint-disable-next-line @typescript-eslint/no-explicit-any
		recognition.onerror = (event: any) => {
			console.error('Speech recognition error:', event.error);
			isRecording = false;
		};

		recognition.onend = () => {
			isRecording = false;
			if (inputValue.trim()) {
				sendMessage();
			}
		};

		recognition.start();
		isRecording = true;
	}

	function stopRecording() {
		if (recognition) {
			recognition.stop();
			recognition = null;
		}
		isRecording = false;
	}

	// Auto-focus textarea when user starts typing anywhere
	function handleGlobalKeydown(e: KeyboardEvent) {
		// Skip if already focused on an input or textarea
		if (document.activeElement?.tagName === 'INPUT' || document.activeElement?.tagName === 'TEXTAREA') {
			return;
		}
		// Skip modifier keys, function keys, and navigation keys
		if (e.ctrlKey || e.metaKey || e.altKey || e.key.length > 1) {
			return;
		}
		// Focus the textarea for printable characters
		if (textareaElement && !isRecording) {
			textareaElement.focus();
		}
	}
</script>

<svelte:head>
	<title>{chatTitle} - GoBot</title>
</svelte:head>

<svelte:window onkeydown={handleGlobalKeydown} />

<div class="flex flex-col h-full bg-base-100">
	<!-- Header -->
	<header class="flex items-center justify-between px-6 h-14 border-b border-base-300 bg-base-100/80 backdrop-blur-sm shrink-0">
		<h1 class="text-lg font-semibold text-base-content truncate">{chatTitle}</h1>
		<div class="flex items-center gap-3 shrink-0">
			{#if wsConnected}
				<div class="flex items-center gap-1.5 text-xs text-success">
					<span class="w-2 h-2 rounded-full bg-success animate-pulse"></span>
					Connected
				</div>
			{:else}
				<div class="flex items-center gap-1.5 text-xs text-warning">
					<span class="w-2 h-2 rounded-full bg-warning"></span>
					Offline
				</div>
			{/if}
		</div>
	</header>

	<!-- Messages Area -->
	<div class="relative flex-1 min-h-0">
		<div
			bind:this={messagesContainer}
			onscroll={handleScroll}
			class="h-full overflow-y-auto overscroll-contain"
		>
			<div class="max-w-4xl mx-auto p-6 space-y-6">
		{#if !chatLoaded}
			<div class="flex items-center justify-center h-full">
				<Loader2 class="w-6 h-6 text-base-content/40 animate-spin" />
			</div>
		{:else if messages.length === 0}
			<div class="flex flex-col items-center justify-center h-full text-center">
				<div class="w-16 h-16 rounded-2xl bg-primary/10 flex items-center justify-center mb-4">
					<Bot class="w-8 h-8 text-primary" />
				</div>
				<h3 class="font-display font-bold text-base-content mb-2">Start chatting</h3>
				<p class="text-base-content/60 max-w-md">
					Send a message to begin the conversation.
				</p>
			</div>
		{:else}
			{#each messages as message (message.id)}
				{#if message.role === 'system'}
					<div class="flex justify-center">
						<div class="bg-base-200 rounded-lg px-3 py-2 text-xs text-base-content/60">
							{#if message.toolCalls?.length}
								{@const tool = message.toolCalls[0]}
								<span class="font-mono text-secondary">{tool.name}</span>
								{#if tool.status === 'running'}
									<Loader2 class="w-3 h-3 inline-block ml-1 animate-spin" />
								{:else if tool.status === 'complete'}
									<span class="text-success ml-1">done</span>
								{/if}
							{:else}
								{message.content}
							{/if}
						</div>
					</div>
				{:else}
					<div class="flex gap-4 {message.role === 'user' ? 'justify-end' : ''}">
						{#if message.role === 'user'}
							<div class="max-w-[80%]">
								<div class="rounded-2xl bg-primary px-4 py-3">
									<p class="text-primary-content whitespace-pre-wrap">{message.content}</p>
								</div>
							</div>
						{:else}
							<div class="max-w-[90%] space-y-2 group">
								<!-- Tool calls indicator -->
								{#if message.toolCalls?.length}
									<div class="flex flex-wrap gap-1.5">
										{#each message.toolCalls as tool}
											<div class="flex items-center gap-1.5 text-xs bg-base-200 rounded-md px-2 py-1">
												<span class="font-mono text-secondary">{tool.name}</span>
												{#if tool.status === 'running'}
													<Loader2 class="w-3 h-3 animate-spin text-base-content/40" />
												{:else if tool.status === 'complete'}
													<span class="text-success">✓</span>
												{/if}
											</div>
										{/each}
									</div>
								{/if}
								<!-- Message content with Markdown -->
								<div class="rounded-2xl bg-base-200/50 px-4 py-3 border border-base-300/50">
									{#if message.streaming}
										<Markdown content={message.content} />
										<span class="inline-block w-2 h-4 bg-base-content/50 animate-pulse ml-0.5"></span>
									{:else}
										<Markdown content={message.content} />
									{/if}
								</div>
								<!-- Action buttons (appear on hover) -->
								<div class="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
									<button
										type="button"
										onclick={() => copyMessage(message.id, message.content)}
										class="p-1.5 rounded-md text-base-content/40 hover:text-base-content hover:bg-base-200 transition-colors"
										title="Copy message"
									>
										{#if copiedMessageId === message.id}
											<Check class="w-4 h-4 text-success" />
										{:else}
											<Copy class="w-4 h-4" />
										{/if}
									</button>
								</div>
							</div>
						{/if}
					</div>
				{/if}
			{/each}
			{#if isLoading && !currentStreamingMessage}
				<div class="flex gap-4">
					<div class="max-w-[90%] rounded-2xl bg-base-200/50 px-4 py-3 border border-base-300/50">
						<div class="flex items-center gap-2">
							<div class="flex gap-1">
								<span class="w-2 h-2 bg-base-content/30 rounded-full animate-bounce" style="animation-delay: 0ms"></span>
								<span class="w-2 h-2 bg-base-content/30 rounded-full animate-bounce" style="animation-delay: 150ms"></span>
								<span class="w-2 h-2 bg-base-content/30 rounded-full animate-bounce" style="animation-delay: 300ms"></span>
							</div>
						</div>
					</div>
				</div>
			{/if}
		{/if}
			</div>
		</div>

		<!-- Scroll to bottom button -->
		{#if showScrollButton}
			<div class="absolute bottom-4 left-1/2 -translate-x-1/2 z-10">
				<button
					type="button"
					onclick={scrollToBottom}
					class="p-2 rounded-full bg-base-200 border border-base-300 text-base-content/60 hover:bg-base-300 hover:text-base-content transition-all shadow-lg"
					title="Scroll to bottom"
				>
					<ArrowDown class="w-5 h-5" />
				</button>
			</div>
		{/if}
	</div>

	<!-- Input Area -->
	<div class="border-t border-base-300 bg-base-100 shrink-0">
		<div class="max-w-4xl mx-auto p-4">
			<div class="flex gap-2">
				<textarea
					bind:this={textareaElement}
					bind:value={inputValue}
					onkeydown={handleKeydown}
					placeholder={isLoading ? 'Type to queue your next message...' : 'Send a message...'}
					class="flex-1 resize-none bg-base-200 rounded-2xl px-4 py-3 text-sm focus:outline-none focus:ring-2 focus:ring-primary/50 min-h-[48px] max-h-32"
					rows="1"
					disabled={isRecording}
				></textarea>
				<button
					type="button"
					onclick={toggleRecording}
					disabled={isLoading}
					class="btn btn-sm btn-square btn-ghost self-end mb-1 {isRecording ? 'text-error animate-pulse' : ''}"
				>
					{#if isRecording}
						<MicOff class="w-4 h-4" />
					{:else}
						<Mic class="w-4 h-4" />
					{/if}
				</button>
				<button
					type="button"
					onclick={sendMessage}
					disabled={!inputValue.trim() || isRecording}
					class="btn btn-sm btn-square btn-primary self-end mb-1"
				>
					<Send class="w-4 h-4" />
				</button>
			</div>
			<div class="flex items-center justify-between mt-2">
				<p class="text-xs text-base-content/40">
					{#if isRecording}
						<span class="text-error">Recording... Click mic to stop</span>
					{:else if messageQueue.length > 0}
						<span class="text-info">{messageQueue.length} message{messageQueue.length > 1 ? 's' : ''} queued</span>
					{:else if isLoading}
						<span>Type to queue your next message</span>
					{:else}
						Press Enter to send, Shift+Enter for new line
					{/if}
				</p>
			</div>
		</div>
	</div>
</div>

<ApprovalModal
	request={pendingApproval}
	onApprove={handleApprove}
	onDeny={handleDeny}
/>
