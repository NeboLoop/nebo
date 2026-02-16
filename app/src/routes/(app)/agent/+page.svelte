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
		type: 'text' | 'tool' | 'image';
		text?: string;          // accumulated text for text blocks
		toolCallIndex?: number; // index into toolCalls for tool blocks
		imageData?: string;     // base64 data for image blocks
		imageMimeType?: string; // e.g. "image/png"
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

	// Streaming TTS queue — speaks sentences as they arrive during streaming
	let ttsSentenceBuffer = ''; // Accumulates text until a sentence boundary
	let ttsQueue: string[] = []; // Sentences waiting to be spoken
	let ttsPlaying = false; // Whether the queue player is active
	let ttsStreamingActive = false; // Whether we're collecting from the stream
	let ttsCancelToken = 0; // Incremented on stop to kill any running playNextTTS loop

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
	let chatInputRef: { focus: () => void; handleDrop: (e: DragEvent) => void } | undefined;
	let isDraggingOver = $state(false);
	let dragCounter = 0; // Track enter/leave for nested elements
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
		// Clean up voice mode (kills stream, monitor, recorder, audio)
		exitVoiceMode();
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

	// Detect Anthropic-format multipart content arrays and convert to ContentBlocks
	function parseMultipartContent(content: string): { text: string; blocks: ContentBlock[] } | null {
		if (!content.startsWith('[')) return null;
		try {
			const parts = JSON.parse(content);
			if (!Array.isArray(parts)) return null;
			const blocks: ContentBlock[] = [];
			const textParts: string[] = [];
			for (const part of parts) {
				if (part.type === 'text' && part.text) {
					blocks.push({ type: 'text', text: part.text });
					textParts.push(part.text);
				} else if (part.type === 'image' && part.source?.data) {
					blocks.push({
						type: 'image',
						imageData: part.source.data,
						imageMimeType: part.source.media_type || 'image/png'
					});
				}
			}
			if (blocks.length === 0) return null;
			return { text: textParts.join('\n'), blocks };
		} catch {
			return null;
		}
	}

	async function loadCompanionChat() {
		try {
			const res = await getCompanionChat();
			chatId = res.chat.id;
			log.debug('Loaded companion chat: ' + chatId);
			messages = (res.messages || []).map((m: ApiChatMessage) => {
				const meta = parseMetadata((m as { metadata?: string }).metadata);
				let content = m.content;
				let contentBlocks = meta.contentBlocks;

				// Detect multipart content (images) stored as JSON arrays
				if (!contentBlocks?.length) {
					const multipart = parseMultipartContent(content);
					if (multipart) {
						content = multipart.text;
						contentBlocks = multipart.blocks;
					}
				}

				return {
					id: m.id,
					role: m.role as 'user' | 'assistant' | 'system',
					content,
					timestamp: new Date(m.createdAt),
					toolCalls: meta.toolCalls,
					thinking: meta.thinking,
					contentBlocks
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

		// Feed chunk to streaming TTS (speaks sentences as they complete)
		if (voiceOutputEnabled && chunk) {
			if (!ttsStreamingActive) {
				// First chunk of a new response — start streaming TTS
				stopTTSQueue();
				ttsStreamingActive = true;
			}
			feedTTSStream(chunk);
		}

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

		// Flush any remaining TTS buffer (streaming TTS already spoke most of it)
		if (voiceOutputEnabled && ttsStreamingActive) {
			flushTTSBuffer();
		} else if (voiceOutputEnabled && completedContent) {
			// Fallback: if streaming TTS wasn't active, speak the full response
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

		// Barge-in: if agent is responding, cancel and send the new message immediately
		if (isLoading) {
			log.debug('Barge-in: cancelling current response and sending: ' + prompt.substring(0, 50));

			// Stop any TTS playback
			stopSpeaking();
			stopTTSQueue();

			// Cancel the active response on the backend
			const client = getWebSocketClient();
			client.send('cancel', { session_id: chatId || '' });

			// Mark the current streaming message as interrupted
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

			// Clear any queued messages — new message supersedes them
			messageQueue = [];

			// Fall through to send the new message immediately
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

	// Voice mode state - conversational loop with interruption support
	// State machine: idle → listening → processing → speaking → listening (loop)
	let voiceMode = $state(false);
	let isTogglingRecording = $state(false);
	let recordingError = $state<string | null>(null);
	let recordingTranscript = $state('');

	// Persistent mic stream — stays alive throughout voice mode
	let voiceStream: MediaStream | null = null;
	let mediaRecorder: MediaRecorder | null = null;
	let audioChunks: Blob[] = [];
	let silenceTimer: ReturnType<typeof setTimeout> | null = null;
	let audioContext: AudioContext | null = null;
	let analyser: AnalyserNode | null = null;
	let voiceMonitorInterval: ReturnType<typeof setInterval> | null = null;
	let voiceMimeType = '';

	const SILENCE_TIMEOUT = 2500; // 2.5s of silence before finishing recording
	const SILENCE_THRESHOLD = 0.06; // RMS threshold for silence detection (raised to ignore keyboard clicks)
	const INTERRUPT_THRESHOLD = 0.02; // LOW threshold — user must be able to interrupt TTS easily

	async function toggleRecording() {
		// Debounce rapid clicks
		if (isTogglingRecording) return;
		isTogglingRecording = true;

		try {
			if (voiceMode) {
				exitVoiceMode();
			} else {
				await enterVoiceMode();
			}
		} finally {
			requestAnimationFrame(() => {
				isTogglingRecording = false;
			});
		}
	}

	async function enterVoiceMode() {
		recordingError = null;
		recordingTranscript = '';

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
			// Get persistent mic stream — stays alive for entire voice mode session
			voiceStream = await navigator.mediaDevices.getUserMedia({ audio: true });

			// Set up audio analysis (persistent)
			audioContext = new AudioContext();
			const source = audioContext.createMediaStreamSource(voiceStream);
			analyser = audioContext.createAnalyser();
			analyser.fftSize = 2048;
			source.connect(analyser);

			// Determine best supported MIME type
			voiceMimeType = MediaRecorder.isTypeSupported('audio/webm;codecs=opus')
				? 'audio/webm;codecs=opus'
				: MediaRecorder.isTypeSupported('audio/webm')
					? 'audio/webm'
					: MediaRecorder.isTypeSupported('audio/ogg;codecs=opus')
						? 'audio/ogg;codecs=opus'
						: '';

			voiceMode = true;
			voiceOutputEnabled = true; // Auto-enable TTS in voice mode

			// Start the persistent voice monitor — handles interruption + silence detection
			startVoiceMonitor();

			// Begin first listening turn
			startListening();

		} catch (err) {
			voiceLog.error('Failed to enter voice mode', err);
			if ((err as Error)?.name === 'NotAllowedError') {
				recordingError = 'Microphone access denied.';
			} else if ((err as Error)?.name === 'NotFoundError') {
				recordingError = 'No microphone found.';
			} else {
				recordingError = 'Failed to start recording.';
			}
		}
	}

	function exitVoiceMode() {
		voiceMode = false;
		voiceMonitorStarting = false;
		stopSpeaking();
		stopListening();
		stopVoiceMonitor();

		// Kill persistent stream
		if (voiceStream) {
			voiceStream.getTracks().forEach(t => t.stop());
			voiceStream = null;
		}
		if (audioContext) {
			audioContext.close().catch(() => {});
			audioContext = null;
		}
		analyser = null;
		isRecording = false;
	}

	// Start recording from the persistent stream
	function startListening() {
		if (!voiceStream || !voiceMode) return;

		// Interrupt TTS immediately when user starts talking
		stopTTSQueue();

		audioChunks = [];
		recordingTranscript = '';

		const options: MediaRecorderOptions = {};
		if (voiceMimeType) options.mimeType = voiceMimeType;

		mediaRecorder = new MediaRecorder(voiceStream, options);

		mediaRecorder.ondataavailable = (event) => {
			if (event.data.size > 0) {
				audioChunks.push(event.data);
			}
		};

		mediaRecorder.onstop = async () => {
			// Capture chunks locally before they get cleared by startListening
			const chunks = audioChunks.slice();
			audioChunks = [];

			// Don't stop stream tracks — they're persistent
			if (chunks.length === 0) {
				restartListeningIfNeeded();
				return;
			}

			const audioBlob = new Blob(chunks, { type: voiceMimeType || 'audio/webm' });

			// Skip tiny recordings (likely just noise)
			if (audioBlob.size < 1000) {
				restartListeningIfNeeded();
				return;
			}

			// Show transcribing state
			inputValue = '🎙️ Transcribing...';

			try {
				const { transcribeAudio } = await import('$lib/api');
				const result = await transcribeAudio(audioBlob);
				const text = result.text?.trim() || '';
				handleRecordingComplete(text);
			} catch (err) {
				voiceLog.error('Transcription failed', err);
				inputValue = '';
				recordingError = 'Transcription failed. Check that whisper-cli is installed.';
				restartListeningIfNeeded();
			}
		};

		mediaRecorder.onerror = () => {
			voiceLog.error('MediaRecorder error');
			isRecording = false;
			recordingError = 'Recording error.';
		};

		mediaRecorder.start(250);
		isRecording = true;
		recordingError = null;
		voiceLog.debug('Listening started');
	}

	function stopListening() {
		if (silenceTimer) {
			clearTimeout(silenceTimer);
			silenceTimer = null;
		}
		if (mediaRecorder && mediaRecorder.state !== 'inactive') {
			// Stop without triggering transcription
			mediaRecorder.ondataavailable = null;
			mediaRecorder.onstop = () => {}; // no-op
			mediaRecorder.stop();
			mediaRecorder = null;
		}
		audioChunks = [];
		isRecording = false;
	}

	function finishRecording() {
		if (mediaRecorder && mediaRecorder.state !== 'inactive') {
			mediaRecorder.stop(); // triggers onstop → transcription
			// Don't set isRecording = false here — let onstop/handleRecordingComplete do it
			// Otherwise the monitor restarts listening before transcription processes
		} else {
			isRecording = false;
		}
	}

	// Persistent voice monitor — runs entire voice mode session
	// Handles: silence detection while recording, interruption detection while speaking
	let voiceMonitorStarting = false; // guard against double-start from interval race

	function startVoiceMonitor() {
		if (!analyser) return;

		const dataArray = new Float32Array(analyser.fftSize);
		let silenceStart: number | null = null;
		let hasSpeech = false;

		voiceMonitorInterval = setInterval(() => {
			if (!analyser || !voiceMode) return;

			analyser.getFloatTimeDomainData(dataArray);

			// Calculate RMS
			let sum = 0;
			for (let i = 0; i < dataArray.length; i++) {
				sum += dataArray[i] * dataArray[i];
			}
			const rms = Math.sqrt(sum / dataArray.length);

			// INTERRUPT MODE: If TTS is playing and user speaks, stop TTS immediately
			if (isSpeaking && rms > INTERRUPT_THRESHOLD) {
				voiceLog.debug('User interrupted TTS — stopping playback');
				stopSpeaking();
				return;
			}

			// LISTENING MODE: Detect silence after speech → finish recording
			if (isRecording) {
				if (rms > SILENCE_THRESHOLD) {
					hasSpeech = true;
					silenceStart = null;
				} else if (hasSpeech) {
					if (silenceStart === null) {
						silenceStart = Date.now();
					} else if (Date.now() - silenceStart > SILENCE_TIMEOUT) {
						voiceLog.debug('Silence detected, finishing recording');
						hasSpeech = false;
						silenceStart = null;
						finishRecording();
					}
				}
			}

			// IDLE: Not recording, not speaking, voice mode active → restart listening
			// Key change: we listen even while isLoading (bot streaming) so user can
			// speak their next message while the response comes in
			if (!isRecording && !isSpeaking && voiceMode && !voiceMonitorStarting) {
				hasSpeech = false;
				silenceStart = null;
				voiceMonitorStarting = true;
				startListening();
				// Clear guard after a tick so startListening has time to set isRecording
				setTimeout(() => { voiceMonitorStarting = false; }, 200);
			}
		}, 100);
	}

	function stopVoiceMonitor() {
		if (voiceMonitorInterval) {
			clearInterval(voiceMonitorInterval);
			voiceMonitorInterval = null;
		}
	}

	function handleRecordingComplete(text: string) {
		isRecording = false;

		if (text && text !== '[BLANK_AUDIO]' && text !== '(silence)') {
			inputValue = text;
			voiceLog.debug('Transcribed: ' + text);

			// Auto-send — always send in voice mode, even if bot is still streaming
			// The WebSocket handles queuing; the user shouldn't have to wait
			sendMessage();
		} else {
			inputValue = '';
			// Empty transcription — restart listening via monitor loop
		}
	}

	function restartListeningIfNeeded() {
		// The voice monitor handles restart — just make sure state is clean
		isRecording = false;
	}

	// Clean markdown/formatting from text for TTS
	function cleanTextForTTS(text: string): string {
		return text
			.replace(/```[\s\S]*?```/g, '') // Remove code blocks
			.replace(/`[^`]+`/g, '') // Remove inline code
			.replace(/\[([^\]]+)\]\([^)]+\)/g, '$1') // Convert links to text
			.replace(/[*_~]+/g, '') // Remove bold/italic/strikethrough
			.replace(/^#+\s*/gm, '') // Remove headers
			.replace(/^[-*]\s*/gm, '') // Remove list markers
			.replace(/\n{2,}/g, '. ') // Collapse paragraph breaks
			.replace(/\n/g, ' ') // Collapse remaining newlines
			.replace(/\s{2,}/g, ' ') // Collapse whitespace
			.trim();
	}

	// Feed streaming text chunks into the TTS sentence buffer
	function feedTTSStream(chunk: string) {
		if (!voiceOutputEnabled || !ttsStreamingActive) return;

		ttsSentenceBuffer += chunk;

		// Extract complete sentences from the buffer
		// Match sentences ending with . ! ? followed by space or end-of-string
		const sentencePattern = /^([\s\S]*?[.!?])(\s|$)/;
		let match;
		while ((match = sentencePattern.exec(ttsSentenceBuffer)) !== null) {
			const sentence = match[1].trim();
			ttsSentenceBuffer = ttsSentenceBuffer.slice(match[0].length);

			const clean = cleanTextForTTS(sentence);
			if (clean.length > 2) { // Skip tiny fragments
				ttsQueue.push(clean);
				playNextTTS(); // Kick the queue player
			}
		}
	}

	// Flush any remaining buffered text (called on stream complete)
	function flushTTSBuffer() {
		if (ttsSentenceBuffer.trim()) {
			const clean = cleanTextForTTS(ttsSentenceBuffer);
			if (clean.length > 2) {
				ttsQueue.push(clean);
				playNextTTS();
			}
		}
		ttsSentenceBuffer = '';
		ttsStreamingActive = false;
	}

	// Play the next sentence from the queue
	async function playNextTTS() {
		if (ttsPlaying || ttsQueue.length === 0) return;
		ttsPlaying = true;
		isSpeaking = true;
		const myToken = ttsCancelToken;

		while (ttsQueue.length > 0) {
			// Bail if cancelled (stopTTSQueue was called)
			if (ttsCancelToken !== myToken) break;

			const sentence = ttsQueue.shift()!;

			try {
				const audioBlob = await speakTTS({
					text: sentence,
					voice: ttsVoice,
					speed: 1.0
				});

				// Check again after async TTS fetch
				if (ttsCancelToken !== myToken) break;

				const audioUrl = URL.createObjectURL(audioBlob);

				await new Promise<void>((resolve) => {
					currentAudio = new Audio(audioUrl);
					currentAudio.onended = () => {
						URL.revokeObjectURL(audioUrl);
						currentAudio = null;
						resolve();
					};
					currentAudio.onerror = () => {
						log.error('Audio playback error');
						URL.revokeObjectURL(audioUrl);
						currentAudio = null;
						resolve();
					};
					currentAudio.play();
				});
			} catch (err) {
				log.error('TTS failed for sentence', err);
			}

			// Check if TTS was stopped mid-playback (e.g., user interrupted)
			if (!voiceOutputEnabled || ttsCancelToken !== myToken) {
				ttsQueue.length = 0;
				break;
			}
		}

		// Only clear state if we're still the active player (not cancelled)
		if (ttsCancelToken === myToken) {
			ttsPlaying = false;
			isSpeaking = false;
		}
	}

	// Stop all TTS playback and clear the queue
	function stopTTSQueue() {
		ttsCancelToken++; // Kill any running playNextTTS loop
		ttsQueue.length = 0;
		ttsSentenceBuffer = '';
		ttsStreamingActive = false;
		ttsPlaying = false;
		if (currentAudio) {
			// Fire onended/onerror to resolve the pending promise, then pause
			const audio = currentAudio;
			currentAudio = null;
			audio.onended = null;
			audio.onerror = null;
			audio.pause();
		}
		isSpeaking = false;
	}

	// Legacy non-streaming TTS (used when voice output is on but not in voice mode streaming)
	async function speakText(text: string) {
		if (!voiceOutputEnabled || !text.trim()) return;

		stopTTSQueue(); // Clear any pending queue

		const cleanText = cleanTextForTTS(text);
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
			if ('speechSynthesis' in window) {
				const utterance = new SpeechSynthesisUtterance(cleanText);
				utterance.onend = () => { isSpeaking = false; };
				utterance.onerror = () => { isSpeaking = false; };
				speechSynthesis.speak(utterance);
			} else {
				isSpeaking = false;
			}
		}
	}

	function stopSpeaking() {
		stopTTSQueue(); // Clear streaming queue + current playback
		// Also stop browser speechSynthesis if it's running
		if ('speechSynthesis' in window) {
			speechSynthesis.cancel();
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
			exitVoiceMode();
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

<!-- File drop zone for the entire chat area -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
	class="flex flex-col h-full bg-base-100"
	ondragover={(e) => e.preventDefault()}
	ondragenter={(e) => {
		e.preventDefault();
		dragCounter++;
		if (e.dataTransfer?.types.includes('Files')) {
			isDraggingOver = true;
		}
	}}
	ondragleave={() => {
		dragCounter--;
		if (dragCounter <= 0) {
			dragCounter = 0;
			isDraggingOver = false;
		}
	}}
	ondrop={(e) => {
		e.preventDefault();
		dragCounter = 0;
		isDraggingOver = false;
		if (e.dataTransfer?.files.length) {
			chatInputRef?.handleDrop(e);
		}
	}}
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
		{isDraggingOver}
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
