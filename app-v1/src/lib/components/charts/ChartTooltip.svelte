<script lang="ts">
	import { fade } from 'svelte/transition';

	interface TooltipData {
		label: string;
		value: string | number;
		percentage?: string;
		color?: string;
		secondaryLabel?: string;
		secondaryValue?: string;
		comparison?: {
			value: string;
			direction: 'up' | 'down' | 'neutral';
		};
	}

	interface Props {
		visible?: boolean;
		x?: number;
		y?: number;
		data?: TooltipData | null;
		containerRef?: HTMLElement | null;
	}

	let {
		visible = false,
		x = 0,
		y = 0,
		data = null,
		containerRef = null
	}: Props = $props();

	// Calculate position to avoid clipping
	const position = $derived.by(() => {
		const tooltipWidth = 180;
		const tooltipHeight = 80;
		const padding = 12;

		let posX = x + padding;
		let posY = y - tooltipHeight - padding;

		// If container ref provided, constrain to container
		if (containerRef) {
			const rect = containerRef.getBoundingClientRect();

			// Keep within right edge
			if (posX + tooltipWidth > rect.width) {
				posX = x - tooltipWidth - padding;
			}

			// Keep within top edge
			if (posY < 0) {
				posY = y + padding;
			}

			// Keep within bottom edge
			if (posY + tooltipHeight > rect.height) {
				posY = rect.height - tooltipHeight - padding;
			}

			// Keep within left edge
			if (posX < 0) {
				posX = padding;
			}
		}

		return { x: posX, y: posY };
	});

	function formatValue(value: string | number): string {
		if (typeof value === 'string') return value;
		if (Math.abs(value) >= 1000000) return `${(value / 1000000).toFixed(1)}M`;
		if (Math.abs(value) >= 1000) return `${(value / 1000).toFixed(1)}K`;
		return value.toLocaleString();
	}
</script>

{#if visible && data}
	<div
		class="chart-tooltip"
		style="left: {position.x}px; top: {position.y}px;"
		transition:fade={{ duration: 150 }}
	>
		<div class="tooltip-header">
			{#if data.color}
				<span class="tooltip-color" style="background-color: {data.color}"></span>
			{/if}
			<span class="tooltip-label">{data.label}</span>
		</div>

		<div class="tooltip-value-row">
			<span class="tooltip-value">{formatValue(data.value)}</span>
			{#if data.percentage}
				<span class="tooltip-percentage">{data.percentage}</span>
			{/if}
		</div>

		{#if data.secondaryLabel && data.secondaryValue}
			<div class="tooltip-secondary">
				<span class="tooltip-secondary-label">{data.secondaryLabel}:</span>
				<span class="tooltip-secondary-value">{data.secondaryValue}</span>
			</div>
		{/if}

		{#if data.comparison}
			<div class="tooltip-comparison tooltip-comparison-{data.comparison.direction}">
				{#if data.comparison.direction === 'up'}
					<svg class="comparison-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
						<path d="M18 15l-6-6-6 6"/>
					</svg>
				{:else if data.comparison.direction === 'down'}
					<svg class="comparison-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
						<path d="M6 9l6 6 6-6"/>
					</svg>
				{:else}
					<svg class="comparison-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
						<path d="M5 12h14"/>
					</svg>
				{/if}
				<span>{data.comparison.value}</span>
			</div>
		{/if}
	</div>
{/if}
