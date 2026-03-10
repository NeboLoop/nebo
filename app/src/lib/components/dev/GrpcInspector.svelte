<!--
  GrpcInspector — Live gRPC traffic viewer for dev apps.
  Shows every request, response, and streaming message between Nebo and the app.
-->

<script lang="ts">
	import { onDestroy } from 'svelte';
	import { Activity, Pause, Play, Trash2, ArrowDown, ChevronDown } from 'lucide-svelte';

	interface Props {
		appId: string;
	}

	let { appId }: Props = $props();

	interface GrpcEvent {
		id: number;
		timestamp: string;
		appId: string;
		method: string;
		type: string;
		direction: string;
		payload: any;
		durationMs?: number;
		error?: string;
		streamSeq?: number;
	}

	let events = $state<GrpcEvent[]>([]);
	let paused = $state(false);
	let autoScroll = $state(true);
	let filterText = $state('');
	let expandedId = $state<number | null>(null);
	let eventSource: EventSource | null = null;
	let listContainer: HTMLDivElement;

	const filteredEvents = $derived(
		filterText
			? events.filter(e => e.method.toLowerCase().includes(filterText.toLowerCase()))
			: events
	);

	// Connect/reconnect when appId changes
	$effect(() => {
		if (appId) {
			connect(appId);
		}
	});

	// Auto-scroll when new events arrive
	$effect(() => {
		if (autoScroll && listContainer && filteredEvents.length > 0) {
			requestAnimationFrame(() => {
				if (listContainer && autoScroll) {
					listContainer.scrollTo({ top: listContainer.scrollHeight, behavior: 'instant' });
				}
			});
		}
	});

	function connect(id: string) {
		disconnect();
		events = [];
		paused = false;

		eventSource = new EventSource(`/api/v1/dev/apps/${id}/grpc`);
		eventSource.onmessage = (e) => {
			if (!paused) {
				try {
					const parsed: GrpcEvent = JSON.parse(e.data);
					events = [...events, parsed];
					if (events.length > 2000) {
						events = events.slice(-1000);
					}
				} catch {
					// ignore malformed
				}
			}
		};
		eventSource.onerror = () => {
			// EventSource will auto-reconnect
		};
	}

	function disconnect() {
		if (eventSource) {
			eventSource.close();
			eventSource = null;
		}
	}

	function clearEvents() {
		events = [];
	}

	function togglePause() {
		paused = !paused;
	}

	function toggleExpand(id: number) {
		expandedId = expandedId === id ? null : id;
	}

	function scrollToEnd() {
		if (listContainer) {
			listContainer.scrollTo({ top: listContainer.scrollHeight, behavior: 'smooth' });
			autoScroll = true;
		}
	}

	function handleScroll() {
		if (listContainer) {
			const { scrollTop, scrollHeight, clientHeight } = listContainer;
			autoScroll = scrollHeight - scrollTop - clientHeight < 50;
		}
	}

	function shortMethod(method: string): string {
		const parts = method.split('/');
		if (parts.length >= 3) {
			const svc = parts[1].split('.').pop() ?? parts[1];
			return `${svc}/${parts[2]}`;
		}
		return method;
	}

	function formatDuration(ms?: number): string {
		if (!ms && ms !== 0) return '';
		if (ms < 1) return '<1ms';
		if (ms < 1000) return `${ms}ms`;
		return `${(ms / 1000).toFixed(2)}s`;
	}

	function formatTime(ts: string): string {
		try {
			return new Date(ts).toLocaleTimeString();
		} catch {
			return ts;
		}
	}

	function typeBadge(type: string): string {
		switch (type) {
			case 'unary': return 'U';
			case 'stream_recv': return 'S\u2193';
			case 'stream_send': return 'S\u2191';
			case 'stream_open': return 'S';
			default: return '?';
		}
	}

	onDestroy(() => {
		disconnect();
	});
</script>

<div class="flex flex-col h-full">
	<!-- Controls -->
	<div class="shrink-0 flex items-center gap-2 px-3 py-2 border-b border-base-300 bg-base-100">
		<input
			type="text"
			bind:value={filterText}
			placeholder="Filter by method..."
			class="input input-bordered input-xs flex-1 min-w-0"
		/>

		<button
			type="button"
			class="btn btn-xs btn-ghost"
			onclick={togglePause}
			title={paused ? 'Resume' : 'Pause'}
		>
			{#if paused}
				<Play class="w-3.5 h-3.5" />
			{:else}
				<Pause class="w-3.5 h-3.5" />
			{/if}
		</button>
		<button
			type="button"
			class="btn btn-xs btn-ghost"
			onclick={clearEvents}
			title="Clear"
		>
			<Trash2 class="w-3.5 h-3.5" />
		</button>
		{#if !autoScroll}
			<button
				type="button"
				class="btn btn-xs btn-ghost"
				onclick={scrollToEnd}
				title="Scroll to bottom"
			>
				<ArrowDown class="w-3.5 h-3.5" />
			</button>
		{/if}
		<span class="text-xs text-base-content/40 flex-shrink-0">
			{filteredEvents.length} events
		</span>
	</div>

	<!-- Event List -->
	{#if !appId}
		<div class="flex flex-col items-center justify-center flex-1 text-base-content/50 gap-2">
			<Activity class="w-8 h-8" />
			<p class="text-sm">No apps loaded</p>
			<p class="text-xs">Sideload an app to inspect gRPC traffic</p>
		</div>
	{:else if filteredEvents.length === 0}
		<div class="flex flex-col items-center justify-center flex-1 text-base-content/50 gap-2">
			<Activity class="w-8 h-8" />
			<p class="text-sm">No gRPC traffic yet</p>
			<p class="text-xs">Traffic will appear here as your app communicates with Nebo</p>
		</div>
	{:else}
		<div
			bind:this={listContainer}
			onscroll={handleScroll}
			class="dev-grpc-viewer flex-1 min-h-0 bg-base-200"
		>
			{#each filteredEvents as event (event.id)}
				<div class="dev-grpc-row {event.error ? 'dev-grpc-error' : ''}">
					<button
						type="button"
						class="dev-grpc-summary"
						onclick={() => toggleExpand(event.id)}
					>
						<span class="dev-grpc-dir {event.direction === 'request'
							? 'dev-grpc-req' : 'dev-grpc-res'}">
							{event.direction === 'request' ? '\u2192' : '\u2190'}
						</span>
						<span class="dev-grpc-badge dev-grpc-badge-{event.type}">
							{typeBadge(event.type)}
						</span>
						<span class="dev-grpc-method">{shortMethod(event.method)}</span>
						{#if event.streamSeq}
							<span class="dev-grpc-seq">#{event.streamSeq}</span>
						{/if}
						<span class="dev-grpc-spacer"></span>
						{#if event.error}
							<span class="dev-grpc-err-badge">ERR</span>
						{/if}
						{#if event.durationMs}
							<span class="dev-grpc-dur">{formatDuration(event.durationMs)}</span>
						{/if}
						<span class="dev-grpc-time">{formatTime(event.timestamp)}</span>
						<ChevronDown class="w-3 h-3 text-base-content/40 transition-transform
							{expandedId === event.id ? 'rotate-180' : ''}" />
					</button>

					{#if expandedId === event.id}
						<div class="dev-grpc-detail">
							{#if event.error}
								<div class="dev-grpc-error-text">{event.error}</div>
							{/if}
							<pre class="dev-grpc-payload">{JSON.stringify(event.payload, null, 2)}</pre>
						</div>
					{/if}
				</div>
			{/each}
		</div>
	{/if}
</div>
