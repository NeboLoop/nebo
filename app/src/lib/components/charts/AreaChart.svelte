<script lang="ts">
	import { onMount } from 'svelte';

	interface Props {
		data: {
			labels?: string[];
			datasets?: Array<{
				label?: string;
				data: number[];
				backgroundColor?: string;
				borderColor?: string;
			}>;
		};
		height?: number;
		showLegend?: boolean;
		showGrid?: boolean;
		smooth?: boolean;
		stacked?: boolean;
		normalized?: boolean; // 100% stacked
	}

	let {
		data,
		height,
		showLegend = true,
		showGrid = true,
		smooth = true,
		stacked = false,
		normalized = false
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
	const padding = { top: 20, right: 20, bottom: 40, left: 50 };
	const viewBoxWidth = $derived(containerWidth);
	const viewBoxHeight = $derived(effectiveHeight);
	const chartWidth = $derived(viewBoxWidth - padding.left - padding.right);
	const chartHeight = $derived(viewBoxHeight - padding.top - padding.bottom);

	const defaultColors = [
		{ fill: 'rgba(99, 102, 241, 0.4)', stroke: 'rgba(99, 102, 241, 1)' },
		{ fill: 'rgba(168, 85, 247, 0.4)', stroke: 'rgba(168, 85, 247, 1)' },
		{ fill: 'rgba(236, 72, 153, 0.4)', stroke: 'rgba(236, 72, 153, 1)' },
		{ fill: 'rgba(251, 146, 60, 0.4)', stroke: 'rgba(251, 146, 60, 1)' },
		{ fill: 'rgba(34, 197, 94, 0.4)', stroke: 'rgba(34, 197, 94, 1)' },
		{ fill: 'rgba(14, 165, 233, 0.4)', stroke: 'rgba(14, 165, 233, 1)' },
	];

	const labels = $derived(data?.labels || []);
	const datasets = $derived(data?.datasets || []);

	// Calculate stacked values
	const stackedData = $derived.by(() => {
		if (!stacked && !normalized) return null;

		const result: number[][] = [];
		const numPoints = labels.length;

		for (let i = 0; i < numPoints; i++) {
			const values: number[] = [];
			let cumulative = 0;
			let total = 0;

			// First pass: calculate total for normalization
			if (normalized) {
				for (const dataset of datasets) {
					total += dataset.data[i] || 0;
				}
			}

			// Second pass: calculate cumulative values
			for (const dataset of datasets) {
				let value = dataset.data[i] || 0;
				if (normalized && total > 0) {
					value = (value / total) * 100;
				}
				cumulative += value;
				values.push(cumulative);
			}
			result.push(values);
		}
		return result;
	});

	const maxValue = $derived.by(() => {
		if (normalized) return 100;

		if (stacked && stackedData) {
			let max = 0;
			for (const point of stackedData) {
				const lastValue = point[point.length - 1] || 0;
				if (lastValue > max) max = lastValue;
			}
			return max || 1;
		}

		let max = 0;
		for (const dataset of datasets) {
			for (const value of dataset.data || []) {
				if (value > max) max = value;
			}
		}
		return max || 1;
	});

	function xScale(index: number): number {
		if (labels.length <= 1) return chartWidth / 2;
		return (index / (labels.length - 1)) * chartWidth;
	}

	function yScale(value: number): number {
		return chartHeight - (value / maxValue) * chartHeight;
	}

	const yTicks = $derived.by(() => {
		const ticks = [];
		const tickCount = 4;
		for (let i = 0; i <= tickCount; i++) {
			const value = (maxValue / tickCount) * i;
			const y = chartHeight - (chartHeight / tickCount) * i;
			ticks.push({ value, y });
		}
		return ticks;
	});

	function getAreaPath(datasetIndex: number): string {
		if (labels.length === 0) return '';

		const points: { x: number; y: number }[] = [];
		const bottomPoints: { x: number; y: number }[] = [];

		for (let i = 0; i < labels.length; i++) {
			const x = xScale(i);
			let y: number;
			let bottomY: number;

			if ((stacked || normalized) && stackedData) {
				y = yScale(stackedData[i][datasetIndex]);
				bottomY = datasetIndex > 0 ? yScale(stackedData[i][datasetIndex - 1]) : chartHeight;
			} else {
				y = yScale(datasets[datasetIndex].data[i] || 0);
				bottomY = chartHeight;
			}

			points.push({ x, y });
			bottomPoints.unshift({ x, y: bottomY });
		}

		let path: string;
		if (smooth && points.length > 2) {
			path = `M${points[0].x},${points[0].y}`;
			for (let i = 1; i < points.length; i++) {
				const prev = points[i - 1];
				const curr = points[i];
				const cpx = (prev.x + curr.x) / 2;
				path += ` C${cpx},${prev.y} ${cpx},${curr.y} ${curr.x},${curr.y}`;
			}
			// Close the path along the bottom
			path += ` L${bottomPoints[0].x},${bottomPoints[0].y}`;
			for (let i = 1; i < bottomPoints.length; i++) {
				const prev = bottomPoints[i - 1];
				const curr = bottomPoints[i];
				const cpx = (prev.x + curr.x) / 2;
				path += ` C${cpx},${prev.y} ${cpx},${curr.y} ${curr.x},${curr.y}`;
			}
			path += ' Z';
		} else {
			path = points.map((p, i) => `${i === 0 ? 'M' : 'L'}${p.x},${p.y}`).join(' ');
			path += bottomPoints.map(p => ` L${p.x},${p.y}`).join('');
			path += ' Z';
		}

		return path;
	}

	function getLinePath(datasetIndex: number): string {
		if (labels.length === 0) return '';

		const points: { x: number; y: number }[] = [];

		for (let i = 0; i < labels.length; i++) {
			const x = xScale(i);
			let y: number;

			if ((stacked || normalized) && stackedData) {
				y = yScale(stackedData[i][datasetIndex]);
			} else {
				y = yScale(datasets[datasetIndex].data[i] || 0);
			}

			points.push({ x, y });
		}

		if (smooth && points.length > 2) {
			let path = `M${points[0].x},${points[0].y}`;
			for (let i = 1; i < points.length; i++) {
				const prev = points[i - 1];
				const curr = points[i];
				const cpx = (prev.x + curr.x) / 2;
				path += ` C${cpx},${prev.y} ${cpx},${curr.y} ${curr.x},${curr.y}`;
			}
			return path;
		}

		return points.map((p, i) => `${i === 0 ? 'M' : 'L'}${p.x},${p.y}`).join(' ');
	}

	function formatValue(value: number): string {
		if (normalized) return `${value.toFixed(0)}%`;
		if (value >= 1000000) return `${(value / 1000000).toFixed(1)}M`;
		if (value >= 1000) return `${(value / 1000).toFixed(1)}K`;
		return value.toFixed(0);
	}

	function getFillColor(dataset: any, index: number): string {
		return dataset.backgroundColor || defaultColors[index % defaultColors.length].fill;
	}

	function getStrokeColor(dataset: any, index: number): string {
		return dataset.borderColor || defaultColors[index % defaultColors.length].stroke;
	}
</script>

<div bind:this={containerRef} class="area-chart-container flex flex-col w-full h-full">
	{#if labels.length === 0 || datasets.length === 0}
		<div class="empty-state flex-1 flex items-center justify-center">
			<p class="text-base-content/60 text-sm">No data to display</p>
		</div>
	{:else}
		<svg viewBox="0 0 {viewBoxWidth} {viewBoxHeight}" preserveAspectRatio="none" class="flex-1 w-full">
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

			<!-- Areas (render in reverse for proper stacking) -->
			<g transform="translate({padding.left}, {padding.top})">
				{#each [...datasets].reverse() as dataset, i}
					{@const actualIndex = datasets.length - 1 - i}
					<path
						d={getAreaPath(actualIndex)}
						fill={getFillColor(dataset, actualIndex)}
						class="transition-all duration-200"
					/>
					<path
						d={getLinePath(actualIndex)}
						fill="none"
						stroke={getStrokeColor(dataset, actualIndex)}
						stroke-width="2"
						stroke-linecap="round"
						stroke-linejoin="round"
					/>
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
						class="text-xs fill-current text-text-muted"
						font-size="10"
					>
						{formatValue(tick.value)}
					</text>
				{/each}
			</g>

			<!-- X-axis -->
			<g transform="translate({padding.left}, {padding.top + chartHeight})">
				<line x1="0" y1="0" x2={chartWidth} y2="0" stroke="#475569" stroke-width="1"/>
				{#each labels as label, i}
					{#if labels.length <= 12 || i % Math.ceil(labels.length / 12) === 0}
						<text
							x={xScale(i)}
							y="20"
							text-anchor="middle"
							class="text-xs fill-current text-text-muted"
							font-size="10"
						>
							{label.length > 8 ? label.slice(0, 8) + '..' : label}
						</text>
					{/if}
				{/each}
			</g>
		</svg>

		<!-- Legend -->
		{#if showLegend && datasets.length > 1}
			<div class="legend">
				{#each datasets as dataset, i}
					<div class="legend-item">
						<span class="legend-dot" style="background-color: {getStrokeColor(dataset, i)}"></span>
						<span class="legend-label">{dataset.label || `Series ${i + 1}`}</span>
					</div>
				{/each}
			</div>
		{/if}
	{/if}
</div>
