<script lang="ts">
	import { onMount } from 'svelte';
	import { tick } from 'svelte';
	import { History } from 'lucide-svelte';
	import { getChannelMessages, sendChannelMessage } from '$lib/api/nebo';
	import MessageGroup from './MessageGroup.svelte';
	import ChatInput from './ChatInput.svelte';

	interface ChannelMessage {
		id: string;
		from: string;
		content: string;
		content_html: string;
		created_at: string;
	}

	interface ChannelMember {
		bot_id: string;
		bot_name: string;
		role: string;
		is_online: boolean;
	}

	// Reuse the same Message interface that MessageGroup expects
	interface Message {
		id: string;
		role: 'user' | 'assistant' | 'system';
		content: string;
		contentHtml?: string;
		timestamp: Date;
		toolCalls?: [];
		streaming?: boolean;
		thinking?: string;
		contentBlocks?: [];
	}

	interface MessageGroupType {
		role: 'user' | 'assistant';
		messages: Message[];
		agentName: string;
	}

	const INITIAL_LIMIT = 30;
	const FULL_LIMIT = 200;

	let {
		channelId,
		channelName,
		loopName
	}: {
		channelId: string;
		channelName: string;
		loopName: string;
	} = $props();

	let rawMessages: ChannelMessage[] = $state([]);
	let memberNames: Record<string, string> = $state({});
	let inputValue = $state('');
	let loading = $state(false);
	let sending = $state(false);
	let messagesContainer: HTMLDivElement | undefined = $state();
	let currentLimit = $state(INITIAL_LIMIT);
	let hasMore = $derived(rawMessages.length >= currentLimit && currentLimit < FULL_LIMIT);

	function resolveName(senderId: string): string {
		if (senderId === 'You') return 'You';
		return memberNames[senderId] || senderId.substring(0, 8) + '…';
	}

	// Map channel messages → MessageGroup format, grouped by consecutive sender
	const groupedMessages = $derived.by((): MessageGroupType[] => {
		const groups: MessageGroupType[] = [];
		let current: MessageGroupType | null = null;

		for (const raw of rawMessages) {
			const isUser = raw.from === 'You';
			const role: 'user' | 'assistant' = isUser ? 'user' : 'assistant';
			const name = isUser ? 'You' : resolveName(raw.from);

			const msg: Message = {
				id: raw.id,
				role,
				content: raw.content,
				contentHtml: raw.content_html || undefined,
				timestamp: new Date(raw.created_at)
			};

			// Group consecutive messages from the same sender
			if (current && current.role === role && current.agentName === name) {
				current.messages.push(msg);
			} else {
				current = { role, messages: [msg], agentName: name };
				groups.push(current);
			}
		}

		return groups;
	});

	async function loadMessages(limit?: number) {
		try {
			loading = true;
			const fetchLimit = limit ?? currentLimit;
			const res = await getChannelMessages({ limit: fetchLimit }, channelId);
			const data = res as unknown as { messages: ChannelMessage[]; members: ChannelMember[] };
			if (data?.messages) {
				rawMessages = data.messages;
			}
			if (data?.members) {
				const names: Record<string, string> = {};
				for (const m of data.members) {
					names[m.bot_id] = m.bot_name || m.bot_id.substring(0, 8);
				}
				memberNames = names;
			}
			await tick();
			scrollToBottom();
		} catch {
			// Channel messages not available
		} finally {
			loading = false;
		}
	}

	function loadOlder() {
		currentLimit = FULL_LIMIT;
		loadMessages(FULL_LIMIT);
	}

	function scrollToBottom() {
		if (messagesContainer) {
			messagesContainer.scrollTop = messagesContainer.scrollHeight;
		}
	}

	async function handleSend() {
		const text = inputValue.trim();
		if (!text || sending) return;

		const optimistic: ChannelMessage = {
			id: `temp-${Date.now()}`,
			from: 'You',
			content: text,
			content_html: '',
			created_at: new Date().toISOString()
		};
		rawMessages = [...rawMessages, optimistic];
		inputValue = '';
		sending = true;

		await tick();
		scrollToBottom();

		try {
			await sendChannelMessage({ text }, channelId);
			await loadMessages();
		} catch {
			rawMessages = rawMessages.filter(m => m.id !== optimistic.id);
		} finally {
			sending = false;
		}
	}

	// Reload when channelId changes
	$effect(() => {
		channelId;
		rawMessages = [];
		memberNames = {};
		currentLimit = INITIAL_LIMIT;
		loadMessages(INITIAL_LIMIT);
	});

	// Auto-refresh every 10s
	onMount(() => {
		const interval = setInterval(loadMessages, 10000);
		return () => clearInterval(interval);
	});
</script>

<div class="channel-chat-container">
	<!-- Header -->
	<div class="channel-chat-header">
		<span class="channel-chat-hash">#</span>
		<span class="channel-chat-name">{channelName}</span>
		{#if loopName}
			<span class="channel-chat-separator">&middot;</span>
			<span class="channel-chat-loop">{loopName}</span>
		{/if}
	</div>

	<!-- Messages — reuses MessageGroup from companion chat -->
	<div class="h-full overflow-y-auto overscroll-contain scroll-pb-4" bind:this={messagesContainer}>
		<div class="max-w-4xl mx-auto p-6 space-y-6">
			{#if hasMore}
				<div class="flex justify-center">
					<button
						type="button"
						onclick={loadOlder}
						class="flex items-center gap-2 px-4 py-2 rounded-lg bg-base-200 text-sm text-base-content/70 hover:bg-base-300 hover:text-base-content transition-colors"
					>
						<History class="w-4 h-4" />
						<span>View older messages</span>
					</button>
				</div>
			{/if}
			{#if loading && rawMessages.length === 0}
				<div class="flex items-center justify-center h-full">
					<span class="spinner-small"></span>
				</div>
			{:else if rawMessages.length === 0}
				<div class="flex items-center justify-center h-full">
					<p class="text-base-content/40 text-sm">No messages yet</p>
				</div>
			{:else}
				{#each groupedMessages as group, i (i)}
					<MessageGroup
						messages={group.messages}
						role={group.role}
						agentName={group.agentName}
					/>
				{/each}
			{/if}
		</div>
	</div>

	<!-- Input — reuses ChatInput from companion chat -->
	<div class="channel-chat-input-area">
		<div class="max-w-4xl mx-auto">
			<ChatInput
				bind:value={inputValue}
				onSend={handleSend}
				placeholder="Message #{channelName}..."
				disabled={sending}
			/>
		</div>
	</div>
</div>
