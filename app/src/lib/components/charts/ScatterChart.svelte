<script lang="ts">
	import { onMount } from 'svelte';

	interface DataPoint {
		x: number;
		y: number;
		size?: number;
		label?: string;
	}

	interface Dataset {
		label?: string;
		data: DataPoint[];
		color?: string;
	}

	interface Props {
		data: {
			datasets: Dataset[];
		};
		height?: number;
		showLegend?: boolean;
		showGrid?: boolean;
		showTrendline?: boolean;
		pointSize?: number;
		xLabel?: string;
		yLabel?: string;
		quadrants?: boolean;
	}

	let {
		data,
		height,
		showLegend = true,
		showGrid = true,
		showTrendline = false,
		pointSize = 6,
		xLabel,
		yLabel,
		quadrants = false
	}: Props = $props();

	let containerRef: HTMLDivElement;
	let containerWidth = $state(400);
	let containerHeight = $state(300);

	onMount(() => {
		if (containerRef) {
			const observer = new ResizeObserver((entries) => {
				for (const entry of entries) {
					containerWidth = Math.max(entry.contentRect.width, 200);
					if (!height) {
						containerHeight = Math.max(entry.contentRect.height, 200);
					}
				}
			});
			observer.observe(containerRef);
			return () => observer.disconnect();
		}
	});

	const effectiveHeight = $derived(height || containerHeight);
	const padding = { top: 20, right: 20, bottom: 50, left: 60 };
	const viewBoxWidth = $derived(containerWidth);
	const viewBoxHeight = $derived(effectiveHeight);
	const chartWidth = $derived(viewBoxWidth - padding.left - padding.right);
	const chartHeight = $derived(viewBoxHeight - padding.top - padding.bottom);

	const defaultColors = [
		'rgba(99, 102, 241, 0.8)',
		'rgba(168, 85, 247, 0.8)',
		'rgba(236, 72, 153, 0.8)',
		'rgba(251, 146, 60, 0.8)',
		'rgba(34, 197, 94, 0.8)',
		'rgba(14, 165, 233, 0.8)',
	];

	const datasets = $derived(data?.datasets || []);

	const allPoints = $derived.by(() => {
		const points: DataPoint[] = [];
		for (const dataset of datasets) {
			for (const point of dataset.data || []) {
				points.push(point);
			}
		}
		return points;
	});

	const xRange = $derived.by(() => {
		if (allPoints.length === 0) return { min: 0, max: 100 };
		const xValues = allPoints.map(p => p.x);
		const min = Math.min(...xValues);
		const max = Math.max(...xValues);
		const padding = (max - min) * 0.1 || 10;
		return { min: min - padding, max: max + padding };
	});

	const yRange = $derived.by(() => {
		if (allPoints.length === 0) return { min: 0, max: 100 };
		const yValues = allPoints.map(p => p.y);
		const min = Math.min(...yValues);
		const max = Math.max(...yValues);
		const padding = (max - min) * 0.1 || 10;
		return { min: min - padding, max: max + padding };
	});

	function xScale(value: number): number {
		return ((value - xRange.min) / (xRange.max - xRange.min)) * chartWidth;
	}

	function yScale(value: number): number {
		return chartHeight - ((value - yRange.min) / (yRange.max - yRange.min)) * chartHeight;
	}

	const xTicks = $derived.by(() => {
		const ticks = [];
		const tickCount = 5;
		const range = xRange.max - xRange.min;
		for (let i = 0; i <= tickCount; i++) {
			const value = xRange.min + (range / tickCount) * i;
			const x = (chartWidth / tickCount) * i;
			ticks.push({ value, x });
		}
		return ticks;
	});

	const yTicks = $derived.by(() => {
		const ticks = [];
		const tickCount = 5;
		const range = yRange.max - yRange.min;
		for (let i = 0; i <= tickCount; i++) {
			const value = yRange.min + (range / tickCount) * i;
			const y = chartHeight - (chartHeight / tickCount) * i;
			ticks.push({ value, y });
		}
		return ticks;
	});

	// Calculate trendline using linear regression
	const trendline = $derived.by(() => {
		if (!showTrendline || allPoints.length < 2) return null;

		const n = allPoints.length;
		const sumX = allPoints.reduce((sum, p) => sum + p.x, 0);
		const sumY = allPoints.reduce((sum, p) => sum + p.y, 0);
		const sumXY = allPoints.reduce((sum, p) => sum + p.x * p.y, 0);
		const sumX2 = allPoints.reduce((sum, p) => sum + p.x * p.x, 0);

		const denom = n * sumX2 - sumX * sumX;
		if (denom === 0) return null;

		const slope = (n * sumXY - sumX * sumY) / denom;
		const intercept = (sumY - slope * sumX) / n;

		const x1 = xRange.min;
		const x2 = xRange.max;
		const y1 = slope * x1 + intercept;
		const y2 = slope * x2 + intercept;

		return { x1, y1, x2, y2 };
	});

	// Quadrant lines (at midpoints)
	const quadrantLines = $derived.by(() => {
		if (!quadrants) return null;
		const midX = (xRange.min + xRange.max) / 2;
		const midY = (yRange.min + yRange.max) / 2;
		return { x: xScale(midX), y: yScale(midY) };
	});

	function formatValue(value: number): string {
		if (Math.abs(value) >= 1000000) return `${(value / 1000000).toFixed(1)}M`;
		if (Math.abs(value) >= 1000) return `${(value / 1000).toFixed(1)}K`;
		if (Math.abs(value) < 1 && value !== 0) return value.toFixed(2);
		return value.toFixed(0);
	}

	function getColor(index: number, dataset: Dataset): string {
		return dataset.color || defaultColors[index % defaultColors.length];
	}

	function getPointSize(point: DataPoint): number {
		// Support bubble chart sizing
		if (point.size !== undefined) {
			return Math.max(4, Math.min(point.size, 30));
		}
		return pointSize;
	}
