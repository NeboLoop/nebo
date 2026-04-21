<!--
  Home — Composer-first layout matching V2 design (page_home.jsx).
  Greeting, large composer with agent picker, starter chips, recent chats grid.
-->

<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { goto } from '$app/navigation';
	import { getActiveAgents, listChats, getUserProfile, listAgents } from '$lib/api/nebo';
	import { getWebSocketClient } from '$lib/websocket/client';
	import type { ActiveAgentEntry, Chat } from '$lib/api/neboComponents';
	import { ChevronDown, ArrowUp, Plus, Zap } from 'lucide-svelte';

	let agents = $state<ActiveAgentEntry[]>([]);
	let allAgents = $state<{ id: string; name: string }[]>([]);
	let chats = $state<Chat[]>([]);
	let isLoading = $state(true);
	let userName = $state('');
	let composerText = $state('');
	let pickerOpen = $state(false);
	let selectedAgentIndex = $state(0);

	const COLORS = ['violet', 'green', 'sky', 'amber', 'rose', 'mint', 'slate', 'peach', 'lilac'];
	function colorFor(i: number) { return COLORS[i % COLORS.length]; }

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
		if (h < 5) return 'Still up';
		if (h < 12) return 'Good morning';
		if (h < 18) return 'Good afternoon';
		return 'Good evening';
	});

	// Selected agent for the picker
	const selectedAgent = $derived(allAgents[selectedAgentIndex] ?? { id: '', name: 'Assistant' });
	const selectedInitial = $derived((selectedAgent.name || 'A').charAt(0).toUpperCase());

	const starterPrompts = [
		'Summarize my morning emails',
		'Draft a follow-up to yesterday\'s thread',
		'Compare these two contracts for me',
		'Build a target account list for Q3',
	];

	function timeAgo(dateStr?: string): string {
		if (!dateStr) return '';
		const diff = Date.now() - new Date(dateStr).getTime();
		const mins = Math.floor(diff / 60000);
		if (mins < 1) return 'Just now';
		if (mins < 60) return `${mins}m ago`;
		const hrs = Math.floor(mins / 60);
		if (hrs < 24) return `${hrs}h ago`;
		const days = Math.floor(hrs / 24);
		return `${days}d ago`;
	}

	async function load() {
		const [r, c, p, a] = await Promise.all([
			getActiveAgents().catch(() => ({ agents: [], count: 0 })),
			listChats({ pageSize: 10 }).catch(() => ({ chats: [], total: 0 })),
			getUserProfile().catch(() => ({ profile: null })),
			listAgents().catch(() => ({ agents: [] })),
		]);
		agents = r?.agents ?? [];
		chats = c?.chats ?? [];
		userName = p?.profile?.displayName || '';
		// Build all-agents list: assistant first, then DB agents
		const dbAgents = (a?.agents ?? []).map((ag: { id: string; name: string }) => ({ id: ag.id, name: ag.name }));
		allAgents = [{ id: '', name: 'Assistant' }, ...dbAgents];
		isLoading = false;
	}

	function selectAgent(index: number) {
		selectedAgentIndex = index;
		pickerOpen = false;
	}

	function sendMessage() {
		if (!composerText.trim()) return;
		const agent = selectedAgent;
		if (agent.id) {
			goto(`/agent/persona/${agent.id}/chat`);
		} else {
			goto('/agent/assistant/chat');
		}
	}

	function usePrompt(prompt: string) {
		composerText = prompt;
	}

	function handleComposerKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && !e.shiftKey) {
			e.preventDefault();
			sendMessage();
		}
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

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="flex-1 overflow-auto" onclick={() => { pickerOpen = false; }}>
	<div class="flex flex-col items-center pt-[60px] px-7 pb-[60px] min-h-full">
		<div class="w-full max-w-[640px] flex flex-col gap-6">
			{#if isLoading}
				<div class="flex items-center justify-center py-20">
					<span class="loading loading-spinner loading-lg"></span>
				</div>
			{:else}
				<!-- Greeting -->
				<h1 class="text-[28px] font-medium tracking-[-0.5px] text-center text-base-content">
					{greeting()}, <span class="text-primary">{userName || 'there'}</span>. What should we tackle?
				</h1>

				<!-- Composer -->
				<div class="relative">
				<div class="bg-white border border-base-300 rounded-[18px] shadow-md focus-within:border-base-300 transition-colors">
					<div class="px-4 pt-4 pb-3">
						<textarea
							bind:value={composerText}
							onkeydown={handleComposerKeydown}
							rows="2"
							placeholder="Ask anything, or describe a task…"
							class="w-full bg-transparent border-none outline-none resize-none text-[15.5px] leading-[1.55] placeholder:text-base-content/40 min-h-[52px]"
						></textarea>
					</div>
					<div class="flex items-center gap-1.5 mt-1.5 px-4 pb-3 text-base-content/60">
						<button class="w-8 h-8 rounded-lg grid place-items-center text-base-content/60 hover:text-base-content transition-colors" title="Attach">
							<Plus class="w-4 h-4" />
						</button>
						<button class="w-8 h-8 rounded-lg grid place-items-center text-base-content/60 hover:text-base-content transition-colors" title="Tools">
							<Zap class="w-[15px] h-[15px]" />
						</button>
						<div class="relative ml-auto">
							<!-- Agent picker (inside composer) -->
							<button
								class="inline-flex items-center gap-1.5 py-[5px] pl-[7px] pr-2.5 rounded-lg border border-base-300 bg-white text-base-content/60 text-[12.5px] cursor-pointer hover:bg-base-200 transition-colors"
								onclick={(e) => { e.stopPropagation(); pickerOpen = !pickerOpen; }}
							>
								<span class="w-[18px] h-[18px] rounded grid place-items-center text-[8px] font-bold {avatarClasses(selectedAgentIndex)}">
									{selectedInitial}
								</span>
								{selectedAgent.name}
								<ChevronDown class="w-[11px] h-[11px]" />
							</button>

							{#if pickerOpen}
								<!-- svelte-ignore a11y_no_static_element_interactions -->
								<div class="fixed inset-0 z-[19]" onclick={(e) => { e.stopPropagation(); pickerOpen = false; }}></div>
								<div class="absolute bottom-[calc(100%+6px)] right-0 w-[260px] bg-white border border-base-300 rounded-xl shadow-lg z-20 p-1.5 max-h-[340px] overflow-y-auto">
									<div class="text-[10.5px] font-bold tracking-[1px] text-base-content/40 uppercase px-2 py-1.5">Select agent</div>
									{#each allAgents as agent, i}
										<button
											class="w-full grid grid-cols-[24px_1fr_auto] gap-2.5 items-center px-2 py-[7px] text-left rounded-[7px] cursor-pointer hover:bg-base-200 transition-colors {i === selectedAgentIndex ? 'bg-primary/5' : ''}"
											onclick={(e) => { e.stopPropagation(); selectAgent(i); }}
										>
											<span class="w-[22px] h-[22px] rounded-[5px] grid place-items-center text-[9px] font-bold shrink-0 {avatarClasses(i)}">
												{agent.name.charAt(0).toUpperCase()}
											</span>
											<div class="min-w-0">
												<div class="text-[13px] truncate">{agent.name}</div>
											</div>
											{#if i === selectedAgentIndex}
												<span class="text-primary">&#10003;</span>
											{/if}
										</button>
									{/each}
								</div>
							{/if}
						</div>
						<button
							onclick={sendMessage}
							disabled={!composerText.trim()}
							class="w-[34px] h-[34px] rounded-[10px] grid place-items-center transition-all {composerText.trim() ? 'bg-primary text-white' : 'bg-base-300 text-white'}"
						>
							<ArrowUp class="w-[15px] h-[15px]" />
						</button>
					</div>
				</div>
				</div>

				<!-- Starter chips -->
				<div class="flex flex-wrap gap-2 justify-center">
					{#each starterPrompts as prompt}
						<button
							class="px-3 py-[7px] rounded-full border border-base-300 bg-white text-[12.5px] text-base-content/60 cursor-pointer hover:border-base-300 hover:text-base-content transition-colors"
							onclick={() => usePrompt(prompt)}
						>
							{prompt}
						</button>
					{/each}
				</div>

				<!-- Recent chats -->
				{#if chats.length > 0}
					<div class="mt-3">
						<div class="flex items-baseline mb-3">
							<span class="text-[13px] font-semibold text-base-content/60">Recent chats</span>
							<a href="/marketplace/agents" class="ml-auto text-xs font-medium text-base-content/40 cursor-pointer">Browse →</a>
						</div>
						<div class="grid grid-cols-2 gap-2.5">
							{#each chats.slice(0, 4) as chat, i (chat.id)}
								<button
									class="text-left py-3 px-3.5 rounded-xl border border-base-300 hover:border-base-300 bg-white transition-colors flex flex-col gap-1.5 cursor-pointer"
									onclick={() => goto(`/agent/${chat.id}`)}
								>
									<div class="text-[13.5px] font-medium text-base-content truncate">{chat.title || 'Untitled'}</div>
									<div class="text-xs text-base-content/60 leading-[1.45] line-clamp-2">{chat.lastMessage || '—'}</div>
									<div class="flex items-center gap-2 text-[11.5px] text-base-content/40">
										<span class="w-3.5 h-3.5 rounded-[3px] grid place-items-center text-[8.5px] font-bold shrink-0 {avatarClasses(i)}">
											{(chat.agentName || 'A').charAt(0).toUpperCase()}
										</span>
										<span>{chat.agentName || 'Assistant'}</span>
										<span class="opacity-50">·</span>
										<span>{timeAgo(chat.updatedAt)}</span>
									</div>
								</button>
							{/each}
						</div>
					</div>
				{/if}
			{/if}
		</div>
	</div>
</div>
