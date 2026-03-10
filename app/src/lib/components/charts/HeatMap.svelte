<script lang="ts">
	import type { HeatMapDataPoint, ChartConfig } from './types';

	interface Props {
		data: HeatMapDataPoint[];
		config?: ChartConfig;
		height?: number;
		colorScheme?: 'indigo' | 'purple' | 'green' | 'red' | 'blue';
		showValues?: boolean;
	}

	let {
		data = [],
		config = {},
		height = 400,
		colorScheme = 'indigo',
		showValues = true
	}: Props = $props();

	let hoveredCell: HeatMapDataPoint | null = $state(null);

	// Get unique x and y labels
	const xLabels = $derived.by(() => {
		const labels = [...new Set(data.map(d => d.x))];
		return labels.sort();
	});

	const yLabels = $derived.by(() => {
		const labels = [...new Set(data.map(d => d.y))];
		return labels.sort();
	});

	// Calculate min and max values for color scaling
	const valueRange = $derived.by(() => {
		if (data.length === 0) return { min: 0, max: 1 };
		const values = data.map(d => d.value);
		return {
			min: Math.min(...values),
			max: Math.max(...values)
		};
	});

	// Dimensions
	const padding = { top: 60, right: 40, bottom: 60, left: 100 };
	const cellSize = 50;
	const width = $derived(xLabels.length * cellSize + padding.left + padding.right);
	const chartHeight = $derived(yLabels.length * cellSize + padding.top + padding.bottom);

	function getColor(value: number): string {
		const { min, max } = valueRange;
		const normalized = (value - min) / (max - min);

		// Color schemes (low to high intensity)
		const schemes = {
			indigo: [
				{ r: 224, g: 231, b: 255 },  // Very light
				{ r: 129, g: 140, b: 248 },  // Medium
				{ r: 67, g: 56, b: 202 }     // Dark
			],
			purple: [
				{ r: 243, g: 232, b: 255 },
				{ r: 168, g: 85, b: 247 },
				{ r: 107, g: 33, b: 168 }
			],
			green: [
				{ r: 220, g: 252, b: 231 },
				{ r: 34, g: 197, b: 94 },
				{ r: 21, g: 128, b: 61 }
			],
			red: [
				{ r: 254, g: 226, b: 226 },
				{ r: 248, g: 113, b: 113 },
				{ r: 185, g: 28, b: 28 }
			],
			blue: [
				{ r: 224, g: 242, b: 254 },
				{ r: 56, g: 189, b: 248 },
				{ r: 3, g: 105, b: 161 }
			]
		};

		const colors = schemes[colorScheme];

		let color;
		if (normalized < 0.5) {
			// Interpolate between first and second color
			const t = normalized * 2;
			color = {
				r: Math.round(colors[0].r + (colors[1].r - colors[0].r) * t),
				g: Math.round(colors[0].g + (colors[1].g - colors[0].g) * t),
				b: Math.round(colors[0].b + (colors[1].b - colors[0].b) * t)
			};
		} else {
			// Interpolate between second and third color
			const t = (normalized - 0.5) * 2;
			color = {
				r: Math.round(colors[1].r + (colors[2].r - colors[1].r) * t),
				g: Math.round(colors[1].g + (colors[2].g - colors[1].g) * t),
				b: Math.round(colors[1].b + (colors[2].b - colors[1].b) * t)
			};
		}

		return `rgb(${color.r}, ${color.g}, ${color.b})`;
	}

	function getTextColor(value: number): string {
		const { min, max } = valueRange;
		const normalized = (value - min) / (max - min);
		// Use white text for darker cells
		return normalized > 0.6 ? '#ffffff' : '#1e293b';
	}

	function formatValue(value: number): string {
		if (value >= 1000000) {
			return `${(value / 1000000).toFixed(1)}M`;
		} else if (value >= 1000) {
			return `${(value / 1000).toFixed(1)}K`;
		} else if (value < 1) {
			return value.toFixed(2);
		}
		return value.toFixed(0);
	}

	function getDataPoint(x: string, y: string): HeatMapDataPoint | undefined {
		return data.find(d => d.x === x && d.y === y);
	}
</script>

