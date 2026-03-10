<script lang="ts">
	import { onMount } from 'svelte';

	interface RadarDataset {
		label: string;
		data: number[];
		color?: string;
		fill?: boolean;
	}

	interface Props {
		labels: string[];
		datasets: RadarDataset[];
		height?: number;
		showLegend?: boolean;
		maxValue?: number;
		levels?: number;
	}

	let {
		labels = [],
		datasets = [],
		height,
		showLegend = true,
		maxValue,
		levels = 5
	}: Props = $props();

	let containerRef: HTMLDivElement;
	let containerSize = $state(300);

	onMount(() => {
		if (containerRef) {
			const observer = new ResizeObserver((entries) => {
				for (const entry of entries) {
					const minDim = Math.min(entry.contentRect.width, entry.contentRect.height);
					containerSize = Math.max(minDim, 200);
				}
			});
			observer.observe(containerRef);
			return () => observer.disconnect();
		}
	});

	const size = $derived(height || containerSize);
	const cx = $derived(size / 2);
	const cy = $derived(size / 2);
	const radius = $derived((size / 2) - 60);

	const defaultColors = [
		{ stroke: 'rgba(99, 102, 241, 1)', fill: 'rgba(99, 102, 241, 0.2)' },
		{ stroke: 'rgba(168, 85, 247, 1)', fill: 'rgba(168, 85, 247, 0.2)' },
		{ stroke: 'rgba(236, 72, 153, 1)', fill: 'rgba(236, 72, 153, 0.2)' },
		{ stroke: 'rgba(251, 146, 60, 1)', fill: 'rgba(251, 146, 60, 0.2)' },
		{ stroke: 'rgba(34, 197, 94, 1)', fill: 'rgba(34, 197, 94, 0.2)' },
	];

	const numAxes = $derived(labels.length);
	const angleSlice = $derived((Math.PI * 2) / numAxes);

	// Calculate max value from data if not provided
	const effectiveMaxValue = $derived.by(() => {
		if (maxValue) return maxValue;
		let max = 0;
		for (const dataset of datasets) {
			for (const value of dataset.data) {
				if (value > max) max = value;
			}
		}
		return max || 100;
	});

	// Generate grid circles
	const gridCircles = $derived.by(() => {
		const circles = [];
		for (let i = 1; i <= levels; i++) {
			circles.push({
				radius: (radius / levels) * i,
				value: (effectiveMaxValue / levels) * i
			});
		}
		return circles;
	});

	// Generate axis lines
	const axisLines = $derived.by(() => {
		return labels.map((label, i) => {
			const angle = angleSlice * i - Math.PI / 2;
			return {
				x: cx + Math.cos(angle) * radius,
				y: cy + Math.sin(angle) * radius,
				labelX: cx + Math.cos(angle) * (radius + 25),
				labelY: cy + Math.sin(angle) * (radius + 25),
				label,
				angle
			};
		});
	});

	// Generate data polygons
	function getPolygonPoints(data: number[]): string {
		return data.map((value, i) => {
			const angle = angleSlice * i - Math.PI / 2;
			const r = (value / effectiveMaxValue) * radius;
			const x = cx + Math.cos(angle) * r;
			const y = cy + Math.sin(angle) * r;
			return `${x},${y}`;
		}).join(' ');
	}

	function getDataPoints(data: number[]): Array<{ x: number; y: number; value: number; label: string }> {
		return data.map((value, i) => {
			const angle = angleSlice * i - Math.PI / 2;
			const r = (value / effectiveMaxValue) * radius;
			return {
				x: cx + Math.cos(angle) * r,
				y: cy + Math.sin(angle) * r,
				value,
				label: labels[i]
			};
		});
	}

	function getColor(index: number, dataset: RadarDataset): { stroke: string; fill: string } {
		if (dataset.color) {
			return {
				stroke: dataset.color,
				fill: dataset.color.replace('1)', '0.2)').replace('0.8)', '0.2)')
			};
		}
		return defaultColors[index % defaultColors.length];
	}

	function formatValue(value: number): string {
		if (value >= 1000000) return `${(value / 1000000).toFixed(1)}M`;
		if (value >= 1000) return `${(value / 1000).toFixed(1)}K`;
		return value.toFixed(0);
	}
</script>

<div bind:this={containerRef} class="radar-chart-container flex flex-col items-center justify-center w-full h-full">
	{#if labels.length === 0 || datasets.length === 0}
		<div class="empty-state flex items-center justify-center">
			<p class="text-base-content/60 text-sm">No data to display</p>
		</div>
	{:else}
		<svg width={size} height={size} viewBox="0 0 {size} {size}">
			<!-- Grid circles -->
			{#each gridCircles as circle}
				<circle
					cx={cx}
					cy={cy}
					r={circle.radius}
					fill="none"
					stroke="rgba(51, 65, 85, 0.3)"
					stroke-width="1"
				/>
				<!-- Value labels on grid -->
				<text
					x={cx + 5}
					y={cy - circle.radius + 3}
					class="text-xs fill-base-content/60"
					font-size="9"
				>
					{formatValue(circle.value)}
				</text>
			{/each}

			<!-- Axis lines -->
			{#each axisLines as axis}
				<line
					x1={cx}
					y1={cy}
					x2={axis.x}
					y2={axis.y}
					stroke="rgba(51, 65, 85, 0.4)"
					stroke-width="1"
				/>
				<!-- Axis labels -->
				<text
					x={axis.labelX}
					y={axis.labelY}
					text-anchor="middle"
					dominant-baseline="middle"
					class="text-xs fill-base-content/70"
					font-size="11"
				>
					{axis.label}
				</text>
			{/each}

			<!-- Data polygons -->
			{#each datasets as dataset, i}
				{@const colors = getColor(i, dataset)}
				<!-- Fill area -->
				{#if dataset.fill !== false}
					<polygon
						points={getPolygonPoints(dataset.data)}
						fill={colors.fill}
						stroke="none"
						class="transition-all duration-300"
					/>
				{/if}
				<!-- Outline -->
				<polygon
					points={getPolygonPoints(dataset.data)}
					fill="none"
					stroke={colors.stroke}
					stroke-width="2"
					stroke-linejoin="round"
					class="transition-all duration-300"
				/>
				<!-- Data points -->
				{#each getDataPoints(dataset.data) as point}
					<circle
						cx={point.x}
						cy={point.y}
						r="4"
						fill={colors.stroke}
						stroke="#1e293b"
						stroke-width="2"
						class="transition-all duration-200 hover:r-6 cursor-pointer"
					>
						<title>{dataset.label} - {point.label}: {formatValue(point.value)}</title>
					</circle>
				{/each}
			{/each}
		</svg>

		<!-- Legend -->
		{#if showLegend && datasets.length > 0}
			<div class="legend mt-4">
				{#each datasets as dataset, i}
					{@const colors = getColor(i, dataset)}
					<div class="legend-item">
						<span class="legend-dot" style="background-color: {colors.stroke}"></span>
						<span class="legend-label">{dataset.label}</span>
					</div>
				{/each}
			</div>
		{/if}
	{/if}
</div>
