<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { listAgentSessions, getAgentSessionMessages } from '$lib/api/nebo';
	import type { AgentSession, SessionMessage } from '$lib/api/neboComponents';
	import Markdown from '$lib/components/ui/Markdown.svelte';
	import ToolCard from '$lib/components/chat/ToolCard.svelte';
	import { t } from 'svelte-i18n';

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

	/** Build a map from tool_call_id → {output, isError} from all tool-role messages */
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

	let sessions: AgentSession[] = $state([]);
	let loading = $state(true);

	// Inline session viewer state
	let selectedSessionId = $state<string | null>(null);
	let selectedSessionLabel = $state('');
	let sessionMessages: SessionMessage[] = $state([]);
	let loadingMessages = $state(false);

	function toDate(v: string | number): Date {
		const n = typeof v === 'number' ? v : Number(v);
		// Unix seconds (< 1e12) need *1000; milliseconds are fine as-is
		return new Date(n < 1e12 ? n * 1000 : n);
	}

	function timeAgo(dateStr: string | number): string {
		const diff = Date.now() - toDate(dateStr).getTime();
		const mins = Math.floor(diff / 60000);
		if (mins < 1) return $t('time.justNow');
		if (mins < 60) return $t('time.minutesAgo', { values: { n: mins } });
		const hrs = Math.floor(mins / 60);
		if (hrs < 24) return $t('time.hoursAgo', { values: { n: hrs } });
		const days = Math.floor(hrs / 24);
		if (days < 7) return $t('time.daysAgo', { values: { n: days } });
		return toDate(dateStr).toLocaleDateString();
	}

	function formatTime(dateStr: string | number): string {
		return toDate(dateStr).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
	}

	async function openSession(session: AgentSession) {
		selectedSessionId = session.id;
		selectedSessionLabel = session.summary || session.name || $t('agent.chatSession');
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

	async function loadData() {
		loading = true;
		try {
			const sessRes = await listAgentSessions().catch(() => null);
			if (sessRes?.sessions) {
				// Filter to companion sessions (no role: or channel: prefix)
				sessions = sessRes.sessions.filter(s => {
					const name = s.name || '';
					return !name.startsWith('role:') && !name.startsWith('channel:');
				});
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
				selectedSessionLabel = $t('agent.chatSession');
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
	<title>Nebo - {$t('agent.assistant')} - {$t('agent.activity')}</title>
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
					{$t('common.back')}
				</button>
				<h2 class="text-sm font-semibold truncate">{selectedSessionLabel}</h2>
			</div>

			{#if loadingMessages}
				<div class="flex justify-center py-12">
					<span class="loading loading-spinner loading-md text-primary"></span>
				</div>
			{:else if sessionMessages.length === 0}
				<div class="flex flex-col items-center py-12 text-center">
					<p class="text-sm text-base-content/50">{$t('agent.noMessages')}</p>
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
			<!-- Chat history -->
			<section>
				<h2 class="text-sm text-base-content/60 uppercase tracking-wider font-semibold mb-3">{$t('agent.chatHistory')}</h2>
				{#if sessions.length === 0}
					<p class="text-sm text-base-content/50">{$t('agent.noChatHistory')}</p>
				{:else}
					<div class="flex flex-col gap-1">
						{#each sessions as session (session.id)}
							<button
								class="flex items-center gap-3 px-3 py-3 rounded-lg hover:bg-base-content/5 transition-colors text-left w-full"
								onclick={() => openSession(session)}
							>
								<div class="flex-1 min-w-0">
									<p class="text-sm font-medium truncate">{session.summary || session.name || $t('agent.chatSession')}</p>
									<p class="text-xs text-base-content/50 mt-0.5">
										{$t('agent.messageCount', { values: { count: session.messageCount } })}
										{' · '}{timeAgo(session.updatedAt)}
									</p>
								</div>
							</button>
						{/each}
					</div>
				{/if}
			</section>
		{/if}
	</div>
</div>
