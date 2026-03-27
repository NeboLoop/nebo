<script lang="ts">
	import { onMount, onDestroy, getContext } from 'svelte';
	import { page } from '$app/stores';
	import { listAgentSessions, listMemories, getRoleWorkflows, getActiveRoles, getAgentSessionMessages, getRoleStats, listRoleRuns } from '$lib/api/nebo';
	import type { AgentSession, MemoryItem, RoleWorkflowEntry, SessionMessage, RoleWorkflowStats, WorkflowRun, WorkflowRunError } from '$lib/api/neboComponents';
	import Markdown from '$lib/components/ui/Markdown.svelte';
	import ToolCard from '$lib/components/chat/ToolCard.svelte';
	import { getWebSocketClient } from '$lib/websocket/client';
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

	// Workflow run state
	let stats: RoleWorkflowStats | null = $state(null);
	let recentErrors: WorkflowRunError[] = $state([]);
	let runs: WorkflowRun[] = $state([]);
	let runsTotal = $state(0);

	const hasActivityAbove = $derived(
		(stats != null && stats.totalRuns > 0) || runs.length > 0 || recentErrors.length > 0
	);

	// Inline session viewer state
	let selectedSessionId = $state<string | null>(null);
	let selectedSessionLabel = $state('');
	let sessionMessages: SessionMessage[] = $state([]);
	let loadingMessages = $state(false);

	// WS subscriptions
	let unsubs: Array<() => void> = [];

	function toDate(v: string | number): Date {
		const n = typeof v === 'number' ? v : Number(v);
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

	function formatDuration(secs: number): string {
		if (secs < 60) return $t('time.seconds', { values: { n: secs } });
		const mins = Math.floor(secs / 60);
		const rem = secs % 60;
		if (mins < 60) return rem > 0 ? $t('time.minutesSeconds', { values: { mins, secs: rem } }) : $t('time.minutes', { values: { mins } });
		const hrs = Math.floor(mins / 60);
		return $t('time.hoursMinutes', { values: { hrs, mins: mins % 60 } });
	}

	function runDuration(run: WorkflowRun): string {
		if (!run.completedAt) return $t('common.running');
		const secs = run.completedAt - run.startedAt;
		return formatDuration(secs);
	}

	// Group runs by day
	interface DayGroup {
		label: string;
		date: string;
		runs: WorkflowRun[];
		completed: number;
		failed: number;
		running: number;
	}

	const groupedRuns = $derived.by<DayGroup[]>(() => {
		const groups = new Map<string, DayGroup>();
		const today = new Date().toLocaleDateString();
		const yesterday = new Date(Date.now() - 86400000).toLocaleDateString();

		for (const run of runs) {
			const date = toDate(run.startedAt).toLocaleDateString();
			let group = groups.get(date);
			if (!group) {
				let label = date;
				if (date === today) label = $t('agentActivity.today');
				else if (date === yesterday) label = $t('agentActivity.yesterday');
				group = { label, date, runs: [], completed: 0, failed: 0, running: 0 };
				groups.set(date, group);
			}
			group.runs.push(run);
			if (run.status === 'completed') group.completed++;
			else if (run.status === 'failed') group.failed++;
			else if (run.status === 'running') group.running++;
		}

		return Array.from(groups.values());
	});

	// Track which day groups are expanded
	let expandedDays = $state<Set<string>>(new Set([$t('agentActivity.today')]));

	function toggleDay(label: string) {
		const next = new Set(expandedDays);
		if (next.has(label)) next.delete(label);
		else next.add(label);
		expandedDays = next;
	}

	const statusColor: Record<string, string> = {
		completed: 'bg-success',
		failed: 'bg-error',
		running: 'bg-warning',
		cancelled: 'bg-base-content/30',
	};

	function getStatusLabel(status: string): string {
		switch (status) {
			case 'completed': return $t('common.completed');
			case 'failed': return $t('common.failed');
			case 'running': return $t('common.running');
			case 'cancelled': return $t('common.cancelled');
			default: return status;
		}
	}

	let hasMoreMessages = $state(false);
	let loadingMore = $state(false);
	let messagesEndEl: HTMLDivElement | undefined = $state();

	async function openSession(session: AgentSession) {
		selectedSessionId = session.id;
		selectedSessionLabel = session.summary || session.name || $t('agent.chatSession');
		loadingMessages = true;
		try {
			const res = await getAgentSessionMessages(session.id, 50);
			sessionMessages = res.messages || [];
			hasMoreMessages = res.hasMore ?? false;
		} catch {
			sessionMessages = [];
			hasMoreMessages = false;
		} finally {
			loadingMessages = false;
			// Auto-scroll to bottom after render
			requestAnimationFrame(() => {
				messagesEndEl?.scrollIntoView();
			});
		}
	}

	async function loadOlderMessages() {
		if (!selectedSessionId || loadingMore || !hasMoreMessages || sessionMessages.length === 0) return;
		loadingMore = true;
		try {
			const oldestId = sessionMessages[0].id;
			const res = await getAgentSessionMessages(selectedSessionId, 50, oldestId);
			const older = res.messages || [];
			sessionMessages = [...older, ...sessionMessages];
			hasMoreMessages = res.hasMore ?? false;
		} catch {
			// ignore
		} finally {
			loadingMore = false;
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
			const [sessRes, memRes, wfRes, activeRes, statsRes, runsRes] = await Promise.all([
				listAgentSessions().catch(() => null),
				hasRoleId ? listMemories({ namespace: `role:${channelState.activeRoleId}`, page: 1, pageSize: 10 }).catch(() => null) : null,
				hasRoleId ? getRoleWorkflows(channelState.activeRoleId).catch(() => null) : null,
				hasRoleId ? getActiveRoles().catch(() => null) : null,
				hasRoleId ? getRoleStats(channelState.activeRoleId).catch(() => null) : null,
				hasRoleId ? listRoleRuns(channelState.activeRoleId, 100).catch(() => null) : null,
			]);
			isActive = activeRes?.roles?.some(r => r.roleId === channelState.activeRoleId) ?? false;
			if (sessRes?.sessions) {
				const roleLower = channelState.activeRoleName.toLowerCase();
				sessions = sessRes.sessions.filter(s => s.name?.toLowerCase().includes(roleLower));
			}
			if (memRes?.memories) memories = memRes.memories;
			if (wfRes?.workflows) workflows = wfRes.workflows;
			if (statsRes) {
				stats = statsRes.stats;
				recentErrors = statsRes.recentErrors || [];
			}
			if (runsRes) {
				runs = runsRes.runs || [];
				runsTotal = runsRes.total;
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
				selectedSessionId = sid;
				selectedSessionLabel = $t('agent.chatSession');
				loadingMessages = true;
				try {
					const res = await getAgentSessionMessages(sid, 50);
					sessionMessages = res.messages || [];
					hasMoreMessages = res.hasMore ?? false;
				} catch {
					sessionMessages = [];
					hasMoreMessages = false;
				} finally {
					loadingMessages = false;
					requestAnimationFrame(() => messagesEndEl?.scrollIntoView());
				}
			}
		}
	}

	onMount(() => {
		loadData();

		// Subscribe to WS events for live updates
		const ws = getWebSocketClient();
		unsubs.push(
			ws.on('workflow_run_started', (data: { roleId: string }) => {
				if (data.roleId === channelState.activeRoleId) loadData();
			}),
			ws.on('workflow_run_completed', (data: { roleId: string }) => {
				if (data.roleId === channelState.activeRoleId) loadData();
			}),
			ws.on('workflow_run_failed', (data: { roleId: string }) => {
				if (data.roleId === channelState.activeRoleId) loadData();
			}),
		);
	});

	onDestroy(() => {
		unsubs.forEach(fn => fn());
	});
</script>

<svelte:head>
	<title>Nebo - {channelState.activeRoleName || $t('agent.activity')} - {$t('agent.activity')}</title>
</svelte:head>

<div class="flex-1 flex flex-col min-h-0">
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
					<p class="text-sm text-base-content/70">{$t('agent.noMessages')}</p>
				</div>
			{:else}
				{@const toolOutputMap = buildToolOutputMap(sessionMessages)}
				{#if hasMoreMessages}
					<div class="flex justify-center mb-4">
						<button
							class="btn btn-sm btn-ghost text-base-content/70"
							disabled={loadingMore}
							onclick={loadOlderMessages}
						>
							{loadingMore ? $t('common.loading') : $t('agent.loadOlderMessages')}
						</button>
					</div>
				{/if}
				<div class="flex flex-col gap-4">
					{#each sessionMessages as msg (msg.id)}
						{#if msg.role === 'tool'}
							<!-- Skip tool-role messages; outputs shown inline on assistant messages -->
						{:else}
						<div class="flex gap-4 {msg.role === 'user' ? 'justify-end' : ''}">
							{#if msg.role === 'user'}
								<div class="max-w-[80%]">
									<div class="text-sm text-base-content/80 text-right mb-1">
										{formatTime(msg.createdAt)}
									</div>
									<div class="rounded-2xl bg-primary px-4 py-3">
										<p class="text-primary-content whitespace-pre-wrap">{msg.content || ''}</p>
									</div>
								</div>
							{:else if msg.role === 'system'}
								<div class="w-full flex justify-center">
									<div class="bg-base-200 rounded-lg px-3 py-2 text-sm text-base-content/80">
										{msg.content || ''}
									</div>
								</div>
							{:else}
								{@const tools = parseToolCalls(msg.toolCalls)}
								<div class="max-w-[90%]">
									<div class="text-sm text-base-content/80 mb-1">
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
					<div bind:this={messagesEndEl}></div>
				</div>
			{/if}
		{:else}
			<!-- Stats overview -->
			{#if stats && stats.totalRuns > 0}
				<section class="pb-6">
					<div class="flex items-center justify-between mb-3"><h2 class="text-xs text-base-content/80 uppercase tracking-wider font-semibold">{$t('agentActivity.overview')}</h2><span class="btn btn-sm invisible">&#8203;</span></div>

					<div class="grid grid-cols-4 gap-3">
						<div class="rounded-xl border border-base-content/10 p-3 text-center">
							<p class="text-2xl font-bold">{stats.totalRuns}</p>
							<p class="text-xs text-base-content/70">{$t('agentActivity.totalRuns')}</p>
						</div>
						<div class="rounded-xl border border-base-content/10 p-3 text-center">
							<p class="text-2xl font-bold text-success">{stats.completed}</p>
							<p class="text-xs text-base-content/70">{$t('common.completed')}</p>
						</div>
						<div class="rounded-xl border border-base-content/10 p-3 text-center">
							<p class="text-2xl font-bold text-error">{stats.failed}</p>
							<p class="text-xs text-base-content/70">{$t('common.failed')}</p>
						</div>
						<div class="rounded-xl border border-base-content/10 p-3 text-center">
							<p class="text-2xl font-bold">{stats.avgDurationSecs != null ? formatDuration(stats.avgDurationSecs) : '—'}</p>
							<p class="text-xs text-base-content/70">{$t('agentActivity.avgDuration')}</p>
						</div>
					</div>
					{#if stats.running > 0}
						<div class="mt-2 flex items-center gap-2 text-sm text-warning">
							<span class="loading loading-spinner loading-xs"></span>
							{$t('agentActivity.runningNow', { values: { count: stats.running } })}
						</div>
					{/if}
				</section>
			{/if}

			<!-- Workflow runs grouped by day -->
			{#if groupedRuns.length > 0}
				<section class="pb-6 {stats && stats.totalRuns > 0 ? 'border-t border-base-content/10 pt-4 mt-2' : ''}">
					<div class="flex items-center justify-between mb-3"><h2 class="text-xs text-base-content/80 uppercase tracking-wider font-semibold">{$t('agentActivity.workflowRuns')}</h2><span class="btn btn-sm invisible">&#8203;</span></div>
					<div class="flex flex-col gap-2">
						{#each groupedRuns as group}
							<!-- Day header -->
							<button
								type="button"
								class="flex items-center justify-between w-full px-3 py-2 rounded-lg hover:bg-base-content/5 transition-colors text-left"
								onclick={() => toggleDay(group.label)}
							>
								<div class="flex items-center gap-2">
									<svg class="w-3 h-3 text-base-content/70 transition-transform {expandedDays.has(group.label) ? 'rotate-90' : ''}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
										<polyline points="9 18 15 12 9 6" />
									</svg>
									<span class="text-sm font-medium">{group.label}</span>
								</div>
								<div class="flex items-center gap-2 text-xs">
									{#if group.running > 0}
										<span class="text-warning">{$t('agentActivity.runningCount', { values: { count: group.running } })}</span>
									{/if}
									{#if group.failed > 0}
										<span class="text-error">{$t('agentActivity.failedCount', { values: { count: group.failed } })}</span>
									{/if}
									<span class="text-base-content/70">{$t('agentActivity.completedCount', { values: { count: group.completed } })}</span>
								</div>
							</button>

							<!-- Expanded: show individual runs (failed first, then recent) -->
							{#if expandedDays.has(group.label)}
								<div class="flex flex-col gap-0.5 ml-5 border-l border-base-content/10 pl-3">
									{#each group.runs.filter(r => r.status === 'failed') as run (run.id)}
										<div class="flex items-center gap-3 px-3 py-2 rounded-lg hover:bg-base-content/5 transition-colors">
											<div class="w-2 h-2 rounded-full shrink-0 {statusColor[run.status]}"></div>
											<div class="flex-1 min-w-0">
												<div class="flex items-center gap-2">
													<span class="text-sm font-medium text-error">{$t('common.failed')}</span>
													<span class="text-xs text-base-content/70">{run.triggerType}</span>
												</div>
												{#if run.error}
													<p class="text-xs text-error/70 truncate mt-0.5">{run.error}</p>
												{/if}
											</div>
											<div class="text-right shrink-0">
												<p class="text-xs text-base-content/70">{formatTime(run.startedAt)}</p>
												<p class="text-xs text-base-content/70">{runDuration(run)}</p>
											</div>
										</div>
									{/each}
									{#each group.runs.filter(r => r.status === 'running') as run (run.id)}
										<div class="flex items-center gap-3 px-3 py-2 rounded-lg hover:bg-base-content/5 transition-colors">
											<div class="w-2 h-2 rounded-full shrink-0 bg-warning"></div>
											<div class="flex-1 min-w-0">
												<div class="flex items-center gap-2">
													<span class="text-sm font-medium text-warning">{$t('common.running')}</span>
													{#if run.currentActivity}
														<span class="text-xs text-base-content/70">{run.currentActivity}</span>
													{/if}
												</div>
											</div>
											<div class="text-right shrink-0">
												<p class="text-xs text-base-content/70">{formatTime(run.startedAt)}</p>
											</div>
										</div>
									{/each}
									{#if group.completed > 5}
										<div class="px-3 py-1.5 text-xs text-base-content/70">
											{$t('agentActivity.completedCount', { values: { count: group.completed } })}
										</div>
									{:else}
										{#each group.runs.filter(r => r.status === 'completed') as run (run.id)}
											<div class="flex items-center gap-3 px-3 py-1.5 text-xs text-base-content/70">
												<div class="w-1.5 h-1.5 rounded-full shrink-0 bg-success"></div>
												<span>{$t('common.completed')}</span>
												<span>{run.triggerType}</span>
												<span class="ml-auto">{formatTime(run.startedAt)} · {runDuration(run)}</span>
											</div>
										{/each}
									{/if}
								</div>
							{/if}
						{/each}
					</div>
					{#if runsTotal > runs.length}
						<p class="text-xs text-base-content/70 text-center mt-3">{$t('agentActivity.olderRuns', { values: { count: runsTotal - runs.length } })}</p>
					{/if}
				</section>
			{/if}

			<!-- Chat history -->
			<section class="pb-6 {hasActivityAbove ? 'border-t border-base-content/10 pt-4 mt-2' : ''}">
				<div class="flex items-center justify-between mb-3"><h2 class="text-xs text-base-content/80 uppercase tracking-wider font-semibold">{$t('agent.chatHistory')}</h2><span class="btn btn-sm invisible">&#8203;</span></div>
				{#if sessions.length === 0}
					<p class="text-sm text-base-content/70">{$t('agent.noChatHistoryWith', { values: { name: channelState.activeRoleName } })}</p>
				{:else}
					<div class="flex flex-col gap-1">
						{#each sessions as session (session.id)}
							<button
								class="flex items-center gap-3 px-3 py-3 rounded-lg hover:bg-base-content/5 transition-colors text-left w-full"
								onclick={() => openSession(session)}
							>
								<div class="flex-1 min-w-0">
									<p class="text-sm font-medium truncate">{session.summary || session.name || $t('agent.chatSession')}</p>
									<p class="text-xs text-base-content/70 mt-0.5">
										{$t('agent.messageCount', { values: { count: session.messageCount } })}
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
				<section class="pb-12 border-t border-base-content/10 pt-4 mt-2">
					<div class="flex items-center justify-between mb-3">
						<h2 class="text-xs text-base-content/80 uppercase tracking-wider font-semibold flex items-center gap-1.5">
							<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
								<path d="M12 2a9 9 0 0 1 9 9c0 3.88-3.08 7.13-5.5 9.36a2.06 2.06 0 0 1-2.82.08A27 27 0 0 1 3 11a9 9 0 0 1 9-9z" />
								<circle cx="12" cy="11" r="3" />
							</svg>
							{$t('agentActivity.memories')}
						</h2>
						<span class="btn btn-sm invisible">&#8203;</span>
					</div>
					<div class="flex flex-col gap-2">
						{#each memories as mem}
							<div class="rounded-lg border border-base-content/5 p-3">
								<div class="flex items-center gap-1.5 mb-1">
									<span class="text-sm font-semibold text-primary">{mem.key}</span>
									{#if mem.tags && mem.tags.length > 0}
										<span class="text-sm text-base-content/80">{mem.tags.join(', ')}</span>
									{/if}
								</div>
								<p class="text-sm text-base-content/80">{mem.value}</p>
							</div>
						{/each}
					</div>
				</section>
			{/if}

			<!-- Empty state if absolutely nothing -->
			{#if (!stats || stats.totalRuns === 0) && runs.length === 0 && sessions.length === 0 && memories.length === 0}
				<div class="flex flex-col items-center py-12 text-center">
					<p class="text-sm text-base-content/70">{$t('agent.noActivityFor', { values: { name: channelState.activeRoleName } })}</p>
					<p class="text-xs text-base-content/80 mt-1">{$t('agent.noActivityHintRole')}</p>
				</div>
			{/if}
		{/if}
		</div>
	</div>
</div>
