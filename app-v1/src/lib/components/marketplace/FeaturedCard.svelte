<script lang="ts">
	import InstallCode from '$lib/components/InstallCode.svelte';
	import PricePill from './PricePill.svelte';
	import ArtifactIcon from './ArtifactIcon.svelte';
	import { type AppItem, itemHref } from '$lib/types/marketplace';
	import { CheckCircle } from 'lucide-svelte';

	let { item, label }: { item: AppItem; label?: string } = $props();

	const typeLabel = label || `Featured ${item.type === 'agent' ? 'Agent' : item.type === 'workflow' ? 'Workflow' : 'Skill'}`;
</script>

<a
	href={itemHref(item)}
	class="block rounded-2xl {item.iconBg} p-6 sm:p-8 hover:opacity-95 transition-opacity"
>
	<p class="text-sm font-semibold uppercase tracking-wider text-base-content/80 mb-2">{typeLabel}</p>
	<div class="flex items-start gap-4 mb-4">
		<ArtifactIcon emoji={item.iconEmoji} bg="bg-base-100/50" size="xl" />
		<div class="flex-1 min-w-0">
			<h3 class="font-display text-2xl sm:text-3xl font-bold leading-tight">{item.name}</h3>
			<p class="text-base text-base-content/80 mt-2 line-clamp-3 leading-relaxed">{item.description}</p>
		</div>
	</div>
	<div class="flex items-center justify-between mt-4">
		<div class="flex items-center gap-2 min-w-0">
			{#if item.author}
				<span class="text-base font-medium">{item.author}</span>
				{#if item.authorVerified}
					<CheckCircle class="w-3.5 h-3.5 text-info shrink-0" />
				{/if}
			{/if}
			<InstallCode code={item.code} inline />
		</div>
		<PricePill price={item.price} installed={item.installed} />
	</div>
</a>
