<!--
  Step Card Component
  Wrapper card for setup wizard step content with consistent styling
  Uses DaisyUI card classes
-->

<script lang="ts">
	let {
		title,
		description,
		children,
		class: extraClass = ''
	}: {
		title: string;
		description?: string;
		children: any;
		class?: string;
	} = $props();
</script>

<div class="card bg-base-100 shadow-xl {extraClass}">
	<div class="card-body">
		<h2 class="card-title text-2xl">{title}</h2>
		{#if description}
			<p class="text-base-content/70">{description}</p>
		{/if}
		<div class="mt-4">
			{@render children()}
		</div>
	</div>
</div>
