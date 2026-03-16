<script lang="ts">
	import InstallCode from '$lib/components/InstallCode.svelte';
	import PricePill from './PricePill.svelte';
	import ArtifactIcon from './ArtifactIcon.svelte';
	import { type AppItem, itemHref } from '$lib/types/marketplace';

	let { item, rank }: { item: AppItem; rank?: number } = $props();
</script>

<a
	href={itemHref(item)}
	class="flex items-center gap-3 py-3 px-3 border-b border-base-content/5 hover:bg-base-content/[0.03] transition-colors"
>
	{#if rank !== undefined}
		<span class="w-5 text-right text-base text-base-content/80 font-medium shrink-0 tabular-nums">{rank}</span>
	{/if}
	<ArtifactIcon emoji={item.iconEmoji} bg={item.iconBg} size="lg" />
	<div class="flex-1 min-w-0">
		<p class="text-base font-semibold truncate">{item.name}</p>
		<p class="text-sm text-base-content/80 truncate">{item.description}</p>
		<InstallCode code={item.code} inline />
	</div>
	<PricePill price={item.price} installed={item.installed} />
</a>
