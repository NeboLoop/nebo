<!--
  ScrollArea Component
  Custom scrollable area with styled scrollbars
-->

<script lang="ts">
	interface Props {
		children: any;
		class?: string;
		orientation?: 'vertical' | 'horizontal';
	}

	let {
		children,
		class: className = '',
		orientation = 'vertical'
	}: Props = $props();

	const orientationClasses = {
		vertical: 'overflow-y-auto overflow-x-hidden',
		horizontal: 'overflow-x-auto overflow-y-hidden'
	};
</script>

<div class="relative overflow-hidden {className}">
	<div class="h-full w-full rounded-[inherit] {orientationClasses[orientation]}">
		{@render children()}
	</div>
</div>
