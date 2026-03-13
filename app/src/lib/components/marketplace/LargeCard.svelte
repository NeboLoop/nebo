<script lang="ts">
	import InstallCode from '$lib/components/InstallCode.svelte';
	import PricePill from './PricePill.svelte';
	import ArtifactIcon from './ArtifactIcon.svelte';
	import { type AppItem, itemHref } from '$lib/types/marketplace';
	import { CheckCircle } from 'lucide-svelte';

	let { item, label }: { item: AppItem; label?: string } = $props();

	const typeLabel = label || (item.type === 'role' ? 'ROLE' : item.type === 'workflow' ? 'WORKFLOW' : 'SKILL');
</script>

<a
	href={itemHref(item)}
	class="rounded-2xl overflow-hidden border border-base-content/10 hover:border-base-content/20 transition-colors"
>
	<div class="p-5">
		<p class="text-xs font-semibold text-primary uppercase tracking-wider mb-2">{typeLabel}</p>
		<div class="flex items-start gap-4">
			<ArtifactIcon emoji={item.iconEmoji} bg={item.iconBg} size="xl" />
			<div class="flex-1 min-w-0">
				<h3 class="font-display text-lg font-bold leading-tight">{item.name}</h3>
				<p class="text-xs text-base-content/70 mt-1 line-clamp-2 leading-relaxed">{item.description}</p>
				<div class="flex items-center gap-1.5 mt-2">
					{#if item.author}
						<span class="text-xs text-base-content/70 truncate">{item.author}</span>
						{#if item.authorVerified}
							<CheckCircle class="w-3 h-3 text-info shrink-0" />
						{/if}
					{/if}
				</div>
			</div>
			<PricePill price={item.price} installed={item.installed} />
		</div>
		<div class="flex items-center justify-between mt-3">
			<InstallCode code={item.code} inline />
		</div>
	</div>
</a>
