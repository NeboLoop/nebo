<!--
  Home Dashboard — cross-agent view: greeting, briefing, running, stats, attention, chats.
  V2 design: page_home.jsx reference. Pure Tailwind, no custom CSS.
-->

<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { goto } from '$app/navigation';
	import { getActiveAgents, getSimpleAgentStatus, neboLoopJanusUsage, listChats, listMCPIntegrations, getUserProfile } from '$lib/api/nebo';
	import { getWebSocketClient } from '$lib/websocket/client';
	import type { ActiveAgentEntry, SimpleAgentStatusResponse, NeboLoopJanusUsageResponse, Chat } from '$lib/api/neboComponents';
	import { Store, Link2, Settings } from 'lucide-svelte';

	let agents = $state<ActiveAgentEntry[]>([]);
	let agentStatus = $state<SimpleAgentStatusResponse | null>(null);
	let usage = $state<NeboLoopJanusUsageResponse | null>(null);
	let chats = $state<Chat[]>([]);
	let mcpCount = $state(0);
	let isLoading = $state(true);
	let userName = $state('');

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
	function avatarClasses(i: number): string { return AVATAR_BG[colorFor(i)] ?? AVATAR_BG.violet; }

	const greeting = $derived(() => {
		const h = new Date().getHours();
		if (h < 5) return 'Still up,';
		if (h < 12) return 'Good morning,';
		if (h < 18) return 'Good afternoon,';
		return 'Good evening,';
	});

	const activeAgentCount = $derived(agents.length);
	const chatCount = $derived(chats.length);

	async function load() {
		const [r, s, u, c, m, p] = await Promise.all([
			getActiveAgents().catch(() => ({ agents: [], count: 0 })),
			getSimpleAgentStatus().catch(() => null),
			neboLoopJanusUsage().catch(() => null),
			listChats({ pageSize: 10 }).catch(() => ({ chats: [], total: 0 })),
			listMCPIntegrations().catch(() => ({ integrations: [] })),
			getUserProfile().catch(() => ({ profile: null })),
		]);
		agents = r?.agents ?? [];
		agentStatus = s;
		usage = u;
		chats = c?.chats ?? [];
		mcpCount = (m?.integrations ?? []).length;
		userName = p?.profile?.displayName || '';
		isLoading = false;
	}

	const ws = getWebSocketClient();
	let unsubs: Array<() => void> = [];

	onMount(() => {
		load();
		unsubs = [
			ws.on('chat_complete', () => load()),
			ws.on('agent_activated', () => load()),
			ws.on('agent_deactivated', () => load()),
		];
	});

	onDestroy(() => unsubs.forEach((u) => u()));
</script>

