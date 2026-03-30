<script lang="ts">
	import { onMount, getContext } from 'svelte';
	import { goto } from '$app/navigation';
	import { getActiveAgents, listAgentSessions, listAllRuns } from '$lib/api/nebo';
	import type { ActiveAgentEntry, AgentSession } from '$lib/api/neboComponents';
	import type { WorkflowRun } from '$lib/api/nebo';
	import { getWebSocketClient } from '$lib/websocket/client';
	import { t } from 'svelte-i18n';

	const channelState = getContext<{
		activeView: string;
		activeAgentId: string;
		activeAgentName: string;
	}>('channelState');

	const BOT_COLORS = [
		{ bg: 'bg-blue-500/10', text: 'text-blue-500', border: 'border-blue-500/20' },
		{ bg: 'bg-violet-500/10', text: 'text-violet-500', border: 'border-violet-500/20' },
		{ bg: 'bg-emerald-500/10', text: 'text-emerald-500', border: 'border-emerald-500/20' },
		{ bg: 'bg-amber-500/10', text: 'text-amber-500', border: 'border-amber-500/20' },
		{ bg: 'bg-rose-500/10', text: 'text-rose-500', border: 'border-rose-500/20' },
		{ bg: 'bg-cyan-500/10', text: 'text-cyan-500', border: 'border-cyan-500/20' },
	];

	function nameHash(name: string): number {
		let hash = 0;
		for (let i = 0; i < name.length; i++) {
			hash = ((hash << 5) - hash) + name.charCodeAt(i);
			hash |= 0;
		}
		return Math.abs(hash);
	}

	function agentColor(name: string) {
		return BOT_COLORS[nameHash(name) % BOT_COLORS.length];
	}

	function agentInitial(name: string): string {
		return name.charAt(0).toUpperCase();
	}

	function timeAgo(dateStr: string): string {
		const now = Date.now();
		const then = new Date(dateStr).getTime();
		const diff = now - then;
		const mins = Math.floor(diff / 60000);
		if (mins < 1) return $t('time.justNow');
		if (mins < 60) return $t('time.minutesAgo', { values: { n: mins } });
		const hrs = Math.floor(mins / 60);
		if (hrs < 24) return $t('time.hoursAgo', { values: { n: hrs } });
		const days = Math.floor(hrs / 24);
		return $t('time.daysAgo', { values: { n: days } });
	}

	let agents: ActiveAgentEntry[] = $state([]);
	let sessions: AgentSession[] = $state([]);
	let workflowRuns: (WorkflowRun & { workflow_name: string })[] = $state([]);
	let loading = $state(true);

	interface FeedEntry {
		id: string;
		agentName: string;
		icon: string;
		event: string;
		time: string;
		sortTime: number;
		type: 'info' | 'completed' | 'pending' | 'failed';
	}

	const feed = $derived.by<FeedEntry[]>(() => {
		const entries: FeedEntry[] = [];

		// Add sessions as activity (recent conversations)
		for (const s of sessions) {
			// Try to match session to an agent via name pattern
			const matchedAgent = agents.find(r =>
				s.name?.toLowerCase().includes(r.name.toLowerCase())
			);
			entries.push({
				id: `session-${s.id}`,
				agentName: matchedAgent?.name || 'Companion',
				icon: agentInitial(matchedAgent?.name || 'Companion'),
				event: s.summary || `Chat session${s.messageCount > 0 ? ` (${s.messageCount} messages)` : ''}`,
				time: timeAgo(s.updatedAt),
				sortTime: new Date(s.updatedAt).getTime(),
				type: 'info',
			});
		}

		// Add workflow runs
		for (const run of workflowRuns) {
			entries.push({
				id: `run-${run.id}`,
				agentName: run.workflow_name || 'Workflow',
				icon: agentInitial(run.workflow_name || 'W'),
				event: `${run.workflow_name}: ${run.status}${run.current_activity ? ` — ${run.current_activity}` : ''}`,
				time: timeAgo(run.started_at),
				sortTime: new Date(run.started_at).getTime(),
				type: run.status === 'completed' ? 'completed' : run.status === 'failed' ? 'failed' : run.status === 'running' ? 'pending' : 'info',
			});
		}

		// Sort by time descending
		entries.sort((a, b) => b.sortTime - a.sortTime);
		return entries.slice(0, 20);
	});

	const typeDot: Record<string, string> = {
		completed: 'bg-success',
		pending: 'bg-warning',
		failed: 'bg-error',
		info: 'bg-info',
	};

	function selectAgent(agent: ActiveAgentEntry) {
		channelState.activeAgentId = agent.agentId;
		channelState.activeAgentName = agent.name;
		channelState.activeView = 'agent' as any;
		goto(`/agent/persona/${agent.agentId}/chat`);
	}

	function selectCompanion() {
		goto('/agent/assistant/chat');
	}

	async function loadData() {
		loading = true;
		try {
			const [agentsRes, sessionsRes, runsRes] = await Promise.all([
				getActiveAgents().catch(() => null),
				listAgentSessions().catch(() => null),
				listAllRuns().catch(() => null),
			]);
			if (agentsRes?.agents) agents = agentsRes.agents;
			if (sessionsRes?.sessions) sessions = sessionsRes.sessions.slice(0, 10);
			if (runsRes?.runs) workflowRuns = runsRes.runs.slice(0, 10);
		} finally {
			loading = false;
		}
	}

	onMount(() => {
		loadData();

		const ws = getWebSocketClient();
		const unsub1 = ws.on('agent_activated', () => loadData());
		const unsub2 = ws.on('agent_deactivated', () => loadData());
		const unsub3 = ws.on('lane_update', () => loadData());

		return () => { unsub1(); unsub2(); unsub3(); };
	});
