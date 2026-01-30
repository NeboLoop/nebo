<script lang="ts">
	import { onMount, onDestroy, tick } from 'svelte';
	import { browser } from '$app/environment';
	import { Send, Bot, Loader2, Mic, MicOff, Wifi, WifiOff, ArrowDown, Copy, Check, History, Volume2, VolumeOff } from 'lucide-svelte';
	import { getWebSocketClient, type ConnectionStatus } from '$lib/websocket/client';
	import { getCompanionChat } from '$lib/api';
	import type { ChatMessage as ApiChatMessage } from '$lib/api';
	import Markdown from '$lib/components/ui/Markdown.svelte';
	import ApprovalModal from '$lib/components/ui/ApprovalModal.svelte';

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
	}

	interface ToolCall {
		name: string;
		input: string;
		output?: string;
		status?: 'running' | 'complete' | 'error';
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

	// Message queue
	let messageQueue = $state<string[]>([]);
	let textareaElement: HTMLTextAreaElement;

	// Approval requests
	let pendingApproval = $state<ApprovalRequest | null>(null);

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

	async function loadCompanionChat() {
		try {
			const res = await getCompanionChat();
			chatId = res.chat.id;
			messages = (res.messages || []).map((m: ApiChatMessage) => ({
				id: m.id,
				role: m.role as 'user' | 'assistant' | 'system',
				content: m.content,
				timestamp: new Date(m.createdAt)
			}));
			totalMessages = res.totalMessages || messages.length;
			chatLoaded = true;
		} catch (err) {
			console.error('Failed to load companion chat:', err);
			chatLoaded = true; // Still mark as loaded, will show empty state
		}
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
			isLoading = false;
		}
	}

	function handleChatStream(data: Record<string, unknown>) {
		// Accept messages for our chat or if we haven't loaded yet
		if (chatId && data?.session_id !== chatId) {
			return;
		}

		// If we just got a session_id and didn't have one, save it
		if (!chatId && data?.session_id) {
			chatId = data.session_id as string;
		}

		const chunk = (data?.content as string) || '';

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
		if (chatId && data?.session_id !== chatId) return;

		let completedContent = '';
		if (currentStreamingMessage) {
			completedContent = currentStreamingMessage.content;
			currentStreamingMessage.streaming = false;
			messages = [...messages.slice(0, -1), { ...currentStreamingMessage }];
			currentStreamingMessage = null;
		}
		isLoading = false;

		// Speak the response if voice output is enabled
		if (voiceOutputEnabled && completedContent) {
			speakText(completedContent);
		}

		// Process queued messages
		if (messageQueue.length > 0) {
			const nextPrompt = messageQueue[0];
			messageQueue = messageQueue.slice(1);
			handleSendPrompt(nextPrompt);
		}
	}

	function handleChatResponse(data: Record<string, unknown>) {
		if (chatId && data?.session_id !== chatId) return;

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
		if (chatId && data?.session_id !== chatId) return;

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
		if (chatId && data?.session_id !== chatId) return;

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
		if (chatId && data?.session_id !== chatId) return;

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

	function handleApproveAlways(requestId: string) {
		const client = getWebSocketClient();
		client.send('approval_response', {
			request_id: requestId,
			approved: true,
			always: true
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

	function selectSuggestion(text: string) {
		inputValue = text;
		sendMessage();
	}

	// Auto-scroll when new messages arrive or streaming content updates
	$effect(() => {
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
				console.log('[Voice] onresult fired');

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
						console.log('[Voice] Final transcript:', recordingTranscript);
					} else {
						interim += transcript;
					}
				}
				inputValue = recordingTranscript + interim;

				// Start silence timer - if no more speech for 2s, auto-submit
				if (inputValue.trim()) {
					console.log('[Voice] Starting 2s silence timer');
					silenceTimer = setTimeout(() => {
						console.log('[Voice] Silence timer fired, isRecording:', isRecording);
						if (isRecording && inputValue.trim()) {
							console.log('[Voice] Auto-sending!');
							stopRecording();
							sendMessage();
						}
					}, SILENCE_TIMEOUT);
				}
			};

			// eslint-disable-next-line @typescript-eslint/no-explicit-any
			recognition.onerror = (event: any) => {
				console.error('Speech recognition error:', event.error);
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
				console.log('[Voice] onspeechend fired');
				if (silenceTimer) {
					clearTimeout(silenceTimer);
				}
				// Shorter delay after speech officially ends
				silenceTimer = setTimeout(() => {
					console.log('[Voice] onspeechend timer fired');
					if (isRecording && inputValue.trim()) {
						console.log('[Voice] Auto-sending from onspeechend!');
						stopRecording();
						sendMessage();
					}
				}, 1000); // 1 second after speech ends
			};

			recognition.onend = () => {
				console.log('[Voice] onend fired, inputValue:', inputValue.trim().substring(0, 50));
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
					console.log('[Voice] Auto-sending from onend!');
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
							console.log('[Voice] Restarting listening (voice mode)');
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
			console.error('Failed to start speech recognition:', err);
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
			const response = await fetch('/api/v1/voice/tts', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({
					text: cleanText,
					voice: ttsVoice,
					speed: 1.0
				})
			});

			if (!response.ok) {
				console.error('TTS error:', await response.text());
				isSpeaking = false;
				return;
			}

			const audioBlob = await response.blob();
			const audioUrl = URL.createObjectURL(audioBlob);

			currentAudio = new Audio(audioUrl);
			currentAudio.onended = () => {
				isSpeaking = false;
				URL.revokeObjectURL(audioUrl);
				currentAudio = null;
			};
			currentAudio.onerror = () => {
				console.error('Audio playback error');
				isSpeaking = false;
				URL.revokeObjectURL(audioUrl);
				currentAudio = null;
			};
			currentAudio.play();
		} catch (err) {
			console.error('TTS failed:', err);
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
		if (textareaElement && !isRecording) {
			textareaElement.focus();
		}
	}
</script>

<svelte:head>
	<title>Nebo - Your AI Companion</title>
</svelte:head>

<svelte:window onkeydown={handleGlobalKeydown} />

<div class="flex flex-col h-full bg-base-100">
	<!-- Header -->
	<header class="flex items-center justify-between px-6 h-14 border-b border-base-300 bg-base-100/80 backdrop-blur-sm shrink-0">
		<div class="flex items-center gap-3">
			<div class="w-8 h-8 rounded-lg bg-primary/10 flex items-center justify-center">
				<Bot class="w-4 h-4 text-primary" />
			</div>
			<h1 class="text-lg font-semibold text-base-content">Companion</h1>
		</div>
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
								<div class="rounded-2xl bg-base-200/50 px-4 py-3 border border-base-300/50">
									{#if message.streaming}
										<Markdown content={message.content} />
										<span class="inline-block w-2 h-4 bg-base-content/50 animate-pulse ml-0.5"></span>
									{:else}
										<Markdown content={message.content} />
									{/if}
								</div>
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
					class="btn btn-sm btn-square self-end mb-1 {voiceMode ? 'btn-error animate-pulse' : 'btn-ghost'}"
					title={voiceMode ? 'Exit voice mode (Esc)' : 'Enter voice mode'}
				>
					{#if voiceMode}
						<MicOff class="w-4 h-4" />
					{:else}
						<Mic class="w-4 h-4" />
					{/if}
				</button>
				<div class="dropdown dropdown-top dropdown-end self-end mb-1">
				<button
					type="button"
					tabindex="0"
					class="btn btn-sm btn-square {voiceOutputEnabled ? 'btn-success' : 'btn-ghost'} {isSpeaking ? 'animate-pulse' : ''}"
					title="Voice output settings"
				>
					{#if voiceOutputEnabled}
						<Volume2 class="w-4 h-4" />
					{:else}
						<VolumeOff class="w-4 h-4" />
					{/if}
				</button>
				<div tabindex="0" class="dropdown-content z-50 menu p-3 shadow-lg bg-base-200 rounded-box w-52 mb-2">
					<div class="flex items-center justify-between mb-3">
						<span class="text-sm font-medium">Voice Output</span>
						<input
							type="checkbox"
							class="toggle toggle-success toggle-sm"
							checked={voiceOutputEnabled}
							onchange={toggleVoiceOutput}
						/>
					</div>
					{#if voiceOutputEnabled}
						<div class="form-control">
							<label class="label py-1">
								<span class="label-text text-xs">Voice</span>
							</label>
							<select
								class="select select-bordered select-sm w-full"
								bind:value={ttsVoice}
							>
								{#each ttsVoices as voice}
									<option value={voice}>{voice.charAt(0).toUpperCase() + voice.slice(1)}</option>
								{/each}
							</select>
						</div>
						{#if isSpeaking}
							<button
								type="button"
								class="btn btn-sm btn-error mt-3"
								onclick={stopSpeaking}
							>
								Stop Speaking
							</button>
						{/if}
					{/if}
				</div>
			</div>
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
					{#if recordingError}
						<span class="text-error">{recordingError}</span>
					{:else if voiceMode && isRecording}
						<span class="text-error">Voice mode: Listening... (pause to send, Esc to exit)</span>
					{:else if voiceMode}
						<span class="text-warning">Voice mode: Starting...</span>
					{:else if isSpeaking}
						<span class="text-success">Speaking response...</span>
					{:else if messageQueue.length > 0}
						<span class="text-info">{messageQueue.length} message{messageQueue.length > 1 ? 's' : ''} queued</span>
					{:else if isLoading}
						<span>Type to queue your next message</span>
					{:else if voiceOutputEnabled}
						<span class="text-success">Voice output enabled (ElevenLabs)</span>
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
	onApproveAlways={handleApproveAlways}
	onDeny={handleDeny}
/>
