<script lang="ts">
	import { onDestroy } from 'svelte';
	import { Terminal, Pause, Play, Trash2, ArrowDown } from 'lucide-svelte';
	import type * as components from '$lib/api/neboComponents';

	interface Props {
		devApps: components.DevAppItem[];
	}

	let { devApps }: Props = $props();

	let selectedAppId = $state('');
	let selectedStream = $state<'stdout' | 'stderr'>('stdout');
	let logLines = $state<string[]>([]);
	let paused = $state(false);
	let autoScroll = $state(true);
	let filterText = $state('');
	let logContainer: HTMLDivElement;

	let eventSource: EventSource | null = null;

	const filteredLines = $derived(
		filterText
			? logLines.filter(line => line.toLowerCase().includes(filterText.toLowerCase()))
			: logLines
	);

	// Auto-select first app
	$effect(() => {
		if (devApps.length > 0 && !selectedAppId) {
			selectedAppId = devApps[0].appId;
		}
	});

	// Connect/reconnect when app or stream changes
	$effect(() => {
		if (selectedAppId) {
			connectLogs(selectedAppId, selectedStream);
		}
	});

	// Auto-scroll when new lines arrive
	$effect(() => {
		if (autoScroll && logContainer && filteredLines.length > 0) {
			requestAnimationFrame(() => {
				if (logContainer && autoScroll) {
					logContainer.scrollTo({ top: logContainer.scrollHeight, behavior: 'instant' });
				}
			});
		}
	});

	function connectLogs(appId: string, stream: string) {
		disconnectLogs();
		logLines = [];
		paused = false;

		eventSource = new EventSource(`/api/v1/dev/apps/${appId}/logs?stream=${stream}`);
		eventSource.onmessage = (e) => {
			if (!paused) {
				logLines = [...logLines, e.data];
				// Cap at 10000 lines
				if (logLines.length > 10000) {
					logLines = logLines.slice(-5000);
				}
			}
		};
		eventSource.onerror = () => {
			// EventSource will auto-reconnect
		};
	}

	function disconnectLogs() {
		if (eventSource) {
			eventSource.close();
			eventSource = null;
		}
	}

	function clearLogs() {
		logLines = [];
	}

	function togglePause() {
		paused = !paused;
	}

	function scrollToEnd() {
		if (logContainer) {
			logContainer.scrollTo({ top: logContainer.scrollHeight, behavior: 'smooth' });
			autoScroll = true;
		}
	}

	function handleLogScroll() {
		if (logContainer) {
			const { scrollTop, scrollHeight, clientHeight } = logContainer;
			const distanceFromBottom = scrollHeight - scrollTop - clientHeight;
			autoScroll = distanceFromBottom < 50;
		}
	}

	onDestroy(() => {
		disconnectLogs();
	});
</script>

<div class="flex flex-col h-full">
	<!-- Log Controls -->
	<div class="shrink-0 flex items-center gap-2 px-3 py-2 border-b border-base-300 bg-base-100">
		<!-- App Selector -->
		<select
			bind:value={selectedAppId}
			class="select select-bordered select-xs flex-shrink-0"
		>
			{#if devApps.length === 0}
				<option value="">No apps loaded</option>
			{/if}
			{#each devApps as app}
				<option value={app.appId}>{app.name || app.appId}</option>
			{/each}
		</select>

		<!-- Stream Toggle -->
		<div class="btn-group">
			<button
				type="button"
				class="btn btn-xs {selectedStream === 'stdout' ? 'btn-active' : ''}"
				onclick={() => selectedStream = 'stdout'}
			>
				stdout
			</button>
			<button
				type="button"
				class="btn btn-xs {selectedStream === 'stderr' ? 'btn-active' : ''}"
				onclick={() => selectedStream = 'stderr'}
			>
				stderr
			</button>
		</div>

		<!-- Filter -->
		<input
			type="text"
			bind:value={filterText}
			placeholder="Filter..."
			class="input input-bordered input-xs flex-1 min-w-0"
		/>

		<!-- Actions -->
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
			onclick={clearLogs}
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

		<!-- Line count -->
		<span class="text-xs text-base-content/40 flex-shrink-0">
			{filteredLines.length} lines
		</span>
	</div>

	<!-- Log Content -->
	{#if !selectedAppId}
		<div class="flex flex-col items-center justify-center flex-1 text-base-content/50 gap-2">
			<Terminal class="w-8 h-8" />
			<p class="text-sm">No apps loaded</p>
			<p class="text-xs">Sideload an app in Settings &gt; Developer to see logs</p>
		</div>
	{:else if filteredLines.length === 0}
		<div class="flex flex-col items-center justify-center flex-1 text-base-content/50 gap-2">
			<Terminal class="w-8 h-8" />
			<p class="text-sm">No log output yet</p>
			<p class="text-xs">Logs will appear here as your app runs</p>
		</div>
	{:else}
		<div
			bind:this={logContainer}
			onscroll={handleLogScroll}
			class="dev-log-viewer flex-1 min-h-0 bg-base-200"
		>
			{#each filteredLines as line, i (i)}
				<div class="dev-log-line">{line}</div>
			{/each}
		</div>
	{/if}
</div>
