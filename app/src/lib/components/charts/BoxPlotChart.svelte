<script lang="ts">
	import { onMount } from 'svelte';

	interface BoxPlotData {
		label: string;
		min: number;
		q1: number;
		median: number;
		q3: number;
		max: number;
		outliers?: number[];
		color?: string;
	}

	interface Props {
		data: BoxPlotData[];
		height?: number;
		showGrid?: boolean;
		showOutliers?: boolean;
		orientation?: 'vertical' | 'horizontal';
	}

	let {
		data = [],
		height,
		showGrid = true,
		showOutliers = true,
		orientation = 'vertical'
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
		'rgba(99, 102, 241, 0.7)',
		'rgba(168, 85, 247, 0.7)',
		'rgba(236, 72, 153, 0.7)',
		'rgba(251, 146, 60, 0.7)',
		'rgba(34, 197, 94, 0.7)',
	];

	// Calculate value range
	const valueRange = $derived.by(() => {
		if (data.length === 0) return { min: 0, max: 100 };

		let minVal = Infinity;
		let maxVal = -Infinity;

		for (const item of data) {
			minVal = Math.min(minVal, item.min);
			maxVal = Math.max(maxVal, item.max);
			if (showOutliers && item.outliers) {
				for (const outlier of item.outliers) {
					minVal = Math.min(minVal, outlier);
					maxVal = Math.max(maxVal, outlier);
				}
			}
		}

		const range = maxVal - minVal;
		const paddingAmount = range * 0.1;
		return { min: minVal - paddingAmount, max: maxVal + paddingAmount };
	});

	const boxWidth = $derived.by(() => {
		if (data.length === 0) return 0;
		return (chartWidth / data.length) * 0.6;
	});

	function valueScale(value: number): number {
		const range = valueRange.max - valueRange.min;
		return chartHeight - ((value - valueRange.min) / range) * chartHeight;
	}

	function categoryScale(index: number): number {
		return (index + 0.5) * (chartWidth / data.length);
	}

	const yTicks = $derived.by(() => {
		const ticks = [];
		const tickCount = 5;
		const range = valueRange.max - valueRange.min;
		for (let i = 0; i <= tickCount; i++) {
			const value = valueRange.min + (range / tickCount) * i;
			const y = chartHeight - (chartHeight / tickCount) * i;
			ticks.push({ value, y });
		}
		return ticks;
	});

	function formatValue(value: number): string {
		if (Math.abs(value) >= 1000000) return `${(value / 1000000).toFixed(1)}M`;
		if (Math.abs(value) >= 1000) return `${(value / 1000).toFixed(1)}K`;
		if (Math.abs(value) < 1 && value !== 0) return value.toFixed(2);
		return value.toFixed(0);
	}

	function getColor(index: number, item: BoxPlotData): string {
		return item.color || defaultColors[index % defaultColors.length];
	}
</script>

<div bind:this={containerRef} class="boxplot-chart-container flex flex-col w-full h-full">
	{#if data.length === 0}
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
				</g>
			{/if}

			<!-- Box plots -->
			<g transform="translate({padding.left}, {padding.top})">
				{#each data as item, i}
					{@const cx = categoryScale(i)}
					{@const color = getColor(i, item)}

					<!-- Whisker (min to max line) -->
					<line
						x1={cx}
						y1={valueScale(item.min)}
						x2={cx}
						y2={valueScale(item.max)}
						stroke={color}
						stroke-width="2"
					/>

					<!-- Min cap -->
					<line
						x1={cx - boxWidth * 0.3}
						y1={valueScale(item.min)}
						x2={cx + boxWidth * 0.3}
						y2={valueScale(item.min)}
						stroke={color}
						stroke-width="2"
					/>

					<!-- Max cap -->
					<line
						x1={cx - boxWidth * 0.3}
						y1={valueScale(item.max)}
						x2={cx + boxWidth * 0.3}
						y2={valueScale(item.max)}
						stroke={color}
						stroke-width="2"
					/>

					<!-- Box (Q1 to Q3) -->
					<rect
						x={cx - boxWidth / 2}
						y={valueScale(item.q3)}
						width={boxWidth}
						height={valueScale(item.q1) - valueScale(item.q3)}
						fill={color}
						stroke="#1e293b"
						stroke-width="2"
						rx="2"
						class="transition-all duration-200 hover:opacity-80 cursor-pointer"
					>
						<title>
							{item.label}
							Min: {formatValue(item.min)}
							Q1: {formatValue(item.q1)}
							Median: {formatValue(item.median)}
							Q3: {formatValue(item.q3)}
							Max: {formatValue(item.max)}
						</title>
					</rect>

					<!-- Median line -->
					<line
						x1={cx - boxWidth / 2}
						y1={valueScale(item.median)}
						x2={cx + boxWidth / 2}
						y2={valueScale(item.median)}
						stroke="#f8fafc"
						stroke-width="3"
					/>

					<!-- Outliers -->
					{#if showOutliers && item.outliers}
						{#each item.outliers as outlier}
							<circle
								cx={cx}
								cy={valueScale(outlier)}
								r="4"
								fill="none"
								stroke={color}
								stroke-width="2"
							>
								<title>Outlier: {formatValue(outlier)}</title>
							</circle>
						{/each}
					{/if}
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
			</g>

			<!-- X-axis -->
			<g transform="translate({padding.left}, {padding.top + chartHeight})">
				<line x1="0" y1="0" x2={chartWidth} y2="0" stroke="#475569" stroke-width="1"/>
				{#each data as item, i}
					<text
						x={categoryScale(i)}
						y="20"
						text-anchor="middle"
						class="text-xs fill-base-content/60"
						font-size="10"
					>
						{item.label.length > 10 ? item.label.slice(0, 10) + '..' : item.label}
					</text>
				{/each}
			</g>
		</svg>
	{/if}
</div>
