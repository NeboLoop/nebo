<script lang="ts">
	import { onMount } from 'svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import { Package, RefreshCw, Store, Download, Check, WifiOff, Star, ExternalLink, Loader2 } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type { PluginItem, AppItem } from '$lib/api/nebo';
	import AppDetailModal from './AppDetailModal.svelte';

	let plugins = $state<PluginItem[]>([]);
	let storeApps = $state<AppItem[]>([]);
	let neboLoopConnected = $state(false);
	let isLoading = $state(true);
	let isLoadingStore = $state(false);
	let togglingPlugin = $state<string | null>(null);
	let installingApp = $state<string | null>(null);
	let openingApp = $state<string | null>(null);

	let activeTab = $state('installed');

	// Modal state (store apps only)
	let selectedPlugin = $state<PluginItem | null>(null);
	let selectedAppItem = $state<AppItem | null>(null);
	let showModal = $state(false);

	onMount(async () => {
		await loadAll();
	});

	async function loadAll() {
		isLoading = true;
		try {
			const [pluginsResp, loopStatus] = await Promise.all([
				api.listPlugins(),
				api.neboLoopStatus()
			]);
			plugins = (pluginsResp.plugins || []).filter(p => p.pluginType === 'app');
			neboLoopConnected = loopStatus.connected;

			if (neboLoopConnected) {
				loadStoreApps();
			}
		} catch (error) {
			console.error('Failed to load apps:', error);
		} finally {
			isLoading = false;
		}
	}

	async function loadStoreApps() {
		isLoadingStore = true;
		try {
			const resp = await api.listStoreApps();
			storeApps = resp.apps || [];
		} catch (error) {
			console.error('Failed to load store apps:', error);
		} finally {
			isLoadingStore = false;
		}
	}

	async function handleToggle(event: Event, plugin: PluginItem) {
		event.stopPropagation();
		togglingPlugin = plugin.id;
		try {
			await api.togglePlugin({ isEnabled: !plugin.isEnabled }, plugin.id);
			await loadAll();
		} catch (error) {
			console.error('Failed to toggle plugin:', error);
		} finally {
			togglingPlugin = null;
		}
	}

	async function handleOpenUI(event: Event, plugin: PluginItem) {
		event.stopPropagation();
		if (!plugin.appId) return;
		openingApp = plugin.id;
		try {
			const resp = await api.openAppUI(plugin.appId);
			if (!resp.opened && resp.url) {
				window.open(resp.url, `nebo-app-${plugin.appId}`);
			}
		} catch (error) {
			console.error('Failed to open app UI:', error);
		} finally {
			openingApp = null;
		}
	}

	async function handleInstall(app: AppItem) {
		installingApp = app.id;
		try {
			await api.installStoreApp(app.id);
			await loadAll();
		} catch (error) {
			console.error('Failed to install app:', error);
		} finally {
			installingApp = null;
		}
	}

	async function handleUninstall(app: AppItem) {
		installingApp = app.id;
		try {
			await api.uninstallStoreApp(app.id);
			await loadAll();
		} catch (error) {
			console.error('Failed to uninstall app:', error);
		} finally {
			installingApp = null;
		}
	}

	function openDetail(plugin: PluginItem) {
		selectedPlugin = plugin;
		selectedAppItem = null;
		showModal = true;
	}

	function openStoreDetail(app: AppItem) {
		selectedAppItem = app;
		selectedPlugin = null;
		showModal = true;
	}

	function getStatusBadgeClass(status: string): string {
		switch (status) {
			case 'connected': return 'bg-success/10 text-success';
			case 'error': return 'bg-error/10 text-error';
			default: return 'bg-base-content/10 text-base-content/90';
		}
	}

	function getInitial(name: string): string {
		return (name || '?').charAt(0).toUpperCase();
	}

	function hasUI(plugin: PluginItem): boolean {
		return (plugin.capabilities?.includes('ui') ?? false) && !!plugin.appId;
	}

	const installedAppIds = $derived(new Set(plugins.map(p => p.name)));

</script>

