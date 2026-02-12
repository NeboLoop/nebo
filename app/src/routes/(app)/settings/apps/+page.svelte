<script lang="ts">
	import { onMount } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import { Package, RefreshCw, Store, Download, Check, WifiOff, Star } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type { PluginItem, StoreApp } from '$lib/api/nebo';
	import AppDetailModal from './AppDetailModal.svelte';

	let plugins = $state<PluginItem[]>([]);
	let storeApps = $state<StoreApp[]>([]);
	let neboLoopConnected = $state(false);
	let isLoading = $state(true);
	let isLoadingStore = $state(false);
	let togglingPlugin = $state<string | null>(null);
	let installingApp = $state<string | null>(null);

	// Modal state
	let selectedPlugin = $state<PluginItem | null>(null);
	let selectedStoreApp = $state<StoreApp | null>(null);
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

	async function handleInstall(app: StoreApp) {
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

	async function handleUninstall(app: StoreApp) {
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
		selectedStoreApp = null;
		showModal = true;
	}

	function openStoreDetail(app: StoreApp) {
		selectedStoreApp = app;
		selectedPlugin = null;
		showModal = true;
	}

	function getStatusBadgeClass(status: string): string {
		switch (status) {
			case 'connected': return 'badge-success';
			case 'disconnected': return 'badge-ghost';
			case 'error': return 'badge-error';
			default: return 'badge-ghost';
		}
	}

	function getInitial(name: string): string {
		return (name || '?').charAt(0).toUpperCase();
	}

	const installedAppIds = $derived(new Set(plugins.map(p => p.name)));
</script>

<div class="mb-6 flex items-center justify-between">
	<div>
		<h2 class="font-display text-xl font-bold text-base-content mb-1">Apps</h2>
		<p class="text-sm text-base-content/60">Installed apps and the app store</p>
	</div>
	<Button type="ghost" onclick={loadAll}>
		<RefreshCw class="w-4 h-4 mr-2" />
		Refresh
	</Button>
</div>

{#if isLoading}
	<Card>
		<div class="py-12 text-center text-base-content/60">
			<span class="loading loading-spinner loading-md"></span>
			<p class="mt-2">Loading apps...</p>
		</div>
	</Card>
{:else}
	<!-- Installed Apps -->
	<div class="mb-8">
		<h3 class="text-sm font-semibold uppercase tracking-wider text-base-content/40 mb-4">Installed Apps</h3>

		{#if plugins.length > 0}
			<div class="flex flex-col gap-3">
				{#each plugins as plugin}
					<Card>
						<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
						<div
							class="flex items-center gap-4 cursor-pointer"
							onclick={() => openDetail(plugin)}
						>
							<!-- Icon -->
							<div class="w-12 h-12 rounded-xl bg-primary/10 flex items-center justify-center shrink-0">
								{#if plugin.icon}
									<img src={plugin.icon} alt={plugin.displayName} class="w-8 h-8 rounded" />
								{:else}
									<span class="text-lg font-bold text-primary">{getInitial(plugin.displayName)}</span>
								{/if}
							</div>

							<!-- Info -->
							<div class="flex-1 min-w-0">
								<div class="flex items-center gap-2 mb-0.5">
									<h3 class="font-display font-bold text-base-content">{plugin.displayName || plugin.name}</h3>
									<span class="badge badge-sm badge-outline">v{plugin.version}</span>
									<span class="badge badge-sm {getStatusBadgeClass(plugin.connectionStatus)}">
										{plugin.connectionStatus}
									</span>
								</div>
								<p class="text-sm text-base-content/60 truncate">{plugin.description}</p>
							</div>

							<!-- Actions -->
							<div class="flex items-center gap-2 shrink-0">
								<input
									type="checkbox"
									class="toggle toggle-primary toggle-sm"
									checked={plugin.isEnabled}
									disabled={togglingPlugin === plugin.id}
									onclick={(e) => e.stopPropagation()}
									onchange={(e) => handleToggle(e, plugin)}
								/>
							</div>
						</div>
					</Card>
				{/each}
			</div>
		{:else}
			<Card>
				<div class="py-12 text-center text-base-content/60">
					<Package class="w-12 h-12 mx-auto mb-4 opacity-20" />
					<p class="font-medium mb-2">No apps installed</p>
					<p class="text-sm">Browse the App Store to get started.</p>
				</div>
			</Card>
		{/if}
	</div>

	<!-- App Store -->
	<div>
		<h3 class="text-sm font-semibold uppercase tracking-wider text-base-content/40 mb-4">App Store</h3>

		{#if !neboLoopConnected}
			<Card>
				<div class="py-8 text-center text-base-content/60">
					<WifiOff class="w-10 h-10 mx-auto mb-3 opacity-20" />
					<p class="font-medium mb-2">Connect to NeboLoop to browse the App Store</p>
					<a href="/settings/status" class="btn btn-sm btn-primary mt-2">
						Go to Status
					</a>
				</div>
			</Card>
		{:else if isLoadingStore}
			<Card>
				<div class="py-8 text-center text-base-content/60">
					<span class="loading loading-spinner loading-md"></span>
					<p class="mt-2">Loading store...</p>
				</div>
			</Card>
		{:else if storeApps.length > 0}
			<div class="grid sm:grid-cols-2 lg:grid-cols-3 gap-4">
				{#each storeApps as app}
					<Card class="hover:border-primary/30 transition-colors" onclick={() => openStoreDetail(app)}>
						<div class="flex items-start gap-3">
							<div class="w-10 h-10 rounded-xl bg-base-200 flex items-center justify-center shrink-0">
								{#if app.icon}
									<img src={app.icon} alt={app.name} class="w-8 h-8 rounded" />
								{:else}
									<Store class="w-5 h-5 text-base-content/40" />
								{/if}
							</div>
							<div class="flex-1 min-w-0">
								<h3 class="font-display font-bold text-base-content mb-0.5">{app.name}</h3>
								<p class="text-xs text-base-content/50 mb-1">
									by {app.author.name}
									{#if app.author.verified}
										<Check class="w-3 h-3 inline text-success" />
									{/if}
								</p>
								<p class="text-sm text-base-content/60 mb-3 line-clamp-2">{app.description}</p>

								<div class="flex items-center justify-between">
									<div class="flex items-center gap-3 text-xs text-base-content/40">
										{#if app.rating > 0}
											<span class="flex items-center gap-1">
												<Star class="w-3 h-3" />
												{app.rating.toFixed(1)}
											</span>
										{/if}
										{#if app.installCount > 0}
											<span class="flex items-center gap-1">
												<Download class="w-3 h-3" />
												{app.installCount}
											</span>
										{/if}
									</div>

									{#if app.isInstalled || installedAppIds.has(app.slug)}
										<button
											class="btn btn-xs btn-ghost text-success"
											onclick={(e) => { e.stopPropagation(); handleUninstall(app); }}
											disabled={installingApp === app.id}
										>
											{#if installingApp === app.id}
												<span class="loading loading-spinner loading-xs"></span>
											{:else}
												<Check class="w-3 h-3" />
												Installed
											{/if}
										</button>
									{:else}
										<button
											class="btn btn-xs btn-primary"
											onclick={(e) => { e.stopPropagation(); handleInstall(app); }}
											disabled={installingApp === app.id}
										>
											{#if installingApp === app.id}
												<span class="loading loading-spinner loading-xs"></span>
											{:else}
												Install
											{/if}
										</button>
									{/if}
								</div>
							</div>
						</div>
					</Card>
				{/each}
			</div>
		{:else}
			<Card>
				<div class="py-8 text-center text-base-content/60">
					<Store class="w-10 h-10 mx-auto mb-3 opacity-20" />
					<p class="font-medium mb-1">No apps available yet</p>
					<p class="text-sm">Check back later for new apps.</p>
				</div>
			</Card>
		{/if}
	</div>
{/if}

<AppDetailModal
	plugin={selectedPlugin}
	storeApp={selectedStoreApp}
	bind:show={showModal}
	onclose={() => { selectedPlugin = null; selectedStoreApp = null; }}
	onupdated={loadAll}
/>
