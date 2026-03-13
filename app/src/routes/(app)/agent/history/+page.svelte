<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { ArrowLeft, Calendar, MessageSquare, Search, Loader2 } from 'lucide-svelte';
	import { listChatDays, getHistoryByDay, searchChatMessages } from '$lib/api';
	import type { ChatMessage as ApiChatMessage, DayInfo } from '$lib/api';
	import * as neboApi from '$lib/api/nebo';
	import Markdown from '$lib/components/ui/Markdown.svelte';

	interface Message {
		id: string;
		role: 'user' | 'assistant' | 'system';
		content: string;
		timestamp: Date;
	}

	let days = $state<DayInfo[]>([]);
	let selectedDay = $state<string | null>(null);
	let messages = $state<Message[]>([]);
	let isLoading = $state(false);
	let isLoadingDays = $state(true);
	let searchQuery = $state('');
	let searchResults = $state<Message[]>([]);
	let isSearching = $state(false);
	let searchMode = $state(false);

	// Session mode state
	let sessionId = $state<string | null>(null);
	let sessionLabel = $state('');
	let sessionBadge = $state<{ type: string; label: string } | null>(null);

	const badgeColors: Record<string, string> = {
		chat: 'text-primary bg-primary/10',
		role: 'text-info bg-info/10',
		channel: 'text-success bg-success/10',
		heartbeat: 'text-warning bg-warning/10',
		workflow: 'text-secondary bg-secondary/10',
	};

	function parseSessionName(name: string): { badge: { type: string; label: string }; display: string } {
		if (name.startsWith('role:')) {
			const parts = name.split(':');
			const display = parts.length > 2 ? parts.slice(2).join(':') : parts[1] || 'Role';
			return { badge: { type: 'role', label: 'Role' }, display };
		}
		if (name.startsWith('channel:')) {
			return { badge: { type: 'channel', label: 'Channel' }, display: name.slice('channel:'.length) };
		}
		if (name.startsWith('heartbeat:')) {
			return { badge: { type: 'heartbeat', label: 'Heartbeat' }, display: name.slice('heartbeat:'.length) || 'Heartbeat' };
		}
		if (name.startsWith('workflow:')) {
			return { badge: { type: 'workflow', label: 'Workflow' }, display: name.slice('workflow:'.length) };
		}
		// Companion chat — truncate UUID
		const display = /^[0-9a-f]{8}-[0-9a-f]{4}/.test(name) ? name.slice(0, 8) + '...' : name;
		return { badge: { type: 'chat', label: 'Chat' }, display };
	}

	onMount(async () => {
		const sid = page.url.searchParams.get('session');
		if (sid) {
			sessionId = sid;
			await loadSessionMessages(sid);
		} else {
			await loadDays();
		}
	});

	async function loadSessionMessages(id: string) {
		isLoading = true;
		try {
			const res = await neboApi.getAgentSessionMessages(id);
			messages = (res.messages || []).map((m) => ({
				id: String(m.id),
				role: m.role as 'user' | 'assistant' | 'system',
				content: m.content || '',
				timestamp: new Date(m.createdAt)
			}));

			// Try to get session info for label
			const sessionsRes = await neboApi.listAgentSessions();
			const session = (sessionsRes.sessions || []).find((s) => s.id === id);
			if (session) {
				const parsed = parseSessionName(session.name || session.id);
				sessionLabel = parsed.display;
				sessionBadge = parsed.badge;
			} else {
				sessionLabel = `Session ${id.slice(0, 8)}...`;
				sessionBadge = { type: 'chat', label: 'Session' };
			}
		} catch (err) {
			console.error('Failed to load session messages:', err);
		}
		isLoading = false;
	}

	function exitSessionMode() {
		sessionId = null;
		sessionLabel = '';
		sessionBadge = null;
		messages = [];
		goto('/agent/history');
		loadDays();
	}

	async function loadDays() {
		isLoadingDays = true;
		try {
			const res = await listChatDays({ page: 1, pageSize: 100 });
			days = res.days || [];
		} catch (err) {
			console.error('Failed to load days:', err);
		}
		isLoadingDays = false;
	}

	async function selectDay(day: string) {
		selectedDay = day;
		searchMode = false;
		isLoading = true;
		try {
			const res = await getHistoryByDay(day);
			messages = (res.messages || []).map((m: ApiChatMessage) => ({
				id: m.id,
				role: m.role as 'user' | 'assistant' | 'system',
				content: m.content,
				timestamp: new Date(m.createdAt)
			}));
		} catch (err) {
			console.error('Failed to load messages:', err);
		}
		isLoading = false;
	}

	async function handleSearch() {
		if (!searchQuery.trim()) {
			searchMode = false;
			searchResults = [];
			return;
		}

		searchMode = true;
		selectedDay = null;
		isSearching = true;
		try {
			const res = await searchChatMessages({ query: searchQuery.trim(), page: 1, pageSize: 50 });
			searchResults = (res.messages || []).map((m: ApiChatMessage) => ({
				id: m.id,
				role: m.role as 'user' | 'assistant' | 'system',
				content: m.content,
				timestamp: new Date(m.createdAt)
			}));
		} catch (err) {
			console.error('Failed to search:', err);
		}
		isSearching = false;
	}

	function formatDate(dateStr: string) {
		const date = new Date(dateStr);
		const today = new Date();
		const yesterday = new Date(today);
		yesterday.setDate(yesterday.getDate() - 1);

		if (dateStr === today.toISOString().split('T')[0]) {
			return 'Today';
		}
		if (dateStr === yesterday.toISOString().split('T')[0]) {
			return 'Yesterday';
		}
		return date.toLocaleDateString([], { weekday: 'short', month: 'short', day: 'numeric' });
	}

	function formatTime(date: Date) {
		return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
	}