<div class="mb-6 flex items-center justify-between">
	<div>
		<h2 class="font-display text-xl font-bold text-base-content mb-1">Apps</h2>
		<p class="text-base text-base-content/80">Installed apps and the marketplace</p>
	</div>
	<button
		class="h-8 px-3 rounded-lg bg-base-content/5 border border-base-content/10 text-sm font-medium text-base-content/60 hover:border-base-content/40 hover:text-base-content transition-colors flex items-center gap-1.5"
		onclick={loadAll}
	>
		<RefreshCw class="w-3.5 h-3.5" />
		Refresh
	</button>
</div>

{#if isLoading}
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 py-12 text-center text-base-content/90">
		<Spinner class="w-5 h-5 mx-auto mb-2" />
		<p class="text-base">Loading apps...</p>
	</div>
{:else}
	<div class="flex gap-1 mb-2">
		<button
			class="h-8 px-3 rounded-lg text-sm font-medium transition-colors {activeTab === 'installed' ? 'bg-base-content/10 text-base-content' : 'text-base-content/80 hover:text-base-content/60'}"
			onclick={() => activeTab = 'installed'}
		>
			Installed{plugins.length ? ` (${plugins.length})` : ''}
		</button>
		<button
			class="h-8 px-3 rounded-lg text-sm font-medium transition-colors {activeTab === 'store' ? 'bg-base-content/10 text-base-content' : 'text-base-content/80 hover:text-base-content/60'}"
			onclick={() => activeTab = 'store'}
		>
			Marketplace
		</button>
	</div>

	<div class="mt-4">
		{#if activeTab === 'installed'}
			{#if plugins.length > 0}
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 divide-y divide-base-content/10">
					{#each plugins as plugin}
						<button
							type="button"
							class="w-full flex items-center gap-4 p-4 text-left hover:bg-base-content/5 transition-colors first:rounded-t-2xl last:rounded-b-2xl"
							onclick={() => openDetail(plugin)}
						>
							<div class="w-11 h-11 rounded-xl bg-primary/10 flex items-center justify-center shrink-0">
								{#if plugin.icon}
									<img src={plugin.icon} alt={plugin.displayName} class="w-7 h-7 rounded" />
								{:else}
									<span class="text-lg font-bold text-primary">{getInitial(plugin.displayName)}</span>
								{/if}
							</div>
							<div class="flex-1 min-w-0">
								<div class="flex items-center gap-2 mb-0.5">
									<h3 class="font-display font-bold text-base text-base-content">{plugin.displayName || plugin.name}</h3>
									<span class="text-sm font-medium px-1.5 py-0.5 rounded bg-base-content/10 text-base-content/60">v{plugin.version}</span>
									<span class="text-sm font-semibold uppercase px-1.5 py-0.5 rounded {getStatusBadgeClass(plugin.connectionStatus)}">
										{plugin.connectionStatus}
									</span>
								</div>
								<p class="text-base text-base-content/80 truncate">{plugin.description}</p>
							</div>
							<div class="flex items-center gap-2 shrink-0" onclick={(e) => e.stopPropagation()}>
								{#if hasUI(plugin)}
									<button
										class="h-7 px-2.5 rounded-md bg-primary text-primary-content text-sm font-semibold flex items-center gap-1 hover:brightness-110 transition-all disabled:opacity-50"
										onclick={(e) => handleOpenUI(e, plugin)}
										disabled={openingApp === plugin.id}
										title="Open app"
									>
										{#if openingApp === plugin.id}
											<Loader2 class="w-3 h-3 animate-spin" />
										{:else}
											<ExternalLink class="w-3.5 h-3.5" />
											Open
										{/if}
									</button>
								{/if}
								<input
									type="checkbox"
									class="toggle toggle-primary toggle-sm"
									checked={plugin.isEnabled}
									disabled={togglingPlugin === plugin.id}
									onchange={(e) => handleToggle(e, plugin)}
								/>
							</div>
						</button>
					{/each}
				</div>
			{:else}
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 py-12 text-center text-base-content/90">
					<Package class="w-12 h-12 mx-auto mb-4 opacity-20" />
					<p class="font-medium mb-2">No apps installed</p>
					<p class="text-base">Browse the Marketplace to get started.</p>
				</div>
			{/if}
		{:else if activeTab === 'store'}
			{#if !neboLoopConnected}
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 py-8 text-center text-base-content/90">
					<WifiOff class="w-10 h-10 mx-auto mb-3 opacity-20" />
					<p class="font-medium mb-2">Connect to NeboLoop to browse the Marketplace</p>
					<a href="/settings/status" class="inline-flex h-8 px-4 rounded-full bg-primary text-primary-content text-base font-bold items-center hover:brightness-110 transition-all mt-2">
						Go to Status
					</a>
				</div>
			{:else if isLoadingStore}
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 py-8 text-center text-base-content/90">
					<Spinner class="w-5 h-5 mx-auto mb-2" />
					<p class="text-base">Loading store...</p>
				</div>
			{:else if storeApps.length > 0}
				<div class="grid sm:grid-cols-2 lg:grid-cols-3 gap-3">
					{#each storeApps as app}
						<button
							type="button"
							class="text-left rounded-xl bg-base-100 p-4 shadow-sm ring-1 ring-base-content/5 transition-all hover:shadow-md hover:ring-primary/20"
							onclick={() => openStoreDetail(app)}
						>
							<div class="flex items-start gap-3">
								<div class="w-10 h-10 rounded-xl bg-base-200 flex items-center justify-center shrink-0">
									{#if app.icon}
										<img src={app.icon} alt={app.name} class="w-8 h-8 rounded" />
									{:else}
										<Store class="w-5 h-5 text-base-content/90" />
									{/if}
								</div>
								<div class="flex-1 min-w-0">
									<h3 class="font-display font-bold text-base text-base-content mb-0.5">{app.name}</h3>
									<p class="text-sm text-base-content/60 mb-1">
										by {app.author.name}
										{#if app.author.verified}
											<Check class="w-2.5 h-2.5 inline text-success" />
										{/if}
									</p>
									<p class="text-base text-base-content/80 mb-3 line-clamp-2">{app.description}</p>

									<div class="flex items-center justify-between">
										<div class="flex items-center gap-3 text-sm text-base-content/60">
											{#if app.rating > 0}
												<span class="flex items-center gap-0.5">
													<Star class="w-3 h-3" />
													{app.rating.toFixed(1)}
												</span>
											{/if}
											{#if app.installCount > 0}
												<span class="flex items-center gap-0.5">
													<Download class="w-3 h-3" />
													{app.installCount}
												</span>
											{/if}
										</div>

										{#if app.isInstalled || installedAppIds.has(app.slug)}
											<button
												class="h-7 px-2.5 rounded-md bg-success/10 text-success text-sm font-semibold flex items-center gap-1 hover:bg-success/20 transition-colors disabled:opacity-50"
												onclick={(e) => { e.stopPropagation(); handleUninstall(app); }}
												disabled={installingApp === app.id}
											>
												{#if installingApp === app.id}
													<Loader2 class="w-3 h-3 animate-spin" />
												{:else}
													<Check class="w-3 h-3" />
													Installed
												{/if}
											</button>
										{:else}
											<button
												class="h-7 px-2.5 rounded-md bg-primary text-primary-content text-sm font-semibold flex items-center gap-1 hover:brightness-110 transition-all disabled:opacity-50"
												onclick={(e) => { e.stopPropagation(); handleInstall(app); }}
												disabled={installingApp === app.id}
											>
												{#if installingApp === app.id}
													<Loader2 class="w-3 h-3 animate-spin" />
												{:else}
													Install
												{/if}
											</button>
										{/if}
									</div>
								</div>
							</div>
						</button>
					{/each}
				</div>
			{:else}
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 py-8 text-center text-base-content/90">
					<Store class="w-10 h-10 mx-auto mb-3 opacity-20" />
					<p class="font-medium mb-1">No apps available yet</p>
					<p class="text-base">Check back later for new apps.</p>
				</div>
			{/if}
		{/if}
	</div>
{/if}

<AppDetailModal
	plugin={selectedPlugin}
	storeApp={selectedAppItem}
	bind:show={showModal}
	onclose={() => { selectedPlugin = null; selectedAppItem = null; }}
	onupdated={loadAll}
/>
