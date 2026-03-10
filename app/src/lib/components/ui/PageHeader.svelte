<script lang="ts">
	import type { Snippet, Component } from 'svelte';

	interface Props {
		title: string;
		subtitle?: string;
		icon?: Component<{ class?: string }>;
		actions?: Snippet;
	}

	let { title, subtitle, icon, actions }: Props = $props();
</script>

<div class="mb-6">
	<div class="flex flex-col items-start justify-between gap-4 sm:flex-row sm:items-center">
		<div>
			<h1 class="text-3xl font-bold flex items-center gap-3">
				{#if icon}
					{@const Icon = icon}
					<Icon class="h-8 w-8 text-primary" />
				{/if}
				<span>{title}</span>
			</h1>
			{#if subtitle}
				<p class="mt-2 text-sm text-base-content/60">{subtitle}</p>
			{/if}
		</div>
		{#if actions}
			<div class="flex items-center gap-3">
				{@render actions()}
			</div>
		{/if}
	</div>
</div>
