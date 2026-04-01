<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
	import ArtifactIcon from '$lib/components/marketplace/ArtifactIcon.svelte';
	import { Trash2, RefreshCw, PackageCheck, ExternalLink } from 'lucide-svelte';
	import webapi from '$lib/api/gocliRequest';
	import * as api from '$lib/api/nebo';
	import { type AppItem, toAppItem, itemHref } from '$lib/types/marketplace';

	let isLoading = $state(true);
	let error = $state('');
	let installed = $state<AppItem[]>([]);
	let uninstallingId = $state<string | null>(null);

	let installedAgents = $derived(installed.filter(i => i.type === 'agent' || i.type === 'workflow'));
	let installedSkills = $derived(installed.filter(i => i.type === 'skill'));

	onMount(async () => {
		await loadInstalled();
	});

	async function loadInstalled() {
		isLoading = true;
		error = '';
		try {
			// Fetch all products and filter to installed
			const [skillsRes, agentsRes, workflowsRes] = await Promise.all([
				webapi.get<any>('/api/v1/store/products', { type: 'skill', pageSize: 100 }).catch(() => ({ skills: [] })),
				webapi.get<any>('/api/v1/store/products', { type: 'agent', pageSize: 100 }).catch(() => ({ skills: [] })),
				webapi.get<any>('/api/v1/store/products', { type: 'workflow', pageSize: 100 }).catch(() => ({ skills: [] }))
			]);

			const all = [
				...(skillsRes.skills || []).map((s: any, i: number) => toAppItem({ ...s, type: s.type || 'skill' }, i)),
				...(agentsRes.skills || []).map((s: any, i: number) => toAppItem({ ...s, type: s.type || 'agent' }, i + 100)),
				...(workflowsRes.skills || []).map((s: any, i: number) => toAppItem({ ...s, type: s.type || 'workflow' }, i + 200))
			];

			installed = all.filter(item => item.installed);
		} catch (err: any) {
			error = err?.message || 'Failed to load installed items';
		} finally {
			isLoading = false;
		}
	}

	async function uninstall(item: AppItem) {
		if (!confirm($t('marketplace.installedPage.uninstallConfirm', { values: { name: item.name } }))) return;
		uninstallingId = item.id;
		try {
			await api.uninstallStoreProduct(item.id);
			installed = installed.filter(i => i.id !== item.id);
		} catch (err: any) {
			error = err?.message || 'Failed to uninstall';
		} finally {
			uninstallingId = null;
		}
	}
</script>

<div class="max-w-3xl mx-auto px-6 pt-8">
	<div class="mb-6 flex items-center justify-between">
		<div>
			<h2 class="font-display text-2xl font-bold text-base-content">{$t('marketplace.installedPage.title')}</h2>
			<p class="text-base text-base-content/80 mt-1">{$t('marketplace.installedPage.subtitle')}</p>
		</div>
		<button
			type="button"
			class="text-base text-base-content/80 hover:text-primary transition-colors"
			onclick={loadInstalled}
			aria-label={$t('common.refresh')}
		>
			<RefreshCw class="w-4 h-4" />
		</button>
	</div>

	{#if isLoading}
		<div class="flex items-center justify-center gap-3 py-16">
			<Spinner size={20} />
			<span class="text-base text-base-content/80">{$t('common.loading')}</span>
		</div>
	{:else}
		{#if error}
			<Alert type="error" title={$t('common.error')}>{error}</Alert>
		{/if}

		{#if installed.length === 0}
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<div class="py-10 text-center">
					<PackageCheck class="w-10 h-10 mx-auto mb-3 text-base-content/40" />
					<p class="text-base font-medium text-base-content/80 mb-1">{$t('marketplace.installedPage.nothingInstalled')}</p>
					<p class="text-sm text-base-content/60 mb-4">{$t('marketplace.installedPage.browseDescription')}</p>
					<a
						href="/marketplace"
						class="inline-block h-10 px-6 leading-10 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all"
					>
						{$t('marketplace.installedPage.browseMarketplace')}
					</a>
				</div>
			</div>
		{:else}
			{#if installedAgents.length > 0}
				<div class="mb-6">
					<h3 class="text-lg font-semibold text-base-content mb-3">{$t('marketplace.agents')}</h3>
					<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
						<div class="space-y-2">
							{#each installedAgents as item (item.id)}
								<div class="flex items-center justify-between py-2.5 px-4 rounded-xl bg-base-content/5 border border-base-content/10">
									<a href={itemHref(item)} class="flex items-center gap-3 min-w-0 no-underline group">
										<ArtifactIcon emoji={item.iconEmoji} bg={item.iconBg} size="sm" />
										<div class="min-w-0">
											<p class="text-base font-medium text-base-content group-hover:text-primary transition-colors truncate">
												{item.name}
											</p>
											<p class="text-sm text-base-content/60 truncate">{item.description}</p>
										</div>
									</a>
									<div class="flex items-center gap-3 shrink-0 ml-3">
										<a
											href={itemHref(item)}
											class="text-base text-base-content/80 hover:text-primary transition-colors"
											title={$t('marketplace.installedPage.viewDetails')}
										>
											<ExternalLink class="w-4 h-4" />
										</a>
										<button
											type="button"
											class="text-base text-base-content/80 hover:text-error transition-colors"
											title={$t('common.uninstall')}
											onclick={() => uninstall(item)}
											disabled={uninstallingId === item.id}
										>
											{#if uninstallingId === item.id}
												<Spinner size={14} />
											{:else}
												<Trash2 class="w-4 h-4" />
											{/if}
										</button>
									</div>
								</div>
							{/each}
						</div>
					</div>
					<p class="text-sm text-base-content/40 mt-2 text-center">{$t('marketplace.installedPage.agentsInstalled', { values: { count: installedAgents.length } })}</p>
				</div>
			{/if}

			{#if installedSkills.length > 0}
				<div class="mb-6">
					<h3 class="text-lg font-semibold text-base-content mb-3">{$t('marketplace.skills')}</h3>
					<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
						<div class="space-y-2">
							{#each installedSkills as item (item.id)}
								<div class="flex items-center justify-between py-2.5 px-4 rounded-xl bg-base-content/5 border border-base-content/10">
									<a href={itemHref(item)} class="flex items-center gap-3 min-w-0 no-underline group">
										<ArtifactIcon emoji={item.iconEmoji} bg={item.iconBg} size="sm" />
										<div class="min-w-0">
											<p class="text-base font-medium text-base-content group-hover:text-primary transition-colors truncate">
												{item.name}
											</p>
											<p class="text-sm text-base-content/60 truncate">{item.description}</p>
										</div>
									</a>
									<div class="flex items-center gap-3 shrink-0 ml-3">
										<a
											href={itemHref(item)}
											class="text-base text-base-content/80 hover:text-primary transition-colors"
											title={$t('marketplace.installedPage.viewDetails')}
										>
											<ExternalLink class="w-4 h-4" />
										</a>
										<button
											type="button"
											class="text-base text-base-content/80 hover:text-error transition-colors"
											title={$t('common.uninstall')}
											onclick={() => uninstall(item)}
											disabled={uninstallingId === item.id}
										>
											{#if uninstallingId === item.id}
												<Spinner size={14} />
											{:else}
												<Trash2 class="w-4 h-4" />
											{/if}
										</button>
									</div>
								</div>
							{/each}
						</div>
					</div>
					<p class="text-sm text-base-content/40 mt-2 text-center">{$t('marketplace.installedPage.skillsInstalled', { values: { count: installedSkills.length } })}</p>
				</div>
			{/if}
		{/if}
	{/if}
</div>
