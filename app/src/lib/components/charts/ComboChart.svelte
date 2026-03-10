<script lang="ts">
	import { onMount } from 'svelte';

	interface Props {
		data: {
			labels?: string[];
			datasets?: Array<{
				label?: string;
				data: number[];
				type: 'bar' | 'line';
				backgroundColor?: string;
				borderColor?: string;
				yAxisID?: 'left' | 'right';
			}>;
		};
		height?: number;
		showLegend?: boolean;
		showGrid?: boolean;
		showRightAxis?: boolean;
	}

	let {
		data,
		height,
		showLegend = true,
		showGrid = true,
		showRightAxis = false
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
	const padding = $derived({ top: 20, right: showRightAxis ? 60 : 20, bottom: 40, left: 50 });
	const viewBoxWidth = $derived(containerWidth);
	const viewBoxHeight = $derived(effectiveHeight);
	const chartWidth = $derived(viewBoxWidth - padding.left - padding.right);
	const chartHeight = $derived(viewBoxHeight - padding.top - padding.bottom);

	const defaultBarColors = [
		'rgba(99, 102, 241, 0.8)',
		'rgba(168, 85, 247, 0.8)',
		'rgba(34, 197, 94, 0.8)',
	];

	const defaultLineColors = [
		'rgba(251, 146, 60, 1)',
		'rgba(236, 72, 153, 1)',
		'rgba(14, 165, 233, 1)',
	];

	const labels = $derived(data?.labels || []);
	const datasets = $derived(data?.datasets || []);

	const barDatasets = $derived(datasets.filter(d => d.type === 'bar'));
	const lineDatasets = $derived(datasets.filter(d => d.type === 'line'));

	// Separate scales for left and right Y-axes
	const leftAxisDatasets = $derived(datasets.filter(d => d.yAxisID !== 'right'));
	const rightAxisDatasets = $derived(datasets.filter(d => d.yAxisID === 'right'));

	const leftMaxValue = $derived.by(() => {
		let max = 0;
		for (const dataset of leftAxisDatasets) {
			for (const value of dataset.data || []) {
				if (value > max) max = value;
			}
		}
		return max || 1;
	});

	const rightMaxValue = $derived.by(() => {
		let max = 0;
		for (const dataset of rightAxisDatasets) {
			for (const value of dataset.data || []) {
				if (value > max) max = value;
			}
		}
		return max || 1;
	});

	const barGroupWidth = $derived.by(() => {
		if (labels.length === 0) return 0;
		return chartWidth / labels.length;
	});

	const barWidth = $derived.by(() => {
		const numBarDatasets = barDatasets.length || 1;
		const groupPadding = barGroupWidth * 0.2;
		return (barGroupWidth - groupPadding) / numBarDatasets;
	});

	function yScaleLeft(value: number): number {
		return chartHeight - (value / leftMaxValue) * chartHeight;
	}

	function yScaleRight(value: number): number {
		return chartHeight - (value / rightMaxValue) * chartHeight;
	}

	function xScale(index: number): number {
		if (labels.length <= 1) return chartWidth / 2;
		return (index / (labels.length - 1)) * chartWidth;
	}

	const yTicksLeft = $derived.by(() => {
		const ticks = [];
		const tickCount = 4;
		for (let i = 0; i <= tickCount; i++) {
			const value = (leftMaxValue / tickCount) * i;
			const y = chartHeight - (chartHeight / tickCount) * i;
			ticks.push({ value, y });
		}
		return ticks;
	});

	const yTicksRight = $derived.by(() => {
		if (!showRightAxis || rightAxisDatasets.length === 0) return [];
		const ticks = [];
		const tickCount = 4;
		for (let i = 0; i <= tickCount; i++) {
			const value = (rightMaxValue / tickCount) * i;
			const y = chartHeight - (chartHeight / tickCount) * i;
			ticks.push({ value, y });
		}
		return ticks;
	});

	function getLinePath(dataset: any): string {
		const points = dataset.data.map((value: number, i: number) => ({
			x: xScale(i),
			y: dataset.yAxisID === 'right' ? yScaleRight(value) : yScaleLeft(value)
		}));

		if (points.length === 0) return '';

		// Smooth curve
		let path = `M${points[0].x},${points[0].y}`;
		for (let i = 1; i < points.length; i++) {
			const prev = points[i - 1];
			const curr = points[i];
			const cpx = (prev.x + curr.x) / 2;
			path += ` C${cpx},${prev.y} ${cpx},${curr.y} ${curr.x},${curr.y}`;
		}
		return path;
	}

	function formatValue(value: number): string {
		if (value >= 1000000) return `${(value / 1000000).toFixed(1)}M`;
		if (value >= 1000) return `${(value / 1000).toFixed(1)}K`;
		return value.toFixed(0);
	}

	function getBarColor(index: number, dataset: any): string {
		return dataset.backgroundColor || defaultBarColors[index % defaultBarColors.length];
	}

	function getLineColor(index: number, dataset: any): string {
		return dataset.borderColor || defaultLineColors[index % defaultLineColors.length];
	}
</script>

<div bind:this={containerRef} class="flex flex-col w-full h-full">
	{#if labels.length === 0 || datasets.length === 0}
		<div class="flex-1 flex items-center justify-center">
			<p class="text-base-500 text-sm">No data to display</p>
		</div>
	{:else}
		<svg viewBox="0 0 {viewBoxWidth} {viewBoxHeight}" preserveAspectRatio="none" class="flex-1 w-full">
			<!-- Grid lines -->
			{#if showGrid}
				<g transform="translate({padding.left}, {padding.top})">
					{#each yTicksLeft as tick}
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
				</g>
			{/if}

			<!-- Bars -->
			<g transform="translate({padding.left}, {padding.top})">
				{#each labels as label, labelIndex}
					{@const groupX = labelIndex * barGroupWidth + barGroupWidth * 0.1}
					{#each barDatasets as dataset, datasetIndex}
						{@const value = dataset.data[labelIndex] || 0}
						{@const barX = groupX + datasetIndex * barWidth}
						{@const yScale = dataset.yAxisID === 'right' ? yScaleRight : yScaleLeft}
						{@const barHeight = (value / (dataset.yAxisID === 'right' ? rightMaxValue : leftMaxValue)) * chartHeight}
						{@const barY = chartHeight - barHeight}
						<rect
							x={barX}
							y={barY}
							width={barWidth * 0.9}
							height={barHeight}
							fill={getBarColor(datasetIndex, dataset)}
							rx="2"
							class="transition-all duration-200 hover:opacity-80"
						>
							<title>{dataset.label || 'Value'}: {formatValue(value)}</title>
						</rect>
					{/each}
				{/each}
			</g>

			<!-- Lines -->
			<g transform="translate({padding.left}, {padding.top})">
				{#each lineDatasets as dataset, i}
					<path
						d={getLinePath(dataset)}
						fill="none"
						stroke={getLineColor(i, dataset)}
						stroke-width="3"
						stroke-linecap="round"
						stroke-linejoin="round"
					/>
					<!-- Data points -->
					{#each dataset.data as value, pointIndex}
						{@const yScale = dataset.yAxisID === 'right' ? yScaleRight : yScaleLeft}
						<circle
							cx={xScale(pointIndex)}
							cy={yScale(value)}
							r="5"
							fill={getLineColor(i, dataset)}
							stroke="#1e293b"
							stroke-width="2"
							class="transition-all duration-200 hover:r-7"
						>
							<title>{dataset.label || 'Value'}: {formatValue(value)}</title>
						</circle>
					{/each}
				{/each}
			</g>

			<!-- Left Y-axis -->
			<g transform="translate({padding.left}, {padding.top})">
				<line x1="0" y1="0" x2="0" y2={chartHeight} stroke="#475569" stroke-width="1"/>
				{#each yTicksLeft as tick}
					<text
						x="-8"
						y={tick.y}
						text-anchor="end"
						dominant-baseline="middle"
						class="text-xs fill-base-500"
						font-size="10"
					>
						{formatValue(tick.value)}
					</text>
				{/each}
			</g>

			<!-- Right Y-axis -->
			{#if showRightAxis && yTicksRight.length > 0}
				<g transform="translate({padding.left + chartWidth}, {padding.top})">
					<line x1="0" y1="0" x2="0" y2={chartHeight} stroke="#475569" stroke-width="1"/>
					{#each yTicksRight as tick}
						<text
							x="8"
							y={tick.y}
							text-anchor="start"
							dominant-baseline="middle"
							class="text-xs fill-base-500"
							font-size="10"
						>
							{formatValue(tick.value)}
						</text>
					{/each}
				</g>
			{/if}

			<!-- X-axis -->
			<g transform="translate({padding.left}, {padding.top + chartHeight})">
				<line x1="0" y1="0" x2={chartWidth} y2="0" stroke="#475569" stroke-width="1"/>
				{#each labels as label, i}
					<text
						x={i * barGroupWidth + barGroupWidth / 2}
						y="20"
						text-anchor="middle"
						class="text-xs fill-base-500"
						font-size="10"
					>
						{label.length > 10 ? label.slice(0, 10) + '..' : label}
					</text>
				{/each}
			</g>
		</svg>

		<!-- Legend -->
		{#if showLegend}
			<div class="flex flex-wrap justify-center gap-4 pt-3">
				{#each barDatasets as dataset, i}
					<div class="flex items-center gap-1.5">
						<span class="w-3 h-3 rounded" style="background-color: {getBarColor(i, dataset)}"></span>
						<span class="text-xs text-text-muted">{dataset.label || `Bar ${i + 1}`}</span>
					</div>
				{/each}
				{#each lineDatasets as dataset, i}
					<div class="flex items-center gap-1.5">
						<span class="w-4 h-0.5 rounded" style="background-color: {getLineColor(i, dataset)}"></span>
						<span class="text-xs text-text-muted">{dataset.label || `Line ${i + 1}`}</span>
					</div>
				{/each}
			</div>
		{/if}
	{/if}
</div>