<div class="relative w-full overflow-x-auto" style="height: {height}px;">
	<svg width={width} height={Math.max(chartHeight, height)} class="block">
		<!-- Y-axis labels -->
		<g transform={`translate(${padding.left - 10}, ${padding.top})`}>
			{#each yLabels as yLabel, i}
				<text
					x="0"
					y={i * cellSize + cellSize / 2}
					text-anchor="end"
					dominant-baseline="middle"
					class="text-sm fill-base-500 font-medium"
				>
					{yLabel}
				</text>
			{/each}
		</g>

		<!-- X-axis labels -->
		<g transform={`translate(${padding.left}, ${padding.top - 10})`}>
			{#each xLabels as xLabel, i}
				<text
					x={i * cellSize + cellSize / 2}
					y="0"
					text-anchor="end"
					dominant-baseline="middle"
					transform={`rotate(-45, ${i * cellSize + cellSize / 2}, 0)`}
					class="text-sm fill-base-500 font-medium"
				>
					{xLabel}
				</text>
			{/each}
		</g>

		<!-- Heatmap cells -->
		<g transform={`translate(${padding.left}, ${padding.top})`}>
			{#each yLabels as yLabel, yIndex}
				{#each xLabels as xLabel, xIndex}
					{@const dataPoint = getDataPoint(xLabel, yLabel)}
					{#if dataPoint}
						<g
							onmouseenter={() => hoveredCell = dataPoint}
							onmouseleave={() => hoveredCell = null}
							class="cursor-pointer transition-all"
							role="img"
							aria-label="{xLabel}, {yLabel}: {dataPoint.value}"
						>
							<rect
								x={xIndex * cellSize + 1}
								y={yIndex * cellSize + 1}
								width={cellSize - 2}
								height={cellSize - 2}
								fill={getColor(dataPoint.value)}
								stroke={hoveredCell === dataPoint ? '#ffffff' : 'transparent'}
								stroke-width={hoveredCell === dataPoint ? 2 : 0}
								rx="4"
								class="transition-all duration-200"
								style="filter: {hoveredCell === dataPoint ? 'brightness(1.1)' : 'none'}"
							/>
							{#if showValues}
								<text
									x={xIndex * cellSize + cellSize / 2}
									y={yIndex * cellSize + cellSize / 2}
									text-anchor="middle"
									dominant-baseline="middle"
									fill={getTextColor(dataPoint.value)}
									class="text-xs font-semibold pointer-events-none"
								>
									{formatValue(dataPoint.value)}
								</text>
							{/if}
						</g>
					{:else}
						<rect
							x={xIndex * cellSize + 1}
							y={yIndex * cellSize + 1}
							width={cellSize - 2}
							height={cellSize - 2}
							fill="rgba(51, 65, 85, 0.1)"
							stroke="rgba(51, 65, 85, 0.2)"
							stroke-width="1"
							rx="4"
						/>
					{/if}
				{/each}
			{/each}
		</g>

		<!-- Legend -->
		<g transform={`translate(${width - padding.right - 30}, ${padding.top})`}>
			<text
				x="0"
				y="-10"
				class="text-xs fill-base-500 font-medium"
			>
				Scale
			</text>
			{#each [0, 0.25, 0.5, 0.75, 1] as position, i}
				{@const value = valueRange.min + (valueRange.max - valueRange.min) * position}
				<rect
					x="0"
					y={i * 30}
					width="20"
					height="25"
					fill={getColor(value)}
					rx="2"
				/>
				<text
					x="25"
					y={i * 30 + 12.5}
					text-anchor="start"
					dominant-baseline="middle"
					class="text-xs fill-base-500"
				>
					{formatValue(value)}
				</text>
			{/each}
		</g>
	</svg>

	<!-- Tooltip -->
	{#if hoveredCell}
		<div
			class="absolute bg-base-900/95 text-white px-3 py-2 rounded-lg shadow-xl border border-base-700 pointer-events-none z-10 top-2.5 right-2.5"
		>
			<div class="text-xs text-base-400">{hoveredCell.x} x {hoveredCell.y}</div>
			<div class="text-sm font-semibold">{formatValue(hoveredCell.value)}</div>
			{#if hoveredCell.label}
				<div class="text-xs text-base-300 mt-1">{hoveredCell.label}</div>
			{/if}
		</div>
	{/if}
</div>
