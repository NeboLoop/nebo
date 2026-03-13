<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import {
		Zap,
		RefreshCw,
		Clock,
		ChevronRight,
		Activity,
		Filter
	} from 'lucide-svelte';

	interface NeboEvent {
		id: string;
		type: string;
		source: string;
		payload: string;
		created_at: string;
	}

	let events = $state<NeboEvent[]>([]);
	let isLoading = $state(true);
	let selectedType = $state<string>('all');
	let pollInterval: ReturnType<typeof setInterval> | null = null;

	const eventTypes = $derived([
		'all',
		...Array.from(new Set(events.map(e => e.type)))
	]);

	const filtered = $derived(
		selectedType === 'all' ? events : events.filter(e => e.type === selectedType)
	);

	onMount(async () => {
		await load();
		pollInterval = setInterval(load, 5000);
	});

	onDestroy(() => {
		if (pollInterval) clearInterval(pollInterval);
	});

	async function load() {
		// Events endpoint — placeholder until event bus is built
		// For now show empty state with the right structure
		isLoading = false;
	}

	function formatDate(ts: string): string {
		const d = new Date(ts);
		return d.toLocaleDateString([], { month: 'short', day: 'numeric' }) + ' ' +
			d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
	}

	function eventColor(type: string): string {
		if (type.startsWith('workflow')) return 'text-primary';
		if (type.startsWith('tool')) return 'text-info';
		if (type.startsWith('agent')) return 'text-success';
		if (type.startsWith('error') || type.startsWith('fail')) return 'text-error';
		return 'text-base-content/70';
	}
</script>

<!-- Header -->
<div class="mb-6 flex items-center justify-between">
	<div>
		<h2 class="font-display text-xl font-bold text-base-content mb-1">Events</h2>
		<p class="text-sm text-base-content/70">System events — workflow triggers, tool completions, agent activity</p>
	</div>
	<button class="btn btn-ghost btn-sm" onclick={load} disabled={isLoading}>
		<RefreshCw class="w-4 h-4 {isLoading ? 'animate-spin' : ''}" />
		Refresh
	</button>
</div>

{#if isLoading}
	<Card>
		<div class="py-12 text-center text-base-content/70">
			<span class="loading loading-spinner loading-md"></span>
		</div>
	</Card>
{:else if events.length > 0}
	<!-- Type filter -->
	<div class="flex items-center gap-2 mb-4 flex-wrap">
		<Filter class="w-3.5 h-3.5 text-base-content/70" />
		{#each eventTypes as type}
			<button
				class="badge cursor-pointer transition-colors {selectedType === type ? 'badge-primary' : 'badge-ghost hover:badge-outline'}"
				onclick={() => selectedType = type}
			>
				{type}
			</button>
		{/each}
	</div>

	<div class="rounded-xl bg-base-100 ring-1 ring-base-content/5 overflow-hidden">
		{#each filtered as event, i}
			<div class="flex items-start gap-3 px-4 py-3 {i > 0 ? 'border-t border-base-content/5' : ''}">
				<Zap class="w-3.5 h-3.5 mt-0.5 shrink-0 {eventColor(event.type)}" />
				<div class="flex-1 min-w-0">
					<div class="flex items-center gap-2 mb-0.5">
						<span class="text-xs font-mono font-medium {eventColor(event.type)}">{event.type}</span>
						<span class="text-[10px] text-base-content/70">{event.source}</span>
					</div>
					{#if event.payload}
						<pre class="text-[10px] text-base-content/70 font-mono leading-relaxed truncate">{event.payload}</pre>
					{/if}
				</div>
				<span class="text-[10px] text-base-content/70 tabular-nums shrink-0 mt-0.5">{formatDate(event.created_at)}</span>
			</div>
		{/each}
	</div>
{:else}
	<Card>
		<div class="py-16 text-center text-base-content/70">
			<Activity class="w-12 h-12 mx-auto mb-4 opacity-20" />
			<p class="font-medium mb-2">No events yet</p>
			<p class="text-sm text-base-content/70">Events will appear here as workflows run, tools execute, and agents act.</p>
			<p class="text-xs text-base-content/70 mt-4">Event bus coming in a future release.</p>
		</div>
	</Card>
{/if}
