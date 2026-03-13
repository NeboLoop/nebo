<script lang="ts">
	import PricePill from './PricePill.svelte';
	import ArtifactIcon from './ArtifactIcon.svelte';
	import { type AppItem, itemHref } from '$lib/types/marketplace';

	let { item }: { item: AppItem } = $props();
</script>

<a
	href={itemHref(item)}
	class="flex-shrink-0 w-28 flex flex-col items-center gap-2 p-3 rounded-2xl hover:bg-base-content/[0.03] transition-colors"
>
	<ArtifactIcon emoji={item.iconEmoji} bg={item.iconBg} size="lg" />
	<p class="text-xs font-semibold text-center truncate w-full">{item.name}</p>
	<PricePill price={item.price} installed={item.installed} />
</a>