</script>

<svelte:head>
	<title>History - Nebo</title>
</svelte:head>

<div class="flex flex-col h-full bg-base-100">
	<!-- Header -->
	<header class="flex items-center gap-4 px-6 h-14 border-b border-base-300 bg-base-100/80 backdrop-blur-sm shrink-0">
		{#if sessionId}
			<button
				type="button"
				class="btn btn-sm btn-ghost gap-2"
				onclick={exitSessionMode}
			>
				<ArrowLeft class="w-4 h-4" />
				<span class="hidden sm:inline">Back to History</span>
			</button>
			<div class="flex-1"></div>
			<div class="flex items-center gap-2">
				{#if sessionBadge}
					<span class="text-[11px] font-semibold uppercase tracking-wider px-1.5 py-0.5 rounded {badgeColors[sessionBadge.type] || 'text-base-content/50 bg-base-content/5'}">
						{sessionBadge.label}
					</span>
				{/if}
				<h1 class="text-lg font-semibold text-base-content">{sessionLabel}</h1>
			</div>
			<div class="flex-1"></div>
			<div class="w-24"></div>
		{:else}
			<a
				href="/agent"
				class="btn btn-sm btn-ghost gap-2"
			>
				<ArrowLeft class="w-4 h-4" />
				<span class="hidden sm:inline">Back to Chat</span>
			</a>
			<div class="flex-1"></div>
			<h1 class="text-lg font-semibold text-base-content">History</h1>
			<div class="flex-1"></div>
			<div class="w-24"></div>
		{/if}
	</header>

	{#if sessionId}
		<!-- Session mode: full-width message view -->
		<main class="flex-1 overflow-y-auto overscroll-contain">
			<div class="max-w-4xl mx-auto p-6 space-y-4">
				{#if isLoading}
					<div class="flex items-center justify-center h-64">
						<Loader2 class="w-6 h-6 text-base-content/70 animate-spin" />
					</div>
				{:else if messages.length === 0}
					<div class="flex flex-col items-center justify-center h-64 text-center">
						<MessageSquare class="w-8 h-8 text-base-content/70 mb-3" />
						<p class="text-base-content/70">No messages in this session</p>
					</div>
				{:else}
					{#each messages as message (message.id)}
						<div class="flex gap-4 {message.role === 'user' ? 'justify-end' : ''}">
							{#if message.role === 'user'}
								<div class="max-w-[80%]">
									<div class="text-xs text-base-content/70 text-right mb-1">
										{formatTime(message.timestamp)}
									</div>
									<div class="rounded-2xl bg-primary px-4 py-3">
										<p class="text-primary-content whitespace-pre-wrap">{message.content}</p>
									</div>
								</div>
							{:else if message.role === 'system'}
								<div class="w-full flex justify-center">
									<div class="bg-base-200 rounded-lg px-3 py-2 text-xs text-base-content/70">
										{message.content}
									</div>
								</div>
							{:else}
								<div class="max-w-[90%]">
									<div class="text-xs text-base-content/70 mb-1">
										{formatTime(message.timestamp)}
									</div>
									<div class="rounded-2xl bg-base-200/50 px-4 py-3 border border-base-300/50">
										<Markdown content={message.content} />
									</div>
								</div>
							{/if}
						</div>
					{/each}
				{/if}
			</div>
		</main>
	{:else}
		<!-- Day mode: sidebar + messages -->
		<div class="flex flex-1 min-h-0">
			<!-- Day List Sidebar -->
			<aside class="w-64 border-r border-base-300 flex flex-col shrink-0">
				<!-- Search -->
				<div class="p-3 border-b border-base-300">
					<div class="relative">
						<Search class="absolute left-3 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-base-content/70" />
						<input
							type="text"
							bind:value={searchQuery}
							onkeydown={(e) => e.key === 'Enter' && handleSearch()}
							placeholder="Search messages..."
							class="w-full pl-9 pr-3 py-2 text-xs bg-base-200 rounded-lg focus:outline-none focus:ring-1 focus:ring-primary/50"
						/>
					</div>
				</div>

				<!-- Days List -->
				<nav class="flex-1 overflow-y-auto overscroll-contain p-2 space-y-0.5">
					{#if isLoadingDays}
						<div class="flex items-center justify-center h-32">
							<Loader2 class="w-5 h-5 text-base-content/70 animate-spin" />
						</div>
					{:else if days.length === 0}
						<div class="flex flex-col items-center justify-center h-32 text-center px-4">
							<Calendar class="w-6 h-6 text-base-content/70 mb-2" />
							<p class="text-xs text-base-content/70">No history yet</p>
						</div>
					{:else}
						{#each days as day}
							<button
								type="button"
								onclick={() => selectDay(day.day)}
								class="flex items-center justify-between w-full px-3 py-2.5 rounded-lg text-sm transition-colors {selectedDay === day.day
									? 'bg-primary/10 text-primary'
									: 'text-base-content/70 hover:bg-base-200 hover:text-base-content'}"
							>
								<span>{formatDate(day.day)}</span>
								<span class="text-xs text-base-content/70">{day.messageCount}</span>
							</button>
						{/each}
					{/if}
				</nav>
			</aside>

			<!-- Messages View -->
			<main class="flex-1 overflow-y-auto overscroll-contain">
				<div class="max-w-4xl mx-auto p-6 space-y-4">
					{#if isLoading || isSearching}
						<div class="flex items-center justify-center h-64">
							<Loader2 class="w-6 h-6 text-base-content/70 animate-spin" />
						</div>
					{:else if searchMode}
						{#if searchResults.length === 0}
							<div class="flex flex-col items-center justify-center h-64 text-center">
								<Search class="w-8 h-8 text-base-content/70 mb-3" />
								<p class="text-base-content/70">No results for "{searchQuery}"</p>
							</div>
						{:else}
							<h2 class="text-sm font-medium text-base-content/70 mb-4">
								{searchResults.length} result{searchResults.length !== 1 ? 's' : ''} for "{searchQuery}"
							</h2>
							{#each searchResults as message (message.id)}
								<div class="p-4 rounded-lg bg-base-200/50 border border-base-300/50">
									<div class="flex items-center gap-2 mb-2">
										<span class="text-xs font-medium text-base-content/70 capitalize">{message.role}</span>
										<span class="text-xs text-base-content/70">{formatTime(message.timestamp)}</span>
									</div>
									<div class="text-sm">
										{#if message.role === 'assistant'}
											<Markdown content={message.content} />
										{:else}
											<p class="whitespace-pre-wrap">{message.content}</p>
										{/if}
									</div>
								</div>
							{/each}
						{/if}
					{:else if selectedDay}
						{#if messages.length === 0}
							<div class="flex flex-col items-center justify-center h-64 text-center">
								<MessageSquare class="w-8 h-8 text-base-content/70 mb-3" />
								<p class="text-base-content/70">No messages on this day</p>
							</div>
						{:else}
							<h2 class="text-sm font-medium text-base-content/70 mb-4">
								{formatDate(selectedDay)}
							</h2>
							{#each messages as message (message.id)}
								<div class="flex gap-4 {message.role === 'user' ? 'justify-end' : ''}">
									{#if message.role === 'user'}
										<div class="max-w-[80%]">
											<div class="text-xs text-base-content/70 text-right mb-1">
												{formatTime(message.timestamp)}
											</div>
											<div class="rounded-2xl bg-primary px-4 py-3">
												<p class="text-primary-content whitespace-pre-wrap">{message.content}</p>
											</div>
										</div>
									{:else if message.role === 'system'}
										<div class="w-full flex justify-center">
											<div class="bg-base-200 rounded-lg px-3 py-2 text-xs text-base-content/70">
												{message.content}
											</div>
										</div>
									{:else}
										<div class="max-w-[90%]">
											<div class="text-xs text-base-content/70 mb-1">
												{formatTime(message.timestamp)}
											</div>
											<div class="rounded-2xl bg-base-200/50 px-4 py-3 border border-base-300/50">
												<Markdown content={message.content} />
											</div>
										</div>
									{/if}
								</div>
							{/each}
						{/if}
					{:else}
						<div class="flex flex-col items-center justify-center h-64 text-center">
							<Calendar class="w-8 h-8 text-base-content/70 mb-3" />
							<p class="text-base-content/70">Select a day to view messages</p>
						</div>
					{/if}
				</div>
			</main>
		</div>
	{/if}
</div>
