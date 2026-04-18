<!--
  Agents Management Page — grid/list view of all installed agents with stats.
  V2 design: search, filters (all/multi/active/paused), grid + list toggle.
-->

<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { listAgents, getActiveAgents, listAgentChats, getAgentStats } from '$lib/api/nebo';
	import type { InstalledAgent, ActiveAgentEntry, AgentWorkflowStats } from '$lib/api/neboComponents';
	import { Search, LayoutGrid, List, Plus } from 'lucide-svelte';

	// V2 agent colors
	const COLORS = ['violet', 'green', 'sky', 'amber', 'rose', 'mint', 'slate', 'peach', 'lilac'];
	function colorFor(i: number) { return COLORS[i % COLORS.length]; }

	// Pre-defined class strings so Tailwind JIT detects them at build time.
	const AVATAR_BG: Record<string, string> = {
		violet: 'bg-[var(--agent-violet-bg)] text-[var(--agent-violet-ink)]',
		green: 'bg-[var(--agent-green-bg)] text-[var(--agent-green-ink)]',
		sky: 'bg-[var(--agent-sky-bg)] text-[var(--agent-sky-ink)]',
		amber: 'bg-[var(--agent-amber-bg)] text-[var(--agent-amber-ink)]',
		rose: 'bg-[var(--agent-rose-bg)] text-[var(--agent-rose-ink)]',
		mint: 'bg-[var(--agent-mint-bg)] text-[var(--agent-mint-ink)]',
		slate: 'bg-[var(--agent-slate-bg)] text-[var(--agent-slate-ink)]',
		peach: 'bg-[var(--agent-peach-bg)] text-[var(--agent-peach-ink)]',
		lilac: 'bg-[var(--agent-lilac-bg)] text-[var(--agent-lilac-ink)]',
	};
	function avatarClasses(color: string): string { return AVATAR_BG[color] ?? AVATAR_BG.violet; }

	interface AgentRow {
		id: string;
		name: string;
		description: string;
		source: string;
		active: boolean;
		multi: boolean;
		chats: number;
		runs: number;
		colorIdx: number;
	}

	let agents = $state<AgentRow[]>([]);
	let isLoading = $state(true);
	let query = $state('');
	let filter = $state<'all' | 'multi' | 'active' | 'paused'>('all');
	let view = $state<'grid' | 'list'>('grid');

	const filtered = $derived(() => {
		let list = agents;
		if (filter === 'multi') list = list.filter(a => a.multi);
		if (filter === 'active') list = list.filter(a => a.active);
		if (filter === 'paused') list = list.filter(a => !a.active);
		if (query.trim()) {
			const q = query.toLowerCase().trim();
			list = list.filter(a => a.name.toLowerCase().includes(q) || a.description.toLowerCase().includes(q));
		}
		return list;
	});

	const totalCount = $derived(agents.length);
	const filteredCount = $derived(filtered().length);

	async function load() {
		try {
			const [installed, active] = await Promise.all([
				listAgents().catch(() => ({ agents: [], filesystemAgents: [], total: 0 })),
				getActiveAgents().catch(() => ({ agents: [], count: 0 })),
			]);

			const activeIds = new Set((active?.agents ?? []).map(a => a.agentId));
			const allAgents = installed?.agents ?? [];

			// Build rows with basic info; stats loaded async below
			const rows: AgentRow[] = allAgents.map((a, i) => ({
				id: a.id,
				name: a.name,
				description: a.description || '',
				source: a.source,
				active: activeIds.has(a.id),
				multi: false,
				chats: 0,
				runs: 0,
				colorIdx: i,
			}));

			// Also add filesystem agents
			for (const fa of installed?.filesystemAgents ?? []) {
				rows.push({
					id: fa.name,
					name: fa.name,
					description: fa.description || '',
					source: fa.source,
					active: activeIds.has(fa.name),
					multi: false,
					chats: 0,
					runs: 0,
					colorIdx: rows.length,
				});
			}

			agents = rows;
			isLoading = false;

			// Load chat counts + stats in parallel (non-blocking)
			for (const row of rows) {
				Promise.all([
					listAgentChats(row.id).catch(() => null),
					getAgentStats(row.id).catch(() => null),
				]).then(([chatsRes, statsRes]) => {
					const idx = agents.findIndex(a => a.id === row.id);
					if (idx < 0) return;
					agents[idx] = {
						...agents[idx],
						chats: chatsRes?.total ?? chatsRes?.chats?.length ?? 0,
						multi: (chatsRes?.total ?? chatsRes?.chats?.length ?? 0) > 1,
						runs: statsRes?.stats?.totalRuns ?? 0,
					};
					agents = agents; // trigger reactivity
				});
			}
		} catch {
			isLoading = false;
		}
	}

	onMount(() => { load(); });

	function openAgent(agent: AgentRow) {
		goto(`/agent/persona/${agent.id}/chat`);
	}

	const filterOptions: Array<{ key: typeof filter; label: string }> = [
		{ key: 'all', label: 'All' },
		{ key: 'multi', label: 'Multi-chat' },
		{ key: 'active', label: 'Active' },
		{ key: 'paused', label: 'Paused' },
	];
