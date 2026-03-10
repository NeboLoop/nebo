<script lang="ts">
	import { onMount } from 'svelte';
	import ChartTooltip from './ChartTooltip.svelte';

	interface ChartJsData {
		labels?: string[];
		datasets?: Array<{
			label?: string;
			data: number[];
			backgroundColor?: string | string[];
			borderColor?: string | string[];
		}>;
	}

	interface Props {
		data: ChartJsData;
		height?: number;
		showLegend?: boolean;
		type?: 'pie' | 'doughnut';
		onSliceClick?: (data: { label: string; value: number; percentage: number; index: number }) => void;
	}

	let {
		data,
		height,
		showLegend = true,
		type = 'doughnut',
		onSliceClick
	}: Props = $props();

	// Container reference for responsive sizing
	let containerRef = $state<HTMLDivElement | undefined>(undefined);
	let containerWidth = $state(400);
	let containerHeight = $state(300);
	let isNarrow = $state(false);

	// Tooltip state
	let tooltipVisible = $state(false);
	let tooltipX = $state(0);
	let tooltipY = $state(0);
	let tooltipData = $state<{
		label: string;
		value: string | number;
		percentage?: string;
		color?: string;
	} | null>(null);

	// Hover state for highlighting
	let hoveredSlice = $state<number | null>(null);

	onMount(() => {
		if (containerRef) {
			const observer = new ResizeObserver((entries) => {
				for (const entry of entries) {
					containerWidth = entry.contentRect.width;
					containerHeight = entry.contentRect.height;
					// Switch to stacked layout when container is narrow
					isNarrow = containerWidth < 400;
				}
			});
			observer.observe(containerRef);
			return () => observer.disconnect();
		}
	});

	// Chart dimensions - responsive based on container
	const size = $derived.by(() => {
		if (height) return Math.min(height, containerWidth - 20);
		// For horizontal layout, use half width for chart; for stacked, use full width
		const availableWidth = showLegend && !isNarrow ? containerWidth * 0.55 : containerWidth;
		const availableHeight = showLegend && isNarrow ? containerHeight * 0.6 : containerHeight;
		return Math.max(Math.min(availableWidth, availableHeight) - 20, 120);
	});
	const cx = $derived(size / 2);
	const cy = $derived(size / 2);
	const radius = $derived((size / 2) - 20);
	const innerRadius = $derived(type === 'doughnut' ? radius * 0.6 : 0);

	// Default colors
	const defaultColors = [
		'rgba(99, 102, 241, 0.85)',   // Indigo
		'rgba(168, 85, 247, 0.85)',   // Purple
		'rgba(236, 72, 153, 0.85)',   // Pink
		'rgba(251, 146, 60, 0.85)',   // Orange
		'rgba(34, 197, 94, 0.85)',    // Green
		'rgba(14, 165, 233, 0.85)',   // Sky
	];

	// Extract data
	const labels = $derived(data?.labels || []);
	const values = $derived(data?.datasets?.[0]?.data || []);
	const colors = $derived(data?.datasets?.[0]?.backgroundColor || []);

	// Calculate total
	const total = $derived(values.reduce((sum, v) => sum + (v || 0), 0) || 1);

	// Generate pie slices
	const slices = $derived.by(() => {
		const result: Array<{
			path: string;
			color: string;
			label: string;
			value: number;
			percentage: string;
			percentageNum: number;
			labelX: number;
			labelY: number;
		}> = [];

		let currentAngle = -90; // Start from top

		values.forEach((value, i) => {
			const percentage = (value / total) * 100;
			const angle = (value / total) * 360;
			const startAngle = currentAngle;
			const endAngle = currentAngle + angle;

			// Calculate path
			const startRad = (startAngle * Math.PI) / 180;
			const endRad = (endAngle * Math.PI) / 180;

			const x1 = cx + radius * Math.cos(startRad);
			const y1 = cy + radius * Math.sin(startRad);
			const x2 = cx + radius * Math.cos(endRad);
			const y2 = cy + radius * Math.sin(endRad);

			const ix1 = cx + innerRadius * Math.cos(startRad);
			const iy1 = cy + innerRadius * Math.sin(startRad);
			const ix2 = cx + innerRadius * Math.cos(endRad);
			const iy2 = cy + innerRadius * Math.sin(endRad);

			const largeArc = angle > 180 ? 1 : 0;

			let path: string;
			if (innerRadius > 0) {
				// Doughnut
				path = `M ${x1} ${y1} A ${radius} ${radius} 0 ${largeArc} 1 ${x2} ${y2} L ${ix2} ${iy2} A ${innerRadius} ${innerRadius} 0 ${largeArc} 0 ${ix1} ${iy1} Z`;
			} else {
				// Pie
				path = `M ${cx} ${cy} L ${x1} ${y1} A ${radius} ${radius} 0 ${largeArc} 1 ${x2} ${y2} Z`;
			}

			// Calculate label position (midpoint of arc)
			const midAngle = ((startAngle + endAngle) / 2 * Math.PI) / 180;
			const labelRadius = (radius + innerRadius) / 2;
			const labelX = cx + labelRadius * Math.cos(midAngle);
			const labelY = cy + labelRadius * Math.sin(midAngle);

			// Get color
			const color = Array.isArray(colors) ? colors[i] : defaultColors[i % defaultColors.length];

			result.push({
				path,
				color: color || defaultColors[i % defaultColors.length],
				label: labels[i] || `Item ${i + 1}`,
				value,
				percentage: percentage.toFixed(1),
				percentageNum: percentage,
				labelX,
				labelY
			});

			currentAngle = endAngle;
		});

		return result;
	});

	function formatValue(value: number | string): string {
		const num = typeof value === 'number' ? value : parseFloat(value);
		if (isNaN(num)) return String(value);
		if (num >= 1000000) return `${(num / 1000000).toFixed(1)}M`;
		if (num >= 1000) return `${(num / 1000).toFixed(1)}K`;
		return num.toFixed(0);
	}

	function handleSliceMouseEnter(event: MouseEvent, slice: typeof slices[0], index: number) {
		hoveredSlice = index;

		if (!containerRef) return;
		const rect = containerRef.getBoundingClientRect();
		tooltipX = event.clientX - rect.left;
		tooltipY = event.clientY - rect.top;

		tooltipData = {
			label: slice.label,
			value: slice.value,
			percentage: `${slice.percentage}%`,
			color: slice.color
		};
		tooltipVisible = true;
	}

	function handleSliceMouseMove(event: MouseEvent) {
		if (tooltipVisible && containerRef) {
			const rect = containerRef.getBoundingClientRect();
			tooltipX = event.clientX - rect.left;
			tooltipY = event.clientY - rect.top;
		}
	}

	function handleSliceMouseLeave() {
		hoveredSlice = null;
		tooltipVisible = false;
		tooltipData = null;
	}

	function handleSliceClick(slice: typeof slices[0], index: number) {
		if (onSliceClick) {
			onSliceClick({
				label: slice.label,
				value: slice.value,
				percentage: slice.percentageNum,
				index
			});
		}
	}

	function isSliceDimmed(index: number): boolean {
		if (hoveredSlice === null) return false;
		return hoveredSlice !== index;
	}

	function isSliceHighlighted(index: number): boolean {
		if (hoveredSlice === null) return false;
		return hoveredSlice === index;
	}

	// Calculate scale for highlighted slice (slight expansion)
	function getSliceTransform(index: number): string {
		if (!isSliceHighlighted(index)) return '';
		// Get the midpoint angle of the slice to calculate expansion direction
		const slice = slices[index];
		if (!slice) return '';

		// Find the midpoint angle
		let currentAngle = -90;
		for (let i = 0; i < index; i++) {
			const angle = (values[i] / total) * 360;
			currentAngle += angle;
		}
		const angle = (values[index] / total) * 360;
		const midAngle = currentAngle + angle / 2;
		const midRad = (midAngle * Math.PI) / 180;

		// Expand outward by 4px
		const expandX = Math.cos(midRad) * 4;
		const expandY = Math.sin(midRad) * 4;

		return `translate(${expandX}, ${expandY})`;
	}
