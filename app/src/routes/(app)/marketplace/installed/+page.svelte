<script lang="ts">
	import { onMount } from 'svelte';
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

	onMount(async () => {
		await loadInstalled();
	});

	async function loadInstalled() {
		isLoading = true;
		error = '';
		try {
			// Fetch all products and filter to installed
			const [skillsRes, rolesRes, workflowsRes] = await Promise.all([
				webapi.get<any>('/api/v1/store/products', { type: 'skill' }).catch(() => ({ skills: [] })),
				webapi.get<any>('/api/v1/store/products', { type: 'role' }).catch(() => ({ skills: [] })),
				webapi.get<any>('/api/v1/store/products', { type: 'workflow' }).catch(() => ({ skills: [] }))
			]);

			const all = [
				...(skillsRes.skills || []).map((s: any, i: number) => toAppItem({ ...s, type: s.type || 'skill' }, i)),
				...(rolesRes.skills || []).map((s: any, i: number) => toAppItem({ ...s, type: s.type || 'role' }, i + 100)),
				...(workflowsRes.skills || []).map((s: any, i: number) => toAppItem({ ...s, type: s.type || 'workflow' }, i + 200))
			];

			installed = all.filter(item => item.installed);
		} catch (err: any) {
			error = err?.message || 'Failed to load installed skills';
		} finally {
			isLoading = false;
		}
	}

	async function uninstall(item: AppItem) {
		if (!confirm(`Uninstall ${item.name}?`)) return;
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
			<h2 class="font-display text-2xl font-bold text-base-content">Installed</h2>
			<p class="text-base text-base-content/80 mt-1">Skills installed on your Nebo</p>
		</div>
		<button
			type="button"
			class="text-base text-base-content/80 hover:text-primary transition-colors"
			onclick={loadInstalled}
			aria-label="Refresh"
		>
			<RefreshCw class="w-4 h-4" />
		</button>
	</div>

	{#if isLoading}
		<div class="flex items-center justify-center gap-3 py-16">
			<Spinner size={20} />
			<span class="text-base text-base-content/80">Loading...</span>
		</div>
	{:else}
		{#if error}
			<Alert type="error" title="Error">{error}</Alert>
		{/if}

		{#if installed.length === 0}
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<div class="py-10 text-center">
					<PackageCheck class="w-10 h-10 mx-auto mb-3 text-base-content/40" />
					<p class="text-base font-medium text-base-content/80 mb-1">Nothing installed yet</p>
					<p class="text-sm text-base-content/60 mb-4">Browse the marketplace to find skills for your Nebo</p>
					<a
						href="/marketplace"
						class="inline-block h-10 px-6 leading-10 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all"
					>
						Browse Marketplace
					</a>
				</div>
			</div>
		{:else}
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<div class="space-y-2">
					{#each installed as item (item.id)}
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
									title="View details"
								>
									<ExternalLink class="w-4 h-4" />
								</a>
								<button
									type="button"
									class="text-base text-base-content/80 hover:text-error transition-colors"
									title="Uninstall"
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

			<p class="text-sm text-base-content/40 mt-3 text-center">{installed.length} skill{installed.length !== 1 ? 's' : ''} installed</p>
		{/if}
	{/if}
</div>
