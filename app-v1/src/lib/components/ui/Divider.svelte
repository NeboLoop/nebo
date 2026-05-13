<!--
  Divider Component
  Section dividers with optional labels using DaisyUI
-->

<script lang="ts">
	interface Props {
		label?: string;
		orientation?: 'horizontal' | 'vertical';
		variant?: 'solid' | 'dashed' | 'dotted';
	}

	let {
		label,
		orientation = 'horizontal',
		variant = 'solid'
	}: Props = $props();

	const variantClasses = {
		solid: '',
		dashed: 'border-dashed',
		dotted: 'border-dotted'
	};
</script>

{#if orientation === 'horizontal'}
	{#if label}
		<div class="divider {variantClasses[variant]}">{label}</div>
	{:else}
		<div class="divider {variantClasses[variant]}"></div>
	{/if}
{:else}
	<div class="divider divider-horizontal {variantClasses[variant]}"></div>
{/if}
