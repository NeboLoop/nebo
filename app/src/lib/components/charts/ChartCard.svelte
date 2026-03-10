<script lang="ts">
	import { RefreshCw, Download, Maximize2, Minimize2, X, AlertCircle } from 'lucide-svelte';
	import type { Snippet } from 'svelte';

	interface Props {
		title: string;
		subtitle?: string;
		description?: string;
		loading?: boolean;
		error?: string;
		lastUpdated?: Date;
		onRefresh?: () => void | Promise<void>;
		onExport?: (format: 'png' | 'csv' | 'json') => void | Promise<void>;
		children: Snippet;
		class?: string;
	}

	let {
		title,
		subtitle,
		description,
		loading = false,
		error,
		lastUpdated,
		onRefresh,
		onExport,
		children,
		class: className = ''
	}: Props = $props();

	let isFullscreen = $state(false);
	let isRefreshing = $state(false);
	let showExportMenu = $state(false);

	async function handleRefresh() {
		if (!onRefresh || isRefreshing) return;
		isRefreshing = true;
		try {
			await onRefresh();
		} finally {
			isRefreshing = false;
		}
	}

	async function handleExport(format: 'png' | 'csv' | 'json') {
		if (!onExport) return;
		showExportMenu = false;
		await onExport(format);
	}

	function toggleFullscreen() {
		isFullscreen = !isFullscreen;
	}

	function formatLastUpdated(date: Date): string {
		const now = new Date();
		const diff = now.getTime() - date.getTime();
		const minutes = Math.floor(diff / 60000);
		const hours = Math.floor(diff / 3600000);
		const days = Math.floor(diff / 86400000);

		if (minutes < 1) return 'Just now';
		if (minutes < 60) return `${minutes}m ago`;
		if (hours < 24) return `${hours}h ago`;
		return `${days}d ago`;
	}
</script>

{#if isFullscreen}
	<!-- Backdrop - rendered first so it's behind the card -->
	<button
		class="fixed inset-0 bg-black/80 backdrop-blur-sm z-40"
		onclick={toggleFullscreen}
		aria-label="Close fullscreen"
	></button>
{/if}

<div
	class="card relative transition-all duration-300 {className} {isFullscreen ? 'fixed! top-4! right-4! bottom-4! left-4! z-50! flex! flex-col! shadow-2xl! animate-in zoom-in-95 duration-200' : ''}"
>
	<!-- Header -->
	<div class="card-header">
		<div class="flex items-start justify-between gap-4">
			<div class="flex-1 min-w-0">
				<h3 class="card-title truncate">{title}</h3>
				{#if subtitle}
					<p class="card-subtitle truncate">{subtitle}</p>
				{/if}
				{#if description}
					<p class="text-xs text-base-500 mt-1">{description}</p>
				{/if}
			</div>

			<div class="flex items-center gap-2 flex-shrink-0">
				{#if lastUpdated}
					<span class="text-xs text-base-500 hidden sm:inline">
						{formatLastUpdated(lastUpdated)}
					</span>
				{/if}

				{#if onRefresh}
					<button
						class="btn btn-ghost btn-sm p-2"
						onclick={handleRefresh}
						disabled={isRefreshing || loading}
						title="Refresh"
						aria-label="Refresh chart"
					>
						<RefreshCw class="w-4 h-4 {isRefreshing ? 'animate-spin' : ''}" />
					</button>
				{/if}

				{#if onExport}
					<div class="dropdown dropdown-end">
						<button
							class="btn btn-ghost btn-sm p-2"
							onclick={() => showExportMenu = !showExportMenu}
							title="Export"
							aria-label="Export chart"
						>
							<Download class="w-4 h-4" />
						</button>

						{#if showExportMenu}
							<ul
								class="dropdown-menu mt-2 w-36"
								onmouseleave={() => showExportMenu = false}
								role="menu"
								tabindex="-1"
							>
								<li>
									<button
										class="dropdown-item"
										onclick={() => handleExport('png')}
									>
										Export as PNG
									</button>
								</li>
								<li>
									<button
										class="dropdown-item"
										onclick={() => handleExport('csv')}
									>
										Export as CSV
									</button>
								</li>
								<li>
									<button
										class="dropdown-item"
										onclick={() => handleExport('json')}
									>
										Export as JSON
									</button>
								</li>
							</ul>
						{/if}
					</div>
				{/if}

				<button
					class="btn btn-ghost btn-sm p-2"
					onclick={toggleFullscreen}
					title={isFullscreen ? 'Exit fullscreen' : 'Fullscreen'}
					aria-label={isFullscreen ? 'Exit fullscreen' : 'Enter fullscreen'}
				>
					{#if isFullscreen}
						<Minimize2 class="w-4 h-4" />
					{:else}
						<Maximize2 class="w-4 h-4" />
					{/if}
				</button>

				{#if isFullscreen}
					<button
						class="btn btn-ghost btn-sm p-2"
						onclick={toggleFullscreen}
						title="Close"
						aria-label="Close fullscreen"
					>
						<X class="w-4 h-4" />
					</button>
				{/if}
			</div>
		</div>
	</div>

	<!-- Body -->
	<div class="card-body {isFullscreen ? 'flex-1 overflow-auto' : ''}">
		{#if error}
			<div class="alert alert-error flex items-start gap-3">
				<AlertCircle class="w-5 h-5 text-error flex-shrink-0 mt-0.5" />
				<div class="flex-1">
					<p class="font-medium text-error">Error loading chart</p>
					<p class="text-sm text-error/80">{error}</p>
				</div>
				{#if onRefresh}
					<button
						class="btn btn-secondary btn-sm"
						onclick={handleRefresh}
					>
						Retry
					</button>
				{/if}
			</div>
		{:else if loading}
			<div class="flex flex-col items-center justify-center py-12">
				<span class="loading loading-spinner loading-lg text-primary mb-4"></span>
				<p class="text-sm text-base-500">Loading chart data...</p>
			</div>
		{:else}
			{@render children()}
		{/if}
	</div>
</div>
