<script lang="ts">
	import { onMount, getContext } from 'svelte';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { listAgentSessions, listMemories, getRoleWorkflows, getActiveRoles, getAgentSessionMessages } from '$lib/api/nebo';
	import type { AgentSession, MemoryItem, RoleWorkflowEntry, SessionMessage } from '$lib/api/neboComponents';
	import Markdown from '$lib/components/ui/Markdown.svelte';
	import ToolCard from '$lib/components/chat/ToolCard.svelte';

	interface ToolCall {
		id?: string;
		name: string;
		input?: unknown;
	}

	interface ToolResult {
		tool_call_id?: string;
		content?: string;
		is_error?: boolean;
	}

	function parseToolCalls(json?: string): ToolCall[] {
		if (!json) return [];
		try { return JSON.parse(json); } catch { return []; }
	}

	function parseToolResults(json?: string): ToolResult[] {
		if (!json) return [];
		try { return JSON.parse(json); } catch { return []; }
	}

	function buildToolOutputMap(messages: SessionMessage[]): Map<string, { output: string; isError: boolean }> {
		const map = new Map<string, { output: string; isError: boolean }>();
		for (const msg of messages) {
			if (msg.role !== 'tool') continue;
			for (const tr of parseToolResults(msg.toolResults)) {
				if (tr.tool_call_id) {
					map.set(tr.tool_call_id, { output: tr.content || '', isError: tr.is_error || false });
				}
			}
		}
		return map;
	}

	const channelState = getContext<{
		activeRoleId: string;
		activeRoleName: string;
	}>('channelState');

	let sessions: AgentSession[] = $state([]);
	let memories: MemoryItem[] = $state([]);
	let workflows: RoleWorkflowEntry[] = $state([]);
	let isActive = $state(false);
	let loading = $state(true);

	// Inline session viewer state
	let selectedSessionId = $state<string | null>(null);
	let selectedSessionLabel = $state('');
	let sessionMessages: SessionMessage[] = $state([]);
	let loadingMessages = $state(false);

	function toDate(v: string | number): Date {
		const n = typeof v === 'number' ? v : Number(v);
		return new Date(n < 1e12 ? n * 1000 : n);
	}

	function timeAgo(dateStr: string | number): string {
		const diff = Date.now() - toDate(dateStr).getTime();
		const mins = Math.floor(diff / 60000);
		if (mins < 1) return 'just now';
		if (mins < 60) return `${mins}m ago`;
		const hrs = Math.floor(mins / 60);
		if (hrs < 24) return `${hrs}h ago`;
		const days = Math.floor(hrs / 24);
		if (days < 7) return `${days}d ago`;
		return toDate(dateStr).toLocaleDateString();
	}

	function formatTime(dateStr: string | number): string {
		return toDate(dateStr).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
	}

	interface ActivityEntry {
		time: string;
		sortTime: number;
		event: string;
		type: 'info' | 'completed' | 'pending' | 'failed';
	}

	const recentActivity = $derived.by<ActivityEntry[]>(() => {
		const entries: ActivityEntry[] = [];

		for (const s of sessions.slice(0, 5)) {
			entries.push({
				time: timeAgo(s.updatedAt),
				sortTime: new Date(s.updatedAt).getTime(),
				event: s.summary || `Chat session${s.messageCount > 0 ? ` — ${s.messageCount} messages` : ''}`,
				type: 'completed',
			});
		}

		if (isActive) {
			entries.push({
				time: 'Active now',
				sortTime: Date.now(),
				event: 'Role activated and running',
				type: 'info',
			});
		}

		if (workflows.length > 0) {
			entries.push({
				time: 'on activate',
				sortTime: Date.now() - 1000,
				event: `${workflows.length} workflow${workflows.length !== 1 ? 's' : ''} registered`,
				type: 'info',
			});
		}

		entries.sort((a, b) => b.sortTime - a.sortTime);
		return entries.slice(0, 8);
	});

	const typeBg: Record<string, string> = {
		completed: 'bg-success/10',
		pending: 'bg-warning/10',
		failed: 'bg-error/10',
		info: 'bg-info/10',
	};

	const typeColor: Record<string, string> = {
		completed: 'text-success',
		pending: 'text-warning',
		failed: 'text-error',
		info: 'text-info',
	};

	async function openSession(session: AgentSession) {
		selectedSessionId = session.id;
		selectedSessionLabel = session.summary || session.name || 'Chat session';
		loadingMessages = true;
		try {
			const res = await getAgentSessionMessages(session.id);
			sessionMessages = res.messages || [];
		} catch {
			sessionMessages = [];
		} finally {
			loadingMessages = false;
		}
	}

	function closeSession() {
		selectedSessionId = null;
		selectedSessionLabel = '';
		sessionMessages = [];
	}

	const hasRoleId = $derived(!!channelState.activeRoleId);

	async function loadData() {
		loading = true;
		try {
			const [sessRes, memRes, wfRes, activeRes] = await Promise.all([
				listAgentSessions().catch(() => null),
				hasRoleId ? listMemories({ namespace: `role:${channelState.activeRoleId}`, page: 1, pageSize: 10 }).catch(() => null) : null,
				hasRoleId ? getRoleWorkflows(channelState.activeRoleId).catch(() => null) : null,
				hasRoleId ? getActiveRoles().catch(() => null) : null,
			]);
			isActive = activeRes?.roles?.some(r => r.roleId === channelState.activeRoleId) ?? false;
			if (sessRes?.sessions) {
				const roleLower = channelState.activeRoleName.toLowerCase();
				sessions = sessRes.sessions.filter(s => s.name?.toLowerCase().includes(roleLower));
			}
			if (memRes?.memories) {
				memories = memRes.memories;
			}
			if (wfRes?.workflows) {
				workflows = wfRes.workflows;
			}
		} catch {
			// ignore
		} finally {
			loading = false;
		}

		// Check if URL has a session param to auto-open
		const sid = $page.url.searchParams.get('session');
		if (sid) {
			const match = sessions.find(s => s.id === sid);
			if (match) {
				openSession(match);
			} else {
				// Session exists but may not be in filtered list — try loading directly
				selectedSessionId = sid;
				selectedSessionLabel = 'Session';
				loadingMessages = true;
				try {
					const res = await getAgentSessionMessages(sid);
					sessionMessages = res.messages || [];
				} catch {
					sessionMessages = [];
				} finally {
					loadingMessages = false;
				}
			}
		}
	}

	onMount(() => loadData());
