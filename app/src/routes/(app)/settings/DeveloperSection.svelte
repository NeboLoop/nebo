<script lang="ts">
	import { onMount } from 'svelte';
	import { Code, FolderOpen, Trash2, RefreshCw, Plus, ExternalLink } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type * as components from '$lib/api/neboComponents';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import Toggle from '$lib/components/ui/Toggle.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';

	let isLoading = $state(true);
	let developerMode = $state(false);
	let originalDevMode = $state(false);

	// Sideloading state
	let devApps = $state<components.DevAppItem[]>([]);
	let sideloadPath = $state('');
	let sideloadLoading = $state(false);
	let sideloadError = $state('');
	let sideloadSuccess = $state('');

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

	function clearMessages() {
		sideloadError = '';
		sideloadSuccess = '';
	}

	async function handleSideload() {
		if (!sideloadPath.trim()) return;

		clearMessages();
		sideloadLoading = true;
		try {
			const result = await api.sideload({ path: sideloadPath.trim() });
			sideloadSuccess = `Loaded "${result.name}" (${result.appId})`;
			sideloadPath = '';
			// Refresh the list
			const appsRes = await api.listDevApps();
			devApps = appsRes.apps ?? [];
		} catch (err: any) {
			sideloadError = err?.message || 'Failed to sideload app';
		} finally {
			sideloadLoading = false;
		}
	}

	async function handleUnsideload(appId: string) {
		clearMessages();
		try {
			await api.unsideload(appId);
			devApps = devApps.filter((a) => a.appId !== appId);
		} catch (err: any) {
			sideloadError = err?.message || 'Failed to unload app';
		}
	}

	async function handleRelaunch(appId: string) {
		clearMessages();
		try {
			await api.relaunchDevApp(appId);
			// Refresh the list
			const appsRes = await api.listDevApps();
			devApps = appsRes.apps ?? [];
			sideloadSuccess = 'App relaunched';
		} catch (err: any) {
			sideloadError = err?.message || 'Failed to relaunch app';
		}
	}
</script>

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
				<div class="w-10 h-10 rounded-xl bg-accent/10 flex items-center justify-center">
					<Code class="w-5 h-5 text-accent" />
				</div>
				<div>
					<h2 class="text-lg font-semibold text-base-content">Developer Mode</h2>
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
			<!-- Developer Window -->
			<Card>
				<div class="flex items-center justify-between">
					<div>
						<h3 class="text-sm font-medium text-base-content">Developer Window</h3>
						<p class="text-xs text-base-content/60">
							AI pair programmer + tabbed inspector for app development
						</p>
					</div>
					<Button type="primary" onclick={() => window.open('/dev', 'nebo-dev', 'width=1400,height=900')}>
						<ExternalLink class="w-4 h-4" />
						Open Dev Window
					</Button>
				</div>
			</Card>

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

				{#if sideloadSuccess}
					<Alert type="success" title="Success">{sideloadSuccess}</Alert>
				{/if}

				{#if sideloadError}
					<Alert type="error" title="Error">{sideloadError}</Alert>
				{/if}

				<!-- App List -->
				{#if devApps.length > 0}
					<div class="divide-y divide-base-content/10">
						{#each devApps as app}
							<div class="flex items-center justify-between py-3">
								<div class="flex-1 min-w-0">
									<div class="flex items-center gap-2">
										<p class="text-sm font-medium text-base-content truncate">
											{app.name}
										</p>
										<span class="badge badge-xs badge-accent">dev</span>
										{#if app.running}
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
										onclick={() => handleRelaunch(app.appId)}
									>
										<RefreshCw class="w-3.5 h-3.5" />
									</button>
									<button
										class="btn btn-ghost btn-xs text-error"
										title="Unload"
										onclick={() => handleUnsideload(app.appId)}
									>
										<Trash2 class="w-3.5 h-3.5" />
									</button>
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
