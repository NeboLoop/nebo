<script lang="ts">
	import { onMount, onDestroy, tick } from 'svelte';
	import { browser } from '$app/environment';
	import { t } from 'svelte-i18n';
	import {
		Bot,
		Loader2,
		ArrowDown,
		Copy,
		Check,
		History,
		Settings,
		X
	} from 'lucide-svelte';
	import { getWebSocketClient, type ConnectionStatus } from '$lib/websocket/client';
	import { getCompanionChat, createNewCompanionChat, getChatMessages, speakTTS, getAgentProfile, getChannelMessages, sendChannelMessage } from '$lib/api';
	import { goto } from '$app/navigation';
	import { getAgent, editChatMessage } from '$lib/api/nebo';
	import { logger } from '$lib/monitoring/logger';

	const log = logger.child({ component: 'Chat' });
	const voiceLog = logger.child({ component: 'Voice' });
	import type { ChatMessage as ApiChatMessage } from '$lib/api';
	import type { ChannelMessage, GetChannelMessagesResponse } from '$lib/api/neboComponents';
	import Markdown from '$lib/components/ui/Markdown.svelte';
	import ApprovalModal from '$lib/components/ui/ApprovalModal.svelte';
	import BrowserExtensionModal from '$lib/components/ui/BrowserExtensionModal.svelte';
	import Toast from '$lib/components/ui/Toast.svelte';
	import CodeInstallModal from '$lib/components/chat/CodeInstallModal.svelte';
	import { generateUUID } from '$lib/utils';
	import { VoiceSession, type VoiceState as DuplexVoiceState } from '$lib/voice/VoiceSession';
	import {
		MessageGroup,
		ToolOutputSidebar,
		ReadingIndicator,
		ChatInput
	} from '$lib/components/chat';
	import EntityConfigPanel from '$lib/components/chat/EntityConfigPanel.svelte';
	import { parseSlashCommand } from './slash-commands';
	import { executeSlashCommand, type CommandContext } from './slash-command-executor';
	import type { SlashCommand } from './slash-commands';

	// ── Mode prop ──────────────────────────────────────────────────────
	interface ChatMode {
		type: 'companion' | 'channel' | 'agent';
		channelId?: string;
		channelName?: string;
		loopName?: string;
		agentId?: string;
		agentName?: string;
	}

	let { mode }: { mode: ChatMode } = $props();

	const isCompanion = $derived(mode.type === 'companion');
	const isChannel = $derived(mode.type === 'channel');
	const isAgent = $derived(mode.type === 'agent');

	// Entity config panel state
	let showConfig = $state(false);
	const entityType = $derived(isChannel ? 'channel' : isAgent ? 'agent' : 'main');
	const entityId = $derived(isChannel ? (mode.channelId ?? '') : isAgent ? (mode.agentId ?? '') : 'main');

	// ── Shared interfaces ──────────────────────────────────────────────
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
		contentHtml?: string;
		timestamp: Date;
		toolCalls?: ToolCall[];
		streaming?: boolean;
		thinking?: string;
		contentBlocks?: ContentBlock[];
		senderName?: string; // For channel mode multi-participant display
		proactive?: boolean; // Agent-initiated proactive message
	}

	interface ToolCall {
		id?: string;
		name: string;
		input: string;
		output?: string;
		status?: 'running' | 'complete' | 'error';
	}

	interface SubagentState {
		taskId: string;
		description: string;
		agentType: string;
		status: 'pending' | 'running' | 'complete' | 'error';
		toolCount: number;
		tokenCount: number;
		currentOperation: string;
	}

	interface ContentBlock {
		type: 'text' | 'tool' | 'image' | 'ask' | 'subagent_tree';
		text?: string;
		toolCallIndex?: number;
		imageData?: string;
		imageMimeType?: string;
		imageURL?: string;
		askRequestId?: string;
		askPrompt?: string;
		askWidgets?: Array<{
			type: 'buttons' | 'select' | 'text_input' | 'confirm' | 'radio' | 'checkbox';
			label?: string;
			options?: string[];
			default?: string;
		}>;
		askResponse?: string;
		subagents?: SubagentState[];
	}

	// ── Shared state ───────────────────────────────────────────────────
	let chatId = $state<string | null>(null);
	let messages = $state<Message[]>([]);
	let totalMessages = $state<number>(0);
	let inputValue = $state('');
	let isLoading = $state(false);
	let wsConnected = $state(false);
	let agentName = $state('Nebo');
	let agentDescription = $state('');
	let messagesContainer: HTMLDivElement;
	let currentStreamingMessage = $state<Message | null>(null);
	let chatLoaded = $state(false);
	let initialScrollDone = $state(false);
	let copiedMessageId = $state<string | null>(null);
	let showScrollButton = $state(false);
	let autoScrollEnabled = $state(true);
	let scrollingProgrammatically = false;
	let draftInitialized = $state(false);

	// ── Subagent tracking ────────────────────────────────────────────
	let activeSubagents = $state<Map<string, SubagentState>>(new Map());

	// ── Slash commands ────────────────────────────────────────────────
	let verboseMode = $state(false);
	let thinkingLevel = $state('off');
	let focusModeEnabled = $state(false);

	function addSystemMessage(content: string) {
		messages = [...messages, {
			id: `cmd-${Date.now()}`,
			role: 'assistant' as const,
			content,
			timestamp: new Date()
		}];
	}

	function toggleFocusMode() {
		focusModeEnabled = !focusModeEnabled;
		window.dispatchEvent(new CustomEvent('nebo:focus-mode', { detail: focusModeEnabled }));
	}

	async function handleSlashSelect(cmd: SlashCommand) {
		// Command was auto-executed from menu (no-arg commands)
		const client = getWebSocketClient();
		const ctx: CommandContext = {
			messages, chatId, isLoading,
			onNewChat: newChat,
			onNewSession: resetChat,
			onCancel: cancelMessage,
			onToggleDuplex: undefined,
			addSystemMessage,
			clearMessages: () => { messages = []; },
			setVerboseMode: (on) => { verboseMode = on; },
			setThinkingLevel: (level) => { thinkingLevel = level; },
			toggleFocusMode,
			wsSend: (type, data) => client.send(type, data)
		};
		inputValue = '';
		await executeSlashCommand(cmd.name, '', ctx);
	}

	// ── Virtual scroll (top-truncation) ───────────────────────────────
	const VS_WINDOW = 10;
	const VS_LOAD_MORE = 10;
	let renderStart = $state(0);
	let loadingMore = false;

	// ── Companion-only state ───────────────────────────────────────────
	let voiceOutputEnabled = $state(false);
	let isSpeaking = $state(false);
	let currentAudio: HTMLAudioElement | null = null;
	let ttsVoice = $state('rachel');
	let ttsSentenceBuffer = '';
	let ttsQueue: string[] = [];
	let ttsPlaying = false;
	let ttsStreamingActive = false;
	let ttsCancelToken = 0;
	const ttsVoices = ['rachel', 'domi', 'bella', 'antoni', 'elli', 'josh', 'arnold', 'adam', 'sam'];

	let duplexSession: VoiceSession | null = null;
	let duplexState = $state<DuplexVoiceState>('idle');
	let duplexTranscript = $state('');
	let duplexVadActive = $state(false);
	let showModelDownload = $state(false);
	let modelDownloadProgress = $state<Record<string, { downloaded: number; total: number; done: boolean }>>({});
	let modelDownloadError = $state('');
	let wakeWordEnabled = $state(false);
	let wakeWordSession: VoiceSession | null = null;
	let duplexLevel = $state(0);
	let sidebarTool = $state<ToolCall | null>(null);
	let codeStatusMessageId = $state<string | null>(null);
	let showInstallModal = $state(false);
	let installModal: CodeInstallModal;

	// ── Channel-only state ─────────────────────────────────────────────
	let channelMemberNames = $state<Record<string, string>>({});
	let channelSending = $state(false);

	// ── Shared derived / grouping ──────────────────────────────────────
	interface MessageGroupType {
		role: 'user' | 'assistant';
		messages: Message[];
		agentName: string;
	}

	function resolveChannelName(msg: Message): string {
		if (msg.senderName) return msg.senderName;
		return $t('common.unknown');
	}

	const groupedMessages = $derived.by((): MessageGroupType[] => {
		const groups: MessageGroupType[] = [];
		let currentGroup: MessageGroupType | null = null;

		for (const msg of messages) {
			if (msg.role === 'system' || (msg.role as string) === 'tool') {
				continue;
			}
			// Skip hidden steering messages
			if (msg.metadata && typeof msg.metadata === 'object' && (msg.metadata as any).hidden) {
				continue;
			}
			if (typeof msg.metadata === 'string') {
				try { if (JSON.parse(msg.metadata).hidden) continue; } catch {}
			}

			const role = msg.role as 'user' | 'assistant';
			const name = isChannel ? resolveChannelName(msg) : (role === 'assistant' ? agentName : $t('common.you'));

			// Break group on role change or name change (channels only)
			if (!currentGroup || currentGroup.role !== role || (isChannel && currentGroup.agentName !== name)) {
				currentGroup = { role, messages: [], agentName: name };
				groups.push(currentGroup);
			}
			currentGroup.messages.push(msg);
		}

		return groups;
	});

	// Windowed slice for rendering — always includes the tail so streaming works
	const renderedGroups = $derived.by(() => {
		return groupedMessages.slice(renderStart).map((g, i) => ({ group: g, idx: renderStart + i }));
	});

	// When total groups change, keep the window pinned to the tail
	let prevGroupCount = 0;
	$effect(() => {
		const total = groupedMessages.length;
		if (total !== prevGroupCount) {
			prevGroupCount = total;
			if (total <= VS_WINDOW) {
				renderStart = 0;
			} else if (!loadingMore) {
				renderStart = total - VS_WINDOW;
			}
		}
	});

	// Message queue (companion-only)
	interface QueuedMessage {
		id: string;
		content: string;
	}
	let messageQueue = $state<QueuedMessage[]>([]);
	let chatInputRef: { focus: () => void; handleDrop: (e: DragEvent) => void; insertFilePaths: (paths: string[]) => void } | undefined;
	let isDraggingOver = $state(false);
	let dragCounter = 0;
	let cancelTimeoutId: ReturnType<typeof setTimeout> | null = null;
	let pendingScrollRAF: number | null = null;
	let staleCheckIntervalId: ReturnType<typeof setInterval> | null = null;

	// Stream staleness detection (companion-only)
	let lastEventTime = $state(Date.now());
	let staleWarning = $state(false);
	let pendingAskRequest = $state(false);

	function markActivity() {
		lastEventTime = Date.now();
		staleWarning = false;
	}

	function replaceMessageById(updatedMsg: Message): void {
		const idx = messages.findIndex((m) => m.id === updatedMsg.id);
		if (idx >= 0) {
			messages = [...messages.slice(0, idx), updatedMsg, ...messages.slice(idx + 1)];
		}
	}

	function hasRunningTools(): boolean {
		return !!currentStreamingMessage?.toolCalls?.some((tc) => tc.status === 'running');
	}

	function resetLoadingTimeout() {
		markActivity();
	}

	// Stream staleness check (companion-only)
	$effect(() => {
		if (!isCompanion) return;
		if (!isLoading) {
			staleWarning = false;
			pendingAskRequest = false;
			if (staleCheckIntervalId) {
				clearInterval(staleCheckIntervalId);
				staleCheckIntervalId = null;
			}
			return;
		}
		staleCheckIntervalId = setInterval(() => {
			if (Date.now() - lastEventTime > 600_000 && !pendingAskRequest && !hasRunningTools()) {
				staleWarning = true;
			}
		}, 5000);
		return () => {
			if (staleCheckIntervalId) {
				clearInterval(staleCheckIntervalId);
				staleCheckIntervalId = null;
			}
		};
	});

	// Approval request queue (companion-only)
	let approvalQueue = $state<ApprovalRequest[]>([]);
	const pendingApproval = $derived(approvalQueue.length > 0 ? approvalQueue[0] : null);

	let unsubscribers: (() => void)[] = [];

	let warningToast = $state(false);
	let warningMessage = $state('');

	// Browser extension modal state
	let showBrowserExtModal = $state(false);
	let browserExtReason = $state<'not_connected' | 'reconnecting'>('not_connected');
	let lastBrowserExtModalTime = 0;
	const BROWSER_EXT_MODAL_COOLDOWN = 30_000;

	const suggestionKeys = [
		'chat.suggestion1',
		'chat.suggestion2',
		'chat.suggestion3',
		'chat.suggestion4'
	];

	// Marketplace agents for empty state (loaded once, no tokens — pure UI)
	interface MarketplaceAgent { id: string; name: string; description: string; icon: string; }
	let marketplaceAgents = $state<MarketplaceAgent[]>([]);

	async function loadMarketplaceAgents() {
		try {
			const res = await fetch('/api/v1/store/products?type=role&pageSize=6');
			if (res.ok) {
				const data = await res.json();
				const items = data.skills || data.items || data.products || [];
				marketplaceAgents = items.slice(0, 6).map((r: any) => ({
					id: r.id || r.slug || '',
					name: r.name || '',
					description: r.description || '',
					icon: r.icon || '',
				}));
			}
		} catch { /* ignore — marketplace might not be available */ }
	}

	// ── Lifecycle ──────────────────────────────────────────────────────
	onMount(async () => {
		const client = getWebSocketClient();

		// Sync focus mode from external sources (e.g. sidebar expand button)
		function handleFocusSync(e: Event) {
			focusModeEnabled = (e as CustomEvent).detail;
		}
		window.addEventListener('nebo:focus-mode', handleFocusSync);
		unsubscribers.push(() => window.removeEventListener('nebo:focus-mode', handleFocusSync));

		// Load marketplace roles for empty state (no tokens — pure UI)
		if (isCompanion) loadMarketplaceAgents();

		// Tauri native drag-and-drop via Rust eval() → global functions.
		// Registered FIRST (synchronously) so they're available immediately.
		(window as any).__NEBO_DRAG_ENTER__ = () => { isDraggingOver = true; };
		(window as any).__NEBO_DRAG_LEAVE__ = () => { isDraggingOver = false; };
		(window as any).__NEBO_INSERT_FILES__ = (paths: string[]) => {
			isDraggingOver = false;
			if (paths?.length) {
				const joined = paths.join(' ');
				inputValue = inputValue.trim() ? `${inputValue.trimEnd()} ${joined} ` : `${joined} `;
			}
		};
		unsubscribers.push(() => {
			delete (window as any).__NEBO_DRAG_ENTER__;
			delete (window as any).__NEBO_DRAG_LEAVE__;
			delete (window as any).__NEBO_INSERT_FILES__;
		});

		unsubscribers.push(
			client.onStatus((status: ConnectionStatus) => {
				wsConnected = status === 'connected';
			})
		);

		// Fetch agent profile for display name (both modes need it)
		try {
			const profile = await getAgentProfile();
			if (profile.name) agentName = profile.name;
		} catch {}

		if (isCompanion || isAgent) {
			unsubscribers.push(
				client.on('chat_stream', handleChatStream),
				client.on('chat_complete', handleChatComplete),
				client.on('chat_response', handleChatResponse),
				client.on('tool_start', handleToolStart),
				client.on('tool_result', handleToolResult),
				client.on('image', handleImage),
				client.on('thinking', handleThinking),
				client.on('error', handleError),
				client.on('approval_request', handleApprovalRequest),
				client.on('stream_status', handleStreamStatus),
				client.on('chat_cancelled', handleChatCancelled),
				client.on('reminder_complete', handleReminderComplete),
				client.on('dm_user_message', handleDMUserMessage),
				client.on('ask_request', handleAskRequest),
				client.on('subagent_start', handleSubagentStart),
				client.on('subagent_progress', handleSubagentProgress),
				client.on('subagent_complete', handleSubagentComplete),
				client.on('agent_warning', (data: Record<string, unknown>) => {
					warningMessage = (data?.message as string) || $t('chat.retryWarning');
					warningToast = true;
				}),
				client.on('quota_warning', (data: Record<string, unknown>) => {
					if (data?.session_id === chatId) {
						warningMessage = (data?.message as string) || $t('chat.quotaWarning');
						warningToast = true;
					}
				}),
				client.on('code_processing', (data: Record<string, unknown>) => {
					isLoading = false; // Code intercepted server-side — no chat stream coming
					installModal?.onCodeProcessing(data);
				}),
				client.on('code_result', handleCodeResult),
				client.on('plugin_installing', (data: Record<string, unknown>) => installModal?.onPluginInstalling(data)),
				client.on('plugin_installed', (data: Record<string, unknown>) => installModal?.onPluginInstalled(data)),
				client.on('plugin_error', (data: Record<string, unknown>) => installModal?.onPluginError(data)),
				client.on('plugin_auth_required', (data: Record<string, unknown>) => installModal?.onPluginAuthRequired(data)),
				client.on('plugin_auth_complete', (data: Record<string, unknown>) => installModal?.onPluginAuthComplete(data)),
				client.on('plugin_auth_error', (data: Record<string, unknown>) => installModal?.onPluginAuthError(data)),
				client.on('dep_pending', (data: Record<string, unknown>) => installModal?.onDepPending(data)),
				client.on('dep_installed', (data: Record<string, unknown>) => installModal?.onDepInstalled(data)),
				client.on('dep_failed', (data: Record<string, unknown>) => installModal?.onDepFailed(data)),
				client.on('dep_cascade_complete', (data: Record<string, unknown>) => installModal?.onDepCascadeComplete(data)),
				client.on('chat_ack', (data: Record<string, unknown>) => {
					if (data?.session_id === chatId) {
						log.debug('chat_ack received for session ' + chatId);
					}
				}),
				client.on('session_reset', (data: Record<string, unknown>) => {
					if (data?.session_id === chatId && data?.success) {
						loadCompanionChat();
					}
				}),
				client.on('session_compact', (data: Record<string, unknown>) => {
					if (data?.session_id === chatId) {
						if (data?.success) {
							loadCompanionChat();
						} else {
							addSystemMessage('Compaction failed: ' + (data?.error || 'unknown error'));
						}
					}
				}),
				client.on('browser_extension_disconnected', (data: Record<string, unknown>) => {
					if (chatId && data?.session_id !== chatId) return;
					const now = Date.now();
					if (now - lastBrowserExtModalTime < BROWSER_EXT_MODAL_COOLDOWN) return;
					lastBrowserExtModalTime = now;
					browserExtReason = data?.reason === 'reconnecting' ? 'reconnecting' : 'not_connected';
					showBrowserExtModal = true;
				})
			);

			if (isCompanion && browser) {
				const savedDraft = localStorage.getItem(DRAFT_STORAGE_KEY);
				if (savedDraft) {
					inputValue = savedDraft;
				}
				draftInitialized = true;
			}

			if (isAgent) {
				// Agent chat: set chatId to agent-scoped session key, load existing messages
				chatId = `agent:${mode.agentId}:web`;
				agentName = mode.agentName || $t('common.agent');
				// Fetch agent details for empty state display
				if (mode.agentId) {
					getAgent(mode.agentId).then((data) => {
						if (data?.agent?.description) agentDescription = data.agent.description;
					}).catch(() => {});
				}
				await loadAgentChat();
			} else {
				await loadCompanionChat();
			}
		} else {
			// Channel mode
			unsubscribers.push(
				client.on('loop_channel_message', handleLoopChannelMessage)
			);
			await loadChannelMessages();
		}
	});

	onDestroy(() => {
		unsubscribers.forEach((unsub) => unsub());
		if (pendingScrollRAF) {
			cancelAnimationFrame(pendingScrollRAF);
			pendingScrollRAF = null;
		}
		if (staleCheckIntervalId) {
			clearInterval(staleCheckIntervalId);
			staleCheckIntervalId = null;
		}
		if (duplexSession) {
			duplexSession.disconnect();
			duplexSession = null;
		}
		if (wakeWordSession) {
			wakeWordSession.disconnect();
			wakeWordSession = null;
		}
	});

	// Save draft to localStorage (companion-only)
	$effect(() => {
		if (!isCompanion) return;
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

	// ── Channel message handlers ───────────────────────────────────────
	async function loadChannelMessages() {
		try {
			chatLoaded = false;
			const res = await getChannelMessages({ limit: 200 }, mode.channelId!);
			const data = res as GetChannelMessagesResponse;
			if (data?.messages) {
				messages = (data.messages as ChannelMessage[]).map((m) => {
					const isOwner = m.from === 'You' || m.role === 'user';
					return {
						id: m.id,
						role: (isOwner ? 'user' : 'assistant') as 'user' | 'assistant',
						content: m.content,
						contentHtml: m.contentHtml || undefined,
						timestamp: new Date(m.createdAt * 1000),
						senderName: isOwner ? $t('common.you') : resolveNameFromId(m.from)
					};
				});
			}
			if (data?.members) {
				const names: Record<string, string> = {};
				for (const m of data.members) {
					names[m.botId] = m.botName || m.botId.substring(0, 8);
				}
				channelMemberNames = names;
			}
			chatLoaded = true;
			if (messages.length > 0) {
				scrollToBottomOnLoad();
			} else {
				initialScrollDone = true;
			}
		} catch {
			chatLoaded = true;
			initialScrollDone = true;
		}
	}

	function resolveNameFromId(senderId: string): string {
		if (senderId === 'You') return $t('common.you');
		if (senderId === 'bot') return agentName;
		return channelMemberNames[senderId] || senderId.substring(0, 8) + '\u2026';
	}

	function handleLoopChannelMessage(data: Record<string, unknown>) {
		if (data?.channel_id !== mode.channelId) return;

		const senderId = (data?.sender_id as string) || '';
		const role = (data?.role as string) || '';
		const isOwner = role === 'user' || senderId === 'You'; // role-based with backward compat
		const source = (data?.source as string) || '';

		// Dedup echo-back against optimistic message
		if (source === 'owner_send') {
			const tempIdx = messages.findIndex(
				(m) => m.id.startsWith('temp-') && m.content === data?.content
			);
			if (tempIdx >= 0) {
				messages = [...messages.slice(0, tempIdx), ...messages.slice(tempIdx + 1)];
			}
		}

		const msg: Message = {
			id: (data?.message_id as string) || generateUUID(),
			role: isOwner ? 'user' : 'assistant',
			content: (data?.content as string) || '',
			timestamp: new Date((data?.timestamp as string) || Date.now()),
			senderName: (data?.sender_name as string) || resolveNameFromId(senderId)
		};
		messages = [...messages, msg];
	}

	function sendChannelMsg() {
		const text = inputValue.trim();
		if (!text || channelSending) return;

		const tempId = `temp-${Date.now()}`;
		messages = [
			...messages,
			{
				id: tempId,
				role: 'user',
				content: text,
				timestamp: new Date(),
				senderName: $t('common.you')
			}
		];
		inputValue = '';
		channelSending = true;

		autoScrollEnabled = true;
		showScrollButton = false;

		sendChannelMessage({ text }, mode.channelId!)
			.catch(() => {
				messages = messages.filter((m) => m.id !== tempId);
			})
			.finally(() => {
				channelSending = false;
			});
	}

	// ── Companion chat handlers ────────────────────────────────────────
	interface ParsedMetadata {
		toolCalls?: ToolCall[];
		thinking?: string;
		contentBlocks?: ContentBlock[];
		proactive?: boolean;
	}

	function parseMetadata(metadata: string | undefined): ParsedMetadata {
		if (!metadata) return {};
		try {
			const parsed = JSON.parse(metadata);
			const result: ParsedMetadata = {};

			if (parsed.toolCalls && Array.isArray(parsed.toolCalls)) {
				result.toolCalls = parsed.toolCalls.map(
					(tc: { id?: string; name: string; input: string; output?: string; status?: string }) => ({
						id: tc.id,
						name: tc.name,
						input: tc.input,
						output: tc.output,
						status: (tc.status === 'error' ? 'error' : 'complete') as 'complete' | 'error'
					})
				);
			}

			if (parsed.thinking && typeof parsed.thinking === 'string') {
				result.thinking = parsed.thinking;
			}

			if (parsed.contentBlocks && Array.isArray(parsed.contentBlocks)) {
				result.contentBlocks = parsed.contentBlocks;
			}

			if (parsed.proactive === true) {
				result.proactive = true;
			}

			return result;
		} catch {
			// Invalid JSON
		}
		return {};
	}

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
					contentHtml: m.contentHtml || undefined,
					timestamp: new Date(m.createdAt * 1000),
					toolCalls: meta.toolCalls,
					thinking: meta.thinking,
					contentBlocks,
					proactive: meta.proactive
				};
			});
			totalMessages = res.totalMessages || messages.length;
			chatLoaded = true;
			log.debug('Messages loaded: ' + messages.length + ' total: ' + totalMessages);

			if (messages.length > 0) {
				scrollToBottomOnLoad();
			} else {
				initialScrollDone = true;
			}

			checkForActiveStream();
		} catch (err) {
			log.error('Failed to load companion chat', err);
			chatLoaded = true;
			initialScrollDone = true;
		}
	}

	async function loadAgentChat() {
		try {
			const res = await getChatMessages(chatId);
			messages = (res.messages || []).map((m: ApiChatMessage) => {
				const meta = parseMetadata((m as { metadata?: string }).metadata);
				let content = m.content;
				let contentBlocks = meta.contentBlocks;
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
					contentHtml: m.contentHtml || undefined,
					timestamp: new Date(m.createdAt * 1000),
					toolCalls: meta.toolCalls,
					thinking: meta.thinking,
					contentBlocks,
					proactive: meta.proactive
				};
			});
			totalMessages = res.totalMessages || messages.length;
			chatLoaded = true;
			if (messages.length > 0) {
				scrollToBottomOnLoad();
			} else {
				initialScrollDone = true;
			}
			checkForActiveStream();
		} catch {
			// Chat may not exist yet (first interaction) — that's OK
			chatLoaded = true;
			initialScrollDone = true;
		}
	}

	let streamCheckResponded = false;

	function checkForActiveStream() {
		streamCheckResponded = false;
		const client = getWebSocketClient();

		setTimeout(() => {
			if (!streamCheckResponded && messages.length === 0 && chatId && !isLoading) {
				log.warn('Stream check timed out — requesting introduction');
				requestIntroduction();
			}
		}, 5000);

		if (!client.isConnected() || !chatId) {
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

	const hasMoreHistory = $derived(totalMessages > messages.length);

	function sendToAgent(prompt: string) {
		isLoading = true;
		const client = getWebSocketClient();

		if (client.isConnected()) {
			const payload: Record<string, unknown> = {
				session_id: chatId || '',
				prompt: prompt,
				companion: !isAgent
			};
			if (isAgent && mode.agentId) {
				payload.agent_id = mode.agentId;
			}
			client.send('chat', payload);
		} else {
			log.warn('WebSocket not connected, cannot send message');
			isLoading = false;
			if (messageQueue.length > 0) {
				setTimeout(() => processQueue(), 100);
			}
		}
	}

	function handleChatStream(data: Record<string, unknown>) {
		log.debug('handleChatStream called: ' + data?.session_id + ' chatId: ' + chatId);
		if (chatId && data?.session_id !== chatId) {
			log.debug('handleChatStream: session_id mismatch, ignoring');
			return;
		}

		if (!chatId && data?.session_id) {
			chatId = data.session_id as string;
		}

		const isDMStream = data?.source === 'dm';
		if (!isLoading && !isDMStream) {
			log.debug('handleChatStream: re-arming isLoading (stream resumed after timeout)');
			isLoading = true;
		}
		resetLoadingTimeout();

		const chunk = (data?.content as string) || '';

		if (voiceOutputEnabled && chunk) {
			if (!ttsStreamingActive) {
				stopTTSQueue();
				ttsStreamingActive = true;
			}
			feedTTSStream(chunk);
		}

		if (currentStreamingMessage) {
			if (currentStreamingMessage.toolCalls?.length) {
				const hasRunning = currentStreamingMessage.toolCalls.some((tc) => tc.status === 'running');
				if (hasRunning) {
					log.debug('handleChatStream: text arrived with running tools — marking all complete');
					currentStreamingMessage.toolCalls = currentStreamingMessage.toolCalls.map((tc) =>
						tc.status === 'running' ? { ...tc, status: 'complete' as const } : tc
					);
				}
			}
			// Ensure separator when text follows a tool result
			if (!currentStreamingMessage.contentBlocks) {
				currentStreamingMessage.contentBlocks = [];
			}
			const blocks = currentStreamingMessage.contentBlocks;
			const lastBlockType = blocks.length > 0 ? blocks[blocks.length - 1].type : 'text';
			if (lastBlockType !== 'text' && currentStreamingMessage.content.length > 0 && !currentStreamingMessage.content.endsWith('\n')) {
				currentStreamingMessage.content += '\n';
			}
			currentStreamingMessage.content += chunk;
			if (blocks.length === 0 || blocks[blocks.length - 1].type !== 'text') {
				blocks.push({ type: 'text', text: chunk });
			} else {
				blocks[blocks.length - 1] = {
					...blocks[blocks.length - 1],
					text: (blocks[blocks.length - 1].text || '') + chunk
				};
			}
			currentStreamingMessage.contentBlocks = [...blocks];
			replaceMessageById({ ...currentStreamingMessage });
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
			log.debug('Processing queued message: ' + next.content.substring(0, 50));

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

	function recallFromQueue(): string | null {
		if (messageQueue.length === 0) return null;
		const last = messageQueue[messageQueue.length - 1];
		messageQueue = messageQueue.slice(0, -1);
		return last.content;
	}

	function cancelQueuedMessage(queuedId: string) {
		const item = messageQueue.find((q) => q.id === queuedId);
		if (!item) return;
		messageQueue = messageQueue.filter((q) => q.id !== queuedId);
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
			log.debug(
				'handleChatComplete: session mismatch, expected ' + chatId + ' got ' + data?.session_id
			);
			return;
		}

		const isDMComplete = data?.source === 'dm';

		if (isDMComplete) {
			if (currentStreamingMessage) {
				currentStreamingMessage.streaming = false;
				if (currentStreamingMessage.toolCalls?.length) {
					currentStreamingMessage.toolCalls = currentStreamingMessage.toolCalls.map((tc) =>
						tc.status === 'running' ? { ...tc, status: 'complete' as const } : tc
					);
				}
				replaceMessageById({ ...currentStreamingMessage });
				currentStreamingMessage = null;
			}
			return;
		}

		if (cancelTimeoutId) {
			clearTimeout(cancelTimeoutId);
			cancelTimeoutId = null;
		}

		let completedContent = '';
		if (currentStreamingMessage) {
			completedContent = currentStreamingMessage.content;
			currentStreamingMessage.streaming = false;
			if (currentStreamingMessage.toolCalls?.length) {
				const beforeStatuses = currentStreamingMessage.toolCalls.map((t) => t.status);
				currentStreamingMessage.toolCalls = currentStreamingMessage.toolCalls.map((tc) =>
					tc.status === 'running' ? { ...tc, status: 'complete' as const } : tc
				);
				const afterStatuses = currentStreamingMessage.toolCalls.map((t) => t.status);
				log.debug(
					'Safety net: tool statuses before: ' +
						beforeStatuses.join(',') +
						' after: ' +
						afterStatuses.join(',')
				);
			}
			const finalMsg = { ...currentStreamingMessage };
			replaceMessageById(finalMsg);
			currentStreamingMessage = null;
		} else {
			log.debug('handleChatComplete: NO currentStreamingMessage!');
			const lastIdx = messages.length - 1;
			if (
				lastIdx >= 0 &&
				messages[lastIdx].role === 'assistant' &&
				messages[lastIdx].toolCalls?.length
			) {
				const lastMsg = messages[lastIdx];
				const hasRunningInLast = lastMsg.toolCalls!.some((tc) => tc.status === 'running');
				if (hasRunningInLast) {
					log.debug(
						'Safety net (non-streaming): marking running tools as complete in last message'
					);
					const updatedTools = lastMsg.toolCalls!.map((tc) =>
						tc.status === 'running' ? { ...tc, status: 'complete' as const } : tc
					);
					messages = [...messages.slice(0, lastIdx), { ...lastMsg, toolCalls: updatedTools }];
				}
			}
		}
		isLoading = false;

		if (voiceOutputEnabled && ttsStreamingActive) {
			flushTTSBuffer();
		} else if (voiceOutputEnabled && completedContent) {
			speakText(completedContent);
		}

		processQueue();
	}

	function cancelMessage() {
		const client = getWebSocketClient();
		client.send('cancel', {
			session_id: chatId || ''
		});

		if (cancelTimeoutId) clearTimeout(cancelTimeoutId);
		cancelTimeoutId = setTimeout(() => {
			cancelTimeoutId = null;
			if (isLoading) {
				log.warn('Cancel timeout - force resetting loading state');
				if (currentStreamingMessage) {
					currentStreamingMessage.streaming = false;
					if (currentStreamingMessage.content) {
						currentStreamingMessage.content += `\n\n${$t('chat.generationCancelled')}`;
					} else {
						currentStreamingMessage.content = $t('chat.generationCancelled');
					}
					if (currentStreamingMessage.toolCalls?.length) {
						currentStreamingMessage.toolCalls = currentStreamingMessage.toolCalls.map((tc) =>
							tc.status === 'running' ? { ...tc, status: 'complete' as const } : tc
						);
					}
					const finalMsg = { ...currentStreamingMessage };
					replaceMessageById(finalMsg);
					currentStreamingMessage = null;
				}
				isLoading = false;
				processQueue();
			}
		}, 2000);
	}

	function handleChatCancelled(data: Record<string, unknown>) {
		if (chatId && data?.session_id !== chatId) return;

		if (cancelTimeoutId) {
			clearTimeout(cancelTimeoutId);
			cancelTimeoutId = null;
		}

		if (currentStreamingMessage) {
			currentStreamingMessage.streaming = false;
			if (currentStreamingMessage.content) {
				currentStreamingMessage.content += `\n\n${$t('chat.generationCancelled')}`;
			} else {
				currentStreamingMessage.content = $t('chat.generationCancelled');
			}
			if (currentStreamingMessage.toolCalls?.length) {
				currentStreamingMessage.toolCalls = currentStreamingMessage.toolCalls.map((tc) =>
					tc.status === 'running' ? { ...tc, status: 'complete' as const } : tc
				);
			}
			const finalMsg = { ...currentStreamingMessage };
			replaceMessageById(finalMsg);
			currentStreamingMessage = null;
		}
		isLoading = false;
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

		resetLoadingTimeout();

		const toolName = data?.tool as string;
		const toolID = (data?.tool_id as string) || '';
		const rawInput = data?.input;
		const toolInput =
			typeof rawInput === 'string'
				? rawInput
				: rawInput != null
					? JSON.stringify(rawInput)
					: '';
		const newToolCall: ToolCall = {
			id: toolID,
			name: toolName,
			input: toolInput,
			status: 'running'
		};

		if (currentStreamingMessage) {
			if (!currentStreamingMessage.toolCalls) {
				currentStreamingMessage.toolCalls = [];
			}
			const toolIndex = currentStreamingMessage.toolCalls.length;
			currentStreamingMessage.toolCalls = [...currentStreamingMessage.toolCalls, newToolCall];
			if (!currentStreamingMessage.contentBlocks) {
				currentStreamingMessage.contentBlocks = [];
			}
			currentStreamingMessage.contentBlocks = [
				...currentStreamingMessage.contentBlocks,
				{ type: 'tool' as const, toolCallIndex: toolIndex }
			];
			replaceMessageById({ ...currentStreamingMessage });
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
		log.debug('handleToolResult: ' + ((data?.tool_name as string) || (data?.tool_id as string)));

		if (chatId && data?.session_id !== chatId) {
			log.debug('handleToolResult: session mismatch');
			return;
		}

		resetLoadingTimeout();

		const result = (data?.result as string) || '';
		const toolID = (data?.tool_id as string) || '';
		const toolName = (data?.tool_name as string) || '';
		const imageURL = (data?.image_url as string) || '';
		log.debug(
			'Tool result received: ' + toolName + ' id: ' + toolID + ' result_length: ' + result?.length
		);

		const findAndUpdateTool = (toolCalls: ToolCall[]): ToolCall[] | null => {
			const updated = [...toolCalls];
			if (toolID) {
				const idx = updated.findIndex((tc) => tc.id === toolID);
				if (idx >= 0) {
					log.debug('Found tool by ID at index ' + idx);
					updated[idx] = { ...updated[idx], output: result, status: 'complete' };
					return updated;
				}
			}
			const runningIdx = updated.findIndex((tc) => tc.status === 'running');
			if (runningIdx >= 0) {
				log.debug('Fallback: updating first running tool at index ' + runningIdx);
				updated[runningIdx] = { ...updated[runningIdx], output: result, status: 'complete' };
				return updated;
			}
			return null;
		};

		const appendImageBlock = (msg: Message) => {
			if (!imageURL) return;
			if (!msg.contentBlocks) msg.contentBlocks = [];
			msg.contentBlocks = [...msg.contentBlocks, { type: 'image' as const, imageURL }];
		};

		if (currentStreamingMessage?.toolCalls?.length) {
			const updatedToolCalls = findAndUpdateTool(currentStreamingMessage.toolCalls);
			if (updatedToolCalls) {
				currentStreamingMessage = { ...currentStreamingMessage, toolCalls: updatedToolCalls };
				appendImageBlock(currentStreamingMessage);
				replaceMessageById({ ...currentStreamingMessage });
				log.debug('Updated tool in streaming message');
				return;
			}
		}

		log.debug('handleToolResult: trying fallback to recent assistant messages');
		for (let i = messages.length - 1; i >= Math.max(0, messages.length - 5); i--) {
			const msg = messages[i];
			if (msg.role === 'assistant' && msg.toolCalls?.length) {
				const updatedToolCalls = findAndUpdateTool(msg.toolCalls);
				if (updatedToolCalls) {
					const updatedMsg = { ...msg, toolCalls: updatedToolCalls };
					appendImageBlock(updatedMsg);
					messages = [
						...messages.slice(0, i),
						updatedMsg,
						...messages.slice(i + 1)
					];
					log.debug('Fallback: updated tool in message at index ' + i);
					return;
				}
			}
		}

		log.warn('handleToolResult: SKIP - no suitable tool to update (tool_id: ' + toolID + ')');
	}

	function handleImage(data: Record<string, unknown>) {
		if (chatId && data?.session_id !== chatId) return;
		resetLoadingTimeout();

		const imageURL = (data?.image_url as string) || '';
		if (!imageURL) return;

		if (currentStreamingMessage) {
			if (!currentStreamingMessage.contentBlocks) {
				currentStreamingMessage.contentBlocks = [];
			}
			currentStreamingMessage.contentBlocks = [
				...currentStreamingMessage.contentBlocks,
				{ type: 'image' as const, imageURL }
			];
			replaceMessageById({ ...currentStreamingMessage });
		}
	}

	function handleThinking(data: Record<string, unknown>) {
		log.debug('handleThinking');
		if (chatId && data?.session_id !== chatId) return;
		resetLoadingTimeout();

		const thinkingContent = (data?.content as string) || '';

		if (currentStreamingMessage) {
			currentStreamingMessage.thinking = (currentStreamingMessage.thinking || '') + thinkingContent;
			replaceMessageById({ ...currentStreamingMessage });
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

	function handleSubagentStart(data: Record<string, unknown>) {
		if (chatId && data?.session_id !== chatId) return;
		resetLoadingTimeout();

		const taskId = (data?.task_id as string) || '';
		const description = (data?.description as string) || '';
		const agentType = (data?.agent_type as string) || 'general';

		const agent: SubagentState = {
			taskId,
			description,
			agentType,
			status: 'running',
			toolCount: 0,
			tokenCount: 0,
			currentOperation: ''
		};

		activeSubagents = new Map(activeSubagents).set(taskId, agent);

		if (currentStreamingMessage) {
			// Find or create subagent_tree block
			if (!currentStreamingMessage.contentBlocks) {
				currentStreamingMessage.contentBlocks = [];
			}
			let treeBlock = currentStreamingMessage.contentBlocks.find(
				(b) => b.type === 'subagent_tree'
			);
			if (!treeBlock) {
				treeBlock = { type: 'subagent_tree' as const, subagents: [] };
				currentStreamingMessage.contentBlocks = [
					...currentStreamingMessage.contentBlocks,
					treeBlock
				];
			}
			treeBlock.subagents = [...activeSubagents.values()];
			replaceMessageById({ ...currentStreamingMessage });
		}
	}

	function handleSubagentProgress(data: Record<string, unknown>) {
		if (chatId && data?.session_id !== chatId) return;
		resetLoadingTimeout();

		const taskId = (data?.task_id as string) || '';
		const existing = activeSubagents.get(taskId);
		if (!existing) return;

		const updated: SubagentState = {
			...existing,
			toolCount: (data?.tool_count as number) || existing.toolCount,
			tokenCount: (data?.token_count as number) || existing.tokenCount,
			currentOperation: (data?.current_operation as string) || existing.currentOperation
		};

		activeSubagents = new Map(activeSubagents).set(taskId, updated);

		if (currentStreamingMessage) {
			const treeBlock = currentStreamingMessage.contentBlocks?.find(
				(b) => b.type === 'subagent_tree'
			);
			if (treeBlock) {
				treeBlock.subagents = [...activeSubagents.values()];
				replaceMessageById({ ...currentStreamingMessage });
			}
		}
	}

	function handleSubagentComplete(data: Record<string, unknown>) {
		if (chatId && data?.session_id !== chatId) return;
		resetLoadingTimeout();

		const taskId = (data?.task_id as string) || '';
		const success = data?.success !== false;
		const existing = activeSubagents.get(taskId);
		if (!existing) return;

		const updated: SubagentState = {
			...existing,
			status: success ? 'complete' : 'error',
			toolCount: (data?.tool_count as number) || existing.toolCount,
			tokenCount: (data?.token_count as number) || existing.tokenCount,
			currentOperation: ''
		};

		activeSubagents = new Map(activeSubagents).set(taskId, updated);

		if (currentStreamingMessage) {
			const treeBlock = currentStreamingMessage.contentBlocks?.find(
				(b) => b.type === 'subagent_tree'
			);
			if (treeBlock) {
				treeBlock.subagents = [...activeSubagents.values()];
				replaceMessageById({ ...currentStreamingMessage });
			}
		}

		// Clear activeSubagents when all are done
		const allDone = [...activeSubagents.values()].every(
			(a) => a.status === 'complete' || a.status === 'error'
		);
		if (allDone) {
			activeSubagents = new Map();
		}
	}

	function handleError(data: Record<string, unknown>) {
		if (chatId && data?.session_id !== chatId) return;

		if (currentStreamingMessage?.toolCalls?.length) {
			currentStreamingMessage.toolCalls = currentStreamingMessage.toolCalls.map((tc) =>
				tc.status === 'running' ? { ...tc, status: 'error' as const } : tc
			);
			replaceMessageById({ ...currentStreamingMessage });
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

	function handleReminderComplete(data: Record<string, unknown>) {
		const name = (data?.name as string) || 'Reminder';
		const result = (data?.result as string) || '';
		if (!result) return;

		const reminderMessage: Message = {
			id: generateUUID(),
			role: 'assistant',
			content: `**Reminder — ${name}**\n\n${result}`,
			timestamp: new Date()
		};
		messages = [...messages, reminderMessage];
		scrollToBottom();
	}

	function handleCodeResult(data: Record<string, unknown>) {
		// Route to modal for live progress
		installModal?.onCodeResult(data);

		// Also add a brief chat message for history
		const success = data?.success as boolean;
		const artifactName = (data?.artifact_name as string) || '';
		const codeType = (data?.code_type as string) || '';
		const errorMsg = (data?.error as string) || '';

		let content: string;
		if (success) {
			const msg = (data?.message as string) || 'Done';
			content = `**${msg}**`;
		} else {
			const code = (data?.code as string) || '';
			content = `**Failed to process code \`${code}\`**\n\n${errorMsg}`;
		}

		// Replace the processing message if one exists, otherwise append
		if (codeStatusMessageId) {
			const idx = messages.findIndex((m) => m.id === codeStatusMessageId);
			if (idx >= 0) {
				const updated: Message = {
					...messages[idx],
					content
				};
				messages = [...messages.slice(0, idx), updated, ...messages.slice(idx + 1)];
			}
			codeStatusMessageId = null;
		} else {
			const resultMsg: Message = {
				id: generateUUID(),
				role: 'assistant',
				content,
				timestamp: new Date()
			};
			messages = [...messages, resultMsg];
		}
		scrollToBottom();
	}

	function handleDMUserMessage(data: Record<string, unknown>) {
		if (chatId && data?.session_id !== chatId) return;
		const content = (data?.content as string) || '';
		if (!content) return;

		const source = (data?.source as string) || 'dm';
		const userMsg: Message = {
			id: generateUUID(),
			role: 'user',
			content: content,
			timestamp: new Date()
		};
		messages = [...messages, userMsg];
		log.debug('DM user message from ' + source + ': ' + content.substring(0, 50));
		scrollToBottom();
	}

	function handleAskRequest(data: Record<string, unknown>) {
		markActivity();
		pendingAskRequest = true;

		const requestId = data?.request_id as string;
		const prompt = data?.prompt as string;
		const widgets = data?.widgets as ContentBlock['askWidgets'];

		if (requestId && currentStreamingMessage) {
			const updatedBlocks = [
				...(currentStreamingMessage.contentBlocks ?? []),
				{
					type: 'ask' as const,
					askRequestId: requestId,
					askPrompt: prompt,
					askWidgets: widgets ?? [{ type: 'confirm' as const, options: ['Yes', 'No'] }]
				}
			];
			currentStreamingMessage = {
				...currentStreamingMessage,
				contentBlocks: updatedBlocks
			};
			replaceMessageById(currentStreamingMessage);
		}
	}

	function handleAskSubmit(requestId: string, value: string) {
		pendingAskRequest = false;
		resetLoadingTimeout();
		const client = getWebSocketClient();
		client.send('ask_response', {
			request_id: requestId,
			value
		});

		if (currentStreamingMessage?.contentBlocks) {
			const updatedBlocks = currentStreamingMessage.contentBlocks.map((block) => {
				if (block.type === 'ask' && block.askRequestId === requestId) {
					return { ...block, askResponse: value };
				}
				return block;
			});
			currentStreamingMessage = {
				...currentStreamingMessage,
				contentBlocks: updatedBlocks
			};
			replaceMessageById(currentStreamingMessage);
		}

		messages = messages.map((msg) => {
			if (msg.contentBlocks?.some((b) => b.askRequestId === requestId)) {
				return {
					...msg,
					contentBlocks: msg.contentBlocks!.map((block) => {
						if (block.type === 'ask' && block.askRequestId === requestId) {
							return { ...block, askResponse: value };
						}
						return block;
					})
				};
			}
			return msg;
		});
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

		streamCheckResponded = true;

		if (sessionId !== chatId) {
			log.debug('Stream status for different session, ignoring');
			return;
		}

		if (!active) {
			log.debug('No active stream to resume');
			if (messages.length === 0 && chatId) {
				log.debug('Chat is empty, requesting introduction...');
				requestIntroduction();
			}
			return;
		}

		log.info('Resuming stream with ' + content.length + ' bytes of content');
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

	async function handleEditMessage(messageId: string, newContent: string) {
		try {
			await editChatMessage(messageId, newContent);
			// Truncate local messages after the edited one and update its content
			const idx = messages.findIndex(m => m.id === messageId);
			if (idx !== -1) {
				messages = messages.slice(0, idx + 1);
				messages[idx] = { ...messages[idx], content: newContent };
			}
			// Re-send to agent
			autoScrollEnabled = true;
			showScrollButton = false;
			sendToAgent(newContent);
		} catch (e) {
			log.error('Failed to edit message', e);
		}
	}

	function sendMessage() {
		if (isChannel) {
			sendChannelMsg();
			return;
		}

		if (!inputValue.trim()) return;

		const prompt = inputValue.trim();

		// ── Slash command interception ──
		const parsed = parseSlashCommand(prompt);
		if (parsed) {
			inputValue = '';
			clearDraft();
			const client = getWebSocketClient();
			const ctx: CommandContext = {
				messages, chatId, isLoading,
				onNewChat: newChat,
				onNewSession: resetChat,
				onCancel: cancelMessage,
				onToggleDuplex: undefined,
				addSystemMessage,
				clearMessages: () => { messages = []; },
				setVerboseMode: (on) => { verboseMode = on; },
				setThinkingLevel: (level) => { thinkingLevel = level; },
				toggleFocusMode,
				wsSend: (type, data) => client.send(type, data)
			};
			executeSlashCommand(parsed.command, parsed.args, ctx).then((handled) => {
				if (!handled) {
					// Not handled locally — send to agent as normal message
					const userMessage: Message = {
						id: generateUUID(),
						role: 'user',
						content: prompt,
						timestamp: new Date()
					};
					messages = [...messages, userMessage];
					autoScrollEnabled = true;
					showScrollButton = false;
					handleSendPrompt(prompt);
				}
			});
			return;
		}

		inputValue = '';
		clearDraft();

		if (isLoading) {
			log.debug('Queuing message: ' + prompt.substring(0, 50));
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

		// Detect marketplace codes client-side — show install modal immediately
		const codeMatch = prompt.match(/^(NEBO|SKIL|WORK|AGNT|LOOP|PLUG)-[0-9A-Z]{4}-[0-9A-Z]{4}$/i);
		if (codeMatch) {
			const codeType = { NEBO: 'nebo', SKIL: 'skill', WORK: 'workflow', AGNT: 'agent', LOOP: 'loop', PLUG: 'plugin' }[codeMatch[1].toUpperCase()] || 'code';
			const statusMessage = { nebo: 'Connecting to NeboLoop...', skill: 'Installing skill...', workflow: 'Installing workflow...', agent: 'Installing agent...', loop: 'Joining loop...', plugin: 'Installing plugin...' }[codeType] || 'Processing...';
			installModal?.onCodeProcessing({ code: prompt.toUpperCase(), code_type: codeType, status_message: statusMessage });
		}

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

	// Auto-scroll
	$effect(() => {
		const messageCount = messages.length;
		const streamingContent = currentStreamingMessage?.content;
		const isStreaming = !!streamingContent;

		// Skip during initial load — scrollToBottomOnLoad() handles that path
		// to avoid smooth-scroll animation racing with the instant scroll.
		if (!initialScrollDone) return;

		if (messagesContainer && (messageCount > 0 || streamingContent) && autoScrollEnabled) {
			if (pendingScrollRAF) {
				cancelAnimationFrame(pendingScrollRAF);
			}
			scrollingProgrammatically = true;
			pendingScrollRAF = requestAnimationFrame(() => {
				pendingScrollRAF = null;
				if (messagesContainer && autoScrollEnabled) {
					messagesContainer.scrollTo({
						top: messagesContainer.scrollHeight,
						behavior: isStreaming ? 'instant' : 'smooth'
					});
				}
				requestAnimationFrame(() => {
					scrollingProgrammatically = false;
				});
			});
		}
	});

	function handleScroll() {
		if (!messagesContainer) return;
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

		// Load earlier groups when scrolling near the top
		if (scrollTop < 200 && renderStart > 0 && !loadingMore) {
			loadingMore = true;
			const prevHeight = scrollHeight;
			renderStart = Math.max(0, renderStart - VS_LOAD_MORE);
			tick().then(() => {
				if (messagesContainer) {
					const newHeight = messagesContainer.scrollHeight;
					messagesContainer.scrollTop += newHeight - prevHeight;
				}
				loadingMore = false;
			});
		}

		// Fetch older messages from API when we've revealed all local messages
		if (scrollTop < 200 && renderStart === 0 && messages.length < totalMessages && !loadingMore && initialScrollDone) {
			loadingMore = true;
			const oldestId = messages[0]?.id;
			if (oldestId && (mode.type === 'agent' || mode.type === 'companion')) {
				getChatMessages(chatId, { before: oldestId }).then((res) => {
					const older = (res.messages || []).map((m: ApiChatMessage) => {
						const meta = parseMetadata((m as { metadata?: string }).metadata);
						let content = m.content;
						let contentBlocks = meta.contentBlocks;
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
							contentHtml: m.contentHtml || undefined,
							timestamp: new Date(m.createdAt * 1000),
							toolCalls: meta.toolCalls,
							thinking: meta.thinking,
							contentBlocks
						};
					});
					if (older.length > 0) {
						const prevH = messagesContainer?.scrollHeight ?? 0;
						messages = [...older, ...messages];
						tick().then(() => {
							if (messagesContainer) {
								messagesContainer.scrollTop += messagesContainer.scrollHeight - prevH;
							}
							loadingMore = false;
						});
					} else {
						loadingMore = false;
					}
				}).catch(() => { loadingMore = false; });
			} else {
				loadingMore = false;
			}
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
			requestAnimationFrame(() => {
				requestAnimationFrame(() => {
					scrollingProgrammatically = false;
				});
			});
		}
	}

	/** Instant scroll to bottom after initial load — waits for DOM to fully lay out. */
	function scrollToBottomOnLoad() {
		// Block handleScroll from disabling autoScrollEnabled during the
		// reactive cascade (renderStart effect causes DOM changes → scroll events).
		scrollingProgrammatically = true;
		tick().then(() => {
			requestAnimationFrame(() => {
				requestAnimationFrame(() => {
					if (messagesContainer) {
						messagesContainer.scrollTo({ top: messagesContainer.scrollHeight, behavior: 'instant' });
						showScrollButton = false;
						autoScrollEnabled = true;
					}
					// Third rAF: verify we actually reached the bottom (DOM may
					// still have been settling from the renderStart virtual-scroll
					// effect). Reset scrollingProgrammatically only after this check.
					requestAnimationFrame(() => {
						if (messagesContainer) {
							const { scrollTop, scrollHeight, clientHeight } = messagesContainer;
							if (scrollHeight - scrollTop - clientHeight > 10) {
								messagesContainer.scrollTo({ top: scrollHeight, behavior: 'instant' });
							}
						}
						initialScrollDone = true;
						scrollingProgrammatically = false;
					});
				});
			});
		});
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
		const client = getWebSocketClient();
		if (chatId && client.isConnected()) {
			client.send('session_reset', { session_id: chatId });
		}
		messages = [];
		currentStreamingMessage = null;
		inputValue = '';
		renderStart = 0;
		clearDraft();
	}

	async function newChat() {
		try {
			const res = await createNewCompanionChat();
			chatId = res.chat.id;
			messages = [];
			currentStreamingMessage = null;
			inputValue = '';
			renderStart = 0;
			clearDraft();
			log.debug('Created new companion chat: ' + chatId);
		} catch (err) {
			log.error('Failed to create new chat', err);
			addSystemMessage('Failed to create new session.');
		}
	}

	// ── TTS (companion-only) ───────────────────────────────────────────
	function cleanTextForTTS(text: string): string {
		return text
			.replace(/```[\s\S]*?```/g, '')
			.replace(/`[^`]+`/g, '')
			.replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')
			.replace(/[*_~]+/g, '')
			.replace(/^#+\s*/gm, '')
			.replace(/^[-*]\s*/gm, '')
			.replace(/\n{2,}/g, '. ')
			.replace(/\n/g, ' ')
			.replace(/\s{2,}/g, ' ')
			.trim();
	}

	function feedTTSStream(chunk: string) {
		if (!voiceOutputEnabled || !ttsStreamingActive) return;

		ttsSentenceBuffer += chunk;

		const sentencePattern = /^([\s\S]*?[.!?])(\s|$)/;
		let match;
		while ((match = sentencePattern.exec(ttsSentenceBuffer)) !== null) {
			const sentence = match[1].trim();
			ttsSentenceBuffer = ttsSentenceBuffer.slice(match[0].length);

			const clean = cleanTextForTTS(sentence);
			if (clean.length > 2) {
				ttsQueue.push(clean);
				playNextTTS();
			}
		}
	}

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

	async function playNextTTS() {
		if (ttsPlaying || ttsQueue.length === 0) return;
		ttsPlaying = true;
		isSpeaking = true;
		const myToken = ttsCancelToken;

		while (ttsQueue.length > 0) {
			if (ttsCancelToken !== myToken) break;

			const sentence = ttsQueue.shift()!;

			try {
				const audioBlob = await speakTTS({
					text: sentence,
					voice: ttsVoice,
					speed: 1.0
				});

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

			if (!voiceOutputEnabled || ttsCancelToken !== myToken) {
				ttsQueue.length = 0;
				break;
			}
		}

		if (ttsCancelToken === myToken) {
			ttsPlaying = false;
			isSpeaking = false;
		}
	}

	function stopTTSQueue() {
		ttsCancelToken++;
		ttsQueue.length = 0;
		ttsSentenceBuffer = '';
		ttsStreamingActive = false;
		ttsPlaying = false;
		if (currentAudio) {
			const audio = currentAudio;
			currentAudio = null;
			audio.onended = null;
			audio.onerror = null;
			audio.pause();
		}
		isSpeaking = false;
	}

	async function speakText(text: string) {
		if (!voiceOutputEnabled || !text.trim()) return;

		stopTTSQueue();

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
				utterance.onend = () => {
					isSpeaking = false;
				};
				utterance.onerror = () => {
					isSpeaking = false;
				};
				speechSynthesis.speak(utterance);
			} else {
				isSpeaking = false;
			}
		}
	}

	function stopSpeaking() {
		stopTTSQueue();
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

	// ── Duplex voice (companion-only) ──────────────────────────────────
	async function toggleDuplexVoice() {
		if (duplexSession?.isActive()) {
			duplexSession.disconnect();
			duplexSession = null;
			duplexState = 'idle';
			duplexTranscript = '';
			duplexVadActive = false;
			duplexLevel = 0;
			voiceOutputEnabled = false;
			stopSpeaking();
		} else {
			// Check if voice models are downloaded
			try {
				const resp = await fetch('/api/v1/voice/models/status');
				const status = await resp.json();
				if (!status.ready) {
					showModelDownload = true;
					modelDownloadProgress = {};
					modelDownloadError = '';
					return;
				}
			} catch {
				// Status check failed — try connecting anyway
			}
			voiceOutputEnabled = true;
			await connectDuplexVoice();
		}
	}

	async function connectDuplexVoice() {
		duplexSession = new VoiceSession({
			onStateChange: (state) => {
				duplexState = state;
			},
			onTranscript: (text) => {
				duplexTranscript = text;
			},
			onAudioLevel: (level) => {
				duplexLevel = level;
			},
			onVadState: (active) => {
				duplexVadActive = active;
			},
			onError: (message) => {
				voiceLog.error('Duplex voice error: ' + message);
			}
		});
		try {
			await duplexSession.connect(ttsVoice);
		} catch {
			duplexSession = null;
			duplexState = 'idle';
		}
	}

	async function startModelDownload() {
		modelDownloadError = '';
		try {
			const resp = await fetch('/api/v1/voice/models/download', { method: 'POST' });
			if (!resp.ok) {
				modelDownloadError = 'Download failed: ' + resp.statusText;
				return;
			}

			const reader = resp.body?.getReader();
			if (!reader) {
				modelDownloadError = 'Streaming not supported';
				return;
			}

			const decoder = new TextDecoder();
			let buffer = '';

			while (true) {
				const { done, value } = await reader.read();
				if (done) break;

				buffer += decoder.decode(value, { stream: true });
				const lines = buffer.split('\n');
				buffer = lines.pop() || '';

				for (const line of lines) {
					if (!line.startsWith('data: ')) continue;
					try {
						const data = JSON.parse(line.slice(6));
						if (data.ready) {
							showModelDownload = false;
							await connectDuplexVoice();
							return;
						}
						if (data.error) {
							modelDownloadError = data.error;
							return;
						}
						if (data.model) {
							modelDownloadProgress[data.model] = {
								downloaded: data.downloaded || 0,
								total: data.total || 0,
								done: data.done || false
							};
							modelDownloadProgress = { ...modelDownloadProgress };
						}
					} catch {
						// Skip malformed SSE lines
					}
				}
			}
		} catch (err) {
			modelDownloadError = err instanceof Error ? err.message : 'Download failed';
		}
	}

	async function toggleWakeWord() {
		if (wakeWordEnabled) {
			wakeWordSession?.disconnect();
			wakeWordSession = null;
			wakeWordEnabled = false;
		} else {
			wakeWordSession = new VoiceSession({
				onWakeWord: async () => {
					wakeWordSession?.disconnect();
					wakeWordSession = null;

					try {
						const statusResp = await fetch('/api/v1/voice/models/status');
						const status = await statusResp.json();
						if (!status.ready) {
							showModelDownload = true;
							return;
						}
					} catch {
						// proceed anyway
					}

					await connectDuplexVoice();

					const checkRestart = setInterval(() => {
						if (!duplexSession?.isActive() && wakeWordEnabled) {
							clearInterval(checkRestart);
							startWakeWordListener();
						} else if (!wakeWordEnabled) {
							clearInterval(checkRestart);
						}
					}, 500);
				},
				onError: (message) => {
					voiceLog.error('Wake word error: ' + message);
				}
			});

			try {
				await wakeWordSession.startWakeWordListening();
				wakeWordEnabled = true;
			} catch {
				wakeWordSession = null;
				wakeWordEnabled = false;
			}
		}
	}

	async function startWakeWordListener() {
		if (!wakeWordEnabled || wakeWordSession?.isActive()) return;
		wakeWordSession = new VoiceSession({
			onWakeWord: async () => {
				wakeWordSession?.disconnect();
				wakeWordSession = null;
				await connectDuplexVoice();
				const checkRestart = setInterval(() => {
					if (!duplexSession?.isActive() && wakeWordEnabled) {
						clearInterval(checkRestart);
						startWakeWordListener();
					} else if (!wakeWordEnabled) {
						clearInterval(checkRestart);
					}
				}, 500);
			},
			onError: (message) => {
				voiceLog.error('Wake word error: ' + message);
			}
		});
		try {
			await wakeWordSession.startWakeWordListening();
		} catch {
			wakeWordSession = null;
		}
	}

	// Global keydown handler (companion-only escape handling)
	function handleGlobalKeydown(e: KeyboardEvent) {
		if (!isCompanion) return;

		if (e.key === 'Escape' && duplexSession?.isActive()) {
			e.preventDefault();
			duplexSession.disconnect();
			duplexSession = null;
			duplexState = 'idle';
			return;
		}
		if (e.key === 'Escape' && isLoading) {
			e.preventDefault();
			cancelMessage();
			return;
		}

		if (
			document.activeElement?.tagName === 'INPUT' ||
			document.activeElement?.tagName === 'TEXTAREA'
		) {
			return;
		}
		if (e.ctrlKey || e.metaKey || e.altKey || e.key.length > 1) {
			return;
		}
		if (chatInputRef && duplexState === 'idle') {
			chatInputRef.focus();
		}
	}
</script>

<svelte:window onkeydown={handleGlobalKeydown} />

<!-- File drop zone for the entire chat area -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
	class="relative flex flex-col h-full bg-base-100"
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
	<!-- Drop overlay -->
	{#if isDraggingOver}
		<div class="absolute inset-0 z-50 flex items-center justify-center bg-base-100/80 backdrop-blur-sm border-2 border-dashed border-primary rounded-2xl pointer-events-none">
			<div class="flex flex-col items-center gap-2 text-primary">
				<svg xmlns="http://www.w3.org/2000/svg" class="w-10 h-10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z"/><polyline points="14 2 14 8 20 8"/><line x1="12" y1="18" x2="12" y2="12"/><line x1="9" y1="15" x2="12" y2="12"/><line x1="15" y1="15" x2="12" y2="12"/></svg>
				<span class="text-lg font-medium">{$t('chat.dropFileToInsert')}</span>
			</div>
		</div>
	{/if}

	<!-- Header -->
	{#if isChannel}
		<header class="border-b border-base-300 bg-base-100/80 backdrop-blur-sm shrink-0">
			<div class="flex items-center justify-between px-6 h-14">
				<div class="flex items-center">
					<span class="text-lg font-semibold text-base-content/90 mr-1">#</span>
					<span class="text-lg font-semibold text-base-content">{mode.channelName}</span>
					{#if mode.loopName}
						<span class="mx-2 text-base-content/90">&middot;</span>
						<span class="text-base text-base-content/80">{mode.loopName}</span>
					{/if}
				</div>
				<div class="flex items-center gap-2 shrink-0">
					<button class="btn btn-sm btn-ghost" class:btn-active={showConfig} title={$t('nav.settings')} onclick={() => showConfig = !showConfig}>
						<Settings class="w-4 h-4" />
					</button>
				</div>
			</div>
		</header>
	{:else}
		<!-- Header provided by [name]/+layout.svelte for both agent and companion -->
	{/if}

	<!-- Entity Config Panel -->
	{#if showConfig}
		<EntityConfigPanel {entityType} {entityId} onclose={() => showConfig = false} />
	{/if}

	<!-- Messages Area -->
	<div class="relative flex-1 min-h-0">
		<div
			bind:this={messagesContainer}
			onscroll={handleScroll}
			class="h-full overflow-y-auto overscroll-contain scroll-pb-4"
		>
			<div class="max-w-4xl mx-auto p-6 space-y-6">
				{#if isCompanion && hasMoreHistory}
					<div class="flex justify-center">
						<a
							href="{mode.agentId ? `/agent/persona/${mode.agentId}/activity` : '/agent/assistant/activity'}"
							class="flex items-center gap-2 px-4 py-2 rounded-lg bg-base-200 text-base text-base-content/80 hover:bg-base-300 hover:text-base-content transition-colors"
						>

							<History class="w-4 h-4" />
							<span>{$t('chat.viewEarlierMessages', { values: { count: totalMessages - messages.length } })}</span>
						</a>
					</div>
				{/if}
				{#if !chatLoaded}
					<div class="flex items-center justify-center h-full">
						<Loader2 class="w-6 h-6 text-base-content/90 animate-spin" />
					</div>
				{:else if messages.length === 0}
					{#if isCompanion}
						<!-- Empty state with suggestions + available agents -->
						<div class="flex flex-col items-center justify-center pt-12 text-center">
							<div class="w-16 h-16 rounded-2xl bg-primary/10 flex items-center justify-center mb-4">
								<Bot class="w-8 h-8 text-primary" />
							</div>
							<h2 class="font-display text-xl font-bold text-base-content mb-2">{$t('chat.yourAICompanion')}</h2>
							<p class="text-base text-base-content/80 max-w-md mb-6">
								{$t('chat.tellMeWhatYouNeed')}
							</p>

							<div class="grid grid-cols-1 sm:grid-cols-2 gap-3 max-w-lg w-full mb-8">
								{#each suggestionKeys as key}
									<button
										type="button"
										onclick={() => selectSuggestion($t(key))}
										class="text-left px-4 py-3 rounded-xl bg-base-200 text-sm text-base-content/80 hover:bg-base-300 hover:text-base-content transition-colors"
										disabled={isLoading}
									>
										{$t(key)}
									</button>
								{/each}
							</div>

							{#if marketplaceAgents.length > 0}
								<div class="w-full max-w-lg">
									<p class="text-xs text-base-content/70 uppercase tracking-wider font-semibold mb-3 text-left">{$t('chat.availableAgents')}</p>
									<div class="grid grid-cols-1 sm:grid-cols-2 gap-2">
										{#each marketplaceAgents as agent}
											<button
												type="button"
												class="flex items-center gap-3 w-full text-left px-3 py-2.5 rounded-xl border border-base-content/10 hover:border-primary/30 hover:bg-primary/5 transition-colors"
												onclick={() => goto(`/marketplace/agents/${agent.id}`)}
											>
												<div class="w-8 h-8 rounded-lg bg-primary/10 flex items-center justify-center shrink-0">
													<span class="text-xs font-bold text-primary">{agent.name.charAt(0).toUpperCase()}</span>
												</div>
												<div class="flex-1 min-w-0">
													<p class="text-sm font-medium truncate">{agent.name}</p>
													<p class="text-xs text-base-content/70 truncate">{agent.description}</p>
												</div>
											</button>
										{/each}
									</div>
									<a href="/marketplace" class="text-xs text-primary hover:brightness-110 mt-3 inline-block">{$t('chat.browseAllAgents')}</a>
								</div>
							{/if}
						</div>
					{:else if isAgent}
						<!-- Agent empty state -->
						<div class="flex flex-col items-center justify-center pt-12 text-center">
							<div class="w-16 h-16 rounded-2xl bg-primary/10 flex items-center justify-center mb-4">
								<span class="text-2xl font-bold text-primary">{(mode.agentName || 'R').charAt(0).toUpperCase()}</span>
							</div>
							<h2 class="font-display text-xl font-bold text-base-content mb-2">{mode.agentName || $t('common.agent')}</h2>
							{#if agentDescription}
								<p class="text-base text-base-content/60 max-w-md mb-8">{agentDescription}</p>
							{:else}
								<p class="text-base text-base-content/60 max-w-md mb-8">{$t('chat.startConversation')}</p>
							{/if}

							<div class="grid grid-cols-1 sm:grid-cols-2 gap-3 max-w-lg w-full">
								<button
									type="button"
									onclick={() => selectSuggestion($t('chat.roleSuggestion1'))}
									class="text-left px-4 py-3 rounded-xl bg-base-200 text-base text-base-content/80 hover:bg-base-300 hover:text-base-content transition-colors"
									disabled={isLoading}
								>
									{$t('chat.roleSuggestion1')}
								</button>
								<button
									type="button"
									onclick={() => selectSuggestion($t('chat.roleSuggestion2'))}
									class="text-left px-4 py-3 rounded-xl bg-base-200 text-base text-base-content/80 hover:bg-base-300 hover:text-base-content transition-colors"
									disabled={isLoading}
								>
									{$t('chat.roleSuggestion2')}
								</button>
							</div>
						</div>
					{:else}
						<div class="flex items-center justify-center h-full">
							<p class="text-base-content/80 text-base">{$t('chat.noMessagesYet')}</p>
						</div>
					{/if}
				{:else}
					<!-- Grouped messages (windowed) -->
					{#each renderedGroups as { group, idx } (idx)}
						<MessageGroup
							messages={group.messages}
							role={group.role}
							agentName={group.agentName}
							onCopy={copyMessage}
							copiedId={copiedMessageId}
							onViewToolOutput={(isCompanion || isAgent) ? openToolSidebar : undefined}
							isStreaming={(isCompanion || isAgent) && group.role === 'assistant' &&
								isLoading &&
								idx === groupedMessages.length - 1}
							onAskSubmit={(isCompanion || isAgent) ? handleAskSubmit : undefined}
							onEditMessage={(isCompanion || isAgent) ? handleEditMessage : undefined}
						/>
					{/each}

					<!-- Loading indicator -->
					{#if (isCompanion || isAgent) && isLoading && !currentStreamingMessage && (groupedMessages.length === 0 || groupedMessages[groupedMessages.length - 1]?.role !== 'assistant')}
						<div class="flex gap-3 mb-4">
							<div
								class="w-10 h-10 rounded-lg flex-shrink-0 self-end mb-1 grid place-items-center font-semibold text-base bg-base-300 text-base-content/80"
							>
								A
							</div>
							<div class="flex flex-col gap-0.5 max-w-[min(900px,calc(100%-60px))] items-start">
								<div class="rounded-xl px-3.5 py-2.5 bg-base-200 animate-pulse-border">
									<ReadingIndicator />
								</div>
								<div class="flex gap-2 items-baseline mt-1.5">
									<span class="text-sm font-medium text-base-content/60">{agentName}</span>
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
					class="p-2 rounded-full bg-base-200 border border-base-300 text-base-content/90 hover:bg-base-300 hover:text-base-content transition-all shadow-lg"
					title={$t('chat.scrollToBottom')}
				>
					<ArrowDown class="w-5 h-5" />
				</button>
			</div>
		{/if}
	</div>

	<!-- Stale warning (companion-only) -->
	{#if isCompanion && staleWarning}
		<div class="max-w-4xl mx-auto px-6 pb-2">
			<div class="alert alert-warning text-base py-2">
				<span>{$t('chat.staleWarning')}</span>
				<button class="btn btn-sm btn-ghost" onclick={cancelMessage}>{$t('chat.forceStop')}</button>
			</div>
		</div>
	{/if}

	<!-- Input Area -->
	{#if isChannel}
		<div class="border-t border-base-300 bg-base-100 shrink-0 px-4 py-3">
			<div class="max-w-4xl mx-auto">
				<ChatInput
					bind:value={inputValue}
					onSend={sendMessage}
					placeholder={$t('chat.messageChannel', { values: { channel: mode.channelName } })}
					disabled={channelSending}
				/>
			</div>
		</div>
	{:else}
		<ChatInput
			bind:this={chatInputRef}
			bind:value={inputValue}
			{isLoading}
			duplexActive={duplexState !== 'idle'}
			audioLevel={duplexLevel}
			{isDraggingOver}
			queuedMessages={messageQueue}
			onSend={sendMessage}
			onCancel={cancelMessage}
			onCancelQueued={cancelQueuedMessage}
			onRecallQueue={recallFromQueue}
			onNewSession={resetChat}
			onSlashSelect={handleSlashSelect}
		/>
		<!-- Voice UI hidden — re-enable onToggleDuplex={toggleDuplexVoice} when phonemizer is fixed (see docs/sme/VOICE_DUPLEX.md) -->
	{/if}
</div>

{#if isCompanion}
<ApprovalModal
	request={pendingApproval}
	onApprove={handleApprove}
	onApproveAlways={handleApproveAlways}
	onDeny={handleDeny}
/>

<BrowserExtensionModal
	bind:show={showBrowserExtModal}
	reason={browserExtReason}
	onRetry={() => { showBrowserExtModal = false; }}
	onDismiss={() => { showBrowserExtModal = false; }}
/>

{#if showModelDownload}
<div class="nebo-modal-backdrop" role="dialog" aria-modal="true">
	<button type="button" class="nebo-modal-overlay" onclick={() => { showModelDownload = false; }}></button>
	<div class="nebo-modal-card max-w-md">
		<!-- Header -->
		<div class="flex items-center justify-between px-5 py-4 border-b border-base-content/10">
			<h3 class="font-display text-lg font-bold">{$t('voiceDownload.title')}</h3>
			<button type="button" onclick={() => { showModelDownload = false; }} class="nebo-modal-close" aria-label={$t('common.close')}>
				<X class="w-5 h-5 text-base-content/90" />
			</button>
		</div>
		<!-- Body -->
		<div class="px-5 py-5">
			<p class="text-base text-base-content/80 mb-4">
				{$t('voiceDownload.description')}
			</p>

			{#if modelDownloadError}
				<div class="rounded-xl bg-error/10 border border-error/20 px-4 py-3 text-base text-error mb-4">
					{modelDownloadError}
				</div>
			{/if}

			{#each Object.entries(modelDownloadProgress) as [name, prog]}
				<div class="mb-3">
					<div class="flex justify-between text-base mb-1">
						<span class="font-mono text-sm">{name}</span>
						<span class="text-sm text-base-content/60">
							{#if prog.done}
								{$t('common.done')}
							{:else if prog.total > 0}
								{Math.round((prog.downloaded / prog.total) * 100)}%
							{:else}
								{$t('voiceDownload.starting')}
							{/if}
						</span>
					</div>
					<div class="h-1.5 rounded-full bg-base-content/10 overflow-hidden">
						<div
							class="h-full rounded-full bg-primary transition-all"
							style="width: {prog.total > 0 ? (prog.downloaded / prog.total) * 100 : 0}%"
						></div>
					</div>
				</div>
			{/each}
		</div>
		<!-- Footer -->
		<div class="flex items-center justify-end gap-3 px-5 py-4 border-t border-base-content/10">
			<button
				type="button"
				class="h-10 px-5 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors"
				onclick={() => { showModelDownload = false; }}
			>
				{$t('common.cancel')}
			</button>
			<button
				type="button"
				class="h-10 px-6 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all disabled:opacity-30"
				onclick={startModelDownload}
				disabled={Object.values(modelDownloadProgress).some(p => !p.done && p.downloaded > 0)}
			>
				{#if Object.values(modelDownloadProgress).some(p => !p.done && p.downloaded > 0)}
					<Loader2 class="w-4 h-4 animate-spin" />
					{$t('voiceDownload.downloading')}
				{:else}
					{$t('voiceDownload.download')}
				{/if}
			</button>
		</div>
	</div>
</div>
{/if}

<CodeInstallModal bind:this={installModal} bind:show={showInstallModal} onclose={() => { showInstallModal = false; }} />

<Toast message={warningMessage} type="warning" duration={8000} bind:show={warningToast} />

<ToolOutputSidebar tool={sidebarTool} {chatId} onClose={closeToolSidebar} />
{/if}