</script>

<div class="flex-1 overflow-auto">
	<div class="max-w-[1200px] mx-auto px-8 py-7 pb-16">
		<!-- Header -->
		<div class="flex items-baseline mb-4">
			<h1 class="text-2xl font-semibold tracking-tight">Agents</h1>
			<span class="text-sm text-base-content/50 ml-2.5">
				{filteredCount} of {totalCount}
			</span>
			<div class="ml-auto flex items-center gap-2">
				<!-- Search -->
				<div class="inline-flex items-center gap-2 px-3 py-1.5 rounded-lg border border-base-300 bg-base-100 w-60 text-sm text-base-content/50">
					<Search class="w-3.5 h-3.5 shrink-0" />
					<input
						class="border-0 outline-0 bg-transparent flex-1 text-base-content placeholder:text-base-content/40"
						placeholder="Search agents…"
						bind:value={query}
					/>
				</div>
				<!-- View toggles -->
				<button
					type="button"
					class="inline-flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg border text-xs font-medium transition-colors {view === 'grid' ? 'bg-primary/10 text-primary border-transparent' : 'bg-base-100 text-base-content/60 border-base-300'}"
					onclick={() => view = 'grid'}
				>
					<LayoutGrid class="w-3.5 h-3.5" /> Grid
				</button>
				<button
					type="button"
					class="inline-flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg border text-xs font-medium transition-colors {view === 'list' ? 'bg-primary/10 text-primary border-transparent' : 'bg-base-100 text-base-content/60 border-base-300'}"
					onclick={() => view = 'list'}
				>
					<List class="w-3.5 h-3.5" /> List
				</button>
				<!-- New agent -->
				<a href="/marketplace/agents" class="inline-flex items-center gap-1.5 px-3.5 py-1.5 rounded-lg bg-primary text-primary-content text-sm font-medium">
					<Plus class="w-3.5 h-3.5" /> New agent
				</a>
			</div>
		</div>

		<!-- Filters -->
		<div class="flex gap-1.5 mb-4 text-xs">
			{#each filterOptions as opt}
				<button
					type="button"
					class="px-3 py-1 rounded-full border cursor-pointer transition-colors {filter === opt.key ? 'bg-primary/10 text-primary border-transparent font-medium' : 'bg-base-100 text-base-content/60 border-base-300'}"
					onclick={() => filter = opt.key}
				>
					{opt.label}
				</button>
			{/each}
		</div>

		<!-- Loading -->
		{#if isLoading}
			<div class="flex items-center justify-center py-20">
				<span class="loading loading-spinner loading-lg"></span>
			</div>

		<!-- Grid view -->
		{:else if view === 'grid'}
			<div class="grid grid-cols-[repeat(auto-fill,minmax(260px,1fr))] gap-3.5">
				{#each filtered() as agent (agent.id)}
					{@const color = colorFor(agent.colorIdx)}
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<div
						class="border border-base-300 rounded-xl bg-base-100 p-4 pb-3.5 cursor-pointer relative shadow-sm hover:shadow-md transition-shadow"
						onclick={() => openAgent(agent)}
					>
						<!-- Status bar (top-right) -->
						<div class="absolute top-4 right-4 flex items-center gap-1.5 text-[11px] text-base-content/40">
							{#if agent.multi}
								<span class="text-[9.5px] font-bold tracking-wider text-primary bg-primary/10 px-1.5 py-px rounded">MULTI</span>
							{/if}
							<span class="w-1.5 h-1.5 rounded-full {agent.active ? 'bg-success' : 'bg-base-content/20'}"></span>
							{agent.active ? 'Active' : 'Paused'}
						</div>

						<!-- Agent header -->
						<div class="flex items-start gap-3 mb-2.5">
							<div
								class="w-10 h-10 rounded-[10px] flex items-center justify-center text-sm font-semibold shrink-0 {avatarClasses(color)}"
							>
								{agent.name.charAt(0).toUpperCase()}
							</div>
							<div class="min-w-0 pr-16">
								<div class="text-sm font-semibold tracking-tight">{agent.name}</div>
								<div class="text-xs text-base-content/50 mt-0.5 line-clamp-2">{agent.description || 'No description'}</div>
							</div>
						</div>

						<!-- Stats -->
						<div class="grid grid-cols-3 gap-2 mt-2.5 pt-3 border-t border-base-300">
							<div>
								<div class="text-[15px] font-semibold">{agent.chats}</div>
								<div class="text-[10.5px] text-base-content/40 uppercase tracking-wider">Chats</div>
							</div>
							<div>
								<div class="text-[15px] font-semibold">{agent.runs.toLocaleString()}</div>
								<div class="text-[10.5px] text-base-content/40 uppercase tracking-wider">Runs</div>
							</div>
							<div>
								<div class="text-[15px] font-semibold {agent.active ? 'text-primary' : ''}">
									{agent.active ? 'On' : '—'}
								</div>
								<div class="text-[10.5px] text-base-content/40 uppercase tracking-wider">Status</div>
							</div>
						</div>
					</div>
				{:else}
					<div class="col-span-full text-center py-16 text-base-content/50">
						{query.trim() ? 'No agents match your search' : 'No agents installed yet'}
					</div>
				{/each}
			</div>

		<!-- List view -->
		{:else}
			<div class="border border-base-300 rounded-xl bg-base-100 overflow-hidden">
				<!-- Table header -->
				<div class="grid grid-cols-[34px_1.4fr_1fr_100px_100px_100px] gap-3.5 px-4 py-2.5 bg-base-200/50 text-[10.5px] font-bold tracking-widest uppercase text-base-content/40">
					<span></span>
					<span>Agent</span>
					<span>About</span>
					<span>Chats</span>
					<span>Runs</span>
					<span>Status</span>
				</div>
				{#each filtered() as agent (agent.id)}
					{@const color = colorFor(agent.colorIdx)}
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<div
						class="grid grid-cols-[34px_1.4fr_1fr_100px_100px_100px] gap-3.5 items-center px-4 py-3 border-t border-base-300 cursor-pointer hover:bg-base-200/30 transition-colors"
						onclick={() => openAgent(agent)}
					>
						<div
							class="w-[30px] h-[30px] rounded-lg flex items-center justify-center text-xs font-semibold {avatarClasses(color)}"
						>
							{agent.name.charAt(0).toUpperCase()}
						</div>
						<div class="min-w-0">
							<div class="text-[13.5px] font-medium truncate">
								{agent.name}
								{#if agent.multi}
									<span class="text-[9.5px] font-bold tracking-wider text-primary bg-primary/10 px-1.5 py-px rounded ml-1.5">MULTI</span>
								{/if}
							</div>
							<div class="text-xs text-base-content/40 truncate">{agent.id}</div>
						</div>
						<div class="text-xs text-base-content/50 truncate">{agent.description || '—'}</div>
						<div class="text-xs text-base-content/50">{agent.chats}</div>
						<div class="text-xs text-base-content/50">{agent.runs.toLocaleString()}</div>
						<div class="flex items-center gap-1.5 text-xs text-base-content/50">
							<span class="w-1.5 h-1.5 rounded-full {agent.active ? 'bg-success' : 'bg-base-content/20'}"></span>
							{agent.active ? 'Active' : 'Paused'}
						</div>
					</div>
				{:else}
					<div class="px-4 py-16 text-center text-base-content/50">
						{query.trim() ? 'No agents match your search' : 'No agents installed yet'}
					</div>
				{/each}
			</div>
		{/if}
	</div>
</div>
