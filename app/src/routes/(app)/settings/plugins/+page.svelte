<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import { Puzzle, RefreshCw, ShieldCheck, ShieldAlert, Loader2, Trash2, LogIn, LogOut } from 'lucide-svelte';
	import AlertDialog from '$lib/components/ui/AlertDialog.svelte';
	import { listPlugins, pluginAuthLogin, pluginAuthLogout, pluginAuthStatus, removePlugin } from '$lib/api/nebo';
	import type { InstalledPlugin } from '$lib/api/neboComponents';
	import { getWebSocketClient } from '$lib/websocket/client';
	import { t } from 'svelte-i18n';

	let plugins = $state<InstalledPlugin[]>([]);
	let isLoading = $state(true);
	let connectingSlug = $state<string | null>(null);
	let disconnectingSlug = $state<string | null>(null);
	let removingSlug = $state<string | null>(null);
	let authStatuses = $state<Record<string, 'unknown' | 'connected' | 'error'>>({});

	// Remove confirmation dialog
	let removeTarget = $state<InstalledPlugin | null>(null);
	let showRemoveDialog = $state(false);

	let unsubscribers: Array<() => void> = [];

	onMount(async () => {
		await loadPlugins();

		const client = getWebSocketClient();
		unsubscribers.push(
			client.on('plugin_auth_complete', (data: Record<string, unknown>) => {
				const slug = data.plugin as string;
				if (slug) {
					authStatuses[slug] = 'connected';
					connectingSlug = null;
				}
			}),
			client.on('plugin_auth_error', (data: Record<string, unknown>) => {
				const slug = data.plugin as string;
				if (slug) {
					authStatuses[slug] = 'error';
					connectingSlug = null;
				}
			})
		);
	});

	onDestroy(() => {
		unsubscribers.forEach((fn) => fn());
	});

	async function loadPlugins() {
		isLoading = true;
		try {
			const data = await listPlugins();
			plugins = data.plugins || [];

			for (const p of plugins) {
				if (p.hasAuth) {
					checkAuthStatus(p.slug);
				}
			}
		} catch (error) {
			console.error('Failed to load plugins:', error);
		} finally {
			isLoading = false;
		}
	}

	async function checkAuthStatus(slug: string) {
		try {
			const resp = await pluginAuthStatus(slug);
			authStatuses[slug] = resp.authenticated ? 'connected' : 'unknown';
		} catch {
			authStatuses[slug] = 'unknown';
		}
	}

	async function handleConnect(slug: string) {
		connectingSlug = slug;
		try {
			await pluginAuthLogin(slug);
		} catch (error) {
			console.error('Failed to start auth login:', error);
			connectingSlug = null;
		}
	}

	async function handleDisconnect(slug: string) {
		disconnectingSlug = slug;
		try {
			await pluginAuthLogout(slug);
			authStatuses[slug] = 'unknown';
		} catch (error) {
			console.error('Failed to logout:', error);
		} finally {
			disconnectingSlug = null;
		}
	}

	function promptRemove(plugin: InstalledPlugin) {
		removeTarget = plugin;
		showRemoveDialog = true;
	}

	async function confirmRemove() {
		if (!removeTarget) return;
		const slug = removeTarget.slug;
		showRemoveDialog = false;
		removeTarget = null;
		removingSlug = slug;
		try {
			await removePlugin(slug);
			await loadPlugins();
		} catch (error) {
			console.error('Failed to remove plugin:', error);
		} finally {
			removingSlug = null;
		}
	}
</script>

<div class="mb-6 flex items-center justify-between">
	<div>
		<h2 class="font-display text-xl font-bold text-base-content mb-1">{$t('settingsPlugins.title')}</h2>
		<p class="text-base text-base-content/80">{$t('settingsPlugins.description')}</p>
	</div>
	<button
		class="h-8 px-3 rounded-lg bg-base-content/5 border border-base-content/10 text-sm font-medium text-base-content/60 hover:border-base-content/40 hover:text-base-content transition-colors flex items-center gap-1.5"
		onclick={loadPlugins}
	>
		<RefreshCw class="w-3.5 h-3.5" />
		{$t('common.refresh')}
	</button>
</div>