<div class="flex-1 overflow-auto">
	<div class="max-w-[1080px] mx-auto px-8 py-8 pb-16">
		{#if isLoading}
			<div class="flex items-center justify-center py-20">
				<span class="loading loading-spinner loading-lg"></span>
			</div>
		{:else}
			<!-- Greeting -->
			<h1 class="text-[28px] font-semibold tracking-tight">{greeting()} {userName || 'there'}.</h1>
			<p class="text-sm text-base-content/50 mt-1 mb-7">
				{#if activeAgentCount > 0}{activeAgentCount} agents active{/if}
				{#if chatCount > 0} · {chatCount} recent chats{/if}
				{#if agentStatus?.connected} · connected{/if}
			</p>

			<div class="grid grid-cols-1 lg:grid-cols-[1.3fr_1fr] gap-5">
				<!-- Left column -->
				<div class="flex flex-col gap-4">
					<!-- Briefing card (gradient) -->
					{#if agents.length > 0}
						{@const topAgent = agents[0]}
						<div class="border border-base-300 rounded-2xl p-5 shadow-sm bg-gradient-to-b from-primary/5 to-base-100">
							<!-- Briefing header -->
							<div class="flex items-center gap-2.5 mb-3">
								<div
									class="w-[30px] h-[30px] rounded-[9px] grid place-items-center font-bold text-xs {avatarClasses(0)}"
								>
									{topAgent.name.charAt(0).toUpperCase()}
								</div>
								<div class="flex-1 min-w-0">
									<div class="text-[15px] font-semibold">Overview · {topAgent.name}</div>
									<div class="text-xs text-base-content/40">{activeAgentCount} agents active · {chatCount} chats</div>
								</div>
								<button
									class="text-xs font-medium text-primary ml-auto shrink-0"
									onclick={() => goto(`/agent/persona/${topAgent.agentId}/chat`)}
								>Open in chat →</button>
							</div>
							<!-- Briefing bullets -->
							{#if agents.length >= 1}
								<div class="flex gap-2.5 py-2 border-t border-base-300 text-[13.5px] leading-relaxed">
									<span class="w-1.5 h-1.5 rounded-full bg-success mt-2 shrink-0"></span>
									<div><strong>{agents.length}</strong> {agents.length === 1 ? 'agent is' : 'agents are'} currently active and responding to messages.</div>
								</div>
							{/if}
							{#if chatCount > 0}
								<div class="flex gap-2.5 py-2 border-t border-base-300 text-[13.5px] leading-relaxed">
									<span class="w-1.5 h-1.5 rounded-full bg-warning mt-2 shrink-0"></span>
									<div><strong>{chatCount}</strong> recent conversations across your agents.</div>
								</div>
							{/if}
							{#if mcpCount > 0}
								<div class="flex gap-2.5 py-2 border-t border-base-300 text-[13.5px] leading-relaxed">
									<span class="w-1.5 h-1.5 rounded-full bg-primary mt-2 shrink-0"></span>
									<div><strong>{mcpCount}</strong> integrations connected and available.</div>
								</div>
							{/if}
						</div>
					{/if}

					<!-- Running now card -->
					<div class="border border-base-300 rounded-xl bg-base-100 p-4 shadow-sm">
						<div class="flex items-baseline mb-2.5">
							<span class="text-[13.5px] font-semibold">Running now</span>
							<span class="text-xs text-base-content/40 ml-2">Across all agents</span>
							<a href="/agents" class="ml-auto text-xs font-medium text-primary">View all activity →</a>
						</div>
						{#each agents as agent, i (agent.agentId)}
							<div class="grid grid-cols-[20px_1fr_auto_auto] gap-2.5 items-center py-2 {i > 0 ? 'border-t border-base-300' : ''}">
								<div class="w-3 h-3 rounded-full border-2 border-primary/30 border-t-primary animate-spin"></div>
								<div class="min-w-0">
									<div class="text-[13.5px] truncate">{agent.name}</div>
									<div class="text-xs text-base-content/40">Active · {agent.workflowCount} workflows</div>
								</div>
								<div
									class="w-[22px] h-[22px] rounded-md grid place-items-center text-[9px] font-semibold shrink-0 {avatarClasses(i)}"
								>
									{agent.name.charAt(0).toUpperCase()}
								</div>
								<button
									class="text-xs text-base-content/50 px-2 py-1 rounded-md border border-base-300 hover:bg-base-200 transition-colors"
									onclick={() => goto(`/agent/persona/${agent.agentId}/chat`)}
								>Open</button>
							</div>
						{:else}
							<p class="text-xs text-base-content/40 py-3">No agents running</p>
						{/each}
					</div>

					<!-- Stats grid (4 boxes) -->
					<div class="grid grid-cols-2 sm:grid-cols-4 gap-2.5">
						<div class="border border-base-300 rounded-xl bg-base-100 p-3 shadow-sm">
							<div class="text-[22px] font-semibold tracking-tight">{activeAgentCount}</div>
							<div class="text-[11.5px] text-base-content/40 uppercase tracking-wider mt-0.5">Active</div>
						</div>
						<div class="border border-base-300 rounded-xl bg-base-100 p-3 shadow-sm">
							<div class="text-[22px] font-semibold tracking-tight">{chatCount}</div>
							<div class="text-[11.5px] text-base-content/40 uppercase tracking-wider mt-0.5">Chats</div>
						</div>
						<div class="border border-base-300 rounded-xl bg-base-100 p-3 shadow-sm">
							<div class="text-[22px] font-semibold tracking-tight">{mcpCount}</div>
							<div class="text-[11.5px] text-base-content/40 uppercase tracking-wider mt-0.5">Integrations</div>
						</div>
						<div class="border border-base-300 rounded-xl bg-base-100 p-3 shadow-sm">
							<div class="text-[22px] font-semibold tracking-tight">
								{#if usage?.weekly}
									{Math.round(usage.weekly.percentUsed)}%
								{:else}
									—
								{/if}
							</div>
							<div class="text-[11.5px] text-base-content/40 uppercase tracking-wider mt-0.5">Usage</div>
						</div>
					</div>
				</div>

				<!-- Right column -->
				<div class="flex flex-col gap-4">
					<!-- Needs attention -->
					<div class="border border-base-300 rounded-xl bg-base-100 p-4 shadow-sm">
						<div class="flex items-baseline mb-2.5">
							<span class="text-[13.5px] font-semibold">Needs attention</span>
							<span class="text-xs text-base-content/40 ml-2">{agents.filter(a => a.workflowCount > 0).length}</span>
						</div>
						{#each agents.filter(a => a.workflowCount > 0).slice(0, 5) as agent, i (agent.agentId)}
							<!-- svelte-ignore a11y_no_static_element_interactions -->
							<div
								class="grid grid-cols-[24px_1fr] gap-2.5 items-center py-2 cursor-pointer {i > 0 ? 'border-t border-base-300' : ''}"
								onclick={() => goto(`/agent/persona/${agent.agentId}/chat`)}
							>
								<div
									class="w-6 h-6 rounded-md grid place-items-center text-[10px] font-semibold {avatarClasses(i)}"
								>
									{agent.name.charAt(0).toUpperCase()}
								</div>
								<div class="min-w-0">
									<div class="text-[13.5px] truncate">{agent.name}</div>
									<div class="text-xs text-base-content/40">{agent.workflowCount} workflows · {agent.skillCount} skills</div>
								</div>
							</div>
						{:else}
							<p class="text-xs text-base-content/40 py-3">Nothing needs attention right now</p>
						{/each}
					</div>

					<!-- Recent chats -->
					<div class="border border-base-300 rounded-xl bg-base-100 p-4 shadow-sm">
						<div class="flex items-baseline mb-2.5">
							<span class="text-[13.5px] font-semibold">Recent chats</span>
							<a href="/agents" class="ml-auto text-xs font-medium text-primary">View all →</a>
						</div>
						{#each chats.slice(0, 5) as chat, i (chat.id)}
							<!-- svelte-ignore a11y_no_static_element_interactions -->
							<div
								class="grid grid-cols-[24px_1fr] gap-2.5 items-center py-2 cursor-pointer hover:bg-base-200/30 rounded-lg transition-colors {i > 0 ? 'border-t border-base-300' : ''}"
								onclick={() => goto(`/agent/${chat.id}`)}
							>
								<div
									class="w-6 h-6 rounded-md grid place-items-center text-[10px] font-semibold {avatarClasses(i)}"
								>
									{(chat.title || 'U').charAt(0).toUpperCase()}
								</div>
								<div class="min-w-0">
									<div class="text-[13.5px] font-medium truncate">{chat.title || 'Untitled'}</div>
									<div class="text-xs text-base-content/40">
										{#if chat.updatedAt}
											{new Date(chat.updatedAt).toLocaleDateString()}
										{/if}
									</div>
								</div>
							</div>
						{:else}
							<p class="text-xs text-base-content/40 py-3">No recent chats</p>
						{/each}
					</div>

					<!-- Quick actions -->
					<div class="border border-base-300 rounded-xl bg-base-100 p-4 shadow-sm">
						<div class="flex items-baseline mb-2.5">
							<span class="text-[13.5px] font-semibold">Quick actions</span>
						</div>
						<!-- svelte-ignore a11y_no_static_element_interactions -->
						<div class="flex items-center gap-2.5 py-2.5 cursor-pointer hover:bg-base-200/30 rounded-lg transition-colors" onclick={() => goto('/marketplace')}>
							<Store class="w-3.5 h-3.5 text-base-content/30 shrink-0" />
							<span class="text-[13.5px]">Marketplace</span>
							<span class="text-xs text-base-content/40 ml-auto">Browse agents & skills</span>
						</div>
						<!-- svelte-ignore a11y_no_static_element_interactions -->
						<div class="flex items-center gap-2.5 py-2.5 border-t border-base-300 cursor-pointer hover:bg-base-200/30 rounded-lg transition-colors" onclick={() => goto('/integrations')}>
							<Link2 class="w-3.5 h-3.5 text-base-content/30 shrink-0" />
							<span class="text-[13.5px]">Integrations</span>
							<span class="text-xs text-base-content/40 ml-auto">{mcpCount} connected</span>
						</div>
						<!-- svelte-ignore a11y_no_static_element_interactions -->
						<div class="flex items-center gap-2.5 py-2.5 border-t border-base-300 cursor-pointer hover:bg-base-200/30 rounded-lg transition-colors" onclick={() => goto('/settings/account')}>
							<Settings class="w-3.5 h-3.5 text-base-content/30 shrink-0" />
							<span class="text-[13.5px]">Settings</span>
							<span class="text-xs text-base-content/40 ml-auto">Account & preferences</span>
						</div>
					</div>
				</div>
			</div>
		{/if}
	</div>
</div>
