<script lang="ts">
	import { MessageSquare } from 'lucide-svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import type { Chat } from '$lib/api/neboComponents';

	let {
		chats = [],
		isLoading = true
	}: {
		chats: Chat[];
		isLoading: boolean;
	} = $props();

	function toDate(v: string | number): Date {
		const n = typeof v === 'number' ? v : Number(v);
		return new Date(n < 1e12 ? n * 1000 : n);
	}

	function timeAgo(dateStr: string | number): string {
		const diff = Date.now() - toDate(dateStr).getTime();

		const mins = Math.floor(diff / 60000);
		if (mins < 1) return 'just now';
		if (mins < 60) return `${mins}m ago`;

		const hours = Math.floor(mins / 60);
		if (hours < 24) return `${hours}h ago`;

		const days = Math.floor(hours / 24);
		if (days < 7) return `${days}d ago`;

		return toDate(dateStr).toLocaleDateString();
	}
</script>

<div>
	<div class="dashboard-section-title">Recent Activity</div>

	{#if isLoading}
		<div class="card bg-base-200 border border-base-300">
			<div class="card-body p-0">
				{#each Array(5) as _}
					<div class="flex items-center gap-3 px-5 py-3 border-b border-base-content/5 last:border-b-0">
						<div class="skeleton h-3 w-12"></div>
						<div class="skeleton h-3 w-48"></div>
					</div>
				{/each}
			</div>
		</div>
	{:else if chats.length === 0}
		<div class="card bg-base-200 border border-base-300">
			<EmptyState icon={MessageSquare} title="No recent activity" message="Start a conversation to see it here." />
		</div>
	{:else}
		<div class="card bg-base-200 border border-base-300">
			<div class="card-body p-0 divide-y divide-base-content/10">
				{#each chats as chat (chat.id)}
					<div class="flex items-center gap-3 px-5 py-3 hover:bg-base-content/5 transition-colors">
						<span class="text-sm text-base-content/60 w-16 shrink-0 text-right">{timeAgo(chat.updatedAt)}</span>
						<span class="text-base text-base-content truncate">{chat.title || 'Untitled chat'}</span>
					</div>
				{/each}
			</div>
		</div>
		<div class="mt-2 text-right">
			<a href="/events" class="text-sm text-primary hover:underline">View All &rarr;</a>
		</div>
	{/if}
</div>
