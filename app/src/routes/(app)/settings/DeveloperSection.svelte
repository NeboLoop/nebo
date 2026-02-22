<script lang="ts">
	import { onMount } from 'svelte';
	import { Code, FolderOpen, Trash2, RefreshCw, Plus, ExternalLink, AlertCircle, ChevronRight } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type * as components from '$lib/api/neboComponents';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import Toggle from '$lib/components/ui/Toggle.svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import AppLogs from '$lib/components/dev/AppLogs.svelte';

	let isLoading = $state(true);
	let developerMode = $state(false);
	let originalDevMode = $state(false);

	// Sideloading state
	let devApps = $state<components.DevAppItem[]>([]);
	let sideloadPath = $state('');
	let sideloadLoading = $state(false);

	// Per-app error tracking (client-side)
	let appErrors = $state<Record<string, string>>({});

	// Detail modal
	let selectedApp = $state<components.DevAppItem | null>(null);
	let showDetailModal = $state(false);

	// Load settings and dev apps on mount
	onMount(async () => {
		try {
			const [settingsRes, appsRes] = await Promise.all([
				api.getAgentSettings(),
				api.listDevApps()
			]);

			developerMode = settingsRes.settings.developerMode ?? false;
			originalDevMode = developerMode;
			devApps = appsRes.apps ?? [];
		} catch (err) {
			console.error('Failed to load developer settings:', err);
		} finally {
			isLoading = false;
		}
	});

	async function handleToggle() {
		try {
			const current = await api.getAgentSettings();
			await api.updateAgentSettings({
				...current.settings,
				developerMode
			});
			originalDevMode = developerMode;
		} catch (err: any) {
			// Revert on failure
			developerMode = originalDevMode;
			console.error('Failed to update developer mode:', err);
		}
	}

	async function handleSideload() {
		if (!sideloadPath.trim()) return;

		sideloadLoading = true;
		try {
			await api.sideload({ path: sideloadPath.trim() });
			sideloadPath = '';
			const appsRes = await api.listDevApps();
			devApps = appsRes.apps ?? [];
		} catch (err: any) {
			console.error('Failed to sideload app:', err);
		} finally {
			sideloadLoading = false;
		}
	}

	async function handleUnsideload(e: Event, appId: string) {
		e.stopPropagation();
		try {
			await api.unsideload(appId);
			devApps = devApps.filter((a) => a.appId !== appId);
			delete appErrors[appId];
			if (selectedApp?.appId === appId) {
				showDetailModal = false;
				selectedApp = null;
			}
		} catch (err: any) {
			appErrors[appId] = err?.message || 'Failed to unload app';
		}
	}

	async function handleRelaunch(e: Event, appId: string) {
		e.stopPropagation();
		delete appErrors[appId];
		try {
			await api.relaunchDevApp(appId);
			const appsRes = await api.listDevApps();
			devApps = appsRes.apps ?? [];
		} catch (err: any) {
			appErrors[appId] = err?.message || 'Failed to relaunch app';
		}
	}

	function openDetail(app: components.DevAppItem) {
		selectedApp = app;
		showDetailModal = true;
	}

	async function openDevWindow() {
		try {
			await api.openDevWindow();
		} catch {
			window.location.href = '/dev';
		}
	}
</script>

<div class="mb-6">
	<h2 class="font-display text-xl font-bold text-base-content mb-1">Developer</h2>
	<p class="text-sm text-base-content/60">App development tools and settings</p>
</div>

