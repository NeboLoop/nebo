<script lang="ts">
	import { goto } from '$app/navigation';
	import InstallCode from '$lib/components/InstallCode.svelte';
	import PricePill from './PricePill.svelte';
	import ArtifactIcon from './ArtifactIcon.svelte';
	import InstallFlowModal from '$lib/components/install/InstallFlowModal.svelte';
	import { type AppItem, itemHref } from '$lib/types/marketplace';
	import { installStoreProduct } from '$lib/api/nebo';
	import webapi from '$lib/api/gocliRequest';
	import { CheckCircle } from 'lucide-svelte';

	let { item, label }: { item: AppItem; label?: string } = $props();

	const TYPE_NAMES: Record<AppItem['type'], string> = {
		agent: 'AGENT', app: 'APP', skill: 'SKILL', plugin: 'PLUGIN',
		connector: 'CONNECTOR', workflow: 'WORKFLOW', collection: 'COLLECTION'
	};
	const typeLabel = $derived(label || TYPE_NAMES[item.type] || 'ITEM');

	let installing = $state(false);
	let showSetupModal = $state(false);
	let setupInputs = $state<Record<string, unknown>>({});

	async function handleGetClick(e: MouseEvent) {
		e.preventDefault();
		e.stopPropagation();
		if (installing || item.installed) return;

		// Agents/apps install + configure + activate through the unified flow.
		if (item.type === 'agent' || item.type === 'app') {
			const detail = await webapi.get<any>(`/api/v1/store/products/${item.id}`).catch(() => null);
			setupInputs = detail?.typeConfig?.inputs || detail?.inputs || {};
			showSetupModal = true;
			return;
		}

		// Non-agent artifacts (skill/plugin/connector/workflow/collection) install directly;
		// their declared deps force-cascade on the backend.
		installing = true;
		try {
			await installStoreProduct(item.id);
			item.installed = true;
		} catch {
			// ignore
		} finally {
			installing = false;
		}
	}

	function handleSetupComplete(agentId?: string) {
		showSetupModal = false;
		if (agentId) goto(`/${agentId}/threads`);
	}
</script>

<a
	href={itemHref(item)}
	class="rounded-2xl overflow-hidden border border-base-content/10 hover:border-base-content/40 transition-colors"
>
	<div class="p-5">
		<p class="text-sm font-semibold text-primary uppercase tracking-wider mb-2">{typeLabel}</p>
		<div class="flex items-start gap-4">
			<ArtifactIcon emoji={item.iconEmoji} bg={item.iconBg} size="xl" />
			<div class="flex-1 min-w-0">
				<h3 class="font-display text-lg font-bold leading-tight">{item.name}</h3>
				{#if item.description}
				<p class="text-sm text-base-content/80 mt-1 line-clamp-2 leading-relaxed">{item.description}</p>
			{/if}
				<div class="flex items-center gap-1.5 mt-2">
					{#if item.author}
						<span class="text-sm text-base-content/80 truncate">{item.author}</span>
						{#if item.authorVerified}
							<CheckCircle class="w-3 h-3 text-info shrink-0" />
						{/if}
					{/if}
				</div>
			</div>
			<button
				type="button"
				class="shrink-0"
				onclick={handleGetClick}
				disabled={installing}
			>
				{#if installing}
					<span class="btn-market btn-market-get">
						<span class="loading loading-spinner loading-xs"></span>
					</span>
				{:else}
					<PricePill price={item.price} installed={item.installed} />
				{/if}
			</button>
		</div>
		<div class="flex items-center justify-between mt-3">
			<InstallCode code={item.code} inline />
		</div>
	</div>
</a>

{#if showSetupModal}
	<InstallFlowModal
		mode="product"
		bind:show={showSetupModal}
		appId={item.id}
		agentName={item.name}
		agentDescription={item.description}
		seedInputs={setupInputs}
		dependencies={(item as any)?.dependencies ?? (item as any)?.typeConfig?.dependencies}
		oncomplete={handleSetupComplete}
	/>
{/if}
