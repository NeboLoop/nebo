<script lang="ts">
	import InstallCode from '$lib/components/InstallCode.svelte';
	import PricePill from './PricePill.svelte';
	import ArtifactIcon from './ArtifactIcon.svelte';
	import { type AppItem, itemHref } from '$lib/types/marketplace';

	let { item }: { item: AppItem } = $props();
</script>

<a
	href={itemHref(item)}
	class="flex-shrink-0 w-52 rounded-2xl bg-base-content/5 border border-base-content/10 p-4 hover:bg-base-content/[0.08] transition-colors"
>
	<div class="flex items-center gap-3 mb-3">
		<ArtifactIcon emoji={item.iconEmoji} bg={item.iconBg} size="md" />
		<div class="flex-1 min-w-0">
			<p class="text-base font-bold truncate">{item.name}</p>
		</div>
		<PricePill price={item.price} installed={item.installed} />
	</div>
	<p class="text-sm text-base-content/60 line-clamp-2 leading-relaxed">{item.description}</p>
	<InstallCode code={item.code} inline />
</a>
