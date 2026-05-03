<!--
  Tooltip Component
  Uses DaisyUI tooltip classes with data-tip attribute
-->

<script lang="ts">
	interface Props {
		content: string;
		position?: 'top' | 'bottom' | 'left' | 'right';
		children: any;
	}

	let {
		content,
		position = 'top',
		children
	}: Props = $props();

	const positionClass = $derived({
		top: 'tooltip-top',
		bottom: 'tooltip-bottom',
		left: 'tooltip-left',
		right: 'tooltip-right'
	}[position]);
</script>

<div class="tooltip {positionClass}" data-tip={content}>
	{@render children()}
</div>
