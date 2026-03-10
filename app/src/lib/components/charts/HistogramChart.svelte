<script lang="ts">
	import { onMount } from 'svelte';

	interface Props {
		data: number[];
		bins?: number;
		height?: number;
		showGrid?: boolean;
		showCurve?: boolean; // Overlay normal distribution curve
		color?: string;
		xLabel?: string;
		yLabel?: string;
	}

	let {
		data = [],
		bins = 10,
		height,
		showGrid = true,
		showCurve = false,
		color = 'rgba(99, 102, 241, 0.8)',
		xLabel,
		yLabel = 'Frequency'
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

	// Calculate histogram bins
	const histogram = $derived.by(() => {
		if (data.length === 0) return { bins: [], min: 0, max: 1, maxCount: 1 };

		const min = Math.min(...data);
		const max = Math.max(...data);
		const binWidth = (max - min) / bins;

		const binCounts: number[] = new Array(bins).fill(0);

		for (const value of data) {
			let binIndex = Math.floor((value - min) / binWidth);
			if (binIndex >= bins) binIndex = bins - 1; // Handle edge case for max value
			binCounts[binIndex]++;
		}

		const binData = binCounts.map((count, i) => ({
			start: min + i * binWidth,
			end: min + (i + 1) * binWidth,
			count,
			midpoint: min + (i + 0.5) * binWidth
		}));

		return {
			bins: binData,
			min,
			max,
			binWidth,
			maxCount: Math.max(...binCounts) || 1
		};
	});

	// Statistics for normal curve
	const stats = $derived.by(() => {
		if (data.length === 0) return { mean: 0, stdDev: 1 };

		const n = data.length;
		const mean = data.reduce((sum, v) => sum + v, 0) / n;
		const variance = data.reduce((sum, v) => sum + Math.pow(v - mean, 2), 0) / n;
		const stdDev = Math.sqrt(variance);

		return { mean, stdDev };
	});

	function xScale(value: number): number {
		return ((value - histogram.min) / (histogram.max - histogram.min)) * chartWidth;
	}

	function yScale(count: number): number {
		return chartHeight - (count / histogram.maxCount) * chartHeight;
	}

	// Generate normal distribution curve
	const normalCurve = $derived.by(() => {
		if (!showCurve || data.length === 0) return '';

		const { mean, stdDev } = stats;
		const points: string[] = [];
		const steps = 50;

		for (let i = 0; i <= steps; i++) {
			const x = histogram.min + (i / steps) * (histogram.max - histogram.min);
			// Normal PDF
			const exponent = -Math.pow(x - mean, 2) / (2 * Math.pow(stdDev, 2));
			const pdf = (1 / (stdDev * Math.sqrt(2 * Math.PI))) * Math.exp(exponent);
			// Scale to match histogram
			const scaledY = pdf * data.length * (histogram.binWidth ?? 1);

			const px = xScale(x);
			const py = yScale(scaledY);

			points.push(`${i === 0 ? 'M' : 'L'}${px},${py}`);
		}

		return points.join(' ');
	});

	const xTicks = $derived.by(() => {
		const ticks = [];
		const tickCount = 5;
		const range = histogram.max - histogram.min;
		for (let i = 0; i <= tickCount; i++) {
			const value = histogram.min + (range / tickCount) * i;
			const x = (chartWidth / tickCount) * i;
			ticks.push({ value, x });
		}
		return ticks;
	});

	const yTicks = $derived.by(() => {
		const ticks = [];
		const tickCount = 4;
		for (let i = 0; i <= tickCount; i++) {
			const value = (histogram.maxCount / tickCount) * i;
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
</script>

<div bind:this={containerRef} class="histogram-chart-container flex flex-col w-full h-full">
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

			<!-- Histogram bars -->
			<g transform="translate({padding.left}, {padding.top})">
				{#each histogram.bins as bin}
					{@const barX = xScale(bin.start)}
					{@const barWidth = xScale(bin.end) - xScale(bin.start)}
					{@const barHeight = chartHeight - yScale(bin.count)}
					<rect
						x={barX}
						y={yScale(bin.count)}
						width={Math.max(barWidth - 1, 1)}
						height={barHeight}
						fill={color}
						stroke="#1e293b"
						stroke-width="1"
						class="transition-all duration-200 hover:opacity-80 cursor-pointer"
					>
						<title>Range: {formatValue(bin.start)} - {formatValue(bin.end)}
Count: {bin.count}</title>
					</rect>
				{/each}
			</g>

			<!-- Normal distribution curve -->
			{#if showCurve && normalCurve}
				<g transform="translate({padding.left}, {padding.top})">
					<path
						d={normalCurve}
						fill="none"
						stroke="rgba(251, 146, 60, 1)"
						stroke-width="2"
						stroke-linecap="round"
					/>
				</g>
			{/if}

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

		<!-- Statistics -->
		<div class="flex justify-center gap-6 text-xs text-base-content/60 mt-2">
			<span>n = {data.length}</span>
			<span>μ = {stats.mean.toFixed(2)}</span>
			<span>σ = {stats.stdDev.toFixed(2)}</span>
		</div>
	{/if}
</div>