<div class="space-y-6">
	{#if isLoading}
		<Card>
			<div class="flex flex-col items-center justify-center gap-4 py-8">
				<Spinner size={32} />
				<p class="text-sm text-base-content/60">Loading developer settings...</p>
			</div>
		</Card>
	{:else}
		<!-- Developer Mode Toggle -->
		<Card>
			<div class="flex items-center gap-3 mb-6">
				<div>
					<h3 class="text-lg font-semibold text-base-content">Developer Mode</h3>
					<p class="text-sm text-base-content/60">Enable app development features</p>
				</div>
			</div>

			<div class="flex items-center justify-between py-3">
				<div>
					<p class="text-sm font-medium text-base-content">Enable Developer Mode</p>
					<p class="text-xs text-base-content/60">
						Allows sideloading local apps for testing and development
					</p>
				</div>
				<Toggle bind:checked={developerMode} onchange={handleToggle} />
			</div>
		</Card>

		{#if developerMode}
			<!-- Developer Window (hidden until ready)
			<Card>
				<div class="flex items-center justify-between">
					<div>
						<h3 class="text-sm font-medium text-base-content">Developer Window</h3>
						<p class="text-xs text-base-content/60">
							AI pair programmer + tabbed inspector for app development
						</p>
					</div>
					<Button type="primary" onclick={openDevWindow}>
						<ExternalLink class="w-4 h-4" />
						Open Dev Window
					</Button>
				</div>
			</Card>
			-->

			<!-- Sideloaded Apps -->
			<Card>
				<div class="flex items-center gap-3 mb-6">
					<div class="w-10 h-10 rounded-xl bg-primary/10 flex items-center justify-center">
						<FolderOpen class="w-5 h-5 text-primary" />
					</div>
					<div>
						<h2 class="text-lg font-semibold text-base-content">Sideloaded Apps</h2>
						<p class="text-sm text-base-content/60">
							Load apps from local directories for development
						</p>
					</div>
				</div>

				<!-- Load App Form -->
				<div class="flex gap-2 mb-4">
					<input
						type="text"
						bind:value={sideloadPath}
						placeholder="/path/to/your/app/project"
						class="input input-bordered flex-1 text-sm"
						onkeydown={(e) => {
							if (e.key === 'Enter') handleSideload();
						}}
						disabled={sideloadLoading}
					/>
					<Button type="primary" onclick={handleSideload} disabled={sideloadLoading || !sideloadPath.trim()}>
						{#if sideloadLoading}
							<Spinner size={16} />
						{:else}
							<Plus class="w-4 h-4" />
						{/if}
						Load
					</Button>
				</div>

				<!-- App List -->
				{#if devApps.length > 0}
					<div class="divide-y divide-base-content/10">
						{#each devApps as app}
							<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
							<div
								class="flex items-center justify-between py-3 cursor-pointer hover:bg-base-200/50 -mx-2 px-2 rounded-lg transition-colors"
								onclick={() => openDetail(app)}
							>
								<div class="flex-1 min-w-0">
									<div class="flex items-center gap-2">
										<p class="text-sm font-medium text-base-content truncate">
											{app.name}
										</p>
										<span class="badge badge-xs badge-accent">dev</span>
										{#if appErrors[app.appId]}
											<AlertCircle class="w-3.5 h-3.5 text-error shrink-0" />
										{:else if app.running}
											<span class="badge badge-xs badge-success">running</span>
										{:else}
											<span class="badge badge-xs badge-ghost">stopped</span>
										{/if}
									</div>
									<p class="text-xs text-base-content/50 truncate mt-0.5">
										{app.path}
									</p>
								</div>
								<div class="flex items-center gap-1 ml-3">
									<button
										class="btn btn-ghost btn-xs"
										title="Relaunch"
										onclick={(e) => handleRelaunch(e, app.appId)}
									>
										<RefreshCw class="w-3.5 h-3.5" />
									</button>
									<button
										class="btn btn-ghost btn-xs text-error"
										title="Unload"
										onclick={(e) => handleUnsideload(e, app.appId)}
									>
										<Trash2 class="w-3.5 h-3.5" />
									</button>
									<ChevronRight class="w-4 h-4 text-base-content/30" />
								</div>
							</div>
						{/each}
					</div>
				{:else}
					<div class="text-center py-6">
						<p class="text-sm text-base-content/50">
							No sideloaded apps. Enter a path to your app project above to get started.
						</p>
					</div>
				{/if}
			</Card>

			<!-- Developer Info -->
			<Card>
				<div class="bg-base-200 rounded-lg p-4">
					<p class="text-sm font-medium text-base-content mb-2">How sideloading works</p>
					<ul class="text-xs text-base-content/60 space-y-1 list-disc list-inside">
						<li>Point to a directory with a <code class="bg-base-300 px-1 rounded">manifest.json</code> and compiled binary</li>
						<li>Nebo creates a symlink and launches the app immediately</li>
						<li>Rebuild your binary and the watcher will auto-restart the app</li>
						<li>Unloading removes the symlink but keeps your project files intact</li>
					</ul>
				</div>
			</Card>
		{/if}
	{/if}
</div>

<!-- App Detail Modal -->
{#if selectedApp}
	<Modal bind:show={showDetailModal} title={selectedApp.name} size="lg" onclose={() => { selectedApp = null; }} closeOnBackdrop={true}>
		<!-- Status -->
		<div class="flex items-center gap-2 mb-4">
			<span class="badge badge-sm badge-accent">dev</span>
			{#if appErrors[selectedApp.appId]}
				<span class="badge badge-sm badge-error">error</span>
			{:else if selectedApp.running}
				<span class="badge badge-sm badge-success">running</span>
			{:else}
				<span class="badge badge-sm badge-ghost">stopped</span>
			{/if}
			{#if selectedApp.version}
				<span class="badge badge-sm badge-outline">v{selectedApp.version}</span>
			{/if}
		</div>

		<!-- Path -->
		<div class="mb-4">
			<span class="text-xs text-base-content/40 block mb-0.5">Path</span>
			<p class="text-sm text-base-content font-mono bg-base-200 px-3 py-2 rounded-lg break-all">{selectedApp.path}</p>
		</div>

		<!-- Error -->
		{#if appErrors[selectedApp.appId]}
			<div class="mb-4">
				<span class="text-xs text-base-content/40 block mb-0.5">Error</span>
				<div class="bg-error/10 border border-error/20 rounded-lg px-3 py-2">
					<p class="text-sm text-error font-mono break-all">{appErrors[selectedApp.appId]}</p>
				</div>
			</div>
		{/if}

		<!-- Actions -->
		<div class="flex gap-2 mb-4">
			<Button type="primary" size="sm" onclick={() => { delete appErrors[selectedApp!.appId]; api.relaunchDevApp(selectedApp!.appId).then(async () => { const r = await api.listDevApps(); devApps = r.apps ?? []; }).catch((err: any) => { appErrors[selectedApp!.appId] = err?.message || 'Failed to relaunch'; }); }}>
				<RefreshCw class="w-4 h-4" />
				Relaunch
			</Button>
			<Button type="ghost" size="sm" onclick={() => { api.unsideload(selectedApp!.appId).then(() => { devApps = devApps.filter(a => a.appId !== selectedApp!.appId); delete appErrors[selectedApp!.appId]; showDetailModal = false; selectedApp = null; }).catch((err: any) => { appErrors[selectedApp!.appId] = err?.message || 'Failed to unload'; }); }}>
				<Trash2 class="w-4 h-4" />
				Unload
			</Button>
		</div>

		<!-- Logs -->
		<div class="border-t border-base-300 pt-4">
			<h4 class="text-sm font-medium text-base-content mb-2">Logs</h4>
			<div class="h-72 rounded-lg overflow-hidden border border-base-300">
				<AppLogs appId={selectedApp.appId} />
			</div>
		</div>
	</Modal>
{/if}
