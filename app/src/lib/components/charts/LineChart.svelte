<script lang="ts">
	import { onMount } from 'svelte';

	interface ChartJsData {
		labels?: string[];
		datasets?: Array<{
			label?: string;
			data: number[];
			borderColor?: string;
			backgroundColor?: string;
			fill?: boolean;
			confidenceUpper?: number[];
			confidenceLower?: number[];
		}>;
	}

	interface Props {
		data: ChartJsData;
		height?: number;
		showLegend?: boolean;
		showGrid?: boolean;
		showDots?: boolean;
		smooth?: boolean;
		fill?: boolean;
		lineStyle?: 'solid' | 'step' | 'stepBefore' | 'stepAfter';
		showConfidence?: boolean;
	}

	let {
		data,
		height,
		showLegend = true,
		showGrid = true,
		showDots = true,
		smooth = true,
		fill = false,
		lineStyle = 'solid',
		showConfidence = false
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
		'rgba(99, 102, 241, 1)',
		'rgba(168, 85, 247, 1)',
		'rgba(236, 72, 153, 1)',
		'rgba(251, 146, 60, 1)',
		'rgba(34, 197, 94, 1)',
		'rgba(14, 165, 233, 1)',
	];

	const labels = $derived(data?.labels || []);
	const datasets = $derived(data?.datasets || []);

	const maxValue = $derived.by(() => {
		let max = 0;
		for (const dataset of datasets) {
			for (const value of dataset.data || []) {
				if (value > max) max = value;
			}
			// Also check confidence bounds
			if (showConfidence && dataset.confidenceUpper) {
				for (const value of dataset.confidenceUpper) {
					if (value > max) max = value;
				}
			}
		}
		return max || 1;
	});

	const minValue = $derived.by(() => {
		let min = Infinity;
		for (const dataset of datasets) {
			for (const value of dataset.data || []) {
				if (value < min) min = value;
			}
			// Also check confidence bounds
			if (showConfidence && dataset.confidenceLower) {
				for (const value of dataset.confidenceLower) {
					if (value < min) min = value;
				}
			}
		}
		return min === Infinity ? 0 : min;
	});

	function xScale(index: number): number {
		if (labels.length <= 1) return chartWidth / 2;
		return (index / (labels.length - 1)) * chartWidth;
	}

	function yScale(value: number): number {
		const range = maxValue - minValue || 1;
		return chartHeight - ((value - minValue) / range) * chartHeight;
	}

	const yTicks = $derived.by(() => {
		const ticks = [];
		const tickCount = 4;
		const range = maxValue - minValue || 1;
		for (let i = 0; i <= tickCount; i++) {
			const value = minValue + (range / tickCount) * i;
			const y = chartHeight - (chartHeight / tickCount) * i;
			ticks.push({ value, y });
		}
		return ticks;
	});

	function getLinePath(datasetData: number[]): string {
		if (datasetData.length === 0) return '';

		const points = datasetData.map((value, i) => ({
			x: xScale(i),
			y: yScale(value)
		}));

		if (lineStyle === 'step' || lineStyle === 'stepAfter') {
			// Step after: horizontal then vertical
			let path = `M${points[0].x},${points[0].y}`;
			for (let i = 1; i < points.length; i++) {
				const prev = points[i - 1];
				const curr = points[i];
				path += ` H${curr.x} V${curr.y}`;
			}
			return path;
		}

		if (lineStyle === 'stepBefore') {
			// Step before: vertical then horizontal
			let path = `M${points[0].x},${points[0].y}`;
			for (let i = 1; i < points.length; i++) {
				const curr = points[i];
				path += ` V${curr.y} H${curr.x}`;
			}
			return path;
		}

		if (smooth && points.length > 2) {
			// Smooth curve using bezier
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

	function getAreaPath(datasetData: number[]): string {
		if (datasetData.length === 0) return '';

		const linePath = getLinePath(datasetData);
		const lastX = xScale(datasetData.length - 1);
		const firstX = xScale(0);

		return `${linePath} L${lastX},${chartHeight} L${firstX},${chartHeight} Z`;
	}

	function getConfidencePath(upper: number[], lower: number[]): string {
		if (upper.length === 0 || lower.length === 0) return '';

		// Upper boundary (left to right)
		let path = upper.map((value, i) => {
			const x = xScale(i);
			const y = yScale(value);
			return `${i === 0 ? 'M' : 'L'}${x},${y}`;
		}).join(' ');

		// Lower boundary (right to left)
		for (let i = lower.length - 1; i >= 0; i--) {
			const x = xScale(i);
			const y = yScale(lower[i]);
			path += ` L${x},${y}`;
		}

		return path + ' Z';
	}

	function formatValue(value: number): string {
		if (value >= 1000000) return `${(value / 1000000).toFixed(1)}M`;
		if (value >= 1000) return `${(value / 1000).toFixed(1)}K`;
		return value.toFixed(0);
	}

	function getLineColor(dataset: any, index: number): string {
		return dataset.borderColor || defaultColors[index % defaultColors.length];
	}

	function getFillColor(dataset: any, index: number): string {
		const color = getLineColor(dataset, index);
		return color.replace('1)', '0.2)').replace('0.8)', '0.15)');
	}
</script>

<div bind:this={containerRef} class="line-chart-container flex flex-col w-full h-full">
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

			<!-- Lines and areas -->
			<g transform="translate({padding.left}, {padding.top})">
				{#each datasets as dataset, datasetIndex}
					<!-- Confidence interval -->
					{#if showConfidence && dataset.confidenceUpper && dataset.confidenceLower}
						<path
							d={getConfidencePath(dataset.confidenceUpper, dataset.confidenceLower)}
							fill={getFillColor(dataset, datasetIndex)}
							opacity="0.5"
						/>
					{/if}

					<!-- Area fill -->
					{#if fill || dataset.fill}
						<path
							d={getAreaPath(dataset.data)}
							fill={getFillColor(dataset, datasetIndex)}
						/>
					{/if}

					<!-- Line -->
					<path
						d={getLinePath(dataset.data)}
						fill="none"
						stroke={getLineColor(dataset, datasetIndex)}
						stroke-width="2"
						stroke-linecap="round"
						stroke-linejoin="round"
					/>

					<!-- Data points -->
					{#if showDots}
						{#each dataset.data as value, i}
							<circle
								cx={xScale(i)}
								cy={yScale(value)}
								r="4"
								fill={getLineColor(dataset, datasetIndex)}
								stroke="#1e293b"
								stroke-width="2"
								class="transition-all duration-200 hover:r-6"
							>
								<title>{dataset.label || 'Value'}: {formatValue(value)}</title>
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
				{#each labels as label, i}
					{#if labels.length <= 10 || i % Math.ceil(labels.length / 10) === 0}
						<text
							x={xScale(i)}
							y="20"
							text-anchor="middle"
							class="text-xs fill-base-content/60"
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
						<span class="legend-line" style="background-color: {getLineColor(dataset, i)}"></span>
						<span class="legend-label">{dataset.label || `Series ${i + 1}`}</span>
					</div>
				{/each}
			</div>
		{/if}
	{/if}
</div>
