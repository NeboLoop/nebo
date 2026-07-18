<script lang="ts">
	import { t } from 'svelte-i18n';
	import InstallCode from '$lib/components/InstallCode.svelte';
	import PricePill from './PricePill.svelte';
	import ArtifactIcon from './ArtifactIcon.svelte';
	import { type AppItem, itemHref } from '$lib/types/marketplace';
	import { CheckCircle } from 'lucide-svelte';

	let { item, label }: { item: AppItem; label?: string } = $props();

	const TYPE_KEYS: Record<AppItem['type'], string> = {
		agent: 'marketplace.kind.agent',
		app: 'marketplace.kind.app',
		skill: 'marketplace.kind.skill',
		plugin: 'marketplace.kind.plugin',
		connector: 'marketplace.kind.connector',
		workflow: 'marketplace.kind.workflow',
		collection: 'marketplace.kind.collection'
	};
	const typeLabel = $derived(
		label ||
			$t('marketplace.featuredKind', {
				values: { kind: $t(TYPE_KEYS[item.type] ?? 'marketplace.kind.item') }
			})
	);
</script>

<a
	href={itemHref(item)}
	class="block rounded-2xl bg-gradient-to-br from-accent/10 to-base-200 border border-base-300 p-6 sm:p-8 hover:border-base-content/20 hover:shadow-md transition-all"
>
	<p class="text-xs font-semibold uppercase tracking-wider text-accent mb-2">{typeLabel}</p>
	<div class="flex items-start gap-4 mb-4">
		<ArtifactIcon emoji={item.iconEmoji} bg={item.iconBg} size="xl" />
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