{#if isLoading}
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 py-12 text-center text-base-content/90">
		<Spinner class="w-5 h-5 mx-auto mb-2" />
		<p class="text-base">{$t('settingsPlugins.loading')}</p>
	</div>
{:else if plugins.length === 0}
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 py-12 text-center text-base-content/90">
		<Puzzle class="w-12 h-12 mx-auto mb-4 opacity-20" />
		<p class="font-medium mb-2">{$t('settingsPlugins.noPlugins')}</p>
		<p class="text-base">{$t('settingsPlugins.noPluginsHint')}</p>
	</div>
{:else}
	<div class="space-y-3">
		{#each plugins as plugin}
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-4">
				<!-- Header row -->
				<div class="flex items-center gap-4">
					<div class="w-11 h-11 rounded-xl bg-primary/10 flex items-center justify-center shrink-0">
						<Puzzle class="w-5 h-5 text-primary" />
					</div>
					<div class="flex-1 min-w-0">
						<div class="flex items-center gap-2 mb-0.5">
							<h3 class="font-display font-bold text-base text-base-content">{plugin.name}</h3>
							<span class="text-sm font-medium px-1.5 py-0.5 rounded bg-base-content/10 text-base-content/60">v{plugin.version}</span>
							{#if plugin.source === 'user'}
								<span class="text-sm font-medium px-1.5 py-0.5 rounded bg-warning/15 text-warning">user</span>
							{/if}
						</div>
						<p class="text-base text-base-content/80 truncate">{plugin.description}</p>
						{#if plugin.author}
							<p class="text-sm text-base-content/50">{plugin.author}</p>
						{/if}
					</div>
				</div>

				<!-- Actions row -->
				<div class="flex items-center justify-between mt-3 pt-3 border-t border-base-content/5">
					<!-- Auth actions (left) -->
					<div class="flex items-center gap-2">
						{#if plugin.hasAuth}
							{#if authStatuses[plugin.slug] === 'connected'}
								<span class="flex items-center gap-1.5 text-sm font-semibold text-success mr-2">
									<ShieldCheck class="w-3.5 h-3.5" />
									{$t('settingsPlugins.connected')}
								</span>
								<button
									class="h-7 px-2.5 rounded-md bg-base-content/5 border border-base-content/10 text-sm font-medium text-base-content/60 hover:border-error/30 hover:text-error transition-colors flex items-center gap-1 disabled:opacity-50"
									onclick={() => handleDisconnect(plugin.slug)}
									disabled={disconnectingSlug === plugin.slug}
								>
									{#if disconnectingSlug === plugin.slug}
										<Loader2 class="w-3 h-3 animate-spin" />
									{:else}
										<LogOut class="w-3 h-3" />
									{/if}
									{$t('settingsPlugins.disconnect')}
								</button>
							{:else if connectingSlug === plugin.slug}
								<span class="flex items-center gap-1.5 text-sm font-semibold text-primary">
									<Loader2 class="w-3.5 h-3.5 animate-spin" />
									{$t('settingsPlugins.connecting')}
								</span>
							{:else}
								{#if authStatuses[plugin.slug] === 'error'}
									<span class="flex items-center gap-1.5 text-sm font-semibold text-error mr-2">
										<ShieldAlert class="w-3.5 h-3.5" />
										{$t('settingsPlugins.authFailed')}
									</span>
								{/if}
								<button
									class="h-7 px-2.5 rounded-md bg-primary text-primary-content text-sm font-semibold flex items-center gap-1 hover:brightness-110 transition-all"
									onclick={() => handleConnect(plugin.slug)}
								>
									<LogIn class="w-3 h-3" />
									{$t('settingsPlugins.connect')}
									{#if plugin.authLabel}
										<span class="text-primary-content/70">&middot; {plugin.authLabel}</span>
									{/if}
								</button>
							{/if}
						{:else}
							<span class="text-sm text-base-content/40">{$t('settingsPlugins.noAuthNeeded')}</span>
						{/if}
					</div>

					<!-- Remove (right) -->
					<button
						class="h-7 px-2.5 rounded-md bg-base-content/5 border border-base-content/10 text-sm font-medium text-base-content/60 hover:border-error/30 hover:text-error transition-colors flex items-center gap-1 disabled:opacity-50"
						onclick={() => promptRemove(plugin)}
						disabled={removingSlug === plugin.slug}
					>
						{#if removingSlug === plugin.slug}
							<Loader2 class="w-3 h-3 animate-spin" />
						{:else}
							<Trash2 class="w-3 h-3" />
						{/if}
						{$t('common.remove')}
					</button>
				</div>
			</div>
		{/each}
	</div>
{/if}

<AlertDialog
	bind:open={showRemoveDialog}
	title={$t('settingsPlugins.removeTitle')}
	description={$t('settingsPlugins.removeConfirm', { values: { name: removeTarget?.name ?? '' } })}
	actionLabel={$t('common.remove')}
	actionType="danger"
	onAction={confirmRemove}
	onclose={() => { removeTarget = null; }}
/>
