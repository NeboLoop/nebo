<script lang="ts">
	import { onMount, onDestroy, tick } from 'svelte';
	import { browser } from '$app/environment';
	import { Send, Bot, Loader2, Mic, MicOff, Wifi, WifiOff, ArrowDown, Copy, Check, History, Volume2, VolumeOff } from 'lucide-svelte';
	import { getWebSocketClient, type ConnectionStatus } from '$lib/websocket/client';
	import { getCompanionChat, speakTTS } from '$lib/api';
	import { logger } from '$lib/monitoring/logger';

	const log = logger.child({ component: 'Agent' });
	const voiceLog = logger.child({ component: 'Voice' });
	import type { ChatMessage as ApiChatMessage } from '$lib/api';
	import Markdown from '$lib/components/ui/Markdown.svelte';
	import ApprovalModal from '$lib/components/ui/ApprovalModal.svelte';
	import { generateUUID } from '$lib/utils';
	import { MessageGroup, ToolOutputSidebar, ReadingIndicator, ChatInput } from '$lib/components/chat';

	interface ApprovalRequest {
		requestId: string;
		tool: string;
		input: Record<string, unknown>;
	}

	const DRAFT_STORAGE_KEY = 'nebo_companion_draft';

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
		text?: string;          // accumulated text for text blocks
		toolCallIndex?: number; // index into toolCalls for tool blocks
	}

	let chatId = $state<string | null>(null);
	let messages = $state<Message[]>([]);
	let totalMessages = $state<number>(0); // Total messages in chat (may be more than loaded)
	let inputValue = $state('');
	let isLoading = $state(false);
	let wsConnected = $state(false);
	let messagesContainer: HTMLDivElement;
	let currentStreamingMessage = $state<Message | null>(null);
	let chatLoaded = $state(false);
	let copiedMessageId = $state<string | null>(null);
	let showScrollButton = $state(false);
	let autoScrollEnabled = $state(true);
	let scrollingProgrammatically = $state(false);
	let draftInitialized = $state(false);

	// Voice recording state
	let isRecording = $state(false);

	// Voice output (TTS) state
	let voiceOutputEnabled = $state(false);
	let isSpeaking = $state(false);
	let currentAudio: HTMLAudioElement | null = null;
	let ttsVoice = $state('rachel'); // Default ElevenLabs voice

	// Available ElevenLabs voices
	const ttsVoices = ['rachel', 'domi', 'bella', 'antoni', 'elli', 'josh', 'arnold', 'adam', 'sam'];

	// Tool output sidebar
	let sidebarTool = $state<ToolCall | null>(null);

	// Group consecutive messages by role for Slack-style display
	// Note: system messages are filtered out during grouping, so role is only user/assistant
	interface MessageGroupType {
		role: 'user' | 'assistant';
		messages: Message[];
	}

	const groupedMessages = $derived.by((): MessageGroupType[] => {
		const groups: MessageGroupType[] = [];
		let currentGroup: MessageGroupType | null = null;

		for (const msg of messages) {
			// Skip system messages (tool notifications) in grouping - they're handled inline
			if (msg.role === 'system') {
				// System messages break groups but aren't displayed in groups
				currentGroup = null;
				continue;
			}

			// At this point, role is only 'user' or 'assistant'
			const role = msg.role as 'user' | 'assistant';

			if (!currentGroup || currentGroup.role !== role) {
				currentGroup = { role, messages: [] };
				groups.push(currentGroup);
			}
			currentGroup.messages.push(msg);
		}

		return groups;
	});

	// Message queue — queued messages live here, NOT in messages[]
	interface QueuedMessage {
		id: string;
		content: string;
	}
	let messageQueue = $state<QueuedMessage[]>([]);
	let chatInputRef: { focus: () => void } | undefined;
	let loadingTimeoutId: ReturnType<typeof setTimeout> | null = null;
	let cancelTimeoutId: ReturnType<typeof setTimeout> | null = null;

	// Safety: auto-reset isLoading after 5 minutes of no stream activity
	// This prevents the UI from getting permanently stuck
	const LOADING_TIMEOUT_MS = 5 * 60 * 1000;

	$effect(() => {
		if (isLoading) {
			// Clear previous timeout
			if (loadingTimeoutId) clearTimeout(loadingTimeoutId);
			loadingTimeoutId = setTimeout(() => {
				if (isLoading) {
					log.warn('Loading timeout - force resetting state after ' + (LOADING_TIMEOUT_MS / 1000) + ' seconds');
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

	const suggestions = [
		'Read the README and summarize this project',
		'List all files in the current directory',
		'Search the web for the latest Go release',
		'Help me debug an issue with my code'
	];

	onMount(async () => {
		const client = getWebSocketClient();

		unsubscribers.push(
			client.onStatus((status: ConnectionStatus) => {
				wsConnected = status === 'connected';
			})
		);

		// WebSocket event listeners
		log.debug('Registering WebSocket event listeners');
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
		log.debug('WebSocket event listeners registered');

		// Load draft from localStorage
		if (browser) {
			const savedDraft = localStorage.getItem(DRAFT_STORAGE_KEY);
			if (savedDraft) {
				inputValue = savedDraft;
			}
			draftInitialized = true;
		}

		// Load companion chat
		await loadCompanionChat();
	});

	onDestroy(() => {
		unsubscribers.forEach((unsub) => unsub());
		// Clean up loading timeout
		if (loadingTimeoutId) {
			clearTimeout(loadingTimeoutId);
			loadingTimeoutId = null;
		}
		// Clean up voice mode
		voiceMode = false;
		if (silenceTimer) {
			clearTimeout(silenceTimer);
		}
		if (recognition) {
			recognition.abort();
			recognition = null;
		}
		// Clean up audio playback
		if (currentAudio) {
			currentAudio.pause();
			currentAudio = null;
		}
	});

	// Save draft to localStorage when input changes
	$effect(() => {
		if (browser && draftInitialized) {
			if (inputValue) {
				localStorage.setItem(DRAFT_STORAGE_KEY, inputValue);
			} else {
				localStorage.removeItem(DRAFT_STORAGE_KEY);
			}
		}
	});

	function clearDraft() {
		if (browser) {
			localStorage.removeItem(DRAFT_STORAGE_KEY);
		}
	}

	interface ParsedMetadata {
		toolCalls?: ToolCall[];
		thinking?: string;
		contentBlocks?: ContentBlock[];
	}

	function parseMetadata(metadata: string | undefined): ParsedMetadata {
		if (!metadata) return {};
		try {
			const parsed = JSON.parse(metadata);
			const result: ParsedMetadata = {};

			if (parsed.toolCalls && Array.isArray(parsed.toolCalls)) {
				result.toolCalls = parsed.toolCalls.map((tc: { name: string; input: string; output?: string; status?: string }) => ({
					name: tc.name,
					input: tc.input,
					output: tc.output,
					// When loading from persistence, any tool saved as "running" during a
					// partial save is actually complete (the stream has finished by the time
					// we load history). Only preserve explicit "error" status.
					status: (tc.status === 'error' ? 'error' : 'complete') as 'complete' | 'error'
				}));
			}

			if (parsed.thinking && typeof parsed.thinking === 'string') {
				result.thinking = parsed.thinking;
			}

			if (parsed.contentBlocks && Array.isArray(parsed.contentBlocks)) {
				result.contentBlocks = parsed.contentBlocks;
			}

			return result;
		} catch {
			// Invalid JSON
		}
		return {};
	}

	async function loadCompanionChat() {
		try {
			const res = await getCompanionChat();
			chatId = res.chat.id;
			log.debug('Loaded companion chat: ' + chatId);
			messages = (res.messages || []).map((m: ApiChatMessage) => {
				const meta = parseMetadata((m as { metadata?: string }).metadata);
				return {
					id: m.id,
					role: m.role as 'user' | 'assistant' | 'system',
					content: m.content,
					timestamp: new Date(m.createdAt),
					toolCalls: meta.toolCalls,
					thinking: meta.thinking,
					contentBlocks: meta.contentBlocks
				};
			});
			totalMessages = res.totalMessages || messages.length;
			chatLoaded = true;
			log.debug('Messages loaded: ' + messages.length + ' total: ' + totalMessages);

			// Scroll to bottom after loading history (handles race where $effect
			// fires before messagesContainer is bound via bind:this)
			if (messages.length > 0) {
				tick().then(() => {
					requestAnimationFrame(() => {
						if (messagesContainer) {
							messagesContainer.scrollTo({
								top: messagesContainer.scrollHeight,
								behavior: 'smooth'
							});
						}
					});
				});
			}

			// Check if there's an active stream to resume
			checkForActiveStream();

			// If chat is empty, request introduction from the agent
			if (messages.length === 0 && chatId) {
				log.debug('Chat is empty, requesting introduction...');
				requestIntroduction();
			}
		} catch (err) {
			log.error('Failed to load companion chat', err);
			chatLoaded = true; // Still mark as loaded, will show empty state
		}
	}

	function checkForActiveStream() {
		const client = getWebSocketClient();
		if (!client.isConnected() || !chatId) {
			// Wait for connection
			const unsub = client.onStatus((status: ConnectionStatus) => {
				if (status === 'connected' && chatId) {
					unsub();
					log.debug('Checking for active stream on session: ' + chatId);
					client.send('check_stream', { session_id: chatId });
				}
			});
			return;
		}
		log.debug('Checking for active stream on session: ' + chatId);
		client.send('check_stream', { session_id: chatId });
	}

	function requestIntroduction() {
		const client = getWebSocketClient();
		log.debug('requestIntroduction called, connected: ' + client.isConnected());
		if (!client.isConnected()) {
			// Wait for connection and try again
			log.debug('WebSocket not connected, waiting...');
			const unsub = client.onStatus((status: ConnectionStatus) => {
				log.debug('WebSocket status changed: ' + status);
				if (status === 'connected') {
					unsub();
					doRequestIntroduction();
				}
			});
			return;
		}
		doRequestIntroduction();
	}

	function doRequestIntroduction() {
		log.debug('Sending request_introduction for session: ' + chatId);
		const client = getWebSocketClient();
		isLoading = true;
		client.send('request_introduction', {
			session_id: chatId || ''
		});
	}

	// Calculate if there's more history to view
	const hasMoreHistory = $derived(totalMessages > messages.length);

	function sendToAgent(prompt: string) {
		isLoading = true;
		const client = getWebSocketClient();

		if (client.isConnected()) {
			// Send with companion flag and optional chat ID
			client.send('chat', {
				session_id: chatId || '',
				prompt: prompt,
				companion: true
			});
		} else {
			log.warn('WebSocket not connected, cannot send message');
			isLoading = false;
			// Process remaining queue even when disconnected
			// Use setTimeout to avoid synchronous recursion
			if (messageQueue.length > 0) {
				setTimeout(() => processQueue(), 100);
			}
		}
	}

	function handleChatStream(data: Record<string, unknown>) {
		log.debug('handleChatStream called: ' + data?.session_id + ' chatId: ' + chatId);
		// Accept messages for our chat or if we haven't loaded yet
		if (chatId && data?.session_id !== chatId) {
			log.debug('handleChatStream: session_id mismatch, ignoring');
			return;
		}

		// If we just got a session_id and didn't have one, save it
		if (!chatId && data?.session_id) {
			chatId = data.session_id as string;
		}

		const chunk = (data?.content as string) || '';

		if (currentStreamingMessage) {
			// When new text arrives and there are running tools, mark them complete.
			// In the agentic loop, the AI only produces text AFTER all tool results
			// from the previous iteration have been processed. So any tool that's
			// still marked "running" when text arrives has actually completed — we
			// just missed (or haven't yet processed) its tool_result event.
			if (currentStreamingMessage.toolCalls?.length) {
				const hasRunning = currentStreamingMessage.toolCalls.some(tc => tc.status === 'running');
				if (hasRunning) {
					log.debug('handleChatStream: text arrived with running tools — marking all complete');
					currentStreamingMessage.toolCalls = currentStreamingMessage.toolCalls.map(tc =>
						tc.status === 'running' ? { ...tc, status: 'complete' as const } : tc
					);
				}
			}
			currentStreamingMessage.content += chunk;
			// Track content blocks: append to last text block or create new one
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

	// Process next queued message (if any)
	function processQueue() {
		if (messageQueue.length > 0) {
			const next = messageQueue[0];
			messageQueue = messageQueue.slice(1);
			log.debug('Processing queued message: ' + next.content.substring(0, 50));

			// Now add the user message to the chat
			const userMessage: Message = {
				id: next.id,
				role: 'user',
				content: next.content,
				timestamp: new Date()
			};
			messages = [...messages, userMessage];

			handleSendPrompt(next.content);
		}
	}

	// Cancel a single queued message by its ID
	function cancelQueuedMessage(queuedId: string) {
		const item = messageQueue.find(q => q.id === queuedId);
		if (!item) return;

		messageQueue = messageQueue.filter(q => q.id !== queuedId);
		log.debug('Cancelled queued message: ' + item.content.substring(0, 50));
	}

	function handleChatComplete(data: Record<string, unknown>) {
		log.debug('handleChatComplete called', {
			sessionId: data?.session_id as string,
			chatId: chatId ?? undefined,
			hasStreamingMsg: String(!!currentStreamingMessage),
			toolCount: String(currentStreamingMessage?.toolCalls?.length ?? 0),
			messagesCount: String(messages.length)
		});

		if (chatId && data?.session_id !== chatId) {
			log.debug('handleChatComplete: session mismatch, expected ' + chatId + ' got ' + data?.session_id);
			return;
		}

		// Clear any pending cancel timeout — the request completed (naturally or post-cancel)
		if (cancelTimeoutId) {
			clearTimeout(cancelTimeoutId);
			cancelTimeoutId = null;
		}

		let completedContent = '';
		if (currentStreamingMessage) {
			completedContent = currentStreamingMessage.content;
			currentStreamingMessage.streaming = false;
			// Safety net: mark any still-running tools as complete
			// (tool_result may have been missed due to timing or frame drops)
			if (currentStreamingMessage.toolCalls?.length) {
				const beforeStatuses = currentStreamingMessage.toolCalls.map(t => t.status);
				currentStreamingMessage.toolCalls = currentStreamingMessage.toolCalls.map(tc =>
					tc.status === 'running' ? { ...tc, status: 'complete' as const } : tc
				);
				const afterStatuses = currentStreamingMessage.toolCalls.map(t => t.status);
				log.debug('Safety net: tool statuses before: ' + beforeStatuses.join(',') + ' after: ' + afterStatuses.join(','));
			}
			const finalMsg = { ...currentStreamingMessage };
			messages = [...messages.slice(0, -1), finalMsg];
			currentStreamingMessage = null;
		} else {
			log.debug('handleChatComplete: NO currentStreamingMessage!');
			// Safety net for non-streaming: check last message in array
			const lastIdx = messages.length - 1;
			if (lastIdx >= 0 && messages[lastIdx].role === 'assistant' && messages[lastIdx].toolCalls?.length) {
				const lastMsg = messages[lastIdx];
				const hasRunning = lastMsg.toolCalls!.some(tc => tc.status === 'running');
				if (hasRunning) {
					log.debug('Safety net (non-streaming): marking running tools as complete in last message');
					const updatedTools = lastMsg.toolCalls!.map(tc =>
						tc.status === 'running' ? { ...tc, status: 'complete' as const } : tc
					);
					messages = [...messages.slice(0, lastIdx), { ...lastMsg, toolCalls: updatedTools }];
				}
			}
		}
		isLoading = false;

		// Speak the response if voice output is enabled
		if (voiceOutputEnabled && completedContent) {
			speakText(completedContent);
		}

		// Process queued messages
		processQueue();
	}

	function cancelMessage() {
		const client = getWebSocketClient();
		client.send('cancel', {
			session_id: chatId || ''
		});

		// Optimistic cancel: if server doesn't respond with chat_cancelled within 2s,
		// force-reset the loading state so the UI doesn't get stuck
		if (cancelTimeoutId) clearTimeout(cancelTimeoutId);
		cancelTimeoutId = setTimeout(() => {
			cancelTimeoutId = null;
			if (isLoading) {
				log.warn('Cancel timeout - force resetting loading state');
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

				// Process any queued messages (don't discard them)
				processQueue();
			}
		}, 2000);
	}

	function handleChatCancelled(data: Record<string, unknown>) {
		if (chatId && data?.session_id !== chatId) return;

		// Clear the cancel timeout — server responded, no need for fallback
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
			// Mark any running tools as complete
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

		// Process queued messages — the user typed these while waiting,
		// they're already visible as user bubbles and should be sent now
		processQueue();
	}

	function handleChatResponse(data: Record<string, unknown>) {
		if (chatId && data?.session_id !== chatId) return;

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
		log.debug('handleToolStart: ' + (data?.tool as string));
		if (chatId && data?.session_id !== chatId) {
			log.debug('handleToolStart: session_id mismatch, ignoring');
			return;
		}

		const toolName = data?.tool as string;
		const toolID = (data?.tool_id as string) || '';
		const toolInput = (data?.input as string) || '';
		const newToolCall: ToolCall = { id: toolID, name: toolName, input: toolInput, status: 'running' };

		// Attach tool call to current streaming message OR create one
		if (currentStreamingMessage) {
			if (!currentStreamingMessage.toolCalls) {
				currentStreamingMessage.toolCalls = [];
			}
			const toolIndex = currentStreamingMessage.toolCalls.length;
			currentStreamingMessage.toolCalls = [...currentStreamingMessage.toolCalls, newToolCall];
			// Add tool content block
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
		log.debug('handleToolResult: ' + (data?.tool_name as string || data?.tool_id as string));

		if (chatId && data?.session_id !== chatId) {
			log.debug('handleToolResult: session mismatch');
			return;
		}

		const result = (data?.result as string) || '';
		const toolID = (data?.tool_id as string) || '';
		const toolName = (data?.tool_name as string) || '';
		log.debug('Tool result received: ' + toolName + ' id: ' + toolID + ' result_length: ' + result?.length);

		// Helper to find and update tool by ID or fallback to first running
		const findAndUpdateTool = (toolCalls: ToolCall[]): ToolCall[] | null => {
			const updated = [...toolCalls];
			// Try to find by ID first
			if (toolID) {
				const idx = updated.findIndex(tc => tc.id === toolID);
				if (idx >= 0) {
					log.debug('Found tool by ID at index ' + idx);
					updated[idx] = { ...updated[idx], output: result, status: 'complete' };
					return updated;
				}
			}
			// Fallback: find first running tool
			const runningIdx = updated.findIndex(tc => tc.status === 'running');
			if (runningIdx >= 0) {
				log.debug('Fallback: updating first running tool at index ' + runningIdx);
				updated[runningIdx] = { ...updated[runningIdx], output: result, status: 'complete' };
				return updated;
			}
			return null;
		};

		// Try to update in current streaming message first
		if (currentStreamingMessage?.toolCalls?.length) {
			const updatedToolCalls = findAndUpdateTool(currentStreamingMessage.toolCalls);
			if (updatedToolCalls) {
				currentStreamingMessage = { ...currentStreamingMessage, toolCalls: updatedToolCalls };
				messages = [...messages.slice(0, -1), currentStreamingMessage];
				log.debug('Updated tool in streaming message');
				return;
			}
		}

		// Fallback: search backwards through messages for a matching running tool
		log.debug('handleToolResult: trying fallback to recent assistant messages');
		for (let i = messages.length - 1; i >= Math.max(0, messages.length - 5); i--) {
			const msg = messages[i];
			if (msg.role === 'assistant' && msg.toolCalls?.length) {
				const updatedToolCalls = findAndUpdateTool(msg.toolCalls);
				if (updatedToolCalls) {
					messages = [...messages.slice(0, i), { ...msg, toolCalls: updatedToolCalls }, ...messages.slice(i + 1)];
					log.debug('Fallback: updated tool in message at index ' + i);
					return;
				}
			}
		}

		log.warn('handleToolResult: SKIP - no suitable tool to update (tool_id: ' + toolID + ')');
	}

	function handleThinking(data: Record<string, unknown>) {
		log.debug('handleThinking');
		if (chatId && data?.session_id !== chatId) return;

		const thinkingContent = (data?.content as string) || '';

		// Attach thinking to current streaming message OR create one
		if (currentStreamingMessage) {
			// Append to existing thinking content
			currentStreamingMessage.thinking = (currentStreamingMessage.thinking || '') + thinkingContent;
			messages = [...messages.slice(0, -1), { ...currentStreamingMessage }];
		} else {
			// Create new streaming message with thinking
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
		if (chatId && data?.session_id !== chatId) return;

		// Safety net: mark any running tools as complete before handling error
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

		// Process queued messages even after errors (don't leave orphaned user messages)
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
		log.debug('handleStreamStatus: active=' + data?.active + ' session=' + data?.session_id);
		const sessionId = data?.session_id as string;
		const active = data?.active as boolean;
		const content = (data?.content as string) || '';

		if (!active || sessionId !== chatId) {
			log.debug('No active stream to resume');
			return;
		}

		log.info('Resuming stream with ' + content.length + ' bytes of content');

		// Create or update streaming message with accumulated content
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

	function resolveApproval(requestId: string) {
		approvalQueue = approvalQueue.filter((r) => r.requestId !== requestId);
	}

	function handleApprove(requestId: string) {
		const client = getWebSocketClient();
		client.send('approval_response', {
			request_id: requestId,
			approved: true
		});
		resolveApproval(requestId);
	}

	function handleApproveAlways(requestId: string) {
		const client = getWebSocketClient();
		client.send('approval_response', {
			request_id: requestId,
			approved: true,
			always: true
		});
		resolveApproval(requestId);
	}

	function handleDeny(requestId: string) {
		const client = getWebSocketClient();
		client.send('approval_response', {
			request_id: requestId,
			approved: false
		});
		resolveApproval(requestId);
	}

	function handleSendPrompt(prompt: string) {
		sendToAgent(prompt);
	}

	function sendMessage() {
		if (!inputValue.trim()) return;

		const prompt = inputValue.trim();

		inputValue = '';
		clearDraft();

		// If already loading, queue the message — it stays above the input, not in chat
		if (isLoading) {
			log.debug('Queuing message (agent busy): ' + prompt.substring(0, 50));
			messageQueue = [...messageQueue, { id: generateUUID(), content: prompt }];
			return;
		}

		// Show user message immediately and send
		const userMessage: Message = {
			id: generateUUID(),
			role: 'user',
			content: prompt,
			timestamp: new Date()
		};
		messages = [...messages, userMessage];

		// Force scroll to bottom when user sends — they want to see the response
		autoScrollEnabled = true;
		showScrollButton = false;

		handleSendPrompt(prompt);
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && !e.shiftKey) {
			e.preventDefault();
			sendMessage();
		}
	}

	function selectSuggestion(text: string) {
		inputValue = text;
		sendMessage();
	}

	// Auto-scroll when new messages arrive or streaming content updates
	$effect(() => {
		const messageCount = messages.length;
		const streamingContent = currentStreamingMessage?.content;
		const isStreaming = !!streamingContent;

		if (messagesContainer && (messageCount > 0 || streamingContent) && autoScrollEnabled) {
			// Suppress handleScroll from disabling auto-scroll during programmatic scroll
			scrollingProgrammatically = true;
			// Wait for Svelte to update DOM, then wait for browser to paint
			tick().then(() => {
				requestAnimationFrame(() => {
					if (messagesContainer && autoScrollEnabled) {
						// Use instant scroll during streaming to keep up with content,
						// smooth scroll when just adding new messages
						messagesContainer.scrollTo({
							top: messagesContainer.scrollHeight,
							behavior: isStreaming ? 'instant' : 'smooth'
						});
					}
					// Release the flag after scroll completes
					requestAnimationFrame(() => {
						scrollingProgrammatically = false;
					});
				});
			});
		}
	});

	function handleScroll() {
		if (!messagesContainer) return;
		// Ignore scroll events triggered by programmatic scrolling (auto-scroll effect)
		if (scrollingProgrammatically) return;

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

	function scrollToBottom() {
		if (messagesContainer) {
			scrollingProgrammatically = true;
			messagesContainer.scrollTo({
				top: messagesContainer.scrollHeight,
				behavior: 'smooth'
			});
			showScrollButton = false;
			autoScrollEnabled = true;
			// Release after scroll settles
			requestAnimationFrame(() => {
				requestAnimationFrame(() => {
					scrollingProgrammatically = false;
				});
			});
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
			log.error('Failed to copy', err);
		}
	}

	function openToolSidebar(tool: ToolCall) {
		sidebarTool = tool;
	}

	function closeToolSidebar() {
		sidebarTool = null;
	}

	function resetChat() {
		// Reset chat to start a new session
		messages = [];
		currentStreamingMessage = null;
		inputValue = '';
		clearDraft();
		// Request a new introduction
		if (chatId) {
			requestIntroduction();
		}
	}

	// Voice mode state - continuous listening
	let voiceMode = $state(false);
	let isTogglingRecording = $state(false);
	let recordingError = $state<string | null>(null);
	let recordingTranscript = $state('');
	// eslint-disable-next-line @typescript-eslint/no-explicit-any
	let recognition: any = null;
	let silenceTimer: ReturnType<typeof setTimeout> | null = null;
	const SILENCE_TIMEOUT = 2000;

	async function toggleRecording() {
		// Debounce rapid clicks
		if (isTogglingRecording) return;
		isTogglingRecording = true;

		try {
			if (voiceMode) {
				// Exit voice mode
				voiceMode = false;
				stopRecording();
			} else {
				// Enter voice mode
				voiceMode = true;
				await startRecording();
			}
		} finally {
			requestAnimationFrame(() => {
				isTogglingRecording = false;
			});
		}
	}

	async function startRecording() {
		recordingError = null;
		recordingTranscript = '';

		// eslint-disable-next-line @typescript-eslint/no-explicit-any
		const SpeechRecognition = (window as any).SpeechRecognition || (window as any).webkitSpeechRecognition;
		if (!SpeechRecognition) {
			recordingError = 'Speech recognition not supported. Try Chrome.';
			return;
		}

		// Check microphone permission
		try {
			const permissionStatus = await navigator.permissions.query({ name: 'microphone' as PermissionName });
			if (permissionStatus.state === 'denied') {
				recordingError = 'Microphone permission denied. Allow in browser settings.';
				return;
			}
		} catch {
			// Permission API not supported, continue
		}

		try {
			recognition = new SpeechRecognition();
			recognition.continuous = false; // Stop after pause - triggers auto-send
			recognition.interimResults = true;
			recognition.lang = navigator.language || 'en-US';

			recognition.onstart = () => {
				isRecording = true;
				recordingError = null;
			};

			// eslint-disable-next-line @typescript-eslint/no-explicit-any
			recognition.onresult = (event: any) => {
				voiceLog.debug('onresult fired');

				// Reset silence timer on any speech
				if (silenceTimer) {
					clearTimeout(silenceTimer);
					silenceTimer = null;
				}

				let interim = '';
				for (let i = event.resultIndex; i < event.results.length; i++) {
					const transcript = event.results[i][0].transcript;
					if (event.results[i].isFinal) {
						recordingTranscript += transcript + ' ';
						voiceLog.debug('Final transcript: ' + recordingTranscript);
					} else {
						interim += transcript;
					}
				}
				inputValue = recordingTranscript + interim;

				// Start silence timer - if no more speech for 2s, auto-submit
				if (inputValue.trim()) {
					voiceLog.debug('Starting 2s silence timer');
					silenceTimer = setTimeout(() => {
						voiceLog.debug('Silence timer fired, isRecording: ' + isRecording);
						if (isRecording && inputValue.trim()) {
							voiceLog.debug('Auto-sending!');
							stopRecording();
							sendMessage();
						}
					}, SILENCE_TIMEOUT);
				}
			};

			// eslint-disable-next-line @typescript-eslint/no-explicit-any
			recognition.onerror = (event: any) => {
				voiceLog.error('Speech recognition error: ' + event.error);
				isRecording = false;

				switch (event.error) {
					case 'no-speech':
						recordingError = 'No speech detected. Try again.';
						break;
					case 'not-allowed':
					case 'service-not-allowed':
						recordingError = 'Microphone access denied.';
						break;
					case 'network':
						recordingError = 'Network error. Try Chrome (not Brave).';
						break;
					case 'audio-capture':
						recordingError = 'No microphone found.';
						break;
					case 'aborted':
						break;
					default:
						recordingError = `Error: ${event.error}`;
				}
			};

			// When speech ends (user stops talking), start shorter timer
			recognition.onspeechend = () => {
				voiceLog.debug('onspeechend fired');
				if (silenceTimer) {
					clearTimeout(silenceTimer);
				}
				// Shorter delay after speech officially ends
				silenceTimer = setTimeout(() => {
					voiceLog.debug('onspeechend timer fired');
					if (isRecording && inputValue.trim()) {
						voiceLog.debug('Auto-sending from onspeechend!');
						stopRecording();
						sendMessage();
					}
				}, 1000); // 1 second after speech ends
			};

			recognition.onend = () => {
				voiceLog.debug('onend fired, inputValue: ' + inputValue.trim().substring(0, 50));
				const hadContent = inputValue.trim();
				isRecording = false;
				recognition = null;
				// Clear any pending timer
				if (silenceTimer) {
					clearTimeout(silenceTimer);
					silenceTimer = null;
				}
				// If we have content when recognition ends, send it
				if (hadContent && !isLoading) {
					voiceLog.debug('Auto-sending from onend!');
					sendMessage();
				}
				// If still in voice mode, restart listening after bot responds (and TTS finishes)
				if (voiceMode) {
					const waitForResponse = () => {
						// Wait until bot is done responding AND TTS is done speaking
						if (isLoading || isSpeaking) {
							setTimeout(waitForResponse, 500);
							return;
						}
						if (voiceMode && !isRecording) {
							voiceLog.debug('Restarting listening (voice mode)');
							recordingTranscript = '';
							startRecording();
						}
					};
					// Start checking after a brief delay
					setTimeout(waitForResponse, 1000);
				}
			};

			recognition.start();
		} catch (err) {
			voiceLog.error('Failed to start speech recognition', err);
			recordingError = 'Failed to start recording.';
			recognition = null;
		}
	}

	function stopRecording() {
		if (silenceTimer) {
			clearTimeout(silenceTimer);
			silenceTimer = null;
		}
		if (recognition) {
			recognition.abort();
			recognition = null;
		}
		isRecording = false;
	}

	function exitVoiceMode() {
		voiceMode = false;
		stopRecording();
	}

	// TTS playback function
	async function speakText(text: string) {
		if (!voiceOutputEnabled || !text.trim()) return;

		// Stop any current playback
		if (currentAudio) {
			currentAudio.pause();
			currentAudio = null;
		}

		// Clean text for TTS (remove markdown formatting)
		const cleanText = text
			.replace(/```[\s\S]*?```/g, '') // Remove code blocks
			.replace(/`[^`]+`/g, '') // Remove inline code
			.replace(/\[([^\]]+)\]\([^)]+\)/g, '$1') // Convert links to text
			.replace(/[*_~]+/g, '') // Remove bold/italic/strikethrough
			.replace(/^#+\s*/gm, '') // Remove headers
			.replace(/^[-*]\s*/gm, '') // Remove list markers
			.trim();

		if (!cleanText) return;

		isSpeaking = true;

		try {
			const audioBlob = await speakTTS({
				text: cleanText,
				voice: ttsVoice,
				speed: 1.0
			});
			const audioUrl = URL.createObjectURL(audioBlob);

			currentAudio = new Audio(audioUrl);
			currentAudio.onended = () => {
				isSpeaking = false;
				URL.revokeObjectURL(audioUrl);
				currentAudio = null;
			};
			currentAudio.onerror = () => {
				log.error('Audio playback error');
				isSpeaking = false;
				URL.revokeObjectURL(audioUrl);
				currentAudio = null;
			};
			currentAudio.play();
		} catch (err) {
			log.error('TTS failed', err);
			isSpeaking = false;
		}
	}

	function stopSpeaking() {
		if (currentAudio) {
			currentAudio.pause();
			currentAudio = null;
		}
		isSpeaking = false;
	}

	function toggleVoiceOutput() {
		if (voiceOutputEnabled) {
			voiceOutputEnabled = false;
			stopSpeaking();
		} else {
			voiceOutputEnabled = true;
		}
	}

	// Auto-focus textarea when user starts typing anywhere
	function handleGlobalKeydown(e: KeyboardEvent) {
		// Escape exits voice mode
		if (e.key === 'Escape' && voiceMode) {
			e.preventDefault();
			voiceMode = false;
			stopRecording();
			return;
		}

		if (document.activeElement?.tagName === 'INPUT' || document.activeElement?.tagName === 'TEXTAREA') {
			return;
		}
		if (e.ctrlKey || e.metaKey || e.altKey || e.key.length > 1) {
			return;
		}
		if (chatInputRef && !isRecording) {
			chatInputRef.focus();
		}
	}
</script>

<svelte:head>
	<title>Nebo - Your AI Companion</title>
</svelte:head>

<svelte:window onkeydown={handleGlobalKeydown} />

<!-- Prevent browser from navigating away when files/images are dragged onto the page -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
	class="flex flex-col h-full bg-base-100"
	ondragover={(e) => e.preventDefault()}
	ondrop={(e) => e.preventDefault()}
>
	<!-- Header -->
	<header class="border-b border-base-300 bg-base-100/80 backdrop-blur-sm shrink-0">
		<div class="max-w-4xl mx-auto flex items-center justify-between px-6 h-14">
		<div class="flex flex-col justify-center">
			<h1 class="text-lg font-semibold text-base-content leading-tight">Chat</h1>
			<p class="text-xs text-base-content/50 leading-tight">Direct chat session with your AI companion.</p>
		</div>
		<div class="flex items-center gap-2 shrink-0">
			<!-- Connection status -->
			{#if wsConnected}
				<div class="flex items-center gap-1.5 text-xs text-success px-2">
					<span class="w-1.5 h-1.5 rounded-full bg-success"></span>
					<span class="hidden sm:inline">Connected</span>
				</div>
			{:else}
				<div class="flex items-center gap-1.5 text-xs text-warning px-2">
					<span class="w-1.5 h-1.5 rounded-full bg-warning"></span>
					<span class="hidden sm:inline">Offline</span>
				</div>
			{/if}
			<!-- Voice output toggle -->
			<button
				type="button"
				onclick={toggleVoiceOutput}
				class="btn btn-sm btn-ghost btn-square"
				class:text-primary={voiceOutputEnabled}
				title={voiceOutputEnabled ? 'Disable voice output' : 'Enable voice output'}
			>
				{#if voiceOutputEnabled}
					<Volume2 class="w-4 h-4" />
				{:else}
					<VolumeOff class="w-4 h-4" />
				{/if}
			</button>
			<!-- History link -->
			<a href="/agent/history" class="btn btn-sm btn-ghost gap-1.5" title="View history">
				<History class="w-4 h-4" />
				<span class="hidden sm:inline">History</span>
			</a>
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
			<div class="max-w-4xl mx-auto p-6 space-y-6">
		{#if hasMoreHistory}
			<div class="flex justify-center">
				<a
					href="/agent/history"
					class="flex items-center gap-2 px-4 py-2 rounded-lg bg-base-200 text-sm text-base-content/70 hover:bg-base-300 hover:text-base-content transition-colors"
				>
					<History class="w-4 h-4" />
					<span>View {totalMessages - messages.length} earlier messages in history</span>
				</a>
			</div>
		{/if}
		{#if !chatLoaded}
			<div class="flex items-center justify-center h-full">
				<Loader2 class="w-6 h-6 text-base-content/40 animate-spin" />
			</div>
		{:else if messages.length === 0}
			<!-- Empty state with suggestions -->
			<div class="flex flex-col items-center justify-center pt-12 text-center">
				<div class="w-16 h-16 rounded-2xl bg-primary/10 flex items-center justify-center mb-4">
					<Bot class="w-8 h-8 text-primary" />
				</div>
				<h2 class="font-display text-xl font-bold text-base-content mb-2">Your AI Companion</h2>
				<p class="text-sm text-base-content/60 max-w-md mb-8">
					I'm here to help with tasks like reading files, running commands, searching the web, and more.
				</p>

				<div class="grid grid-cols-1 sm:grid-cols-2 gap-3 max-w-lg w-full">
					{#each suggestions as suggestion}
						<button
							type="button"
							onclick={() => selectSuggestion(suggestion)}
							class="text-left px-4 py-3 rounded-xl bg-base-200 text-sm text-base-content/70 hover:bg-base-300 hover:text-base-content transition-colors"
							disabled={isLoading}
						>
							{suggestion}
						</button>
					{/each}
				</div>
			</div>
		{:else}
			<!-- Grouped messages for Slack-style display -->
			{#each groupedMessages as group, groupIndex (groupIndex)}
				<MessageGroup
					messages={group.messages}
					role={group.role}
					onCopy={copyMessage}
					copiedId={copiedMessageId}
					onViewToolOutput={openToolSidebar}
					isStreaming={group.role === 'assistant' && isLoading && groupIndex === groupedMessages.length - 1}
				/>
			{/each}

			<!-- Loading indicator when waiting for response -->
			{#if isLoading && !currentStreamingMessage && (groupedMessages.length === 0 || groupedMessages[groupedMessages.length - 1]?.role !== 'assistant')}
				<div class="flex gap-3 mb-4">
					<div class="w-10 h-10 rounded-lg flex-shrink-0 self-end mb-1 grid place-items-center font-semibold text-sm bg-base-300 text-base-content/60">
						A
					</div>
					<div class="flex flex-col gap-0.5 max-w-[min(900px,calc(100%-60px))] items-start">
						<div class="rounded-xl px-3.5 py-2.5 bg-base-200 animate-pulse-border">
							<ReadingIndicator />
						</div>
						<div class="flex gap-2 items-baseline mt-1.5">
							<span class="text-xs font-medium text-base-content/50">Assistant</span>
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
	<ChatInput
		bind:this={chatInputRef}
		bind:value={inputValue}
		{isLoading}
		{isRecording}
		{voiceMode}
		queuedMessages={messageQueue}
		onSend={sendMessage}
		onCancel={cancelMessage}
		onCancelQueued={cancelQueuedMessage}
		onNewSession={resetChat}
		onToggleVoice={toggleRecording}
	/>
</div>

<ApprovalModal
	request={pendingApproval}
	onApprove={handleApprove}
	onApproveAlways={handleApproveAlways}
	onDeny={handleDeny}
/>

<ToolOutputSidebar tool={sidebarTool} onClose={closeToolSidebar} />
