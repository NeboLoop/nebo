<!--
  Reusable Card Component
  Ensures 100% consistency for all card-based layouts
-->

<script lang="ts">
	let {
		title,
		subtitle,
		children,
		class: extraClass = '',
		hover = false,
		onclick
	}: {
		title?: string;
		subtitle?: string;
		children: any;
		class?: string;
		hover?: boolean;
		onclick?: () => void;
	} = $props();

	const isClickable = $derived(!!onclick);
	const className = $derived(`card bg-base-100 shadow-xl ${hover ? 'hover:shadow-2xl transition-shadow' : ''} ${isClickable ? 'cursor-pointer' : ''} ${extraClass}`.trim());
</script>

{#snippet cardContent()}
	<div class="card-body">
		{#if title}
			<h2 class="card-title">{title}</h2>
			{#if subtitle}
				<p class="text-base-content/70">{subtitle}</p>
			{/if}
		{/if}
		{@render children()}
	</div>
{/snippet}

{#if isClickable}
	<button type="button" class={className} onclick={onclick}>
		{@render cardContent()}
	</button>
{:else}
	<div class={className}>
		{@render cardContent()}
	</div>
{/if}
