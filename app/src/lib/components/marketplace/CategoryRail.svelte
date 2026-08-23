<script module lang="ts">
	// Which storefront kinds carry a category rail, and which list they use.
	// Employee-type tabs filter by department; tool-type tabs (incl. collections,
	// which the website filters by tool category too) by tool category.
	const DEPT_KINDS = ['employees', 'agents'];
	const TC_KINDS = ['tools', 'apps', 'skills', 'plugins', 'connectors', 'collections'];
	export function hasCategoryRail(kind: string): boolean {
		return DEPT_KINDS.includes(kind) || TC_KINDS.includes(kind);
	}
</script>

<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { loadMarketplaceMap, mapSlugify, type MarketplaceMap } from '$lib/data/marketplaceMap';

	let { kind, activeFilter = '' }: { kind: string; activeFilter?: string } = $props();

	// Curated map — the same single source the website's nav uses.
	let mktMap: MarketplaceMap | null = $state(null);
	onMount(() => {
		loadMarketplaceMap().then((m) => (mktMap = m));
	});

	const rail = $derived.by(() => {
		if (!mktMap) return null;
		if (DEPT_KINDS.includes(kind))
			return {
				headingKey: 'marketplace.departments',
				allKey: 'marketplace.allDepartments',
				names: mktMap.departments,
			};
		if (TC_KINDS.includes(kind))
			return {
				headingKey: 'marketplace.toolCategories',
				allKey: 'marketplace.allToolCategories',
				names: mktMap.toolCategories,
			};
		return null;
	});
</script>

{#if rail}
	<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 px-3.5 pt-3 pb-1">{$t(rail.headingKey)}</div>
	<div class="px-1.5">
		<a
			href="/marketplace?kind={kind}"
			class="flex items-center gap-1.5 py-1 px-2.5 rounded-md text-sm transition-colors border {!activeFilter
				? 'bg-base-100 border-base-300 shadow-sm font-medium'
				: 'border-transparent hover:bg-base-100/70'}"
		>
			<span class="flex-1 truncate">{$t(rail.allKey)}</span>
		</a>
		{#each rail.names as name (name)}
			<a
				href="/marketplace?kind={kind}&filter={mapSlugify(name)}"
				class="flex items-center gap-1.5 py-1 px-2.5 rounded-md text-sm transition-colors border {activeFilter === mapSlugify(name)
					? 'bg-base-100 border-base-300 shadow-sm font-medium'
					: 'border-transparent hover:bg-base-100/70'}"
			>
				<span class="flex-1 truncate">{name}</span>
			</a>
		{/each}
	</div>
{/if}