</script>

<div class="command-center">
	{#if loading}
		<div class="flex items-center justify-center py-16">
			<div class="spinner"></div>
		</div>
	{:else}
		<!-- Agent cards -->
		<div class="mb-6">
			<h2 class="text-sm font-semibold uppercase tracking-wider text-base-content/60 mb-3">{$t('sidebar.agents')}</h2>
			<div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
				<!-- Companion card -->
				<button
					class="flex items-start gap-3 p-4 rounded-xl border border-base-content/10 hover:border-primary/30 hover:bg-primary/5 transition-all text-left cursor-pointer"
					onclick={selectCompanion}
				>
					<div class="flex items-center justify-center w-10 h-10 rounded-lg bg-primary/10 shrink-0">
						<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-primary">
							<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
						</svg>
					</div>
					<div class="flex-1 min-w-0">
						<div class="flex items-center gap-2">
							<span class="text-base font-semibold text-base-content">{$t('agent.assistant')}</span>
						</div>
						<p class="text-sm text-base-content/60 mt-0.5">{$t('agent.yourPersonalAI')}</p>
					</div>
				</button>

				<!-- Agent cards -->
				{#each agents as agent (agent.agentId)}
					{@const c = agentColor(agent.name)}
					<button
						class="flex items-start gap-3 p-4 rounded-xl border border-base-content/10 hover:border-base-content/40 hover:bg-base-200/50 transition-all text-left cursor-pointer"
						onclick={() => selectAgent(agent)}
					>
						<div class="flex items-center justify-center w-10 h-10 rounded-lg {c.bg} shrink-0">
							<span class="{c.text} font-semibold">{agentInitial(agent.name)}</span>
						</div>
						<div class="flex-1 min-w-0">
							<div class="flex items-center gap-2">
								<span class="text-base font-semibold text-base-content">{agent.name}</span>
							</div>
							{#if agent.description}
								<p class="text-sm text-base-content/60 mt-0.5 truncate">{agent.description}</p>
							{/if}
							<div class="flex items-center gap-3 mt-1.5">
								{#if agent.workflowCount > 0}
									<span class="text-sm text-base-content/60">{$t('commander.workflowCount', { values: { count: agent.workflowCount } })}</span>
								{/if}
								{#if agent.skillCount > 0}
									<span class="text-sm text-base-content/60">{$t('agent.skillCount', { values: { count: agent.skillCount } })}</span>
								{/if}
							</div>
						</div>
					</button>
				{/each}

				<!-- Add new bot card -->
				<a
					href="/marketplace?type=agent"
					class="flex items-center justify-center gap-2 p-4 rounded-xl border border-dashed border-base-content/10 hover:border-base-content/40 hover:bg-base-200/30 transition-all text-base-content/60 hover:text-base-content/80 cursor-pointer"
				>
					<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<line x1="12" y1="5" x2="12" y2="19" /><line x1="5" y1="12" x2="19" y2="12" />
					</svg>
					<span class="text-base">{$t('agent.addRole')}</span>
				</a>
			</div>
		</div>

		<!-- Activity feed -->
		<div>
			<div class="flex items-center justify-between mb-3">
				<h2 class="text-sm font-semibold uppercase tracking-wider text-base-content/60">{$t('agent.recentActivity')}</h2>
			</div>

			{#if feed.length === 0}
				<div class="flex flex-col items-center py-12 text-center">
					<svg width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" class="text-base-content/40 mb-3">
						<polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
					</svg>
					<p class="text-base text-base-content/60">{$t('agent.noActivity')}</p>
					<p class="text-sm text-base-content/40 mt-1">{$t('agent.noActivityHint')}</p>
				</div>
			{:else}
				<div class="flex flex-col">
					{#each feed as entry, i}
						{@const c = agentColor(entry.agentName)}
						<div class="flex items-start gap-3 py-2.5 {i < feed.length - 1 ? 'border-b border-base-content/5' : ''}">
							<div class="flex items-center justify-center w-7 h-7 rounded-lg {c.bg} shrink-0 mt-0.5">
								<span class="text-sm font-semibold {c.text}">{entry.icon}</span>
							</div>
							<div class="flex-1 min-w-0">
								<p class="text-base text-base-content/80">
									<span class="font-semibold text-base-content">{entry.agentName}</span>
									{' '}{entry.event}
								</p>
								<span class="text-sm text-base-content/60">{entry.time}</span>
							</div>
							<span class="w-2 h-2 rounded-full {typeDot[entry.type]} shrink-0 mt-2"></span>
						</div>
					{/each}
				</div>
			{/if}
		</div>
	{/if}
</div>