</script>

<div bind:this={containerRef} class="flex flex-col w-full h-full relative overflow-hidden">
	{#if values.length === 0}
		<div class="flex-1 flex items-center justify-center">
			<p class="text-text-muted text-sm">No data to display</p>
		</div>
	{:else}
		<div class="flex-1 flex items-center gap-4 justify-center {isNarrow ? 'flex-col' : 'flex-row'}">
			<svg width={size} height={size} viewBox="0 0 {size} {size}" class="flex-shrink-0" style="max-width: 100%;">
				{#each slices as slice, i}
					<path
						d={slice.path}
						fill={slice.color}
						stroke="white"
						stroke-width="2"
						class="transition-all duration-150 cursor-pointer outline-none focus:outline-none {isSliceHighlighted(i) ? 'brightness-110' : ''} {isSliceDimmed(i) ? 'opacity-40' : ''}"
						transform={getSliceTransform(i)}
						onmouseenter={(e) => handleSliceMouseEnter(e, slice, i)}
						onmousemove={handleSliceMouseMove}
						onmouseleave={handleSliceMouseLeave}
						onclick={() => handleSliceClick(slice, i)}
						role="button"
						tabindex="0"
						onkeydown={(e) => e.key === 'Enter' && handleSliceClick(slice, i)}
					/>
				{/each}

				<!-- Center label for doughnut -->
				{#if type === 'doughnut'}
					<text
						x={cx}
						y={cy - 8}
						text-anchor="middle"
						dominant-baseline="middle"
						class="fill-current text-text font-bold"
						font-size="20"
					>
						{formatValue(total)}
					</text>
					<text
						x={cx}
						y={cy + 12}
						text-anchor="middle"
						dominant-baseline="middle"
						class="fill-current text-text-muted"
						font-size="11"
					>
						Total
					</text>
				{/if}
			</svg>

			<!-- Legend -->
			{#if showLegend}
				<div class="{isNarrow ? 'flex flex-wrap justify-center gap-4 pt-3' : 'flex flex-col gap-2'}">
					{#each slices as slice, i}
						<div
							class="flex items-center gap-1.5 cursor-pointer transition-opacity duration-150 outline-none focus:outline-none {isSliceDimmed(i) ? 'opacity-40' : ''}"
							onmouseenter={() => hoveredSlice = i}
							onmouseleave={() => hoveredSlice = null}
							onclick={() => handleSliceClick(slice, i)}
							role="button"
							tabindex="0"
							onkeydown={(e) => e.key === 'Enter' && handleSliceClick(slice, i)}
						>
							<span class="w-3 h-3 rounded-full" style="background-color: {slice.color}"></span>
							<span class="text-xs text-text-muted">{slice.label}</span>
							{#if !isNarrow}
								<span class="text-xs text-text ml-auto font-medium">{slice.percentage}%</span>
							{/if}
						</div>
					{/each}
				</div>
			{/if}
		</div>

		<!-- Tooltip -->
		<ChartTooltip
			visible={tooltipVisible}
			x={tooltipX}
			y={tooltipY}
			data={tooltipData}
			containerRef={containerRef}
		/>
	{/if}
</div>