</script>

<div bind:this={containerRef} class="scatter-chart-container flex flex-col w-full h-full">
	{#if datasets.length === 0 || allPoints.length === 0}
		<div class="empty-state flex-1 flex items-center justify-center">
			<p class="text-base-content/60 text-sm">No data to display</p>
		</div>
	{:else}
		<svg viewBox="0 0 {viewBoxWidth} {viewBoxHeight}" preserveAspectRatio="xMidYMid meet" class="flex-1 w-full">
			<!-- Grid lines -->
			{#if showGrid}
				<g transform="translate({padding.left}, {padding.top})">
					{#each yTicks as tick}
						<line
							x1="0"
							y1={tick.y}
							x2={chartWidth}
							y2={tick.y}
							stroke="rgba(51, 65, 85, 0.3)"
							stroke-width="1"
							stroke-dasharray="4,4"
						/>
					{/each}
					{#each xTicks as tick}
						<line
							x1={tick.x}
							y1="0"
							x2={tick.x}
							y2={chartHeight}
							stroke="rgba(51, 65, 85, 0.3)"
							stroke-width="1"
							stroke-dasharray="4,4"
						/>
					{/each}
				</g>
			{/if}

			<!-- Quadrant lines -->
			{#if quadrantLines}
				<g transform="translate({padding.left}, {padding.top})">
					<line
						x1={quadrantLines.x}
						y1="0"
						x2={quadrantLines.x}
						y2={chartHeight}
						stroke="rgba(148, 163, 184, 0.5)"
						stroke-width="2"
					/>
					<line
						x1="0"
						y1={quadrantLines.y}
						x2={chartWidth}
						y2={quadrantLines.y}
						stroke="rgba(148, 163, 184, 0.5)"
						stroke-width="2"
					/>
				</g>
			{/if}

			<!-- Trendline -->
			{#if trendline}
				<g transform="translate({padding.left}, {padding.top})">
					<line
						x1={xScale(trendline.x1)}
						y1={yScale(trendline.y1)}
						x2={xScale(trendline.x2)}
						y2={yScale(trendline.y2)}
						stroke="rgba(148, 163, 184, 0.6)"
						stroke-width="2"
						stroke-dasharray="6,4"
					/>
				</g>
			{/if}

			<!-- Data points -->
			<g transform="translate({padding.left}, {padding.top})">
				{#each datasets as dataset, datasetIndex}
					{#each dataset.data as point}
						<circle
							cx={xScale(point.x)}
							cy={yScale(point.y)}
							r={getPointSize(point)}
							fill={getColor(datasetIndex, dataset)}
							stroke="#1e293b"
							stroke-width="2"
							class="transition-all duration-200 hover:opacity-80 cursor-pointer"
						>
							<title>{dataset.label || 'Point'}: ({formatValue(point.x)}, {formatValue(point.y)}){point.label ? ` - ${point.label}` : ''}</title>
						</circle>
					{/each}
				{/each}
			</g>

			<!-- Y-axis -->
			<g transform="translate({padding.left}, {padding.top})">
				<line x1="0" y1="0" x2="0" y2={chartHeight} stroke="#475569" stroke-width="1"/>
				{#each yTicks as tick}
					<text
						x="-8"
						y={tick.y}
						text-anchor="end"
						dominant-baseline="middle"
						class="text-xs fill-base-content/60"
						font-size="10"
					>
						{formatValue(tick.value)}
					</text>
				{/each}
				{#if yLabel}
					<text
						x={-chartHeight / 2}
						y="-45"
						text-anchor="middle"
						transform="rotate(-90)"
						class="text-xs fill-base-content/60"
						font-size="11"
					>
						{yLabel}
					</text>
				{/if}
			</g>

			<!-- X-axis -->
			<g transform="translate({padding.left}, {padding.top + chartHeight})">
				<line x1="0" y1="0" x2={chartWidth} y2="0" stroke="#475569" stroke-width="1"/>
				{#each xTicks as tick}
					<text
						x={tick.x}
						y="20"
						text-anchor="middle"
						class="text-xs fill-base-content/60"
						font-size="10"
					>
						{formatValue(tick.value)}
					</text>
				{/each}
				{#if xLabel}
					<text
						x={chartWidth / 2}
						y="38"
						text-anchor="middle"
						class="text-xs fill-base-content/60"
						font-size="11"
					>
						{xLabel}
					</text>
				{/if}
			</g>
		</svg>

		<!-- Legend -->
		{#if showLegend && datasets.length > 1}
			<div class="legend">
				{#each datasets as dataset, i}
					<div class="legend-item">
						<span class="legend-dot" style="background-color: {getColor(i, dataset)}"></span>
						<span class="legend-label">{dataset.label || `Series ${i + 1}`}</span>
					</div>
				{/each}
			</div>
		{/if}
	{/if}
</div>