</script>

<svelte:head>
	<title>Nebo - {channelState.activeRoleName || 'Activity'} - Activity</title>
</svelte:head>

<div class="flex-1 overflow-y-auto">
	<div class="max-w-3xl mx-auto px-6 py-6">
		{#if loading}
			<div class="flex justify-center py-12">
				<span class="loading loading-spinner loading-md text-primary"></span>
			</div>
		{:else if selectedSessionId}
			<!-- Inline session message viewer -->
			<div class="flex items-center gap-2 mb-4">
				<button
					class="btn btn-sm btn-ghost gap-1.5"
					onclick={closeSession}
				>
					<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<path d="M19 12H5" /><polyline points="12 19 5 12 12 5" />
					</svg>
					Back
				</button>
				<h2 class="text-sm font-semibold truncate">{selectedSessionLabel}</h2>
			</div>

			{#if loadingMessages}
				<div class="flex justify-center py-12">
					<span class="loading loading-spinner loading-md text-primary"></span>
				</div>
			{:else if sessionMessages.length === 0}
				<div class="flex flex-col items-center py-12 text-center">
					<p class="text-sm text-base-content/50">No messages in this session.</p>
				</div>
			{:else}
				{@const toolOutputMap = buildToolOutputMap(sessionMessages)}
				<div class="flex flex-col gap-4">
					{#each sessionMessages as msg (msg.id)}
						{#if msg.role === 'tool'}
							<!-- Skip tool-role messages; outputs shown inline on assistant messages -->
						{:else}
						<div class="flex gap-4 {msg.role === 'user' ? 'justify-end' : ''}">
							{#if msg.role === 'user'}
								<div class="max-w-[80%]">
									<div class="text-sm text-base-content/60 text-right mb-1">
										{formatTime(msg.createdAt)}
									</div>
									<div class="rounded-2xl bg-primary px-4 py-3">
										<p class="text-primary-content whitespace-pre-wrap">{msg.content || ''}</p>
									</div>
								</div>
							{:else if msg.role === 'system'}
								<div class="w-full flex justify-center">
									<div class="bg-base-200 rounded-lg px-3 py-2 text-sm text-base-content/60">
										{msg.content || ''}
									</div>
								</div>
							{:else}
								{@const tools = parseToolCalls(msg.toolCalls)}
								<div class="max-w-[90%]">
									<div class="text-sm text-base-content/60 mb-1">
										{formatTime(msg.createdAt)}
									</div>
									{#if tools.length}
										<div class="flex flex-col gap-1.5 mb-2">
											{#each tools as tc}
												<div class="max-w-md">
													<ToolCard
														name={tc.name}
														input={tc.input}
														output={tc.id ? toolOutputMap.get(tc.id)?.output || '' : ''}
														status={tc.id && toolOutputMap.get(tc.id) ? (toolOutputMap.get(tc.id)?.isError ? 'error' : 'complete') : 'complete'}
													/>
												</div>
											{/each}
										</div>
									{/if}
									{#if msg.content}
										<div class="rounded-2xl bg-base-200/50 px-4 py-3 border border-base-300/50">
											<Markdown content={msg.content} />
										</div>
									{/if}
								</div>
							{/if}
						</div>
						{/if}
					{/each}
				</div>
			{/if}
		{:else}
			<!-- Recent activity timeline -->
			{#if recentActivity.length > 0}
				<section class="pb-6">
					<h2 class="text-sm text-base-content/60 uppercase tracking-wider font-semibold mb-3">Recent activity</h2>
					<div class="flex flex-col">
						{#each recentActivity as entry, i}
							<div class="flex items-start gap-2.5 py-2 {i < recentActivity.length - 1 ? 'border-b border-base-content/5' : ''}">
								<div class="shrink-0 w-5 h-5 rounded-full {typeBg[entry.type]} flex items-center justify-center mt-0.5">
									{#if entry.type === 'completed'}
										<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="{typeColor[entry.type]}">
											<path d="M22 11.08V12a10 10 0 1 1-5.93-9.14" /><polyline points="22 4 12 14.01 9 11.01" />
										</svg>
									{:else if entry.type === 'pending'}
										<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="{typeColor[entry.type]}">
											<path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" /><line x1="12" y1="9" x2="12" y2="13" /><line x1="12" y1="17" x2="12.01" y2="17" />
										</svg>
									{:else if entry.type === 'failed'}
										<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="{typeColor[entry.type]}">
											<circle cx="12" cy="12" r="10" /><line x1="15" y1="9" x2="9" y2="15" /><line x1="9" y1="9" x2="15" y2="15" />
										</svg>
									{:else}
										<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="{typeColor[entry.type]}">
											<polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2" />
										</svg>
									{/if}
								</div>
								<div class="flex-1 min-w-0">
									<p class="text-base text-base-content/80">{entry.event}</p>
									<span class="text-sm text-base-content/60">{entry.time}</span>
								</div>
							</div>
						{/each}
					</div>
				</section>
			{/if}

			<!-- Chat history -->
			<section class="pb-6 border-t border-base-content/10 pt-4">
				<h2 class="text-sm text-base-content/60 uppercase tracking-wider font-semibold mb-3">Chat history</h2>
				{#if sessions.length === 0}
					<p class="text-sm text-base-content/50">No chat history with {channelState.activeRoleName} yet.</p>
				{:else}
					<div class="flex flex-col gap-1">
						{#each sessions as session (session.id)}
							<button
								class="flex items-center gap-3 px-3 py-3 rounded-lg hover:bg-base-content/5 transition-colors text-left w-full"
								onclick={() => openSession(session)}
							>
								<div class="flex-1 min-w-0">
									<p class="text-sm font-medium truncate">{session.summary || session.name || 'Chat session'}</p>
									<p class="text-xs text-base-content/50 mt-0.5">
										{session.messageCount} message{session.messageCount !== 1 ? 's' : ''}
										{' · '}{timeAgo(session.updatedAt)}
									</p>
								</div>
							</button>
						{/each}
					</div>
				{/if}
			</section>

			<!-- Memories -->
			{#if memories.length > 0}
				<section class="pb-12 border-t border-base-content/10 pt-4">
					<h2 class="text-sm text-base-content/60 uppercase tracking-wider font-semibold mb-3 flex items-center gap-1.5">
						<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<path d="M12 2a9 9 0 0 1 9 9c0 3.88-3.08 7.13-5.5 9.36a2.06 2.06 0 0 1-2.82.08A27 27 0 0 1 3 11a9 9 0 0 1 9-9z" />
							<circle cx="12" cy="11" r="3" />
						</svg>
						Memories
					</h2>
					<div class="flex flex-col gap-2">
						{#each memories as mem}
							<div class="rounded-lg border border-base-content/5 p-3">
								<div class="flex items-center gap-1.5 mb-1">
									<span class="text-sm font-semibold text-primary">{mem.key}</span>
									{#if mem.tags && mem.tags.length > 0}
										<span class="text-sm text-base-content/40">{mem.tags.join(', ')}</span>
									{/if}
								</div>
								<p class="text-sm text-base-content/60">{mem.value}</p>
							</div>
						{/each}
					</div>
				</section>
			{/if}

			<!-- Empty state if absolutely nothing -->
			{#if recentActivity.length === 0 && sessions.length === 0 && memories.length === 0}
				<div class="flex flex-col items-center py-12 text-center">
					<p class="text-sm text-base-content/50">No activity yet for {channelState.activeRoleName}.</p>
					<p class="text-xs text-base-content/40 mt-1">Chat with the agent to get started.</p>
				</div>
			{/if}
		{/if}
	</div>
</div>
