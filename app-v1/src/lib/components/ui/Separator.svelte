<!--
  Separator Component
  Horizontal or vertical divider line using DaisyUI
-->

<script lang="ts">
	interface Props {
		orientation?: 'horizontal' | 'vertical';
		decorative?: boolean;
		class?: string;
	}

	let {
		orientation = 'horizontal',
		decorative = true,
		class: className = ''
	}: Props = $props();
</script>

{#if orientation === 'horizontal'}
	<div
		class="divider my-0 {className}"
		role={decorative ? 'presentation' : 'separator'}
		aria-orientation={orientation}
	></div>
{:else}
	<div
		class="divider divider-horizontal mx-0 {className}"
		role={decorative ? 'presentation' : 'separator'}
		aria-orientation={orientation}
	></div>
{/if}
